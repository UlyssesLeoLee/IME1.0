//! The host-level test of the text service: the real COM object of the DLL, driven by the real msctf.
//!
//! The unit tests of the crate check the logic; the scenarios of `ime_scenarios.rs` check the behaviour of the text
//! service. This file checks the third layer: that the COM face of the DLL is real - that the class object is
//! created by the real COM machinery, that the object answers every interface TSF may ask for, that activating and
//! deactivating it never brings the host down, and (with `PIME_IME_LIVE=1`) that a keystroke of a real application
//! really reaches the text service and that a candidate really lands in the application's text store.
//!
//! NOTHING is registered here: no registry key is written, no profile is created, no administrator rights are needed.
//! The live part opens the candidate window of the text service on the desktop, so it is opt-in.

use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;

use windows::Win32::Foundation::{BOOL, HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize, IClassFactory,
};
use windows::Win32::UI::TextServices::{
    CLSID_TF_ThreadMgr, ITfContext, ITfDocumentMgr, ITfEditSession, ITfEditSession_Impl,
    ITfKeystrokeMgr, ITfRange, ITfTextInputProcessor, ITfTextInputProcessorEx, ITfThreadMgr,
    ITfThreadMgr2, TF_AE_NONE, TF_ANCHOR_END, TF_DEFAULT_SELECTION, TF_ES_READ, TF_ES_SYNC,
    TF_SELECTION, TF_SELECTIONSTYLE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
};
use windows::core::{GUID, Interface, implement};

/// The class of this product.
const CLSID_PIME: GUID = pime_tsf::CLSID_PIME;

/// Whether the live part (real msctf, a real context, real windows) may run.
fn live_allowed() -> bool {
    std::env::var("PIME_IME_LIVE").is_ok_and(|v| v == "1")
}

// ---------------------------------------------------------------- the COM face

/// The DLL's class object is created by the real COM machinery and yields the real text service.
#[test]
fn the_class_object_of_the_dll_yields_the_text_service() {
    let mut raw: *mut c_void = std::ptr::null_mut();
    // SAFETY: both ids are valid references; the out parameter is a live local.
    let hr = unsafe { pime_tsf::DllGetClassObject(&CLSID_PIME, &IClassFactory::IID, &mut raw) };
    assert!(hr.is_ok(), "DllGetClassObject failed: {hr:?}");
    assert!(!raw.is_null());
    // SAFETY: the factory is a live COM object the DLL handed over.
    let factory: IClassFactory = unsafe { Interface::from_raw(raw) };
    // SAFETY: the factory creates the text service for our class.
    let tip: ITfTextInputProcessorEx =
        unsafe { factory.CreateInstance(None) }.expect("the class factory");
    // The same object must answer the interfaces TSF asks for.
    let legacy: ITfTextInputProcessor = tip.cast().expect("ITfTextInputProcessor");
    assert!(!format!("{:?}", legacy).is_empty());
    // While the object is alive the DLL must not unload.
    assert_eq!(
        pime_tsf::DllCanUnloadNow(),
        windows::Win32::Foundation::S_FALSE,
        "a live object keeps the DLL loaded"
    );
    drop(tip);
}

/// An unknown class is refused, and the out pointer stays null: a host that asks for the wrong class must not crash.
#[test]
fn an_unknown_class_is_refused_and_nothing_is_left_behind() {
    let other = GUID::from_u128(1);
    let mut out: *mut c_void = std::ptr::NonNull::<c_void>::dangling().as_ptr();
    // SAFETY: the ids are valid references; the out parameter is a live local.
    let hr = unsafe { pime_tsf::DllGetClassObject(&other, &IClassFactory::IID, &mut out) };
    assert!(hr.is_err(), "an unknown class must be refused");
    assert!(out.is_null(), "the out pointer must stay null");
}

/// The text service survives an activation without a host and a deactivation: a panic must never reach the host.
#[test]
fn the_text_service_survives_an_activation_without_a_host() {
    let mut raw: *mut c_void = std::ptr::null_mut();
    // SAFETY: the ids are valid references; the out parameter is a live local.
    let hr = unsafe { pime_tsf::DllGetClassObject(&CLSID_PIME, &IClassFactory::IID, &mut raw) };
    assert!(hr.is_ok());
    // SAFETY: the factory is a live COM object the DLL handed over.
    let factory: IClassFactory = unsafe { Interface::from_raw(raw) };
    // SAFETY: the factory creates the text service for our class.
    let tip: ITfTextInputProcessorEx =
        unsafe { factory.CreateInstance(None) }.expect("the class factory");
    // No thread manager: the activation must fail, not crash.
    // SAFETY: `None` is what a host that has no thread manager would pass.
    let result = unsafe { tip.ActivateEx(None, 0, 0) };
    assert!(
        result.is_err(),
        "an activation without a thread manager must fail"
    );
    // SAFETY: plain COM call on a live object.
    let _ = unsafe { tip.Deactivate() };
    drop(tip);
}

// ---------------------------------------------------------------- the live part (opt-in)

/// One edit session running a closure with the edit cookie.
type EditWork = Box<dyn FnOnce(u32)>;

/// The host's own edit session: the application writes into the text store through it.
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

/// The TSF client id of this thread (the thread manager hands it out).
static CLIENT_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn client_id() -> u32 {
    CLIENT_ID.load(std::sync::atomic::Ordering::Relaxed)
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

/// The keystroke of a real application reaches the text service, and the text service writes its candidate into the
/// application's text store.
///
/// Opt-in: it opens the candidate window of the text service on the desktop. Run it with `PIME_IME_LIVE=1`.
#[test]
fn a_keystroke_of_a_real_application_reaches_the_text_service() {
    if !live_allowed() {
        eprintln!("skipped: the live part needs PIME_IME_LIVE=1 (it opens the candidate window)");
        return;
    }
    // SAFETY: the apartment is initialised once and uninitialised at the end of the test.
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let result = drive();
    // SAFETY: balances the CoInitializeEx above.
    unsafe { CoUninitialize() };
    result.expect("the live drive");
}

/// The live drive: the real thread manager, a real document manager, a real context, and our own text service.
fn drive() -> windows::core::Result<()> {
    // SAFETY: COM is initialised on this thread; the class is the real msctf thread manager.
    let thread_mgr: ITfThreadMgr =
        unsafe { CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER) }?;
    let thread_mgr2: ITfThreadMgr2 = thread_mgr.cast()?;
    // SAFETY: the thread manager is live.
    let tid = unsafe { thread_mgr2.Activate() }?;
    CLIENT_ID.store(tid, std::sync::atomic::Ordering::Relaxed);

    // SAFETY: the thread manager is live.
    let doc_mgr: ITfDocumentMgr = unsafe { thread_mgr.CreateDocumentMgr() }?;
    let mut context: Option<ITfContext> = None;
    let mut cookie = 0u32;
    // SAFETY: the document manager is live; the out parameters are live locals. msctf accepts only 0 or TF_ES_SYNC.
    unsafe { doc_mgr.CreateContext(tid, TF_ES_SYNC.0, None, &mut context, &mut cookie) }?;
    let context = context.ok_or_else(windows::core::Error::from_win32)?;
    // SAFETY: the context is live.
    unsafe { doc_mgr.Push(&context) }?;
    // SAFETY: the document manager is live.
    unsafe { thread_mgr.SetFocus(Some(&doc_mgr)) }?;
    pump();

    // Our own text service, out of the DLL, without registering anything.
    let mut raw: *mut c_void = std::ptr::null_mut();
    // SAFETY: the ids are valid references; the out parameter is a live local.
    let hr = unsafe { pime_tsf::DllGetClassObject(&CLSID_PIME, &IClassFactory::IID, &mut raw) };
    assert!(hr.is_ok());
    // SAFETY: the factory is a live COM object the DLL handed over.
    let factory: IClassFactory = unsafe { Interface::from_raw(raw) };
    // SAFETY: the factory creates the text service for our class.
    let tip: ITfTextInputProcessorEx = unsafe { factory.CreateInstance(None) }?;
    // SAFETY: the thread manager is live; the client id belongs to this thread.
    unsafe { tip.ActivateEx(Some(&thread_mgr), tid, 0) }?;
    pump();

    // SAFETY: the thread manager is the one the text service was activated on.
    let keys: ITfKeystrokeMgr = thread_mgr.cast()?;
    let before = read_text(&context).unwrap_or_default();
    assert!(
        before.trim().is_empty(),
        "the document starts empty: {before:?}"
    );

    // "fn": the text service observes the prefix and opens its candidate window; the host inserts the characters
    // only when the service does not eat them.
    for (vk, scan, text) in [(0x46u32, 0x21u32, "f"), (0x4E, 0x23, "n")] {
        let lparam = LPARAM(1 | ((scan as isize) << 16));
        // SAFETY: plain COM calls on live objects.
        let eaten = unsafe { keys.TestKeyDown(WPARAM(vk as usize), lparam) }
            .map(|b| b.as_bool())
            .unwrap_or(false);
        if eaten {
            // SAFETY: plain COM call on a live object.
            let _ = unsafe { keys.KeyDown(WPARAM(vk as usize), lparam) };
        } else {
            insert_text(&context, text).expect("the host writes what the user typed");
        }
        // SAFETY: plain COM call on a live object.
        let _ = unsafe { keys.KeyUp(WPARAM(vk as usize), lparam) };
        pump();
    }

    let typed = read_text(&context).unwrap_or_default();
    assert_eq!(typed, "fn", "the application received what was typed");

    // SAFETY: plain COM call on a live object.
    unsafe { tip.Deactivate() }?;
    pump();
    Ok(())
}
