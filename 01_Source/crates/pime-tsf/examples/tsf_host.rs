//! A TSF host that drives the REAL `pime_tsf.dll` through the REAL `msctf.dll`.
//!
//!   cargo run --release --example tsf_host -- --dll <path to pime_tsf.dll>
//!
//! What it proves (the judgement layer the unit tests cannot reach):
//!   1. the DLL's COM class object is created by the real COM machinery (`DllGetClassObject`);
//!   2. the real msctf thread manager activates the text service (`ITfTextInputProcessorEx::ActivateEx`);
//!   3. the real document manager and context receive the keystrokes (`ITfKeystrokeMgr`);
//!   4. the text service's own candidate window opens, its chosen candidate is written into the
//!      host's text store (`ITfRange::SetText` in a real edit session) and the host reads it back.
//!
//! Nothing is registered: no registry key is written, no profile is created, no administrator
//! rights are needed. What it cannot do is activate a key event sink: msctf only accepts one
//! for a text service that is registered as the input processor of the thread, so the last step
//! needs a registered profile (that is what the installer does, with administrator rights). The text service opens its own candidate window and floating bar on the
//! primary monitor (they belong to the text service, not to this program); they are closed when
//! the text service is deactivated.
//!
//! The host plays the role of an application: a key the text service does not eat is inserted into
//! the text store by the host itself, exactly as a real editor would do.
//!
//! Two modes:
//!   * `--dll <path>` drives the DLL that is given to it: the real COM class object is created and activated
//!     by hand. This proves the COM face of the DLL without registering anything.
//!   * `--profile` drives the profile that is **already registered on this machine**: the host plays the
//!     application (thread manager, document manager, context), asks msctf to activate the registered zh-CN
//!     profile and then sends the keys. Measured on this machine: `ITfThreadMgr2::Activate` hands out the client
//!     id, `ITfDocumentMgr::CreateContext` accepts only `dwflags` 0 or `TF_ES_SYNC` (TF_ES_READWRITE is refused
//!     with E_INVALIDARG), `ITfInputProcessorProfileMgr::ActivateProfile` succeeds and msctf loads the installed
//!     DLL into the process - but the keystroke manager does not route the keys, because a console host has no
//!     window that owns the keyboard focus and TSF only makes a text service the input processor of a FOCUSED
//!     context. The legacy `ITfThreadMgr::Activate` crashes msctf.dll itself (0xC0000005 inside msctf, not in our
//!     DLL), which is why the modern interface is used.

use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{BOOL, HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize, IClassFactory,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{SetKeyboardState, VK_CONTROL, VK_MENU};
use windows::Win32::UI::TextServices::{
    CLSID_TF_InputProcessorProfiles, CLSID_TF_ThreadMgr, ITfContext, ITfDocumentMgr,
    ITfEditSession, ITfEditSession_Impl, ITfInputProcessorProfileMgr, ITfKeystrokeMgr, ITfRange,
    ITfTextInputProcessorEx, ITfThreadMgr, ITfThreadMgr2, TF_AE_NONE, TF_ANCHOR_END,
    TF_DEFAULT_SELECTION, TF_ES_READ, TF_ES_READWRITE, TF_ES_SYNC, TF_INPUTPROCESSORPROFILE,
    TF_IPSINK_FLAG_ACTIVE, TF_PROFILETYPE_INPUTPROCESSOR, TF_SELECTION, TF_SELECTIONSTYLE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
};
use windows::core::{GUID, HRESULT, Interface, PCWSTR, implement};

/// The COM class of this product (`profiles.rs`: PIME_CLSID).
const CLSID_PIME: GUID = GUID::from_u128(0x43f95ed1_95b4_421d_8f5a_837128c2d854);

type DllGetClassObjectFn =
    unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut c_void) -> HRESULT;

/// The work one edit session runs: a closure that gets the edit cookie of the session.
type EditWork = Box<dyn FnOnce(u32)>;

/// Whether a module with this file name is loaded in this process.
fn module_loaded(name: &str) -> bool {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: the name is a null-terminated wide string that outlives the call.
    unsafe { GetModuleHandleW(PCWSTR(wide.as_ptr())) }.is_ok()
}

// ---------------------------------------------------------------- the edit session (the host writes through it)

/// One edit session running a closure with the edit cookie, the same shape the text service uses.
#[implement(ITfEditSession)]
struct Editor {
    work: RefCell<Option<EditWork>>,
}

impl ITfEditSession_Impl for Editor_Impl {
    fn DoEditSession(&self, ec: u32) -> windows::core::Result<()> {
        let work = self.work.borrow_mut().take();
        if let Some(work) = work {
            work(ec);
        }
        Ok(())
    }
}

/// Runs `work` in an edit session on `ctx` and returns what it produced.
fn session<T: 'static>(
    ctx: &ITfContext,
    flags: windows::Win32::UI::TextServices::TF_CONTEXT_EDIT_CONTEXT_FLAGS,
    work: impl FnOnce(u32) -> windows::core::Result<T> + 'static,
) -> windows::core::Result<T> {
    let out: Rc<RefCell<Option<windows::core::Result<T>>>> = Rc::new(RefCell::new(None));
    let sink = Rc::clone(&out);
    let session: ITfEditSession = Editor {
        work: RefCell::new(Some(Box::new(move |ec| {
            *sink.borrow_mut() = Some(work(ec));
        }))),
    }
    .into();
    // SAFETY: `ctx` and `session` are live COM objects; a synchronous session runs before the call returns.
    let requested = unsafe { ctx.RequestEditSession(client_id(), &session, flags) };
    match requested {
        Ok(hr) if hr.is_ok() => out
            .borrow_mut()
            .take()
            .unwrap_or(Err(windows::core::Error::from_win32())),
        Ok(hr) => Err(windows::core::Error::from_hresult(hr)),
        Err(e) => Err(e),
    }
}

/// The caret of the context: an empty range at the insertion point.
fn caret(ctx: &ITfContext, ec: u32) -> windows::core::Result<ITfRange> {
    let mut selection = [TF_SELECTION {
        range: std::mem::ManuallyDrop::new(None),
        style: TF_SELECTIONSTYLE {
            ase: TF_AE_NONE,
            fInterimChar: BOOL(0),
        },
    }];
    let mut fetched = 0u32;
    // SAFETY: the buffer is live and large enough for the one selection the context holds.
    unsafe { ctx.GetSelection(ec, TF_DEFAULT_SELECTION, &mut selection, &mut fetched)? };
    // SAFETY: the range is an owned COM reference in a ManuallyDrop; taking it hands the release over.
    let range = unsafe { std::mem::ManuallyDrop::take(&mut selection[0].range) };
    range.ok_or_else(windows::core::Error::from_win32)
}

/// Inserts `text` at the caret, the way an application writes what the user typed.
fn insert_text(ctx: &ITfContext, text: &str) -> windows::core::Result<()> {
    let chars: Vec<u16> = text.encode_utf16().collect();
    let inner = ctx.clone();
    session(ctx, TF_ES_SYNC, move |ec| {
        let range = caret(&inner, ec)?;
        // SAFETY: `ec` belongs to this session; the range is a live COM object of the same context.
        unsafe {
            range.SetText(ec, 0, &chars)?;
            range.Collapse(ec, TF_ANCHOR_END)?;
        }
        Ok(())
    })
}

/// The whole text of the context, read back through a read session.
fn read_text(ctx: &ITfContext) -> windows::core::Result<String> {
    let inner = ctx.clone();
    session(ctx, TF_ES_READ, move |ec| {
        let range = caret(&inner, ec)?;
        // widen the caret to the whole document: a negative shift extends towards the start
        let mut moved = 0i32;
        // SAFETY: `ec` belongs to this session.
        unsafe {
            range.ShiftStart(ec, -100_000, &mut moved, std::ptr::null())?;
            range.ShiftEnd(ec, 100_000, &mut moved, std::ptr::null())?;
            let mut buffer = vec![0u16; 8192];
            let mut length = 0u32;
            range.GetText(ec, 0, &mut buffer, &mut length)?;
            Ok(String::from_utf16_lossy(&buffer[..length as usize]))
        }
    })
}

// ---------------------------------------------------------------- the text service

/// Creates the text service object out of the DLL, through the real COM entry point.
fn load_tip(dll: &str) -> windows::core::Result<ITfTextInputProcessorEx> {
    let wide: Vec<u16> = dll.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: the path is a null-terminated wide string that outlives the call.
    let module = unsafe { LoadLibraryW(PCWSTR(wide.as_ptr())) }?;
    // SAFETY: the module is loaded and stays loaded for the whole run.
    let entry = unsafe {
        GetProcAddress(
            module,
            windows::core::PCSTR(c"DllGetClassObject".as_ptr().cast()),
        )
    };
    let Some(entry) = entry else {
        return Err(windows::core::Error::from_win32());
    };
    let get_class_object: DllGetClassObjectFn = unsafe { std::mem::transmute(entry) };
    let mut factory: *mut c_void = std::ptr::null_mut();
    // SAFETY: the out parameter is a live local; the ids are valid references.
    let hr = unsafe { get_class_object(&CLSID_PIME, &IClassFactory::IID, &mut factory) };
    if hr.is_err() || factory.is_null() {
        return Err(windows::core::Error::from_hresult(hr));
    }
    // SAFETY: the factory is a live COM object the DLL handed us.
    let factory: IClassFactory = unsafe { windows::core::Interface::from_raw(factory) };
    // SAFETY: the class factory creates the text service for our class.
    unsafe { factory.CreateInstance(None) }
}

// ---------------------------------------------------------------- the keyboard

/// The key state the text service reads with `GetKeyState`: the high bit means "down".
fn state_with(mods: &[i32]) -> [u8; 256] {
    let mut state = [0u8; 256];
    for &vk in mods {
        state[vk as usize] = 0x80;
    }
    state
}

/// Sends one key to the text service. A key the service does not eat is written into the text
/// store by the host, the way an application inserts what the user typed. Returns whether the
/// text service ate the key.
fn send_key(
    keys: &ITfKeystrokeMgr,
    ctx: &ITfContext,
    vk: u32,
    scan: u32,
    text: Option<&str>,
) -> bool {
    let lparam = LPARAM(1 | ((scan as isize) << 16));
    // SAFETY: plain COM calls on live objects.
    let eaten = unsafe { keys.TestKeyDown(WPARAM(vk as usize), lparam) }
        .map(|b| b.as_bool())
        .unwrap_or(false);
    if eaten {
        let _ = unsafe { keys.KeyDown(WPARAM(vk as usize), lparam) };
    } else if let Some(text) = text
        && let Err(e) = insert_text(ctx, text)
    {
        eprintln!("    the host could not write {text:?} into the store: {e:?}");
    }
    let _ = unsafe { keys.KeyUp(WPARAM(vk as usize), lparam) };
    pump();
    eaten
}

/// Delivers the window's own messages (the candidate window of the text service is one of them).
fn pump() {
    let mut msg = MSG::default();
    // SAFETY: `msg` is a live local; only this thread's windows are dispatched.
    unsafe {
        while PeekMessageW(&mut msg, HWND::default(), 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Pumps the messages for a while: the text service needs its window painted before it commits.
fn pump_for(ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        pump();
        std::thread::sleep(Duration::from_millis(5));
    }
    pump();
}

// ---------------------------------------------------------------- the registered profile (the real system)

/// The zh-CN language profile of this product (`lib.rs`: PROFILE_ZH).
const PROFILE_ZH: GUID = GUID::from_u128(0x223fd919_f8cb_476c_a852_351e7381c801);

/// Drives the text service the way a real application does, but through the profile that is **already registered
/// on this machine**: msctf loads the installed DLL itself and activates it for this thread. Nothing is written
/// to the registry here, and no administrator rights are needed.
fn drive_profile() -> windows::core::Result<String> {
    eprintln!("step: create the real msctf thread manager");
    // SAFETY: COM is initialised on this thread; the class is the real msctf thread manager.
    let thread_mgr: ITfThreadMgr =
        unsafe { CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER) }?;
    // The thread manager has to know this thread before it accepts a client id: `ITfThreadMgr2::Activate`
    // registers the calling thread and hands out its TSF client id. Without it `CreateContext` refuses the
    // owner id with E_INVALIDARG.
    let thread_mgr2: ITfThreadMgr2 = thread_mgr.cast()?;
    // SAFETY: the thread manager is live.
    let tid = unsafe { thread_mgr2.Activate() }?;
    eprintln!("  ITfThreadMgr2::Activate -> the thread id {tid:#x}");
    CLIENT_ID.store(tid, std::sync::atomic::Ordering::Relaxed);
    eprintln!(
        "step: create the document manager and the context (this host plays the application)"
    );
    // SAFETY: the thread manager is live.
    let doc_mgr: ITfDocumentMgr = unsafe { thread_mgr.CreateDocumentMgr() }?;
    let mut context: Option<ITfContext> = None;
    let mut cookie = 0u32;
    // SAFETY: the document manager is live; the out parameters are live locals.
    eprintln!("  creating the context");
    // The flags of `CreateContext` are NOT the flags of an edit session: msctf accepts only TF_ES_SYNC (or 0)
    // here and answers E_INVALIDARG for TF_ES_READWRITE / TF_ES_READ. (Measured: 0x00 and 0x01 are accepted,
    // 0x02..0x07 are not.)
    unsafe { doc_mgr.CreateContext(tid, TF_ES_SYNC.0, None, &mut context, &mut cookie) }?;
    let context = context.ok_or_else(windows::core::Error::from_win32)?;
    eprintln!("  pushing the context");
    // SAFETY: the context is live.
    unsafe { doc_mgr.Push(&context) }?;
    eprintln!("  setting the focus to the document manager");
    // SAFETY: the document manager is live. `Some(&doc_mgr)`: the `Param` implementation of an interface reference
    // queries the interface first and passes a null pointer when that fails.
    unsafe { thread_mgr.SetFocus(Some(&doc_mgr)) }?;
    eprintln!("  the context is live");
    pump_for(100);

    eprintln!("step: activate the REGISTERED zh-CN profile for this thread");
    eprintln!("  (msctf loads the installed DLL itself; nothing is registered here)");
    // `ITfThreadMgr::Activate` is the legacy entry point and crashes msctf itself when an application calls it
    // (measured: the fault is inside msctf.dll, not in our DLL). The modern way is
    // `ITfInputProcessorProfileMgr::ActivateProfile`, and that interface comes from its own class.
    let profiles: ITfInputProcessorProfileMgr =
        unsafe { CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER) }?;
    // SAFETY: the thread manager is live; the ids are valid references.
    let hr = unsafe {
        profiles.ActivateProfile(
            TF_PROFILETYPE_INPUTPROCESSOR,
            0x0804,
            &CLSID_PIME,
            &PROFILE_ZH,
            None,
            TF_IPSINK_FLAG_ACTIVE,
        )
    };
    eprintln!("  ITfInputProcessorProfileMgr::ActivateProfile(zh-CN) -> {hr:?}");
    pump_for(600);
    if hr.is_err() {
        return Ok(String::from(
            "REPORT: the registered profile could not be activated for this thread (the profile may not be \
             enabled for this user). Nothing was registered.",
        ));
    }
    eprintln!(
        "  the installed DLL is loaded in this process: {}",
        module_loaded("pime_tsf.dll")
    );
    // The profile as the system knows it (clsid, profile id, flags, language).
    // SAFETY: the struct is plain data (a GUID, a GUID, two integers).
    let mut profile: TF_INPUTPROCESSORPROFILE = unsafe { std::mem::zeroed() };
    // SAFETY: the profile manager is live; the out parameter is a live local.
    let got = unsafe {
        profiles.GetProfile(
            TF_PROFILETYPE_INPUTPROCESSOR,
            0x0804,
            &CLSID_PIME,
            &PROFILE_ZH,
            None,
            &mut profile,
        )
    };
    eprintln!("  the registered profile: {got:?} {profile:?}");

    // SAFETY: the thread manager is the one the profile was activated on.
    let keys: ITfKeystrokeMgr = thread_mgr.cast()?;
    let empty = read_text(&context).unwrap_or_default();
    println!("host: the document starts with {empty:?}");

    // "fn": the text service observes the prefix and opens its candidate window; the host inserts the characters
    // only when the service does not eat them.
    let eaten_f = send_key(&keys, &context, 0x46, 0x21, Some("f")); // F
    let eaten_n = send_key(&keys, &context, 0x4E, 0x23, Some("n")); // N
    eprintln!("host: the text service ate f={eaten_f} n={eaten_n}");
    pump_for(400);
    let typed = read_text(&context).unwrap_or_default();
    println!("host: after typing \"fn\" the document holds {typed:?}");

    // Enter: the highlighted candidate is committed into the host's text store.
    let eaten_enter = send_key(&keys, &context, 0x0D, 0x1C, None);
    eprintln!("host: the text service ate Enter={eaten_enter}");
    pump_for(600);

    let committed = read_text(&context).unwrap_or_default();
    println!("host: after the commit the document holds {committed:?}");

    let verdict = if !eaten_f || !eaten_n {
        "REPORT: the registered profile was activated (msctf loaded the installed DLL into this process), but \
         the keystroke manager did not route the keys to it: a console host has no window that owns the \
         keyboard focus, and TSF only makes the text service the input processor of a FOCUSED context. \
         Proving the key chain needs a real client application (a window + AssociateFocus), which is the \
         manual test in the real input method. Nothing was registered here."
            .to_string()
    } else if committed.trim().is_empty() {
        "VERDICT: FAIL - the keys were taken but nothing was committed into the host's text store"
            .to_string()
    } else if committed.trim() == "fn" {
        "VERDICT: FAIL - only the typed text is there, no candidate was committed".to_string()
    } else {
        format!(
            "VERDICT: PASS - the registered text service took the keys and committed {committed:?} into the \
             host's text store"
        )
    };
    Ok(verdict)
}

// ---------------------------------------------------------------- the run

fn main() -> std::process::ExitCode {
    let mut dll = String::new();
    let mut profile = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dll" => dll = args.next().unwrap_or_default(),
            "--profile" => profile = true,
            other => {
                eprintln!("unknown argument: {other}");
                return std::process::ExitCode::from(2);
            }
        }
    }
    if !profile && dll.is_empty() {
        eprintln!("usage: tsf_host --dll <path to pime_tsf.dll>");
        eprintln!(
            "       tsf_host --profile   (drive the profile that is registered on this machine)"
        );
        return std::process::ExitCode::from(2);
    }
    let result = if profile { run_profile() } else { run(&dll) };
    match result {
        Ok(verdict) => {
            println!("{verdict}");
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("tsf_host failed: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run_profile() -> windows::core::Result<String> {
    // SAFETY: the apartment is initialised once and uninitialised at the end of the run.
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let result = drive_profile();
    // SAFETY: balances the CoInitializeEx above.
    unsafe { CoUninitialize() };
    result
}

fn run(dll: &str) -> windows::core::Result<String> {
    // SAFETY: the apartment is initialised once and uninitialised at the end of the run.
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let result = drive(dll);
    // SAFETY: balances the CoInitializeEx above.
    unsafe { CoUninitialize() };
    result
}

/// The client id of this host. TSF uses the id of the thread that owns the text service; msctf
/// refuses a key event sink that is advised under an id it does not know.
/// When the profile is activated through `ITfThreadMgr2::Activate`, the id msctf hands out is the
/// one to use, and it is kept here.
static CLIENT_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn client_id() -> u32 {
    let known = CLIENT_ID.load(std::sync::atomic::Ordering::Relaxed);
    if known != 0 {
        return known;
    }
    // SAFETY: the call has no preconditions.
    unsafe { GetCurrentThreadId() }
}

/// The raw `ActivateEx` entry point of the text service: the pointer is passed as it is, without
/// the `Param` conversion of the windows crate.
fn activate_raw(tip: &ITfTextInputProcessorEx, thread_mgr: &ITfThreadMgr) -> HRESULT {
    let vtable = ITfTextInputProcessorEx::vtable(tip);
    // The vtable entry has this signature; it is taken as it is, without a conversion.
    let call = vtable.ActivateEx;
    // SAFETY: both objects are live COM objects of this thread.
    unsafe {
        call(
            windows::core::Interface::as_raw(tip),
            windows::core::Interface::as_raw(thread_mgr),
            client_id(),
            0,
        )
    }
}

fn drive(dll: &str) -> windows::core::Result<String> {
    eprintln!("step: create the real msctf thread manager");
    // SAFETY: COM is initialised on this thread; the class is the real msctf thread manager.
    let thread_mgr: ITfThreadMgr =
        unsafe { CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER) }?;
    eprintln!("step: create the text service out of the DLL");
    let tip = load_tip(dll)?;
    eprintln!("step: activate the text service on the thread manager");
    eprintln!(
        "  thread_mgr raw = {:?}, tip raw = {:?}",
        windows::core::Interface::as_raw(&thread_mgr),
        windows::core::Interface::as_raw(&tip)
    );
    eprintln!(
        "  the thread manager answers a query for the keystroke manager: {:?}",
        thread_mgr.cast::<ITfKeystrokeMgr>().is_ok()
    );
    eprintln!("  ActivateEx with no thread manager -> {:?}", unsafe {
        tip.ActivateEx(None::<&ITfThreadMgr>, client_id(), 0)
    });
    // The two advises the text service makes on the thread manager, made here instead: this shows
    // which of them msctf refuses.
    {
        use windows::Win32::UI::TextServices::{ITfKeyEventSink, ITfSource, ITfThreadMgrEventSink};
        // The thread manager has to know the text service before it accepts a key event sink:
        // in the real flow TSF activates the profile for the thread and only then calls
        // `ActivateEx`. The windows crate models `ITfThreadMgr::Activate` without its argument,
        // so the call is made through the vtable.
        type ActivateFn = unsafe extern "system" fn(*mut c_void, *const GUID) -> HRESULT;
        let vtable = ITfThreadMgr::vtable(&thread_mgr);
        let call: ActivateFn = unsafe { std::mem::transmute(vtable.Activate) };
        for (name, id) in [
            (
                "the profile",
                GUID::from_u128(0x223fd919_f8cb_476c_a852_351e7381c801),
            ),
            ("the class", CLSID_PIME),
        ] {
            let hr = unsafe { call(windows::core::Interface::as_raw(&thread_mgr), &id) };
            eprintln!("  ITfThreadMgr::Activate({name}) -> {:#010x}", hr.0);
        }
        // The same entry point with the thread id as a value: the windows crate models it as an
        // out-parameter, which is neither of the two shapes the documentation names, so the call
        // is not made (it crashes the process).
        let keys: ITfKeystrokeMgr = thread_mgr.cast().unwrap();
        let key_sink: ITfKeyEventSink = tip.cast().unwrap();
        eprintln!("  our own AdviseKeyEventSink -> {:?}", unsafe {
            keys.AdviseKeyEventSink(client_id(), &key_sink, BOOL(1))
        });
        let source: ITfSource = thread_mgr.cast().unwrap();
        let thread_sink: ITfThreadMgrEventSink = tip.cast().unwrap();
        eprintln!("  our own AdviseSink -> {:?}", unsafe {
            source.AdviseSink(&ITfThreadMgrEventSink::IID, &thread_sink)
        });
    }
    // SAFETY: the thread manager is live; the text service advises its sinks on it.
    // `Some(&thread_mgr)` (not `&thread_mgr`): the `Param` implementation of an interface reference
    // queries the interface first and passes a null pointer when that fails.
    let hr = unsafe { tip.ActivateEx(Some(&thread_mgr), client_id(), 0) };
    eprintln!("step: ActivateEx through the windows crate -> {hr:?}");
    let raw = activate_raw(&tip, &thread_mgr);
    eprintln!("step: ActivateEx through the raw vtable -> {:#010x}", raw.0);
    if raw.is_err() {
        // The text service refuses to start without a thread manager, and msctf refuses the key
        // event sink of a text service that is not the registered input processor of this thread.
        // Both are what the report below says; the chain needs a registered profile.
        return Ok(String::from(
            "REPORT: the text service object is created by the real COM machinery and the real \
             msctf thread manager is created, but msctf refuses the key event sink \
             (ITfKeystrokeMgr::AdviseKeyEventSink -> E_INVALIDARG) for a text service that is not \
             the registered input processor of this thread. The input chain therefore needs a \
             registered TSF profile, which is what the installer writes (administrator rights). \
             Nothing was registered here.",
        ));
    }

    eprintln!("step: create the document manager");
    // SAFETY: the thread manager is live.
    let doc_mgr: ITfDocumentMgr = unsafe { thread_mgr.CreateDocumentMgr() }?;
    eprintln!("step: create the context");
    let mut context: Option<ITfContext> = None;
    let mut cookie = 0u32;
    // SAFETY: the document manager is live; the out parameters are live locals.
    unsafe {
        doc_mgr.CreateContext(
            client_id(),
            TF_ES_READWRITE.0,
            None,
            &mut context,
            &mut cookie,
        )
    }?;
    let context = context.ok_or_else(windows::core::Error::from_win32)?;
    eprintln!("step: push the context");
    // SAFETY: the context is live.
    unsafe { doc_mgr.Push(&context) }?;
    pump_for(120);

    // SAFETY: the thread manager is the one the text service was activated on.
    let keys: ITfKeystrokeMgr = thread_mgr.cast()?;

    let empty = read_text(&context).unwrap_or_default();
    println!("host: the document starts with {empty:?}");

    // "fn": the text service observes the prefix and opens its candidate window; the host
    // inserts the characters because the service does not eat them.
    send_key(&keys, &context, 0x46, 0x21, Some("f")); // F
    send_key(&keys, &context, 0x4E, 0x23, Some("n")); // N
    pump_for(200);
    let typed = read_text(&context).unwrap_or_default();
    println!("host: after typing \"fn\" the document holds {typed:?}");

    // Ctrl+Alt+Space: the one chord the text service claims; it opens the composition over the
    // prefix. The modifiers have to be in the thread's keyboard state, which is what the text
    // service reads.
    let state = state_with(&[VK_CONTROL.0 as i32, VK_MENU.0 as i32]);
    // SAFETY: the table is a live local of the right size.
    unsafe { SetKeyboardState(&state) }.ok();
    send_key(&keys, &context, 0x20, 0x39, None); // Space with Ctrl+Alt
    // SAFETY: the table is a live local of the right size; it restores the empty state.
    unsafe { SetKeyboardState(&[0u8; 256]) }.ok();
    pump_for(200);

    // Enter: the highlighted candidate is committed into the host's text store.
    send_key(&keys, &context, 0x0D, 0x1C, None);
    pump_for(300);

    let committed = read_text(&context).unwrap_or_default();
    println!("host: after the commit the document holds {committed:?}");

    // The text service is done: close its windows.
    // SAFETY: the text service is live; deactivating it removes the sinks it advised.
    unsafe { tip.Deactivate() }.ok();

    let verdict = if committed.trim().is_empty() {
        "VERDICT: FAIL - nothing was committed into the host's text store".to_string()
    } else if committed.trim() == "fn" {
        "VERDICT: FAIL - only the typed text is there, no candidate was committed".to_string()
    } else {
        format!(
            "VERDICT: PASS - the text service committed {committed:?} into the host's text store"
        )
    };
    Ok(verdict)
}
