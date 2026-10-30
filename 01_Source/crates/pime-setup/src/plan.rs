//! The installer as data: `install_plan` and `uninstall_plan` turn a request into an ordered list of [`Step`]s without
//! touching the machine. `exec` runs them against a [`crate::backend::Backend`].
//!
//! Safety properties that live here (and are tested here):
//! - the only registry key the plan can name is the product's own Add/Remove Programs key;
//! - the only program the plan can run is the 64-bit `regsvr32.exe` (and `code`), never anything read from a manifest;
//! - every path stays inside the install folder, which is never a system root and never contains `..`;
//! - there is no step that could change the default input method or touch another IME: the text service registers
//!   itself through its own `DllRegisterServer`, the installer only calls `regsvr32 /s <our dll>` (UT-002, UT-061).

use crate::backend::RegValue;
use crate::payload::{CORE, DLL, LICENSE};
use pime_core::json::{self, Json};
use std::fmt;
use std::path::{Path, PathBuf};

pub const PRODUCT: &str = "Programming IME";
pub const PUBLISHER: &str = "Programming IME contributors";
pub const MANIFEST_NAME: &str = "install-manifest.json";
pub const UNINSTALLER_NAME: &str = "uninstall.exe";
/// Under `HKEY_LOCAL_MACHINE`.
pub const ARP_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\ProgrammingIME";
const MANIFEST_SCHEMA: i64 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayloadFile {
    pub name: String,
    pub size: u64,
    pub sha256: [u8; 32],
}

/// What the payload contains, as far as planning is concerned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    /// `publisher.name` of the VS Code extension; present exactly when the payload carries the `.vsix`.
    pub extension_id: Option<String>,
    /// Every file that is copied into the install folder, in order.
    pub files: Vec<PayloadFile>,
}

/// How the VS Code extension step is handled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VscodeMode {
    /// `--no-vscode`: do not touch VS Code at all (the `.vsix` is still copied, so it can be installed later).
    Skip,
    /// An elevated child records the extension in the manifest but leaves running `code` to the non-elevated parent,
    /// so the extension lands in the profile of the user who started the setup, not of the administrator account.
    Defer,
    Install,
}

#[derive(Clone, Debug)]
pub struct SystemPaths {
    pub system_root: PathBuf,
    pub program_files: PathBuf,
    pub program_files_x86: Option<PathBuf>,
    pub program_data: Option<PathBuf>,
    pub user_profile: Option<PathBuf>,
}

impl SystemPaths {
    pub fn from_env() -> SystemPaths {
        let var = |n: &str| std::env::var_os(n).map(PathBuf::from);
        SystemPaths {
            system_root: var("SystemRoot").unwrap_or_else(|| PathBuf::from(r"C:\Windows")),
            program_files: var("ProgramFiles")
                .unwrap_or_else(|| PathBuf::from(r"C:\Program Files")),
            program_files_x86: var("ProgramFiles(x86)"),
            program_data: var("ProgramData"),
            user_profile: var("USERPROFILE"),
        }
    }

    /// The 64-bit regsvr32. This installer is a 64-bit process, so `System32` is the native folder.
    pub fn regsvr32(&self) -> PathBuf {
        self.system_root.join("System32").join("regsvr32.exe")
    }

    pub fn default_prefix(&self) -> PathBuf {
        self.program_files.join(PRODUCT)
    }

    fn protected_roots(&self) -> Vec<&Path> {
        let mut roots = vec![self.program_files.as_path(), self.system_root.as_path()];
        roots.extend(self.program_files_x86.as_deref());
        roots
    }
}

#[derive(Clone, Debug)]
pub struct InstallOptions {
    pub register: bool,
    pub arp: bool,
    pub vscode: VscodeMode,
    pub release: Release,
    pub system: SystemPaths,
}

#[derive(Clone, Debug)]
pub struct UninstallOptions {
    pub vscode: VscodeMode,
    pub system: SystemPaths,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileSource {
    Payload(String),
    /// The running setup itself: it becomes the uninstaller.
    SelfExe,
    /// The manifest, rendered when the step runs (it lists the folders that were really created).
    Manifest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    CreateDir(PathBuf),
    PlaceFile {
        dest: PathBuf,
        source: FileSource,
    },
    RegisterTextService {
        dll: PathBuf,
    },
    WriteArp {
        key: String,
        values: Vec<(String, RegValue)>,
    },
    InstallVscodeExtension {
        vsix: PathBuf,
        extension_id: String,
    },
    UninstallVscodeExtension {
        extension_id: String,
    },
    UnregisterTextService {
        dll: PathBuf,
    },
    DeleteArp {
        key: String,
    },
    RemoveFile(PathBuf),
    RemoveDir(PathBuf),
}

impl Step {
    pub fn describe(&self) -> String {
        match self {
            Step::CreateDir(p) => format!("create folder {}", p.display()),
            Step::PlaceFile { dest, source } => match source {
                FileSource::Payload(n) => format!(
                    "write {} (payload file {n}, SHA-256 verified)",
                    dest.display()
                ),
                FileSource::SelfExe => format!(
                    "write {} (a copy of this setup, used as the uninstaller)",
                    dest.display()
                ),
                FileSource::Manifest => format!(
                    "write {} (every file, folder and registry key this install writes)",
                    dest.display()
                ),
            },
            Step::RegisterTextService { dll } => format!(
                "register the text service: regsvr32.exe /s \"{}\"",
                dll.display()
            ),
            Step::WriteArp { key, .. } => format!("add the Add/Remove Programs entry HKLM\\{key}"),
            Step::InstallVscodeExtension { vsix, .. } => {
                format!(
                    "install the VS Code extension if `code` is found (non-fatal): code --install-extension \"{}\" --force",
                    vsix.display()
                )
            }
            Step::UninstallVscodeExtension { extension_id } => {
                format!(
                    "remove the VS Code extension if `code` is found (non-fatal): code --uninstall-extension {extension_id}"
                )
            }
            Step::UnregisterTextService { dll } => format!(
                "unregister the text service: regsvr32.exe /u /s \"{}\"",
                dll.display()
            ),
            Step::DeleteArp { key } => format!("delete the registry key HKLM\\{key}"),
            Step::RemoveFile(p) => format!("remove file {}", p.display()),
            Step::RemoveDir(p) => format!("remove folder {} (only if it is empty)", p.display()),
        }
    }

    /// A non-fatal step never triggers a rollback; its failure is reported as a warning.
    pub fn is_non_fatal(&self) -> bool {
        matches!(
            self,
            Step::InstallVscodeExtension { .. } | Step::UninstallVscodeExtension { .. }
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Install,
    Uninstall,
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub action: Action,
    pub version: String,
    pub prefix: PathBuf,
    pub steps: Vec<Step>,
    pub manifest: Manifest,
    pub regsvr32: PathBuf,
    pub requires_admin: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PlanError {
    Prefix(String),
    Release(String),
    Manifest(String),
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanError::Prefix(m) => write!(f, "refusing this install folder: {m}"),
            PlanError::Release(m) => write!(f, "cannot plan the install: {m}"),
            PlanError::Manifest(m) => write!(f, "the install manifest cannot be trusted: {m}"),
        }
    }
}

impl std::error::Error for PlanError {}

/// What an install wrote, kept in `<prefix>\install-manifest.json` so that uninstalling removes exactly this.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub version: String,
    pub prefix: PathBuf,
    pub files: Vec<PathBuf>,
    /// Folders the installer created itself (missing parents of the prefix included), top-down.
    pub directories: Vec<PathBuf>,
    /// Full registry paths including the hive.
    pub registry_keys: Vec<String>,
    /// DLLs registered with `regsvr32`.
    pub text_service_dlls: Vec<PathBuf>,
    pub vscode_extension: Option<String>,
}

fn arp_path() -> String {
    format!(r"HKLM\{ARP_KEY}")
}

impl Manifest {
    pub fn path(&self) -> PathBuf {
        self.prefix.join(MANIFEST_NAME)
    }

    pub fn to_json(&self) -> String {
        let s = |v: &str| Json::str(v).to_json_string();
        let list = |items: Vec<String>| {
            if items.is_empty() {
                "[]".to_string()
            } else {
                format!("[\n    {}\n  ]", items.join(",\n    "))
            }
        };
        let paths = |ps: &[PathBuf]| list(ps.iter().map(|p| s(&p.to_string_lossy())).collect());
        let keys = list(self.registry_keys.iter().map(|k| s(k)).collect());
        format!(
            "{{\n  \"schema\": {MANIFEST_SCHEMA},\n  \"product\": {},\n  \"version\": {},\n  \"prefix\": {},\n  \"files\": {},\n  \"directories\": {},\n  \"registry_keys\": {keys},\n  \"text_service_dlls\": {},\n  \"vscode_extension\": {}\n}}\n",
            s(PRODUCT),
            s(&self.version),
            s(&self.prefix.to_string_lossy()),
            paths(&self.files),
            paths(&self.directories),
            paths(&self.text_service_dlls),
            self.vscode_extension
                .as_deref()
                .map_or("null".to_string(), s),
        )
    }

    /// Parses and structurally validates a manifest that was read from the folder `expected_prefix`. A manifest whose
    /// recorded prefix differs from where it was found (copied, moved, edited) is refused.
    pub fn parse(text: &str, expected_prefix: &Path) -> Result<Manifest, PlanError> {
        let bad = |m: &str| PlanError::Manifest(m.to_string());
        let root = json::parse(text, json::Limits::DEFAULT)
            .map_err(|e| PlanError::Manifest(format!("not valid JSON ({e})")))?;
        if root.get("schema").and_then(Json::as_i64) != Some(MANIFEST_SCHEMA) {
            return Err(bad("unknown schema version"));
        }
        let string = |k: &str| root.get(k).and_then(Json::as_str).map(str::to_string);
        let strings = |k: &str| -> Result<Vec<String>, PlanError> {
            root.get(k)
                .and_then(Json::as_array)
                .ok_or_else(|| PlanError::Manifest(format!("\"{k}\" is missing")))?
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| PlanError::Manifest(format!("\"{k}\" must list strings")))
                })
                .collect()
        };
        let paths =
            |k: &str| strings(k).map(|v| v.into_iter().map(PathBuf::from).collect::<Vec<_>>());
        let prefix = PathBuf::from(string("prefix").ok_or_else(|| bad("\"prefix\" is missing"))?);
        if !same_path(&prefix, expected_prefix) {
            return Err(PlanError::Manifest(format!(
                "it records the folder {} but was found in {}",
                prefix.display(),
                expected_prefix.display()
            )));
        }
        Ok(Manifest {
            version: string("version").ok_or_else(|| bad("\"version\" is missing"))?,
            prefix,
            files: paths("files")?,
            directories: paths("directories")?,
            registry_keys: strings("registry_keys")?,
            text_service_dlls: paths("text_service_dlls")?,
            vscode_extension: string("vscode_extension"),
        })
    }

    /// The uninstaller acts with administrator rights on what this file says, so every entry is checked: all paths
    /// stay inside the prefix (no `..`), the prefix itself is acceptable, and the only registry key is our own.
    pub fn validate(&self, system: &SystemPaths) -> Result<(), PlanError> {
        validate_prefix(&self.prefix, system).map_err(|e| PlanError::Manifest(e.to_string()))?;
        let inside = |what: &str, p: &Path, allow_equal: bool| -> Result<(), PlanError> {
            if !inside_prefix(p, &self.prefix, allow_equal) {
                return Err(PlanError::Manifest(format!(
                    "{what} {} is outside the install folder",
                    p.display()
                )));
            }
            Ok(())
        };
        for f in &self.files {
            inside("file", f, false)?;
        }
        // Folders the installer created: the prefix and the parents it had to create above it, never anything else.
        for d in &self.directories {
            if !is_within(&self.prefix, d, true) || validate_prefix(d, system).is_err() {
                return Err(PlanError::Manifest(format!(
                    "folder {} is not the install folder or one of its parents",
                    d.display()
                )));
            }
        }
        for dll in &self.text_service_dlls {
            inside("text service DLL", dll, false)?;
        }
        for key in &self.registry_keys {
            if !key.eq_ignore_ascii_case(&arp_path()) {
                return Err(PlanError::Manifest(format!(
                    "registry key {key} is not one this installer owns"
                )));
            }
        }
        if let Some(id) = &self.vscode_extension
            && !(!id.is_empty()
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')))
        {
            return Err(PlanError::Manifest(
                "the VS Code extension id is not a plain identifier".into(),
            ));
        }
        Ok(())
    }
}

/// Case-folded, backslash-normalised form used to compare Windows paths.
pub fn path_key(p: &Path) -> String {
    p.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    path_key(a) == path_key(b)
}

/// `p` is below `prefix` (or equal when `allow_equal`) and has no `..`, `.`, empty or otherwise suspicious component.
pub fn inside_prefix(p: &Path, prefix: &Path, allow_equal: bool) -> bool {
    p.to_str().is_some_and(|t| split_components(t).is_ok()) && is_within(p, prefix, allow_equal)
}

/// Case-insensitive (Windows) containment test on already-normalised paths.
pub fn is_within(child: &Path, root: &Path, allow_equal: bool) -> bool {
    let (c, r) = (path_key(child), path_key(root));
    if c == r {
        return allow_equal;
    }
    c.strip_prefix(&r)
        .is_some_and(|rest| rest.starts_with('\\'))
}

const RESERVED_DEVICES: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Top-level folders of a drive that can never be an install folder.
const SYSTEM_TOP_LEVEL: [&str; 11] = [
    "windows",
    "program files",
    "program files (x86)",
    "programdata",
    "users",
    "documents and settings",
    "system volume information",
    "$recycle.bin",
    "recovery",
    "perflogs",
    "boot",
];

/// Splits `C:\a\b` (already using backslashes) into its components after checking each of them.
fn split_components(path: &str) -> Result<Vec<&str>, String> {
    let bytes = path.as_bytes();
    if !(bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && bytes[2] == b'\\')
    {
        return Err("it must be an absolute path on a local drive such as C:\\Tools\\PIME (relative, UNC and \\\\?\\ paths are refused)".into());
    }
    let rest = path[3..].trim_end_matches('\\');
    if rest.is_empty() {
        return Ok(Vec::new());
    }
    let comps: Vec<&str> = rest.split('\\').collect();
    for c in &comps {
        let lower = c.to_lowercase();
        let stem = lower.split('.').next().unwrap_or_default();
        if c.is_empty() {
            return Err("it contains an empty path component (\\\\)".into());
        } else if *c == ".." {
            return Err("it contains `..`".into());
        } else if *c == "." {
            return Err("it contains `.`".into());
        } else if c
            .chars()
            .any(|ch| ch.is_control() || "<>:\"|?*/".contains(ch))
        {
            return Err(format!(
                "component {c:?} contains a character Windows does not allow in names"
            ));
        } else if c.ends_with('.') || c.ends_with(' ') {
            return Err(format!(
                "component {c:?} ends in a dot or a space (Windows would silently rewrite it)"
            ));
        } else if RESERVED_DEVICES.contains(&stem) {
            return Err(format!("component {c:?} is a reserved device name"));
        } else if lower
            .as_bytes()
            .windows(2)
            .any(|w| w[0] == b'~' && w[1].is_ascii_digit())
        {
            return Err(format!(
                "component {c:?} looks like an 8.3 short name; use the long name"
            ));
        }
    }
    Ok(comps)
}

/// Normalises and checks an install folder. Returns the path with backslashes and without a trailing separator.
pub fn validate_prefix(raw: &Path, system: &SystemPaths) -> Result<PathBuf, PlanError> {
    let refuse = |m: String| PlanError::Prefix(format!("{}: {m}", raw.display()));
    let text = raw
        .to_str()
        .ok_or_else(|| refuse("it is not valid Unicode".into()))?;
    let text = text.replace('/', "\\");
    let comps = split_components(&text).map_err(refuse)?;
    let first = comps.first().map(|c| c.to_lowercase());
    if comps.is_empty() {
        return Err(refuse("a drive root cannot be the install folder".into()));
    }
    if first.as_deref() == Some("windows") {
        return Err(refuse("it is inside the Windows folder".into()));
    }
    if comps.len() == 1 && SYSTEM_TOP_LEVEL.contains(&first.as_deref().unwrap_or_default()) {
        return Err(refuse("it is a system folder".into()));
    }
    let normalized = PathBuf::from(format!("{}\\{}", &text[..2], comps.join("\\")));
    let mut forbidden_equal: Vec<&Path> = vec![system.program_files.as_path()];
    forbidden_equal.extend(system.program_files_x86.as_deref());
    forbidden_equal.extend(system.program_data.as_deref());
    forbidden_equal.extend(system.user_profile.as_deref());
    if is_within(&normalized, &system.system_root, true) {
        return Err(refuse("it is inside the Windows folder".into()));
    }
    if forbidden_equal.iter().any(|f| same_path(&normalized, f)) {
        return Err(refuse(
            "it is a system or profile root; choose a sub-folder of it".into(),
        ));
    }
    Ok(normalized)
}

fn arp_values(prefix: &Path, release: &Release) -> Vec<(String, RegValue)> {
    let uninstaller = prefix.join(UNINSTALLER_NAME);
    let quoted = format!("\"{}\"", uninstaller.display());
    let kib = release
        .files
        .iter()
        .map(|f| f.size)
        .sum::<u64>()
        .div_ceil(1024);
    let s = |k: &str, v: String| (k.to_string(), RegValue::Str(v));
    vec![
        s("DisplayName", PRODUCT.to_string()),
        s("DisplayVersion", release.version.clone()),
        s("Publisher", PUBLISHER.to_string()),
        s("InstallLocation", prefix.display().to_string()),
        s("UninstallString", format!("{quoted} --uninstall")),
        s(
            "QuietUninstallString",
            format!("{quoted} --uninstall --silent"),
        ),
        (
            "EstimatedSize".to_string(),
            RegValue::Dword(u32::try_from(kib).unwrap_or(u32::MAX)),
        ),
        ("NoModify".to_string(), RegValue::Dword(1)),
        ("NoRepair".to_string(), RegValue::Dword(1)),
    ]
}

/// Administrator rights are needed to write HKLM and to write under Program Files / Windows.
fn needs_admin(prefix: &Path, writes_hklm: bool, system: &SystemPaths) -> bool {
    writes_hklm
        || system
            .protected_roots()
            .iter()
            .any(|r| is_within(prefix, r, true))
}

/// `install_plan(prefix, options)`: pure, nothing is read from or written to the machine.
pub fn install_plan(prefix: &Path, options: &InstallOptions) -> Result<Plan, PlanError> {
    let prefix = validate_prefix(prefix, &options.system)?;
    let release = &options.release;
    for needed in [DLL, CORE, LICENSE] {
        if !release.files.iter().any(|f| f.name == needed) {
            return Err(PlanError::Release(format!("the payload has no {needed}")));
        }
    }
    let manifest_path = prefix.join(MANIFEST_NAME);
    let uninstaller = prefix.join(UNINSTALLER_NAME);
    let dll = prefix.join(DLL);

    // The manifest goes first: it is written ahead of the files it lists, so an install that is killed half way can
    // still be uninstalled (removing a file that never arrived is not an error).
    let mut steps = vec![
        Step::CreateDir(prefix.clone()),
        Step::PlaceFile {
            dest: manifest_path.clone(),
            source: FileSource::Manifest,
        },
    ];
    let mut files = vec![manifest_path];
    for f in &release.files {
        let dest = prefix.join(&f.name);
        files.push(dest.clone());
        steps.push(Step::PlaceFile {
            dest,
            source: FileSource::Payload(f.name.clone()),
        });
    }
    files.push(uninstaller.clone());
    steps.push(Step::PlaceFile {
        dest: uninstaller,
        source: FileSource::SelfExe,
    });

    let mut manifest = Manifest {
        version: release.version.clone(),
        prefix: prefix.clone(),
        files,
        directories: Vec::new(),
        registry_keys: Vec::new(),
        text_service_dlls: Vec::new(),
        vscode_extension: None,
    };
    if options.register {
        steps.push(Step::RegisterTextService { dll: dll.clone() });
        manifest.text_service_dlls.push(dll);
    }
    if options.arp {
        steps.push(Step::WriteArp {
            key: ARP_KEY.to_string(),
            values: arp_values(&prefix, release),
        });
        manifest.registry_keys.push(arp_path());
    }
    if let (Some(id), true) = (&release.extension_id, options.vscode != VscodeMode::Skip) {
        manifest.vscode_extension = Some(id.clone());
        if options.vscode == VscodeMode::Install {
            let vsix = prefix.join(crate::payload::VSIX);
            steps.push(Step::InstallVscodeExtension {
                vsix,
                extension_id: id.clone(),
            });
        }
    }

    let mut warnings = Vec::new();
    let admin_only = options
        .system
        .program_files_x86
        .iter()
        .chain([&options.system.program_files])
        .any(|r| is_within(&prefix, r, true));
    if options.register && !admin_only {
        warnings.push(format!(
            "{} is outside Program Files: anyone who can write there can replace the DLL that Windows loads into other programs. Use the default folder unless this is a disposable test machine.",
            prefix.display()
        ));
    }
    Ok(Plan {
        action: Action::Install,
        version: release.version.clone(),
        requires_admin: needs_admin(&prefix, options.register || options.arp, &options.system),
        regsvr32: options.system.regsvr32(),
        prefix,
        steps,
        manifest,
        warnings,
    })
}

/// `uninstall_plan(manifest)`: removes exactly what the manifest lists, nothing else.
pub fn uninstall_plan(manifest: &Manifest, options: &UninstallOptions) -> Result<Plan, PlanError> {
    manifest.validate(&options.system)?;
    let prefix = validate_prefix(&manifest.prefix, &options.system)?;
    let mut steps = Vec::new();
    if let (Some(id), VscodeMode::Install) = (&manifest.vscode_extension, options.vscode) {
        steps.push(Step::UninstallVscodeExtension {
            extension_id: id.clone(),
        });
    }
    for dll in &manifest.text_service_dlls {
        steps.push(Step::UnregisterTextService { dll: dll.clone() });
    }
    if !manifest.registry_keys.is_empty() {
        steps.push(Step::DeleteArp {
            key: ARP_KEY.to_string(),
        });
    }
    // The uninstaller and the manifest go last: if anything above fails half way, the folder can still be uninstalled again.
    let uninstaller = prefix.join(UNINSTALLER_NAME);
    let manifest_path = manifest.path();
    let is_special = |f: &PathBuf| same_path(f, &uninstaller) || same_path(f, &manifest_path);
    for f in manifest.files.iter().filter(|f| !is_special(f)) {
        steps.push(Step::RemoveFile(f.clone()));
    }
    if manifest.files.iter().any(|f| same_path(f, &uninstaller)) {
        steps.push(Step::RemoveFile(uninstaller));
    }
    steps.push(Step::RemoveFile(manifest_path));
    for d in manifest.directories.iter().rev() {
        steps.push(Step::RemoveDir(d.clone()));
    }
    let writes_hklm = !manifest.text_service_dlls.is_empty() || !manifest.registry_keys.is_empty();
    Ok(Plan {
        action: Action::Uninstall,
        version: manifest.version.clone(),
        requires_admin: needs_admin(&prefix, writes_hklm, &options.system),
        regsvr32: options.system.regsvr32(),
        prefix,
        steps,
        manifest: manifest.clone(),
        warnings: Vec::new(),
    })
}

/// Text of `--dry-run`.
pub fn render(plan: &Plan, notes: &[String]) -> String {
    let verb = match plan.action {
        Action::Install => "install",
        Action::Uninstall => "uninstall",
    };
    let mut out = format!(
        "{PRODUCT} {} - {verb} plan (dry run: nothing is changed)\n",
        plan.version
    );
    out += &format!("  folder: {}\n", plan.prefix.display());
    out += &format!(
        "  administrator rights needed: {}\n",
        if plan.requires_admin {
            "yes (the setup restarts itself elevated)"
        } else {
            "no"
        }
    );
    for n in notes {
        out += &format!("  {n}\n");
    }
    for w in &plan.warnings {
        out += &format!("  WARNING: {w}\n");
    }
    out += "  steps:\n";
    for (i, s) in plan.steps.iter().enumerate() {
        out += &format!("    {:>2}. {}\n", i + 1, s.describe());
    }
    out
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn system() -> SystemPaths {
        SystemPaths {
            system_root: PathBuf::from(r"C:\Windows"),
            program_files: PathBuf::from(r"C:\Program Files"),
            program_files_x86: Some(PathBuf::from(r"C:\Program Files (x86)")),
            program_data: Some(PathBuf::from(r"C:\ProgramData")),
            user_profile: Some(PathBuf::from(r"C:\Users\me")),
        }
    }

    pub fn release(version: &str) -> Release {
        let file = |name: &str, size: u64| PayloadFile {
            name: name.to_string(),
            size,
            sha256: [7; 32],
        };
        Release {
            version: version.to_string(),
            extension_id: Some("pub.ext".to_string()),
            files: vec![
                file(DLL, 2048),
                file(CORE, 4096),
                file(LICENSE, 1100),
                file(crate::payload::VSIX, 5000),
            ],
        }
    }

    pub fn options() -> InstallOptions {
        InstallOptions {
            register: true,
            arp: true,
            vscode: VscodeMode::Install,
            release: release("0.2.0"),
            system: system(),
        }
    }

    pub const PREFIX: &str = r"C:\Program Files\Programming IME";

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn payload(name: &str) -> FileSource {
        FileSource::Payload(name.to_string())
    }

    fn place(name: &str, source: FileSource) -> Step {
        Step::PlaceFile {
            dest: Path::new(PREFIX).join(name),
            source,
        }
    }

    #[test]
    fn st_035_install_plan_is_exactly_this_for_the_default_options() {
        let plan = install_plan(Path::new(PREFIX), &options()).unwrap();
        let expected = vec![
            Step::CreateDir(p(PREFIX)),
            place(MANIFEST_NAME, FileSource::Manifest),
            place(DLL, payload(DLL)),
            place(CORE, payload(CORE)),
            place(LICENSE, payload(LICENSE)),
            place(crate::payload::VSIX, payload(crate::payload::VSIX)),
            place(UNINSTALLER_NAME, FileSource::SelfExe),
            Step::RegisterTextService {
                dll: p(&format!(r"{PREFIX}\pime_tsf.dll")),
            },
            Step::WriteArp {
                key: ARP_KEY.to_string(),
                values: arp_values(Path::new(PREFIX), &release("0.2.0")),
            },
            Step::InstallVscodeExtension {
                vsix: p(&format!(r"{PREFIX}\programming-ime.vsix")),
                extension_id: "pub.ext".to_string(),
            },
        ];
        assert_eq!(plan.steps, expected);
        assert!(plan.requires_admin && plan.warnings.is_empty());
        assert_eq!(plan.regsvr32, p(r"C:\Windows\System32\regsvr32.exe"));
        let m = &plan.manifest;
        assert_eq!(m.files.len(), 6);
        assert_eq!(
            m.registry_keys,
            [r"HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall\ProgrammingIME"]
        );
        assert_eq!(m.text_service_dlls, [p(&format!(r"{PREFIX}\pime_tsf.dll"))]);
        assert_eq!(m.vscode_extension.as_deref(), Some("pub.ext"));
    }

    #[test]
    fn st_035_the_add_remove_programs_entry_describes_the_uninstaller() {
        let values = arp_values(Path::new(PREFIX), &release("0.2.0"));
        let get = |n: &str| values.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());
        assert_eq!(
            get("DisplayName"),
            Some(RegValue::Str("Programming IME".into()))
        );
        assert_eq!(get("DisplayVersion"), Some(RegValue::Str("0.2.0".into())));
        assert_eq!(get("InstallLocation"), Some(RegValue::Str(PREFIX.into())));
        assert_eq!(
            get("UninstallString"),
            Some(RegValue::Str(format!(
                r#""{PREFIX}\uninstall.exe" --uninstall"#
            )))
        );
        assert_eq!(
            get("QuietUninstallString"),
            Some(RegValue::Str(format!(
                r#""{PREFIX}\uninstall.exe" --uninstall --silent"#
            )))
        );
        assert_eq!(get("EstimatedSize"), Some(RegValue::Dword(12))); // 12244 bytes -> 12 KiB
        assert_eq!(get("NoModify"), Some(RegValue::Dword(1)));
    }

    #[test]
    fn st_035_each_no_flag_removes_exactly_its_steps() {
        let base = install_plan(Path::new(PREFIX), &options()).unwrap();
        let without = |f: &dyn Fn(&mut InstallOptions)| {
            let mut o = options();
            f(&mut o);
            install_plan(Path::new(PREFIX), &o).unwrap()
        };
        let no_register = without(&|o| o.register = false);
        assert_eq!(no_register.steps.len(), base.steps.len() - 1);
        assert!(
            !no_register
                .steps
                .iter()
                .any(|s| matches!(s, Step::RegisterTextService { .. }))
        );
        assert!(no_register.manifest.text_service_dlls.is_empty());

        let no_arp = without(&|o| o.arp = false);
        assert!(
            !no_arp
                .steps
                .iter()
                .any(|s| matches!(s, Step::WriteArp { .. }))
        );
        assert!(no_arp.manifest.registry_keys.is_empty());

        // --no-vscode still copies the .vsix (so it can be installed later) but never records or runs the extension step
        let skip = without(&|o| o.vscode = VscodeMode::Skip);
        assert!(
            !skip
                .steps
                .iter()
                .any(|s| matches!(s, Step::InstallVscodeExtension { .. }))
        );
        assert_eq!(skip.manifest.vscode_extension, None);
        assert!(skip.steps.iter().any(
            |s| matches!(s, Step::PlaceFile { dest, .. } if dest.ends_with("programming-ime.vsix"))
        ));

        // the elevated child: the extension is recorded, running `code` is left to the parent
        let defer = without(&|o| o.vscode = VscodeMode::Defer);
        assert_eq!(defer.manifest.vscode_extension.as_deref(), Some("pub.ext"));
        assert!(
            !defer
                .steps
                .iter()
                .any(|s| matches!(s, Step::InstallVscodeExtension { .. }))
        );
        assert_eq!(defer.steps.len(), base.steps.len() - 1);
    }

    #[test]
    fn ut_002_ut_061_the_plan_cannot_express_anything_but_our_own_files_key_and_dll() {
        for vscode in [VscodeMode::Skip, VscodeMode::Defer, VscodeMode::Install] {
            let mut o = options();
            o.vscode = vscode;
            let plan = install_plan(Path::new(PREFIX), &o).unwrap();
            for step in &plan.steps {
                match step {
                    Step::WriteArp { key, .. } => assert_eq!(key, ARP_KEY),
                    Step::RegisterTextService { dll } => {
                        assert_eq!(dll, &p(&format!(r"{PREFIX}\pime_tsf.dll")))
                    }
                    Step::CreateDir(d) => assert!(inside_prefix(d, Path::new(PREFIX), true)),
                    Step::PlaceFile { dest, .. } => {
                        assert!(inside_prefix(dest, Path::new(PREFIX), false))
                    }
                    Step::InstallVscodeExtension { vsix, .. } => {
                        assert!(inside_prefix(vsix, Path::new(PREFIX), false))
                    }
                    other => panic!("an install plan must not contain {other:?}"),
                }
                let text = step.describe().to_lowercase();
                assert!(
                    !text.contains("ctf")
                        && !text.contains("keyboard layout")
                        && !text.contains("activate"),
                    "{text}"
                );
            }
        }
    }

    #[test]
    fn st_035_a_temp_prefix_run_with_all_three_no_flags_has_no_registry_or_process_step_and_never_elevates()
     {
        let mut o = options();
        (o.register, o.arp, o.vscode) = (false, false, VscodeMode::Skip);
        let plan =
            install_plan(Path::new(r"C:\Users\me\AppData\Local\Temp\pime-test"), &o).unwrap();
        assert!(!plan.requires_admin);
        assert!(
            plan.steps
                .iter()
                .all(|s| matches!(s, Step::CreateDir(_) | Step::PlaceFile { .. }))
        );
        assert!(plan.warnings.is_empty());
        // while registering from the same folder needs admin rights and warns that the DLL sits in a user-writable place
        o.register = true;
        let plan =
            install_plan(Path::new(r"C:\Users\me\AppData\Local\Temp\pime-test"), &o).unwrap();
        assert!(plan.requires_admin);
        assert!(plan.warnings[0].contains("outside Program Files"));
        // and a protected folder needs admin rights even without registry work
        o.register = false;
        assert!(install_plan(Path::new(PREFIX), &o).unwrap().requires_admin);
        assert!(
            install_plan(Path::new(r"C:\Program Files (x86)\PIME"), &o)
                .unwrap()
                .requires_admin
        );
    }

    #[test]
    fn st_035_prefixes_that_are_system_roots_or_contain_dot_dot_are_refused() {
        let refused = [
            "",
            "C:",
            r"C:\",
            r"relative\dir",
            r"\\server\share\pime",
            r"\\?\C:\pime",
            r"\\.\C:\pime",
            r"C:\Windows",
            r"c:\windows",
            r"C:\Windows\System32\PIME",
            r"C:/Windows/System32",
            r"C:\Program Files",
            r"C:\Program Files\",
            r"C:\Program Files (x86)",
            r"C:\ProgramData",
            r"C:\Users",
            r"C:\Users\me",
            r"C:\Program Files\..\Windows",
            r"C:\Tools\..\..",
            r"C:\Tools\..",
            r"C:\Tools\PIME\.",
            r"C:\a\\b",
            r"C:\Tools\PIME.",
            r"C:\Tools\PIME ",
            r"C:\Tools\CON",
            r"C:\Tools\nul.txt",
            r"C:\PROGRA~1\PIME",
            r"C:\Tools\a:b",
            r"C:\Tools\a<b",
            "C:\\Tools\\a\0b",
            r"D:\Recovery",
            r"D:\Windows",
        ];
        for bad in refused {
            let r = install_plan(Path::new(bad), &options());
            assert!(
                matches!(r, Err(PlanError::Prefix(_))),
                "{bad:?} must be refused, got {r:?}"
            );
        }
    }

    #[test]
    fn st_035_prefixes_with_spaces_and_non_ascii_are_accepted_and_normalised() {
        for (raw, normalized) in [
            (PREFIX, PREFIX),
            (r"C:\Program Files\Programming IME\", PREFIX),
            ("C:/Program Files/Programming IME", PREFIX),
            (
                r"D:\日本語 フォルダ\Programming IME",
                r"D:\日本語 フォルダ\Programming IME",
            ),
            (r"E:\Tools\PIME", r"E:\Tools\PIME"),
            (
                r"C:\Users\me\AppData\Local\Temp\pime x",
                r"C:\Users\me\AppData\Local\Temp\pime x",
            ),
        ] {
            let plan = install_plan(Path::new(raw), &options()).unwrap();
            assert_eq!(plan.prefix, p(normalized), "{raw}");
            assert!(plan.steps.iter().skip(1).all(|s| match s {
                Step::PlaceFile { dest, .. } => dest.starts_with(normalized),
                _ => true,
            }));
        }
    }

    #[test]
    fn st_035_an_incomplete_release_is_refused_before_anything_is_planned() {
        let mut o = options();
        o.release.files.retain(|f| f.name != DLL);
        assert!(
            matches!(install_plan(Path::new(PREFIX), &o), Err(PlanError::Release(m)) if m.contains(DLL))
        );
    }

    #[test]
    fn st_035_dry_run_text_lists_every_step_and_says_nothing_is_changed() {
        let plan = install_plan(Path::new(PREFIX), &options()).unwrap();
        let text = render(&plan, &["payload: ok".to_string()]);
        assert!(
            text.starts_with(
                "Programming IME 0.2.0 - install plan (dry run: nothing is changed)\n"
            )
        );
        assert!(text.contains("administrator rights needed: yes"));
        assert!(text.contains("payload: ok"));
        assert!(
            text.contains(r#"regsvr32.exe /s "C:\Program Files\Programming IME\pime_tsf.dll""#)
        );
        assert_eq!(
            text.lines()
                .filter(|l| l.trim_start().starts_with(|c: char| c.is_ascii_digit()))
                .count(),
            plan.steps.len()
        );
    }

    fn full_manifest() -> Manifest {
        let mut plan_manifest = install_plan(Path::new(PREFIX), &options())
            .unwrap()
            .manifest;
        plan_manifest.directories = vec![p(r"C:\Program Files\Programming IME")];
        plan_manifest
    }

    #[test]
    fn st_035_manifest_json_round_trips_including_quotes_backslashes_and_non_ascii() {
        let mut m = full_manifest();
        m.prefix = p(r"D:\日本語 フォルダ\PIME");
        m.files = vec![p(r#"D:\日本語 フォルダ\PIME\a "quoted" file.dll"#)];
        m.directories = vec![p(r"D:\日本語 フォルダ"), p(r"D:\日本語 フォルダ\PIME")];
        let text = m.to_json();
        assert!(text.contains("\"schema\": 1"));
        let parsed = Manifest::parse(&text, Path::new(r"d:/日本語 フォルダ/pime")).unwrap();
        assert_eq!(parsed, m);
        // empty lists stay valid
        let mut empty = full_manifest();
        (
            empty.registry_keys,
            empty.text_service_dlls,
            empty.vscode_extension,
        ) = (vec![], vec![], None);
        assert_eq!(
            Manifest::parse(&empty.to_json(), Path::new(PREFIX)).unwrap(),
            empty
        );
    }

    #[test]
    fn st_035_a_manifest_that_is_not_where_it_says_it_is_or_is_malformed_is_refused() {
        let text = full_manifest().to_json();
        assert!(
            matches!(Manifest::parse(&text, Path::new(r"D:\Elsewhere")), Err(PlanError::Manifest(m)) if m.contains("was found in"))
        );
        assert!(
            Manifest::parse(
                &text.replace("\"schema\": 1", "\"schema\": 2"),
                Path::new(PREFIX)
            )
            .is_err()
        );
        assert!(Manifest::parse("{not json", Path::new(PREFIX)).is_err());
        assert!(
            Manifest::parse(&text.replace("\"files\"", "\"filez\""), Path::new(PREFIX)).is_err()
        );
        assert!(
            Manifest::parse(
                &text.replace(
                    "\"version\": \"0.2.0\",",
                    "\"version\": \"0.2.0\", \"version\": \"9\","
                ),
                Path::new(PREFIX)
            )
            .is_err(),
            "duplicate keys"
        );
    }

    fn uninstall_options(vscode: VscodeMode) -> UninstallOptions {
        UninstallOptions {
            vscode,
            system: system(),
        }
    }

    #[test]
    fn st_035_uninstall_plan_removes_exactly_the_manifest_with_uninstaller_and_manifest_last() {
        let m = full_manifest();
        let plan = uninstall_plan(&m, &uninstall_options(VscodeMode::Install)).unwrap();
        let dll = p(&format!(r"{PREFIX}\pime_tsf.dll"));
        let file = |n: &str| Step::RemoveFile(Path::new(PREFIX).join(n));
        let expected = vec![
            Step::UninstallVscodeExtension {
                extension_id: "pub.ext".into(),
            },
            Step::UnregisterTextService { dll },
            Step::DeleteArp {
                key: ARP_KEY.to_string(),
            },
            file(DLL),
            file(CORE),
            file(LICENSE),
            file("programming-ime.vsix"),
            file(UNINSTALLER_NAME),
            file(MANIFEST_NAME),
            Step::RemoveDir(p(PREFIX)),
        ];
        assert_eq!(plan.steps, expected);
        assert!(plan.requires_admin);
        // the extension step is the parent's job in the elevated child, and absent with --no-vscode
        for mode in [VscodeMode::Defer, VscodeMode::Skip] {
            let p2 = uninstall_plan(&m, &uninstall_options(mode)).unwrap();
            assert!(
                !p2.steps
                    .iter()
                    .any(|s| matches!(s, Step::UninstallVscodeExtension { .. }))
            );
        }
    }

    #[test]
    fn st_035_uninstall_of_a_no_register_no_arp_install_in_a_temp_folder_needs_no_admin_rights() {
        let mut o = options();
        (o.register, o.arp, o.vscode) = (false, false, VscodeMode::Skip);
        let prefix = r"C:\Users\me\AppData\Local\Temp\pime-test";
        let m = install_plan(Path::new(prefix), &o).unwrap().manifest;
        let plan = uninstall_plan(&m, &uninstall_options(VscodeMode::Install)).unwrap();
        assert!(!plan.requires_admin);
        assert!(
            plan.steps
                .iter()
                .all(|s| matches!(s, Step::RemoveFile(_) | Step::RemoveDir(_)))
        );
    }

    #[test]
    fn st_035_a_tampered_manifest_cannot_make_the_uninstaller_touch_anything_else() {
        let good = full_manifest();
        let reject = |edit: &dyn Fn(&mut Manifest)| {
            let mut m = good.clone();
            edit(&mut m);
            let r = uninstall_plan(&m, &uninstall_options(VscodeMode::Install));
            assert!(matches!(r, Err(PlanError::Manifest(_))), "{m:?} -> {r:?}");
        };
        reject(&|m| m.files.push(p(r"C:\Windows\System32\kernel32.dll")));
        reject(&|m| {
            m.files
                .push(p(&format!(r"{PREFIX}\..\..\Windows\evil.dll")))
        });
        reject(&|m| m.files.push(p(&format!(r"{PREFIX}\sub\..\..\x"))));
        reject(&|m| m.files.push(p(r"D:\Other\file.txt")));
        reject(&|m| m.files.push(p(&format!("{PREFIX}\\a/b"))));
        reject(&|m| m.files.push(p(PREFIX))); // the folder itself is not a file
        reject(&|m| m.directories.push(p(r"C:\Program Files")));
        reject(&|m| m.directories.push(p(r"D:\Other"))); // not a parent of the prefix
        reject(&|m| m.directories.push(p(&format!(r"{PREFIX}\sub")))); // below the prefix, not above it
        reject(&|m| m.directories.push(p(r"C:\Program Files\..")));
        reject(&|m| m.directories.push(p(r"C:\Windows")));
        reject(&|m| m.text_service_dlls.push(p(r"C:\Windows\System32\evil.dll")));
        reject(&|m| {
            m.registry_keys.push(
                r"HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall\OtherProduct".into(),
            )
        });
        reject(&|m| {
            m.registry_keys
                .push(r"HKLM\SOFTWARE\Microsoft\CTF\TIP".into())
        });
        reject(&|m| m.prefix = p(r"C:\Windows"));
        reject(&|m| m.prefix = p(r"C:\"));
        reject(&|m| m.vscode_extension = Some("x --install-extension evil".into()));
        // the key comparison is case-insensitive (registry keys are), and an exact match passes
        let mut ok = good.clone();
        ok.registry_keys =
            vec![r"hklm\SOFTWARE\microsoft\windows\currentversion\uninstall\programmingime".into()];
        assert!(uninstall_plan(&ok, &uninstall_options(VscodeMode::Install)).is_ok());
        // the parents the installer created above the prefix are legitimate entries
        let mut deep = good.clone();
        deep.prefix = p(r"D:\Tools\Deep\PIME");
        deep.files = vec![p(r"D:\Tools\Deep\PIME\a.dll")];
        deep.text_service_dlls = vec![];
        deep.directories = vec![
            p(r"D:\Tools"),
            p(r"D:\Tools\Deep"),
            p(r"D:\Tools\Deep\PIME"),
        ];
        assert!(uninstall_plan(&deep, &uninstall_options(VscodeMode::Install)).is_ok());
    }

    #[test]
    fn st_035_path_helpers_are_case_insensitive_and_component_exact() {
        assert!(is_within(
            Path::new(r"C:\Program Files\X\a"),
            Path::new(r"c:\program files\x"),
            false
        ));
        assert!(!is_within(
            Path::new(r"C:\Program Files\X2\a"),
            Path::new(r"C:\Program Files\X"),
            false
        ));
        assert!(!is_within(
            Path::new(r"C:\Program Files\X"),
            Path::new(r"C:\Program Files\X"),
            false
        ));
        assert!(is_within(
            Path::new(r"C:\Program Files\X"),
            Path::new(r"C:\Program Files\X\"),
            true
        ));
        assert!(same_path(Path::new("C:/A/B/"), Path::new(r"c:\a\b")));
    }
}
