//! Registration of the COM server and the TSF profile: `DllRegisterServer` / `DllUnregisterServer`.
//!
//! NOTHING in the automated tests may write to the real registry or call the real TSF registration APIs. The work is
//! therefore split in three layers: a pure plan ([`registry_plan`], [`removal_plan`]: ordered registry writes, profiles,
//! categories), a [`Backend`] trait that performs single steps, and the transactional drivers [`register_with`] /
//! [`unregister_with`]. Tests run the drivers against an in-memory fake; only [`RealBackend`] (used by the two exported
//! entry points) talks to Windows, and it refuses to open under `cfg(test)`.
//!
//! What a keyboard TIP needs (Microsoft Learn "Text Service Registration", the TSF blog "Text Service Basics", the
//! SampleIME sample and Mozc's `tsf_registrar.cc`):
//! 1. COM in-proc server: `CLSID\{clsid}` (default = class name), `InprocServer32` (default = full path of this DLL,
//!    `ThreadingModel` = `Apartment`). The path is written verbatim and unquoted as REG_SZ: it goes straight to
//!    `LoadLibrary`, quoting is a `LocalServer32` convention.
//! 2. `ITfInputProcessorProfileMgr::RegisterProfile` once per language (zh-CN 0x0804, en-US 0x0409, ja-JP 0x0411).
//! 3. `ITfCategoryMgr::RegisterCategory` for the keyboard category and the capability categories (see
//!    [`product_categories`]).
//!
//! Where the keys land (Windows 11 x64; this DLL is 64-bit, so the 64-bit registry view and 64-bit `regsvr32`):
//! - HKLM, needs an elevated process: with [`Scope::Machine`] (the product default) the COM keys under
//!   `HKLM\Software\Classes\CLSID\{clsid}`; and everything `msctf.dll` writes for steps 2 and 3, which is observed
//!   (not documented) to live under `HKLM\SOFTWARE\Microsoft\CTF\TIP\{clsid}`. Microsoft's TSF blog states that all
//!   three steps write the machine registry and need administrator rights; Mozc says the same. We never write the CTF
//!   keys ourselves: the layout is msctf's private business, the APIs are the contract.
//! - HKCU: this module writes nothing there except the COM keys of [`Scope::User`]
//!   (`HKCU\Software\Classes\CLSID\{clsid}`). The user's own state (`HKCU\Software\Microsoft\CTF\Assemblies`,
//!   `HKCU\Control Panel\International\User Profile`, the default input method) is created and owned by Windows when
//!   the user adds or switches the keyboard. We do not touch it, so it is neither set nor cleaned up (stale entries
//!   after an uninstall are expected to be ignored by Windows; the VM runbook checks that).
//! - Per-user registration: only the COM half can be per-user. `RegisterProfile` / `RegisterCategory` take no hive
//!   argument and write HKLM, so `Scope::User` still needs elevation for the TSF half, and one third-party report says
//!   HKCU-only TIP entries are not functional. `Scope::User` therefore exists for the M5 feasibility experiment in a
//!   VM; it is not wired to any entry point and is unverified.
//! - A 32-bit build would have to be registered by the 32-bit `regsvr32` (Wow6432Node); none is built, so 32-bit
//!   applications do not see this input method.
//!
//! The user's default input method is never touched: [`Backend`] has no activate, default or reorder method at all,
//! and [`ProfileSpec::enabled_by_default`] only means "enabled for users who add this language" (design DD-01 3.1).

use std::panic::{AssertUnwindSafe, catch_unwind};

use windows::Win32::Foundation::{
    BOOL, E_ACCESSDENIED, E_INVALIDARG, E_UNEXPECTED, ERROR_FILE_NOT_FOUND,
    ERROR_INSUFFICIENT_BUFFER, ERROR_NO_UNICODE_TRANSLATION, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS,
    HMODULE, RPC_E_CHANGED_MODE, S_OK, WIN32_ERROR,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize,
};
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    GetModuleFileNameW, GetModuleHandleExW,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
    RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegDeleteTreeW, RegGetValueW,
    RegSetValueExW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::HKL;
use windows::Win32::UI::TextServices::{
    CLSID_TF_CategoryMgr, CLSID_TF_InputProcessorProfiles, GUID_TFCAT_TIP_KEYBOARD,
    GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT, ITfCategoryMgr, ITfInputProcessorProfileMgr,
    ITfInputProcessorProfiles, TF_INPUTPROCESSORPROFILE,
};
use windows::core::{GUID, HRESULT, IUnknown, Interface, PCWSTR};

use crate::{CLSID_PIME, PROFILE_EN, PROFILE_JA, PROFILE_ZH};

const COM_CLASS_NAME: &str = "Programming IME";
const THREADING_MODEL: &str = "Apartment";
/// Explicit `Software\Classes` (not HKCR): HKCR is a merged view and would hide which hive a write lands in.
const CLASSES_CLSID: &str = r"Software\Classes\CLSID";
const LANGID_ZH_CN: u16 = 0x0804;
const LANGID_EN_US: u16 = 0x0409;
const LANGID_JA_JP: u16 = 0x0411;
/// `GetModuleFileNameW` limit for extended-length paths, in UTF-16 units.
const MAX_PATH_UNITS: usize = 32768;

// ---------------------------------------------------------------------------------------------------------------
// The pure plan
// ---------------------------------------------------------------------------------------------------------------

/// Which registry hive receives the COM server keys. See the module documentation for what this does NOT change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// `HKLM\Software\Classes` (administrator; the product default).
    Machine,
    /// `HKCU\Software\Classes` (COM half only; unverified, see the module documentation).
    User,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Root {
    LocalMachine,
    CurrentUser,
}

impl Scope {
    fn root(self) -> Root {
        match self {
            Scope::Machine => Root::LocalMachine,
            Scope::User => Root::CurrentUser,
        }
    }
}

/// A registry key: a hive and a path below it (no leading backslash).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RegKey {
    pub root: Root,
    pub path: String,
}

impl RegKey {
    fn display(&self) -> String {
        let root = match self.root {
            Root::LocalMachine => "HKLM",
            Root::CurrentUser => "HKCU",
        };
        format!(r"{root}\{}", self.path)
    }
}

/// One REG_SZ write. An empty `name` is the key's default value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistryOp {
    pub key: RegKey,
    pub name: &'static str,
    pub value: String,
}

fn set_label(key: &RegKey, name: &str) -> String {
    let name = if name.is_empty() { "(default)" } else { name };
    format!("set {} [{name}]", key.display())
}

impl RegistryOp {
    fn label(&self) -> String {
        set_label(&self.key, self.name)
    }
}

/// One language profile of the text service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileSpec {
    pub langid: u16,
    pub guid: GUID,
    /// The name shown in the Windows keyboard list.
    pub description: &'static str,
    /// Empty: no icon (Windows shows its default keyboard icon). The DLL carries no icon resource yet; pointing at
    /// the DLL with an index would only add a way to get a blank icon.
    pub icon_file: String,
    pub icon_index: u32,
    /// `bEnabledByDefault`: enabled for users who add this language. It does not make the profile the default input
    /// method and does not add the language to anyone's language list. SampleIME passes TRUE too.
    pub enabled_by_default: bool,
    /// `TF_RP_*` flags. 0: visible in Settings, not process- or thread-local.
    pub flags: u32,
}

impl ProfileSpec {
    fn label(&self, verb: &str) -> String {
        format!("{verb} profile {:#06x}", self.langid)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CategorySpec {
    pub name: &'static str,
    pub guid: GUID,
}

impl CategorySpec {
    fn label(&self, verb: &str) -> String {
        format!("{verb} category {}", self.name)
    }
}

/// Everything the registration changes, in the order it is applied. The same text service is always [`CLSID_PIME`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub scope: Scope,
    /// The root of the COM server subtree; removing it removes the whole COM registration.
    pub clsid_key: RegKey,
    /// Empty in a [`removal_plan`].
    pub registry: Vec<RegistryOp>,
    pub profiles: Vec<ProfileSpec>,
    pub categories: Vec<CategorySpec>,
}

impl Plan {
    /// Number of forward steps of [`register_with`]: one per registry write, profile and category.
    pub fn step_count(&self) -> usize {
        self.registry.len() + self.profiles.len() + self.categories.len()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanError {
    Empty,
    /// Not `X:\...` or `\\server\...` / `\\?\...`.
    NotAbsolute,
    /// A double quote or a control character (including NUL): the value would be ambiguous or truncated.
    InvalidCharacter,
}

/// `{43F95ED1-95B4-421D-8F5A-837128C2D854}`: the registry spelling of a GUID (`{:?}` prints lowercase without braces).
pub fn guid_braced(guid: &GUID) -> String {
    format!("{{{guid:?}}}").to_uppercase()
}

fn validate_dll_path(path: &str) -> Result<(), PlanError> {
    if path.is_empty() {
        return Err(PlanError::Empty);
    }
    if path.chars().any(|c| c == '"' || c.is_control()) {
        return Err(PlanError::InvalidCharacter);
    }
    let bytes = path.as_bytes();
    let drive =
        bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\';
    let unc = bytes.len() > 2 && bytes.starts_with(br"\\");
    if drive || unc {
        Ok(())
    } else {
        Err(PlanError::NotAbsolute)
    }
}

fn clsid_key(scope: Scope) -> RegKey {
    RegKey {
        root: scope.root(),
        path: format!(r"{CLASSES_CLSID}\{}", guid_braced(&CLSID_PIME)),
    }
}

fn product_profiles() -> Vec<ProfileSpec> {
    [
        (LANGID_ZH_CN, PROFILE_ZH, "编程输入法 (Programming IME)"),
        (LANGID_EN_US, PROFILE_EN, "Programming IME"),
        (
            LANGID_JA_JP,
            PROFILE_JA,
            "プログラミングIME (Programming IME)",
        ),
    ]
    .into_iter()
    .map(|(langid, guid, description)| ProfileSpec {
        langid,
        guid,
        description,
        icon_file: String::new(),
        icon_index: 0,
        enabled_by_default: true,
        flags: 0,
    })
    .collect()
}

/// The categories this text service registers.
///
/// Registered:
/// - `TIP_KEYBOARD`: required, it makes the service a keyboard TIP at all.
/// - `TIPCAP_SYSTRAYSUPPORT`: lets it be shown with the other input methods in the notification area (Windows 8+).
///
/// Deliberately NOT registered (each one is a promise to the host; add it together with its implementation):
/// - `TIPCAP_UIELEMENTENABLED`: promises that the candidate list is shown through the TSF UI-element protocol
///   (`ITfUIElementMgr` / `ITfCandidateListUIElement`) so that hosts which draw their own candidate UI can take over.
///   The `tip` module draws its own window (`popup`) and implements no UI element yet, so claiming it would be false.
/// - `TIPCAP_IMMERSIVESUPPORT`: without it Windows does not offer the TIP in Store (immersive) apps and Start search;
///   with it the TIP must work there, and immersive hosts only show a TIP's candidates through the UI-element protocol
///   above. Until that exists the service is simply not offered in those hosts (typing there keeps using the other
///   input methods), which is safer than being offered and showing nothing.
/// - `TIPCAP_SECUREMODE`: Windows activates only TIPs that declare it on the secure desktop (logon screen, UAC
///   prompts, credential UI), and Microsoft requires `ITfTextInputProcessorEx` for it. Declaring it would load this
///   unreviewed DLL into those processes; leaving it out is the safe default until the secure-input path has been
///   reviewed (IT-005). It does not decide what happens in ordinary password boxes: there the TIP is loaded and must
///   itself stay silent.
/// - `DISPLAYATTRIBUTEPROVIDER`, `TIPCAP_COMLESS`, `TIPCAP_INPUTMODECOMPARTMENT`: the `tip` module implements none of
///   them yet; add each one together with its implementation.
pub fn product_categories() -> Vec<CategorySpec> {
    vec![
        CategorySpec {
            name: "TIP_KEYBOARD",
            guid: GUID_TFCAT_TIP_KEYBOARD,
        },
        CategorySpec {
            name: "TIPCAP_SYSTRAYSUPPORT",
            guid: GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT,
        },
    ]
}

/// The plan for installing the text service whose DLL is at `dll_path` (an absolute path, any characters except `"`
/// and control characters; spaces and non-ASCII are fine because the value is stored verbatim).
pub fn registry_plan(dll_path: &str, scope: Scope) -> Result<Plan, PlanError> {
    validate_dll_path(dll_path)?;
    let class_key = clsid_key(scope);
    let server_key = RegKey {
        root: class_key.root,
        path: format!(r"{}\InprocServer32", class_key.path),
    };
    let op = |key: &RegKey, name: &'static str, value: &str| RegistryOp {
        key: key.clone(),
        name,
        value: value.to_string(),
    };
    Ok(Plan {
        scope,
        registry: vec![
            op(&class_key, "", COM_CLASS_NAME),
            op(&server_key, "", dll_path),
            op(&server_key, "ThreadingModel", THREADING_MODEL),
        ],
        clsid_key: class_key,
        profiles: product_profiles(),
        categories: product_categories(),
    })
}

/// The plan for uninstalling: needs no DLL path (`DllUnregisterServer` must work even if the module path cannot be
/// read), and lists nothing to write.
pub fn removal_plan(scope: Scope) -> Plan {
    Plan {
        scope,
        clsid_key: clsid_key(scope),
        registry: Vec::new(),
        profiles: product_profiles(),
        categories: product_categories(),
    }
}

/// True for the COM subtree of our own CLSID and nothing else. [`RealBackend`] refuses every other key, so that no
/// bug in a plan can delete or overwrite another component's registration (UT-061).
pub fn is_own_key(key: &RegKey) -> bool {
    let own = format!(r"{CLASSES_CLSID}\{}", guid_braced(&CLSID_PIME));
    let path = key.path.as_bytes();
    let own = own.as_bytes();
    path.eq_ignore_ascii_case(own)
        || (path.len() > own.len()
            && path[own.len()] == b'\\'
            && path[..own.len()].eq_ignore_ascii_case(own))
}

// ---------------------------------------------------------------------------------------------------------------
// The backend contract and the transactional drivers
// ---------------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendError {
    /// The item to remove is not there. Removal treats this as success.
    NotFound,
    Failed(HRESULT),
}

impl BackendError {
    pub fn code(self) -> HRESULT {
        match self {
            BackendError::NotFound => HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0),
            BackendError::Failed(code) => code,
        }
    }
}

/// Maps the "does not exist" HRESULTs (the Win32 file/path-not-found errors) to [`BackendError::NotFound`]. The code
/// real TSF returns for an absent profile or category is NOT verified (it is E_FAIL in the documentation), so absent
/// TSF items may surface as `Failed`; the VM runbook records what happens on a second unregister.
pub fn classify(code: HRESULT) -> BackendError {
    if code == HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0)
        || code == HRESULT::from_win32(ERROR_PATH_NOT_FOUND.0)
    {
        BackendError::NotFound
    } else {
        BackendError::Failed(code)
    }
}

type Step = Result<(), BackendError>;

/// Single steps of a registration. Mutating methods act on our own text service only ([`CLSID_PIME`]).
/// Contract: removing something absent is `Err(NotFound)` or `Ok`, never anything else; there is deliberately no
/// method that activates a profile, changes the default input method or reorders the user's list.
pub trait Backend {
    /// Current REG_SZ value, `None` when the key or value is absent or unreadable.
    fn read_string(&self, key: &RegKey, name: &str) -> Option<String>;
    /// Whether the profile is registered. Unreadable counts as absent.
    fn profile_registered(&mut self, profile: &ProfileSpec) -> bool;
    /// Whether the category is registered for our CLSID. Unreadable counts as absent.
    fn category_registered(&mut self, category: &CategorySpec) -> bool;

    /// Creates the key if needed and writes one REG_SZ value.
    fn set_string(&mut self, key: &RegKey, name: &str, value: &str) -> Step;
    fn delete_value(&mut self, key: &RegKey, name: &str) -> Step;
    /// Deletes the key and everything below it.
    fn delete_tree(&mut self, key: &RegKey) -> Step;
    fn register_profile(&mut self, profile: &ProfileSpec) -> Step;
    fn unregister_profile(&mut self, profile: &ProfileSpec) -> Step;
    fn register_category(&mut self, category: &CategorySpec) -> Step;
    fn unregister_category(&mut self, category: &CategorySpec) -> Step;
    /// Removes the text service entry itself (`ITfInputProcessorProfiles::Unregister`), which can outlive its profiles.
    fn unregister_text_service(&mut self) -> Step;
}

/// A step that failed, by name (no values, no paths: safe to log) and the code it returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub step: String,
    pub code: HRESULT,
}

impl Failure {
    fn new(step: String, error: BackendError) -> Self {
        Failure {
            step,
            code: error.code(),
        }
    }

    fn describe(&self) -> String {
        format!("{} failed: {:#010X}", self.step, self.code.0 as u32)
    }
}

/// [`register_with`] failed at `failure`; everything it had changed was rolled back, except what is listed in
/// `rollback_failures` (steps of the rollback that failed too).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisterError {
    pub failure: Failure,
    pub rollback_failures: Vec<Failure>,
}

impl RegisterError {
    fn describe(&self) -> String {
        let mut text = self.failure.describe();
        for extra in &self.rollback_failures {
            text.push_str("; rollback: ");
            text.push_str(&extra.describe());
        }
        text
    }
}

/// What existed before the run, so that a rollback restores the previous state instead of deleting a working install.
struct Before {
    registry: Vec<Option<String>>,
    profiles: Vec<bool>,
    categories: Vec<bool>,
}

/// How far the forward steps got; the failing step counts as done because it may have been partly applied.
#[derive(Default)]
struct Done {
    registry: usize,
    profiles: usize,
    categories: usize,
}

/// Applies `plan`: registry writes, then profiles, then categories (the SampleIME order). Idempotent. If a step fails,
/// everything this run changed is undone: registry values go back to what they were (the whole COM subtree is removed
/// if none of its values existed), profiles and categories that did not exist before are unregistered. A profile that
/// existed before is left as the run left it, because its old description cannot be read back.
pub fn register_with(backend: &mut dyn Backend, plan: &Plan) -> Result<(), RegisterError> {
    let before = Before {
        registry: plan
            .registry
            .iter()
            .map(|op| backend.read_string(&op.key, op.name))
            .collect(),
        profiles: plan
            .profiles
            .iter()
            .map(|p| backend.profile_registered(p))
            .collect(),
        categories: plan
            .categories
            .iter()
            .map(|c| backend.category_registered(c))
            .collect(),
    };
    let mut done = Done::default();
    match apply(backend, plan, &mut done) {
        Ok(()) => Ok(()),
        Err(failure) => Err(RegisterError {
            failure,
            rollback_failures: rollback(backend, plan, &before, &done),
        }),
    }
}

fn apply(backend: &mut dyn Backend, plan: &Plan, done: &mut Done) -> Result<(), Failure> {
    for op in &plan.registry {
        done.registry += 1;
        backend
            .set_string(&op.key, op.name, &op.value)
            .map_err(|e| Failure::new(op.label(), e))?;
    }
    for profile in &plan.profiles {
        done.profiles += 1;
        backend
            .register_profile(profile)
            .map_err(|e| Failure::new(profile.label("register"), e))?;
    }
    for category in &plan.categories {
        done.categories += 1;
        backend
            .register_category(category)
            .map_err(|e| Failure::new(category.label("register"), e))?;
    }
    Ok(())
}

/// Removal is best effort: a missing item is fine, a real error is recorded and the remaining steps still run.
fn note(failures: &mut Vec<Failure>, step: String, result: Step) {
    if let Err(error) = result
        && error != BackendError::NotFound
    {
        failures.push(Failure::new(step, error));
    }
}

fn rollback(backend: &mut dyn Backend, plan: &Plan, before: &Before, done: &Done) -> Vec<Failure> {
    let mut failures = Vec::new();
    for i in (0..done.categories).rev() {
        if !before.categories[i] {
            let category = &plan.categories[i];
            note(
                &mut failures,
                category.label("unregister"),
                backend.unregister_category(category),
            );
        }
    }
    for i in (0..done.profiles).rev() {
        if !before.profiles[i] {
            let profile = &plan.profiles[i];
            note(
                &mut failures,
                profile.label("unregister"),
                backend.unregister_profile(profile),
            );
        }
    }
    let tsf_touched = done.profiles + done.categories > 0;
    let tsf_existed = before.profiles.iter().chain(&before.categories).any(|&b| b);
    if tsf_touched && !tsf_existed {
        note(
            &mut failures,
            "unregister text service".to_string(),
            backend.unregister_text_service(),
        );
    }
    if done.registry > 0 {
        if before.registry.iter().all(Option::is_none) {
            note(
                &mut failures,
                format!("delete {}", plan.clsid_key.display()),
                backend.delete_tree(&plan.clsid_key),
            );
        } else {
            for i in (0..done.registry).rev() {
                let op = &plan.registry[i];
                let result = match &before.registry[i] {
                    Some(previous) => backend.set_string(&op.key, op.name, previous),
                    None => backend.delete_value(&op.key, op.name),
                };
                note(&mut failures, format!("restore {}", op.label()), result);
            }
        }
    }
    failures
}

/// Removes everything `plan` lists, in reverse order (categories, profiles, the text service entry, the COM subtree).
/// Tolerates missing parts, keeps going after an error, and returns the first real error. Registry writes in `plan`
/// are ignored: only `clsid_key`, `profiles` and `categories` matter, so a [`removal_plan`] is enough.
pub fn unregister_with(backend: &mut dyn Backend, plan: &Plan) -> Result<(), Failure> {
    let mut failures = Vec::new();
    for category in plan.categories.iter().rev() {
        note(
            &mut failures,
            category.label("unregister"),
            backend.unregister_category(category),
        );
    }
    for profile in plan.profiles.iter().rev() {
        note(
            &mut failures,
            profile.label("unregister"),
            backend.unregister_profile(profile),
        );
    }
    note(
        &mut failures,
        "unregister text service".to_string(),
        backend.unregister_text_service(),
    );
    note(
        &mut failures,
        format!("delete {}", plan.clsid_key.display()),
        backend.delete_tree(&plan.clsid_key),
    );
    failures.into_iter().next().map_or(Ok(()), Err)
}

// ---------------------------------------------------------------------------------------------------------------
// The real backend (Windows registry and TSF). Never used by tests.
// ---------------------------------------------------------------------------------------------------------------

/// Keeps COM initialised for the lifetime of the backend. Declared last in [`RealBackend`] so that the interface
/// pointers are released before `CoUninitialize` runs.
struct ComApartment {
    initialised: bool,
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.initialised {
            // SAFETY: balances the successful CoInitializeEx of `RealBackend::open` on this same thread.
            unsafe { CoUninitialize() };
        }
    }
}

/// The real registry and the real TSF registration objects. It writes the machine registry: only construct it from
/// `DllRegisterServer` / `DllUnregisterServer`, inside an elevated, disposable VM until the milestones in
/// docs/RUST_MIGRATION_PLAN.md 13.7 say otherwise.
pub struct RealBackend {
    profile_mgr: Option<ITfInputProcessorProfileMgr>,
    category_mgr: Option<ITfCategoryMgr>,
    _com: ComApartment,
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn root_hkey(root: Root) -> HKEY {
    match root {
        Root::LocalMachine => HKEY_LOCAL_MACHINE,
        Root::CurrentUser => HKEY_CURRENT_USER,
    }
}

fn win32(error: WIN32_ERROR) -> Step {
    if error == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(classify(HRESULT::from_win32(error.0)))
    }
}

fn from_error(error: windows::core::Error) -> BackendError {
    classify(error.code())
}

fn create_instance<T: Interface>(clsid: &GUID) -> Result<T, BackendError> {
    // SAFETY: COM is initialised on this thread by `RealBackend::open`; `clsid` is a valid reference and there is no
    // outer (aggregating) object.
    unsafe { CoCreateInstance(clsid, None::<&IUnknown>, CLSCTX_INPROC_SERVER) }.map_err(from_error)
}

impl RealBackend {
    pub fn open() -> Result<Self, HRESULT> {
        // Hard stop for the unit tests: they must never reach the real registry or TSF, whatever calls this.
        if cfg!(test) {
            return Err(E_ACCESSDENIED);
        }
        // SAFETY: plain COM initialisation of the calling thread; the matching CoUninitialize is in `ComApartment`.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let initialised = if hr.is_ok() {
            true // S_OK and S_FALSE (already initialised, same model) both need a CoUninitialize
        } else if hr == RPC_E_CHANGED_MODE {
            false // the host thread is a multithreaded apartment: use it as it is, and do not uninitialise
        } else {
            return Err(hr);
        };
        Ok(RealBackend {
            profile_mgr: None,
            category_mgr: None,
            _com: ComApartment { initialised },
        })
    }

    fn profile_mgr(&mut self) -> Result<&ITfInputProcessorProfileMgr, BackendError> {
        if self.profile_mgr.is_none() {
            self.profile_mgr = Some(create_instance(&CLSID_TF_InputProcessorProfiles)?);
        }
        self.profile_mgr
            .as_ref()
            .ok_or(BackendError::Failed(E_UNEXPECTED))
    }

    fn category_mgr(&mut self) -> Result<&ITfCategoryMgr, BackendError> {
        if self.category_mgr.is_none() {
            self.category_mgr = Some(create_instance(&CLSID_TF_CategoryMgr)?);
        }
        self.category_mgr
            .as_ref()
            .ok_or(BackendError::Failed(E_UNEXPECTED))
    }
}

fn guard_own(key: &RegKey) -> Step {
    if is_own_key(key) {
        Ok(())
    } else {
        Err(BackendError::Failed(E_INVALIDARG))
    }
}

impl Backend for RealBackend {
    fn read_string(&self, key: &RegKey, name: &str) -> Option<String> {
        let subkey = wide(&key.path);
        let value = wide(name);
        let mut bytes = 0u32;
        // SAFETY: both pointers are NUL-terminated buffers that outlive the call; no data pointer is passed, so the
        // call only reports the size of the value.
        let size = unsafe {
            RegGetValueW(
                root_hkey(key.root),
                PCWSTR(subkey.as_ptr()),
                PCWSTR(value.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                None,
                Some(&mut bytes),
            )
        };
        if size != ERROR_SUCCESS {
            return None;
        }
        let mut buffer = vec![0u16; (bytes as usize).div_ceil(2)];
        let mut capacity = (buffer.len() * 2) as u32;
        // SAFETY: as above; `buffer` is writable for `capacity` bytes.
        let read = unsafe {
            RegGetValueW(
                root_hkey(key.root),
                PCWSTR(subkey.as_ptr()),
                PCWSTR(value.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut capacity),
            )
        };
        if read != ERROR_SUCCESS {
            return None;
        }
        let units = (capacity as usize / 2).min(buffer.len());
        let text = &buffer[..units];
        let end = text.iter().position(|&u| u == 0).unwrap_or(text.len());
        String::from_utf16(&text[..end]).ok()
    }

    fn profile_registered(&mut self, profile: &ProfileSpec) -> bool {
        let Ok(manager) = self.profile_mgr() else {
            return false;
        };
        // SAFETY: COM calls on a live interface; the buffer is a one-element local array and `fetched` a local.
        unsafe {
            let Ok(list) = manager.EnumProfiles(0) else {
                return false;
            };
            let mut item = [TF_INPUTPROCESSORPROFILE::default()];
            for _ in 0..4096 {
                let mut fetched = 0u32;
                if list.Next(&mut item, &mut fetched).is_err() || fetched == 0 {
                    return false;
                }
                let found = &item[0];
                if found.clsid == CLSID_PIME
                    && found.langid == profile.langid
                    && found.guidProfile == profile.guid
                {
                    return true;
                }
            }
        }
        false
    }

    fn category_registered(&mut self, category: &CategorySpec) -> bool {
        let Ok(manager) = self.category_mgr() else {
            return false;
        };
        // SAFETY: COM calls on a live interface; the buffer is a one-element local array and `fetched` a local.
        unsafe {
            let Ok(list) = manager.EnumCategoriesInItem(&CLSID_PIME) else {
                return false;
            };
            let mut item = [GUID::zeroed()];
            for _ in 0..4096 {
                let mut fetched = 0u32;
                if list.Next(&mut item, Some(&mut fetched)).is_err() || fetched == 0 {
                    return false;
                }
                if item[0] == category.guid {
                    return true;
                }
            }
        }
        false
    }

    fn set_string(&mut self, key: &RegKey, name: &str, value: &str) -> Step {
        guard_own(key)?;
        let subkey = wide(&key.path);
        let value_name = wide(name);
        let data: Vec<u8> = wide(value).iter().flat_map(|u| u.to_le_bytes()).collect();
        let mut handle = HKEY::default();
        // SAFETY: `subkey` is NUL-terminated and outlives the call; `handle` is a valid out pointer; no security
        // attributes, no class, no disposition requested.
        win32(unsafe {
            RegCreateKeyExW(
                root_hkey(key.root),
                PCWSTR(subkey.as_ptr()),
                0,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut handle,
                None,
            )
        })?;
        // SAFETY: `handle` was opened with KEY_SET_VALUE just above; `value_name` is NUL-terminated (an empty name
        // is the default value); `data` holds the UTF-16 text including its terminator.
        let written = win32(unsafe {
            RegSetValueExW(handle, PCWSTR(value_name.as_ptr()), 0, REG_SZ, Some(&data))
        });
        // SAFETY: closes the handle opened above, once. A close error cannot change what was written.
        let _ = unsafe { RegCloseKey(handle) };
        written
    }

    fn delete_value(&mut self, key: &RegKey, name: &str) -> Step {
        guard_own(key)?;
        let subkey = wide(&key.path);
        let value_name = wide(name);
        // SAFETY: both strings are NUL-terminated and outlive the call.
        win32(unsafe {
            RegDeleteKeyValueW(
                root_hkey(key.root),
                PCWSTR(subkey.as_ptr()),
                PCWSTR(value_name.as_ptr()),
            )
        })
    }

    fn delete_tree(&mut self, key: &RegKey) -> Step {
        guard_own(key)?;
        let subkey = wide(&key.path);
        // SAFETY: `subkey` is NUL-terminated and outlives the call; `guard_own` limited it to our CLSID subtree.
        win32(unsafe { RegDeleteTreeW(root_hkey(key.root), PCWSTR(subkey.as_ptr())) })
    }

    fn register_profile(&mut self, profile: &ProfileSpec) -> Step {
        let description: Vec<u16> = profile.description.encode_utf16().collect();
        let icon: Vec<u16> = profile.icon_file.encode_utf16().collect();
        let manager = self.profile_mgr()?;
        // SAFETY: the GUID references and both slices live for the call; the slices carry their length (no NUL
        // needed); no substitute keyboard layout and the reserved preferred-layout argument is 0.
        unsafe {
            manager.RegisterProfile(
                &CLSID_PIME,
                profile.langid,
                &profile.guid,
                &description,
                &icon,
                profile.icon_index,
                HKL::default(),
                0,
                BOOL::from(profile.enabled_by_default),
                profile.flags,
            )
        }
        .map_err(from_error)
    }

    fn unregister_profile(&mut self, profile: &ProfileSpec) -> Step {
        let manager = self.profile_mgr()?;
        // SAFETY: the GUID references live for the call.
        unsafe { manager.UnregisterProfile(&CLSID_PIME, profile.langid, &profile.guid, 0) }
            .map_err(from_error)
    }

    fn register_category(&mut self, category: &CategorySpec) -> Step {
        let manager = self.category_mgr()?;
        // SAFETY: the GUID references live for the call. The item GUID is the CLSID, as in SampleIME and Mozc.
        unsafe { manager.RegisterCategory(&CLSID_PIME, &category.guid, &CLSID_PIME) }
            .map_err(from_error)
    }

    fn unregister_category(&mut self, category: &CategorySpec) -> Step {
        let manager = self.category_mgr()?;
        // SAFETY: the GUID references live for the call.
        unsafe { manager.UnregisterCategory(&CLSID_PIME, &category.guid, &CLSID_PIME) }
            .map_err(from_error)
    }

    fn unregister_text_service(&mut self) -> Step {
        let profiles: ITfInputProcessorProfiles = self.profile_mgr()?.cast().map_err(from_error)?;
        // SAFETY: the GUID reference lives for the call.
        unsafe { profiles.Unregister(&CLSID_PIME) }.map_err(from_error)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The exported entry points
// ---------------------------------------------------------------------------------------------------------------

/// Debugger output (DebugView): the failing step and its code. No registry values, no paths, no user data.
fn trace(message: &str) {
    // `Win32_System_Diagnostics_Debug` is not an enabled feature of the pinned `windows` crate, so the one function is
    // declared here; kernel32 is a system DLL like all other imports.
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OutputDebugStringW(text: *const u16);
    }
    let text = wide(&format!("pime-tsf register: {message}\n"));
    // SAFETY: `text` is a NUL-terminated UTF-16 buffer that outlives the call.
    unsafe { OutputDebugStringW(text.as_ptr()) };
}

/// Full path of this DLL (not of the host process): the module that contains this very function.
fn own_dll_path() -> Result<String, HRESULT> {
    let mut module = HMODULE::default();
    // SAFETY: FROM_ADDRESS resolves the module containing the address and UNCHANGED_REFCOUNT leaves its reference
    // count alone; the address is a function of this module, so it is mapped; `module` is a valid out pointer.
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(own_dll_path as *const () as *const u16),
            &mut module,
        )
    }
    .map_err(|e| e.code())?;
    let mut buffer = vec![0u16; 512];
    loop {
        // SAFETY: `module` is the handle of this DLL; the slice is writable for its whole length.
        let length = unsafe { GetModuleFileNameW(module, &mut buffer) } as usize;
        if length == 0 {
            return Err(windows::core::Error::from_win32().code());
        }
        if length < buffer.len() {
            // a length equal to the buffer size means the path was truncated
            return String::from_utf16(&buffer[..length])
                .map_err(|_| HRESULT::from_win32(ERROR_NO_UNICODE_TRANSLATION.0));
        }
        if buffer.len() >= MAX_PATH_UNITS {
            return Err(HRESULT::from_win32(ERROR_INSUFFICIENT_BUFFER.0));
        }
        buffer.resize(buffer.len() * 2, 0);
    }
}

/// Runs an entry point body and turns a panic into an error code. In the release profile (`panic = "abort"`) a panic
/// still ends the process, so the code below avoids panicking paths instead of relying on this.
fn guarded(job: impl FnOnce() -> Result<(), HRESULT>) -> HRESULT {
    match catch_unwind(AssertUnwindSafe(job)) {
        Ok(Ok(())) => S_OK,
        Ok(Err(code)) if code.is_err() => code,
        Ok(Err(_)) => E_UNEXPECTED,
        Err(_) => E_UNEXPECTED,
    }
}

pub(crate) fn register_server() -> HRESULT {
    guarded(|| {
        let path = own_dll_path()?;
        let plan = registry_plan(&path, Scope::Machine).map_err(|error| {
            trace(&format!("plan rejected: {error:?}"));
            E_INVALIDARG
        })?;
        let mut backend = RealBackend::open()?;
        register_with(&mut backend, &plan).map_err(|error| {
            trace(&error.describe());
            error.failure.code
        })
    })
}

pub(crate) fn unregister_server() -> HRESULT {
    guarded(|| {
        let mut backend = RealBackend::open()?;
        unregister_with(&mut backend, &removal_plan(Scope::Machine)).map_err(|failure| {
            trace(&failure.describe());
            failure.code
        })
    })
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use windows::Win32::Foundation::E_FAIL;
    use windows::Win32::UI::TextServices::{
        GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT, GUID_TFCAT_TIPCAP_SECUREMODE,
        GUID_TFCAT_TIPCAP_UIELEMENTENABLED,
    };

    use super::*;

    const DLL: &str = r"C:\Program Files\Programming IME\pime_tsf.dll";
    const CLSID_TEXT: &str = "{43F95ED1-95B4-421D-8F5A-837128C2D854}";

    /// Windows' own and other vendors' registrations that must survive everything we do.
    const OTHER_CLSID: GUID = GUID::from_u128(0x81d4e9c9_1d3b_41bc_9e6c_4b40bf79e35e);
    const OTHER_PROFILE: GUID = GUID::from_u128(0x82590c13_f4dd_44f4_ba1d_8667246fdf8e);
    const OTHER_CATEGORY: GUID = GUID::from_u128(0x13a016df_560b_46cd_947a_4c3af1e0e35d);

    #[derive(Default, Clone, PartialEq, Eq, Debug)]
    struct FakeState {
        keys: BTreeSet<RegKey>,
        values: BTreeMap<(RegKey, String), String>,
        /// (clsid, langid, profile guid) -> description
        profiles: BTreeMap<(u128, u16, u128), String>,
        /// (clsid, category guid)
        categories: BTreeSet<(u128, u128)>,
        /// clsids with a text service entry (created by registering a profile or category)
        text_services: BTreeSet<u128>,
    }

    /// In-memory registry + TSF. Counts every mutating call (reads do not count) and can fail the call with a given
    /// index, either before it has any effect or after it has been applied (a partly applied step).
    #[derive(Default)]
    struct FakeBackend {
        state: FakeState,
        calls: usize,
        log: Vec<String>,
        inject: BTreeMap<usize, (BackendError, bool)>,
        /// Removing something absent answers `NotFound` (like a strict real backend) instead of `Ok`.
        strict_missing: bool,
    }

    impl FakeBackend {
        /// A machine that already has another input method installed.
        fn with_other_ime() -> Self {
            let mut fake = FakeBackend::default();
            let key = RegKey {
                root: Root::LocalMachine,
                path: format!(r"{CLASSES_CLSID}\{}", guid_braced(&OTHER_CLSID)),
            };
            fake.state.keys.insert(key.clone());
            fake.state
                .values
                .insert((key, String::new()), "Other IME".to_string());
            fake.state.profiles.insert(
                (OTHER_CLSID.to_u128(), LANGID_ZH_CN, OTHER_PROFILE.to_u128()),
                "Other IME".to_string(),
            );
            fake.state
                .categories
                .insert((OTHER_CLSID.to_u128(), OTHER_CATEGORY.to_u128()));
            fake.state.text_services.insert(OTHER_CLSID.to_u128());
            fake
        }

        fn fail_at(mut self, index: usize, error: BackendError, applied: bool) -> Self {
            self.inject.insert(index, (error, applied));
            self
        }

        fn run(
            &mut self,
            label: String,
            removal: bool,
            apply: impl FnOnce(&mut FakeState) -> bool,
        ) -> Step {
            let index = self.calls;
            self.calls += 1;
            self.log.push(label);
            if let Some((error, applied)) = self.inject.remove(&index) {
                if applied {
                    apply(&mut self.state);
                }
                return Err(error);
            }
            let found = apply(&mut self.state);
            if removal && !found && self.strict_missing {
                return Err(BackendError::NotFound);
            }
            Ok(())
        }

        fn own_profiles(&self) -> usize {
            self.state
                .profiles
                .keys()
                .filter(|k| k.0 == CLSID_PIME.to_u128())
                .count()
        }
    }

    fn under(key: &RegKey, root: &RegKey) -> bool {
        key.root == root.root
            && (key.path == root.path || key.path.starts_with(&format!(r"{}\", root.path)))
    }

    impl Backend for FakeBackend {
        fn read_string(&self, key: &RegKey, name: &str) -> Option<String> {
            self.state
                .values
                .get(&(key.clone(), name.to_string()))
                .cloned()
        }

        fn profile_registered(&mut self, p: &ProfileSpec) -> bool {
            self.state
                .profiles
                .contains_key(&(CLSID_PIME.to_u128(), p.langid, p.guid.to_u128()))
        }

        fn category_registered(&mut self, c: &CategorySpec) -> bool {
            self.state
                .categories
                .contains(&(CLSID_PIME.to_u128(), c.guid.to_u128()))
        }

        fn set_string(&mut self, key: &RegKey, name: &str, value: &str) -> Step {
            self.run(set_label(key, name), false, |s| {
                s.keys.insert(key.clone());
                s.values
                    .insert((key.clone(), name.to_string()), value.to_string());
                true
            })
        }

        fn delete_value(&mut self, key: &RegKey, name: &str) -> Step {
            self.run(
                format!("delete value {} [{name}]", key.display()),
                true,
                |s| s.values.remove(&(key.clone(), name.to_string())).is_some(),
            )
        }

        fn delete_tree(&mut self, key: &RegKey) -> Step {
            self.run(format!("delete tree {}", key.display()), true, |s| {
                let had = s.keys.iter().any(|k| under(k, key));
                s.keys.retain(|k| !under(k, key));
                s.values.retain(|(k, _), _| !under(k, key));
                had
            })
        }

        fn register_profile(&mut self, p: &ProfileSpec) -> Step {
            self.run(p.label("register"), false, |s| {
                s.profiles.insert(
                    (CLSID_PIME.to_u128(), p.langid, p.guid.to_u128()),
                    p.description.to_string(),
                );
                s.text_services.insert(CLSID_PIME.to_u128());
                true
            })
        }

        fn unregister_profile(&mut self, p: &ProfileSpec) -> Step {
            self.run(p.label("unregister"), true, |s| {
                s.profiles
                    .remove(&(CLSID_PIME.to_u128(), p.langid, p.guid.to_u128()))
                    .is_some()
            })
        }

        fn register_category(&mut self, c: &CategorySpec) -> Step {
            self.run(c.label("register"), false, |s| {
                s.categories
                    .insert((CLSID_PIME.to_u128(), c.guid.to_u128()));
                s.text_services.insert(CLSID_PIME.to_u128());
                true
            })
        }

        fn unregister_category(&mut self, c: &CategorySpec) -> Step {
            self.run(c.label("unregister"), true, |s| {
                s.categories
                    .remove(&(CLSID_PIME.to_u128(), c.guid.to_u128()))
            })
        }

        fn unregister_text_service(&mut self) -> Step {
            self.run("unregister text service".to_string(), true, |s| {
                s.text_services.remove(&CLSID_PIME.to_u128())
            })
        }
    }

    fn plan() -> Plan {
        registry_plan(DLL, Scope::Machine).expect("the test path is valid")
    }

    fn installed(mut fake: FakeBackend) -> FakeBackend {
        register_with(&mut fake, &plan()).expect("registration on a fake succeeds");
        fake.calls = 0;
        fake.log.clear();
        fake
    }

    fn failed() -> BackendError {
        BackendError::Failed(E_FAIL)
    }

    // ---- the plan ----------------------------------------------------------------------------------------

    #[test]
    fn the_registry_spelling_of_a_guid_is_uppercase_with_braces() {
        assert_eq!(guid_braced(&CLSID_PIME), CLSID_TEXT);
    }

    #[test]
    fn machine_plan_writes_exactly_the_com_server_keys_under_hklm_classes() {
        let plan = plan();
        let class = format!(r"Software\Classes\CLSID\{CLSID_TEXT}");
        let server = format!(r"{class}\InprocServer32");
        let hklm = |path: &str| RegKey {
            root: Root::LocalMachine,
            path: path.to_string(),
        };
        let expected = vec![
            RegistryOp {
                key: hklm(&class),
                name: "",
                value: "Programming IME".to_string(),
            },
            RegistryOp {
                key: hklm(&server),
                name: "",
                value: DLL.to_string(),
            },
            RegistryOp {
                key: hklm(&server),
                name: "ThreadingModel",
                value: "Apartment".to_string(),
            },
        ];
        assert_eq!(plan.registry, expected);
        assert_eq!(plan.clsid_key, hklm(&class));
        assert_eq!(plan.scope, Scope::Machine);
        assert_eq!(plan.step_count(), 3 + 3 + 2);
    }

    #[test]
    fn user_plan_differs_from_the_machine_plan_only_in_the_hive() {
        let machine = plan();
        let user = registry_plan(DLL, Scope::User).unwrap();
        assert!(
            user.registry
                .iter()
                .all(|op| op.key.root == Root::CurrentUser)
        );
        assert_eq!(user.clsid_key.root, Root::CurrentUser);
        for (m, u) in machine.registry.iter().zip(&user.registry) {
            assert_eq!(
                (&m.key.path, m.name, &m.value),
                (&u.key.path, u.name, &u.value)
            );
        }
        assert_eq!(machine.profiles, user.profiles);
        assert_eq!(machine.categories, user.categories);
    }

    #[test]
    fn it_001_each_of_the_three_languages_has_exactly_one_own_profile() {
        let plan = plan();
        let langs: Vec<(u16, GUID)> = plan.profiles.iter().map(|p| (p.langid, p.guid)).collect();
        assert_eq!(
            langs,
            vec![
                (0x0804, PROFILE_ZH),
                (0x0409, PROFILE_EN),
                (0x0411, PROFILE_JA)
            ]
        );
        let distinct: BTreeSet<u128> = plan.profiles.iter().map(|p| p.guid.to_u128()).collect();
        assert_eq!(distinct.len(), 3, "three different profile GUIDs");
        assert!(plan.profiles.iter().all(|p| !p.description.is_empty()));
        let names: BTreeSet<&str> = plan.profiles.iter().map(|p| p.description).collect();
        assert_eq!(names.len(), 3, "the keyboard list can tell them apart");
    }

    #[test]
    fn profiles_are_enabled_for_users_but_never_hidden_or_process_local() {
        for p in plan().profiles {
            assert!(
                p.enabled_by_default,
                "bEnabledByDefault is the documented choice"
            );
            assert_eq!(
                p.flags, 0,
                "no TF_RP_HIDDENINSETTINGUI / LOCALPROCESS / LOCALTHREAD"
            );
            assert_eq!(p.icon_file, "", "no icon resource exists yet");
        }
    }

    #[test]
    fn categories_claim_only_what_the_tip_implements() {
        let guids: Vec<GUID> = plan().categories.iter().map(|c| c.guid).collect();
        assert_eq!(
            guids,
            vec![GUID_TFCAT_TIP_KEYBOARD, GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT]
        );
        // promises the tip cannot keep yet: no UI-element protocol (so no UIELEMENTENABLED / IMMERSIVESUPPORT) and no
        // reviewed secure-desktop path (so no SECUREMODE)
        for unsupported in [
            GUID_TFCAT_TIPCAP_UIELEMENTENABLED,
            GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT,
            GUID_TFCAT_TIPCAP_SECUREMODE,
        ] {
            assert!(!guids.contains(&unsupported));
        }
    }

    #[test]
    fn ut_061_every_registry_key_is_inside_the_own_clsid_subtree() {
        for scope in [Scope::Machine, Scope::User] {
            let plan = registry_plan(DLL, scope).unwrap();
            assert!(is_own_key(&plan.clsid_key));
            for op in &plan.registry {
                assert!(is_own_key(&op.key), "{}", op.key.display());
            }
        }
        let foreign = |path: &str| RegKey {
            root: Root::LocalMachine,
            path: path.to_string(),
        };
        for path in [
            r"Software\Classes\CLSID",
            r"Software\Classes\CLSID\{81D4E9C9-1D3B-41BC-9E6C-4B40BF79E35E}",
            r"Software\Microsoft\CTF\TIP",
            r"Control Panel\International\User Profile",
            r"Software\Classes\CLSID\{43F95ED1-95B4-421D-8F5A-837128C2D854}X",
        ] {
            assert!(!is_own_key(&foreign(path)), "{path}");
        }
        let child = foreign(&format!(
            r"Software\Classes\CLSID\{}\InprocServer32",
            CLSID_TEXT
        ));
        assert!(is_own_key(&child));
    }

    #[test]
    fn the_removal_plan_needs_no_dll_path_and_names_the_same_items() {
        let install = plan();
        let removal = removal_plan(Scope::Machine);
        assert!(removal.registry.is_empty());
        assert_eq!(removal.clsid_key, install.clsid_key);
        assert_eq!(removal.profiles, install.profiles);
        assert_eq!(removal.categories, install.categories);
    }

    // ---- the DLL path ------------------------------------------------------------------------------------

    #[test]
    fn a_path_with_spaces_and_non_ascii_characters_is_stored_verbatim_and_unquoted() {
        for path in [
            r"C:\Program Files\Programming IME\pime_tsf.dll",
            r"C:\Users\山田 太郎\アプリ\编程输入法\pime_tsf.dll",
            r"C:\100%\%PATH%\pime_tsf.dll",
            r"\\server\share name\pime_tsf.dll",
            r"\\?\C:\very long\pime_tsf.dll",
        ] {
            let plan = registry_plan(path, Scope::Machine).unwrap();
            let server = &plan.registry[1];
            assert_eq!(server.value, path);
            assert!(!server.value.contains('"'));
        }
    }

    #[test]
    fn the_path_reaches_the_fake_registry_unchanged() {
        let path = r"C:\Program Files\编程 IME\pime_tsf.dll";
        let plan = registry_plan(path, Scope::Machine).unwrap();
        let mut fake = FakeBackend::default();
        register_with(&mut fake, &plan).unwrap();
        assert_eq!(
            fake.read_string(&plan.registry[1].key, ""),
            Some(path.to_string())
        );
    }

    #[test]
    fn paths_that_would_be_ambiguous_or_relative_are_rejected() {
        let cases = [
            ("", PlanError::Empty),
            (r"pime_tsf.dll", PlanError::NotAbsolute),
            (r"..\pime_tsf.dll", PlanError::NotAbsolute),
            (r"\Windows\pime_tsf.dll", PlanError::NotAbsolute),
            (r"C:pime_tsf.dll", PlanError::NotAbsolute),
            ("C:\\a\"b\\pime_tsf.dll", PlanError::InvalidCharacter),
            ("\"C:\\a\\pime_tsf.dll\"", PlanError::InvalidCharacter),
            ("C:\\a\0b\\pime_tsf.dll", PlanError::InvalidCharacter),
            ("C:\\a\nb\\pime_tsf.dll", PlanError::InvalidCharacter),
        ];
        for (path, error) in cases {
            assert_eq!(registry_plan(path, Scope::Machine), Err(error), "{path:?}");
        }
    }

    #[test]
    fn own_dll_path_is_the_path_of_the_running_module() {
        // In the unit tests this crate is linked into the test executable, so "this module" is that executable.
        // Only reads module information: nothing is registered.
        let path = own_dll_path().expect("module path");
        let exe = std::env::current_exe().expect("current exe");
        assert!(std::path::Path::new(&path).is_absolute(), "{path}");
        assert_eq!(std::path::Path::new(&path).file_name(), exe.file_name());
        assert!(registry_plan(&path, Scope::Machine).is_ok());
    }

    // ---- register_with -----------------------------------------------------------------------------------

    #[test]
    fn registration_applies_registry_then_profiles_then_categories() {
        let mut fake = FakeBackend::default();
        register_with(&mut fake, &plan()).unwrap();
        let log: Vec<&str> = fake.log.iter().map(String::as_str).collect();
        assert_eq!(fake.calls, 8);
        assert_eq!(
            &log[..3],
            [
                "set HKLM\\Software\\Classes\\CLSID\\{43F95ED1-95B4-421D-8F5A-837128C2D854} [(default)]",
                "set HKLM\\Software\\Classes\\CLSID\\{43F95ED1-95B4-421D-8F5A-837128C2D854}\\InprocServer32 [(default)]",
                "set HKLM\\Software\\Classes\\CLSID\\{43F95ED1-95B4-421D-8F5A-837128C2D854}\\InprocServer32 [ThreadingModel]",
            ]
        );
        assert_eq!(
            &log[3..],
            [
                "register profile 0x0804",
                "register profile 0x0409",
                "register profile 0x0411",
                "register category TIP_KEYBOARD",
                "register category TIPCAP_SYSTRAYSUPPORT"
            ]
        );
        assert_eq!(fake.own_profiles(), 3);
        assert_eq!(fake.state.categories.len(), 2);
    }

    #[test]
    fn registering_twice_is_fine_and_changes_nothing() {
        let mut fake = FakeBackend::default();
        register_with(&mut fake, &plan()).unwrap();
        let after_first = fake.state.clone();
        register_with(&mut fake, &plan()).unwrap();
        assert_eq!(fake.state, after_first);
    }

    #[test]
    fn registering_again_with_a_moved_dll_updates_the_path_only() {
        let mut fake = installed(FakeBackend::default());
        let moved = r"D:\Tools\IME\pime_tsf.dll";
        let moved_plan = registry_plan(moved, Scope::Machine).unwrap();
        register_with(&mut fake, &moved_plan).unwrap();
        assert_eq!(
            fake.read_string(&moved_plan.registry[1].key, ""),
            Some(moved.to_string())
        );
        assert_eq!(fake.own_profiles(), 3);
        assert_eq!(fake.state.categories.len(), 2);
    }

    #[test]
    fn ut_002_registration_leaves_other_input_methods_untouched() {
        let other = FakeBackend::with_other_ime();
        let before = other.state.clone();
        let fake = installed(other);
        for key in &before.keys {
            assert!(fake.state.keys.contains(key));
        }
        // nothing outside our own subtree was written, and the other IME's items are all still there
        let foreign_keys: BTreeSet<RegKey> = fake
            .state
            .keys
            .iter()
            .filter(|k| !is_own_key(k))
            .cloned()
            .collect();
        assert_eq!(foreign_keys, before.keys);
        let foreign_values: BTreeMap<_, _> = fake
            .state
            .values
            .iter()
            .filter(|((k, _), _)| !is_own_key(k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        assert_eq!(foreign_values, before.values);
        let other_profile = (OTHER_CLSID.to_u128(), LANGID_ZH_CN, OTHER_PROFILE.to_u128());
        assert_eq!(fake.state.profiles[&other_profile], "Other IME");
        assert!(
            fake.state
                .categories
                .contains(&(OTHER_CLSID.to_u128(), OTHER_CATEGORY.to_u128()))
        );
        assert!(fake.state.text_services.contains(&OTHER_CLSID.to_u128()));
    }

    #[test]
    fn a_failure_at_every_step_rolls_everything_back_to_the_start() {
        let steps = plan().step_count();
        for applied in [false, true] {
            for index in 0..steps {
                let start = FakeBackend::with_other_ime();
                let initial = start.state.clone();
                let mut fake = start.fail_at(index, failed(), applied);
                let error = register_with(&mut fake, &plan()).expect_err("step must fail");
                assert_eq!(error.failure.code, E_FAIL, "step {index}");
                assert_eq!(error.failure.step, fake.log[index], "step {index}");
                assert!(
                    error.rollback_failures.is_empty(),
                    "step {index}: {:?}",
                    error.rollback_failures
                );
                assert_eq!(
                    fake.state, initial,
                    "step {index}, applied before failing: {applied}"
                );
            }
        }
    }

    #[test]
    fn a_failing_rerun_restores_the_previous_installation_exactly() {
        let steps = plan().step_count();
        let moved = registry_plan(r"D:\Tools\IME\pime_tsf.dll", Scope::Machine).unwrap();
        for applied in [false, true] {
            for index in 0..steps {
                let mut fake = installed(FakeBackend::with_other_ime());
                let installed_state = fake.state.clone();
                fake.inject.insert(index, (failed(), applied));
                let error = register_with(&mut fake, &moved).expect_err("step must fail");
                assert!(error.rollback_failures.is_empty(), "step {index}");
                assert_eq!(
                    fake.state, installed_state,
                    "step {index}, applied: {applied}"
                );
            }
        }
    }

    #[test]
    fn a_rerun_that_fails_before_any_change_keeps_a_working_install() {
        // the typical real failure: not elevated, so the very first registry write is refused
        let mut fake = installed(FakeBackend::default());
        let installed_state = fake.state.clone();
        fake.inject
            .insert(0, (BackendError::Failed(E_ACCESSDENIED), false));
        let error = register_with(&mut fake, &plan()).unwrap_err();
        assert_eq!(error.failure.code, E_ACCESSDENIED);
        assert_eq!(fake.state, installed_state);
    }

    #[test]
    fn a_failing_rollback_step_is_reported_but_the_original_error_is_returned() {
        // forward step 4 (second profile) fails after being applied; the rollback then starts with that profile's undo
        // at call 5, which fails too
        let mut fake = FakeBackend::default().fail_at(4, failed(), true).fail_at(
            5,
            BackendError::Failed(E_ACCESSDENIED),
            false,
        );
        let error = register_with(&mut fake, &plan()).unwrap_err();
        assert_eq!(error.failure.code, E_FAIL);
        assert_eq!(error.rollback_failures.len(), 1);
        assert_eq!(error.rollback_failures[0].code, E_ACCESSDENIED);
        assert_eq!(error.rollback_failures[0].step, "unregister profile 0x0409");
        // the rest of the rollback still ran: the COM keys and the first profile are gone
        assert!(fake.state.keys.is_empty());
        assert_eq!(
            fake.own_profiles(),
            1,
            "only the profile whose undo failed is left"
        );
    }

    #[test]
    fn register_error_text_names_steps_and_codes_but_no_values() {
        let mut fake =
            FakeBackend::default().fail_at(1, BackendError::Failed(E_ACCESSDENIED), false);
        let error = register_with(&mut fake, &plan()).unwrap_err();
        let text = error.describe();
        assert!(text.contains("InprocServer32"), "{text}");
        assert!(text.contains("0x80070005"), "{text}");
        assert!(!text.contains("pime_tsf.dll"), "{text}");
    }

    // ---- unregister_with ---------------------------------------------------------------------------------

    #[test]
    fn unregistering_a_full_installation_leaves_nothing_of_ours() {
        let mut fake = installed(FakeBackend::default());
        unregister_with(&mut fake, &removal_plan(Scope::Machine)).unwrap();
        assert_eq!(fake.state, FakeState::default());
    }

    #[test]
    fn it_001_and_ut_061_unregister_removes_only_own_guids_and_keeps_other_imes() {
        let other = FakeBackend::with_other_ime();
        let before = other.state.clone();
        let mut fake = installed(other);
        assert_eq!(fake.own_profiles(), 3);
        unregister_with(&mut fake, &removal_plan(Scope::Machine)).unwrap();
        assert_eq!(fake.own_profiles(), 0, "back to zero own profiles");
        assert_eq!(fake.state, before);
    }

    #[test]
    fn unregistering_an_empty_machine_is_fine_also_when_removals_report_not_found() {
        for strict in [false, true] {
            let mut fake = FakeBackend::with_other_ime();
            fake.strict_missing = strict;
            let before = fake.state.clone();
            unregister_with(&mut fake, &removal_plan(Scope::Machine)).unwrap();
            assert_eq!(fake.state, before);
        }
    }

    #[test]
    fn unregistering_partial_installations_removes_what_is_there() {
        let full = plan();
        // (how many forward steps had completed when the installation was interrupted)
        for done in 0..=full.step_count() {
            for strict in [false, true] {
                let mut fake = FakeBackend::with_other_ime().fail_at(done, failed(), false);
                let before = fake.state.clone();
                fake.strict_missing = strict;
                // an installation that was interrupted (or killed) without any rollback
                let _ = apply(&mut fake, &full, &mut Done::default());
                fake.inject.clear();
                unregister_with(&mut fake, &removal_plan(Scope::Machine)).unwrap();
                assert_eq!(
                    fake.state, before,
                    "interrupted after {done} steps, strict: {strict}"
                );
            }
        }
    }

    #[test]
    fn unregister_removes_each_item_itself_then_the_text_service_entry_then_the_com_subtree() {
        let mut fake = installed(FakeBackend::default());
        unregister_with(&mut fake, &removal_plan(Scope::Machine)).unwrap();
        let log: Vec<&str> = fake.log.iter().map(String::as_str).collect();
        assert_eq!(
            log,
            [
                "unregister category TIPCAP_SYSTRAYSUPPORT",
                "unregister category TIP_KEYBOARD",
                "unregister profile 0x0411",
                "unregister profile 0x0409",
                "unregister profile 0x0804",
                "unregister text service",
                "delete tree HKLM\\Software\\Classes\\CLSID\\{43F95ED1-95B4-421D-8F5A-837128C2D854}",
            ]
        );
    }

    #[test]
    fn unregister_keeps_going_after_an_error_and_reports_the_first_one() {
        // call 0 = the SYSTRAYSUPPORT category (removed first), call 3 = profile 0x0409; the first error wins
        let mut fake = installed(FakeBackend::with_other_ime())
            .fail_at(0, BackendError::Failed(E_ACCESSDENIED), false)
            .fail_at(3, failed(), false);
        let error = unregister_with(&mut fake, &removal_plan(Scope::Machine)).unwrap_err();
        assert_eq!(error.code, E_ACCESSDENIED);
        assert_eq!(error.step, "unregister category TIPCAP_SYSTRAYSUPPORT");
        // everything that could be removed was removed
        assert!(fake.state.keys.iter().all(|k| !is_own_key(k)));
        assert_eq!(
            fake.own_profiles(),
            1,
            "only the profile whose removal failed"
        );
        assert_eq!(
            fake.state.categories.len(),
            1 + 1,
            "other IME's plus the one that failed"
        );
        assert!(!fake.state.text_services.contains(&CLSID_PIME.to_u128()));
        assert!(fake.state.text_services.contains(&OTHER_CLSID.to_u128()));
    }

    // ---- error classification ----------------------------------------------------------------------------

    #[test]
    fn only_the_win32_not_found_codes_count_as_missing() {
        assert_eq!(
            classify(HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0)),
            BackendError::NotFound
        );
        assert_eq!(
            classify(HRESULT::from_win32(ERROR_PATH_NOT_FOUND.0)),
            BackendError::NotFound
        );
        assert_eq!(
            classify(E_ACCESSDENIED),
            BackendError::Failed(E_ACCESSDENIED)
        );
        assert_eq!(classify(E_FAIL), BackendError::Failed(E_FAIL));
        assert_eq!(
            BackendError::NotFound.code(),
            HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0)
        );
    }

    #[test]
    fn entry_points_return_failure_codes_for_errors_and_panics() {
        assert_eq!(guarded(|| Ok(())), S_OK);
        assert_eq!(guarded(|| Err(E_ACCESSDENIED)), E_ACCESSDENIED);
        assert_eq!(
            guarded(|| Err(S_OK)),
            E_UNEXPECTED,
            "an error must never turn into success"
        );
        assert_eq!(guarded(|| panic!("boom")), E_UNEXPECTED);
    }
}
