//! The live harness of the **real** input method: a real window that owns the keyboard focus, the real `msctf.dll`,
//! the profile of this product that is registered on this machine, and the real keyboard.
//!
//! What the older probes measured, and what this harness adds:
//!
//! * `examples/tsf_host.rs --profile` could activate the registered zh-CN profile, but its console host has no window
//!   that owns the keyboard focus, and TSF only makes a text service the input processor of a FOCUSED context.
//! * The console host also asked for the keys with `ITfKeystrokeMgr::TestKeyDown` **outside** of a window procedure.
//!   A real application asks TSF about the key **in its window procedure**, and the window procedure of this harness
//!   does exactly that: it asks, and it writes the character into its own text store when the answer is "not mine".
//! * The window has to be in front before the document is associated with it (a real application associates the
//!   document in `WM_CREATE`, long before the window is in front): the harness therefore creates the window, builds
//!   the TSF objects, associates them, and only then takes the focus.
//!
//! Nothing is registered here: the profile that is driven is the one that is already registered (the installer wrote
//! it). The harness writes nothing to the registry and needs no administrator rights.
//!
//! Measured on this machine (2026-11-19, the facts the regression test keeps):
//!
//! 1. `ITfInputProcessorProfileMgr::ActivateProfile(zh-CN)` succeeds, msctf loads the **installed** `pime_tsf.dll`
//!    into the process, and the input method really comes alive: its windows (`ProgrammingIME.Glow`,
//!    `ProgrammingIME.LaserPanel`, `ProgrammingIME.Beam`, `PIME1AgentBridge`) belong to our thread.
//! 2. The text service holds the **key event sink** of the client id of the thread: a second advise of a sink of the
//!    host is refused by msctf with `E_INVALIDARG` (one key event sink per client id).
//! 3. The real keyboard reaches the window procedure of the host, and the window procedure asks TSF about the key.
//! 4. **The key does not reach the text service**: `TestKeyDown` answers "not mine" for a letter, for the space and
//!    for `Ctrl+F12` (the chord that switches the input mode, which the text service must always take). See D-170.

use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;

use windows::Win32::Foundation::{BOOL, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    IClassFactory,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ActivateKeyboardLayout, GetAsyncKeyState, GetKeyboardLayout, HKL, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, GetFocus, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, LoadKeyboardLayoutW, SendInput, SetFocus,
    SetKeyboardState, ToUnicodeEx, VIRTUAL_KEY, VK_CONTROL,
};
use windows::Win32::UI::TextServices::{
    CLSID_TF_InputProcessorProfiles, CLSID_TF_ThreadMgr, ITfContext, ITfDocumentMgr, ITfEditSession,
    ITfEditSession_Impl, ITfInputProcessorProfileMgr, ITfKeyEventSink, ITfKeyEventSink_Impl,
    ITfKeystrokeMgr, ITfRange, ITfTextInputProcessorEx, ITfThreadMgr, ITfThreadMgr2, TF_AE_NONE,
    TF_ANCHOR_END, TF_DEFAULT_SELECTION, TF_ES_READ, TF_ES_READWRITE, TF_ES_SYNC,
    TF_IPSINK_FLAG_ACTIVE, TF_PROFILETYPE_INPUTPROCESSOR, TF_SELECTION, TF_SELECTIONSTYLE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, EnumThreadWindows, EnumWindows,
    GetClassNameW, GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, MSG, PM_REMOVE, PeekMessageW, PostMessageW, RegisterClassW,
    SendMessageW, SetForegroundWindow, ShowWindow, TranslateMessage, SW_SHOW, WM_CLOSE,
    WM_COPYDATA, WM_INPUTLANGCHANGEREQUEST, WM_KEYDOWN, WM_SETFOCUS, WNDCLASSW,
    WINDOW_EX_STYLE, WINDOW_STYLE, WS_EX_TOOLWINDOW, WS_OVERLAPPEDWINDOW,
};
use windows::core::{GUID, HRESULT, Interface, PCWSTR, implement};

/// The COM class of this product (`lib.rs`: CLSID_PIME).
pub const CLSID_PIME: GUID = GUID::from_u128(0x43f95ed1_95b4_421d_8f5a_837128c2d854);
/// The zh-CN language profile of this product (`lib.rs`: PROFILE_ZH).
pub const PROFILE_ZH: GUID = GUID::from_u128(0x223fd919_f8cb_476c_a852_351e7381c801);
/// The language id of zh-CN.
pub const LANG_ZH_CN: u16 = 0x0804;

/// The COM class of the Microsoft Chinese (Simplified) text service (the control experiment of D-170).
pub const CLSID_MS_CHINESE: GUID = GUID::from_u128(0x81d4e9c9_1d3b_41bc_9e6c_4b40bf79e35e);
/// The zh-CN language profile of the Microsoft Chinese (Simplified) text service.
pub const PROFILE_MS_CHINESE: GUID = GUID::from_u128(0xfa550b04_5ad7_411f_a5ac_ca038ec515d7);

/// The window class of the host.
const HOST_CLASS: &str = "PimeImeLiveHost";
/// The window class of the glow of the laser interface (the sign that the input method came alive).
pub const GLOW_CLASS: &str = "ProgrammingIME.Glow";

// ---------------------------------------------------------------- the edit session (the host writes through it)

type EditWork = Box<dyn FnOnce(u32)>;

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

/// Runs `work` in an edit session on `ctx`.
///
/// msctf refuses a session whose flags it does not accept, so the flags that MSDN names are tried one after the other.
fn session<T: 'static>(
    ctx: &ITfContext,
    flags: &[u32],
    work: impl Fn(u32) -> windows::core::Result<T> + 'static,
) -> windows::core::Result<T> {
    let out: Rc<RefCell<Option<windows::core::Result<T>>>> = Rc::new(RefCell::new(None));
    let work: Rc<dyn Fn(u32) -> windows::core::Result<T>> = Rc::new(work);
    for flag in flags {
        let sink = Rc::clone(&out);
        let work = Rc::clone(&work);
        let session: ITfEditSession = Editor {
            work: RefCell::new(Some(Box::new(move |ec| {
                if sink.borrow().is_none() {
                    *sink.borrow_mut() = Some(work(ec));
                }
            }))),
        }
        .into();
        // SAFETY: `ctx` and `session` are live COM objects; a synchronous session runs before the call returns.
        let requested = unsafe {
            ctx.RequestEditSession(
                client_id(),
                &session,
                windows::Win32::UI::TextServices::TF_CONTEXT_EDIT_CONTEXT_FLAGS(*flag),
            )
        };
        match requested {
            Ok(hr) if hr.is_ok() => {
                if let Some(value) = out.borrow_mut().take() {
                    return value;
                }
            }
            Ok(_) | Err(_) => {}
        }
    }
    Err(windows::core::Error::from_win32())
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
    session(
        ctx,
        &[TF_ES_READWRITE.0, TF_ES_SYNC.0 | TF_ES_READWRITE.0, TF_ES_SYNC.0],
        move |ec| {
            let range = caret(&inner, ec)?;
            // SAFETY: `ec` belongs to this session; the range is a live COM object of the same context.
            unsafe {
                range.SetText(ec, 0, &chars)?;
                range.Collapse(ec, TF_ANCHOR_END)?;
                // The caret of the context has to follow the text that was written, or the next character would
                // replace this one instead of standing behind it.
                let selection = [TF_SELECTION {
                    range: std::mem::ManuallyDrop::new(Some(range.clone())),
                    style: TF_SELECTIONSTYLE {
                        ase: TF_AE_NONE,
                        fInterimChar: BOOL(0),
                    },
                }];
                inner.SetSelection(ec, &selection)?;
            }
            Ok(())
        },
    )
}

/// The whole text of the context, read back through a read session.
fn read_text(ctx: &ITfContext) -> windows::core::Result<String> {
    let inner = ctx.clone();
    session(
        ctx,
        &[TF_ES_READ.0, TF_ES_SYNC.0 | TF_ES_READ.0, TF_ES_SYNC.0],
        move |ec| {
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
        },
    )
}

// ---------------------------------------------------------------- the window that owns the keyboard focus

// What the window procedure needs: the keystroke manager of the thread, the context of the document, and the
// document manager (a real application associates the document with the window and sets the focus of TSF **in its
// window procedure**, when the window receives WM_SETFOCUS).
thread_local! {
    static HOST: RefCell<Option<(ITfKeystrokeMgr, ITfContext, ITfThreadMgr, ITfDocumentMgr)>> =
        const { RefCell::new(None) };
}

/// The character a key press produces with the active layout, the way the text service reads it.
fn produced_char(code: u32, scan: u32) -> Option<char> {
    let state = [0u8; 256];
    let mut buffer = [0u16; 8];
    // SAFETY: the buffers are live for the call.
    let written = unsafe { ToUnicodeEx(code, scan, &state, &mut buffer, 4, GetKeyboardLayout(0)) };
    match written {
        1 => char::from_u32(u32::from(buffer[0])),
        _ => None,
    }
}

/// The character of a real key message.
fn char_of(wparam: WPARAM, lparam: LPARAM) -> Option<char> {
    let scan = ((lparam.0 >> 16) & 0xFF) as u32;
    produced_char(wparam.0 as u32, scan)
}

// The keys the window procedure received: the regression test reads them.
thread_local! {
    static RECEIVED: RefCell<Vec<bool>> = const { RefCell::new(Vec::new()) };
}

/// The keyboard layout of the thread, switched to zh-CN **before** the window is created.
///
/// The input processor that msctf makes the input processor of a thread is the one whose language profile matches the
/// keyboard layout of the focused window: a user who types Chinese has the Chinese layout there. The window inherits
/// the layout of the thread when it is created, so the switch has to come first.
fn use_the_chinese_layout() -> Option<HKL> {
    let name: Vec<u16> = "00000804".encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: the name is a null-terminated wide string that outlives the call.
    let hkl = unsafe {
        LoadKeyboardLayoutW(
            PCWSTR(name.as_ptr()),
            windows::Win32::UI::Input::KeyboardAndMouse::ACTIVATE_KEYBOARD_LAYOUT_FLAGS(0),
        )
    }
    .ok()?;
    // SAFETY: the layout is loaded; the call has no other precondition.
    let active = unsafe {
        ActivateKeyboardLayout(
            hkl,
            windows::Win32::UI::Input::KeyboardAndMouse::ACTIVATE_KEYBOARD_LAYOUT_FLAGS(0),
        )
    }
    .ok()?;
    Some(active)
}

// The answer of the bridge of the input method (the window of the agent protocol).
thread_local! {
    static REPLY: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The `CopyDataStruct` of Windows, declared here instead of taken from the `windows` crate.
///
/// The Windows definition is `ULONG_PTR dwData; DWORD cbData; PVOID lpData;` - on x64 that is 24 bytes with the
/// data pointer at 16. The crate's own bridge declares it the same way (the struct travels between two processes, and
/// a layout that is one byte off would make the message carry nonsense).
#[repr(C)]
#[derive(Clone, Copy)]
struct CopyDataStruct {
    dw_data: usize,
    cb_data: u32,
    lp_data: *mut c_void,
}

/// The class of the bridge window of the input method (one per process that has the input method active).
pub const BRIDGE_CLASS: &str = "PIME1AgentBridge";
/// The `dwData` of a request of the bridge.
pub const COPYDATA_REQUEST: u32 = 0x5049_4D01;
/// The `dwData` of the answer of the bridge.
pub const COPYDATA_REPLY: u32 = 0x5049_4D02;

/// The window of the bridge of the input method, **wherever it is**: the window belongs to the process that has the
/// input method active - in the real Notepad that is the Notepad's own process, not ours.
pub fn bridge_window_anywhere() -> Option<HWND> {
    let mut found: Option<HWND> = None;
    // SAFETY: the pointer is a live local for the whole enumeration.
    unsafe {
        let _ = EnumWindows(
            Some(find_bridge_window),
            LPARAM((&mut found as *mut Option<HWND>) as isize),
        );
    }
    found
}

/// Asks the bridge of the input method and returns its answer. The answer goes to the window `reply_to` names.
pub fn ask_the_bridge(reply_to: HWND, payload: &str) -> Option<String> {
    let bridge = bridge_window_anywhere()?;
    REPLY.with(|slot| *slot.borrow_mut() = None);
    let mut bytes = payload.as_bytes().to_vec();
    let data = CopyDataStruct {
        dw_data: COPYDATA_REQUEST as usize,
        cb_data: bytes.len() as u32,
        lp_data: bytes.as_mut_ptr().cast(),
    };
    // SAFETY: the payload lives until the call returns; the bridge answers the window we name.
    unsafe {
        let _ = SendMessageW(
            bridge,
            WM_COPYDATA,
            WPARAM(reply_to.0 as usize),
            LPARAM((&data as *const CopyDataStruct) as isize),
        );
    }
    pump_for(200);
    REPLY.with(|slot| slot.borrow().clone())
}

unsafe extern "system" fn find_bridge_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let mut buffer = [0u16; 64];
    // SAFETY: the buffer is live and large enough for the class name.
    let len = unsafe { GetClassNameW(hwnd, &mut buffer) };
    if len > 0 {
        let class = String::from_utf16_lossy(&buffer[..len as usize]);
        if class == BRIDGE_CLASS {
            // SAFETY: the pointer was made from a live local in `bridge_window`.
            let slot = unsafe { &mut *(lparam.0 as *mut Option<HWND>) };
            *slot = Some(hwnd);
            return BOOL(0); // one is enough
        }
    }
    BOOL(1)
}

/// What the host knows about the input method, asked through the bridge of the agent protocol.
///
/// The bridge is the product's own channel to the text service: it answers what the input method is doing (its
/// language and its input mode) and it can write text into the focused document. Both are asked here so that the
/// test does not have to guess.
impl EditHost {
    /// The handle of the edit control of the host (the document the input method writes into).
    pub fn edit_window(&self) -> HWND {
        self.edit
    }

    /// Asks the bridge of the input method and returns its answer.
    pub fn ask_the_bridge(&self, payload: &str) -> Option<String> {
        ask_the_bridge(self.window.hwnd, payload)
    }

    /// Asks the bridge what the input method is doing: the answer names the language and the mode.
    pub fn the_mode_of_the_input_method(&self) -> Option<String> {
        self.ask_the_bridge(&format!(
            "{{\"v\":1,\"op\":\"status\",\"reply\":{}}}",
            self.window.hwnd.0 as usize
        ))
    }

    /// Asks the bridge to write `text` into the focused document (the path of the agents).
    pub fn ask_the_bridge_to_write(&self, text: &str) -> Option<String> {
        let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
        self.ask_the_bridge(&format!(
            "{{\"v\":1,\"op\":\"insert\",\"text\":\"{escaped}\",\"reply\":{}}}",
            self.window.hwnd.0 as usize
        ))
    }
}

/// Runs the drive of the edit control **in a child process** and returns what it printed.
///
/// The drive needs a clean process: the input method is activated for the thread that has the focus, and the other
/// cases of this file activate it too (and deactivate it again when they end) ⇒ in one process the state of the
/// input method is whatever the case before left behind. The child starts fresh, and the report comes back through
/// its output.
pub fn run_the_drive_in_a_child(arguments: &[&str]) -> String {
    // SAFETY: the call has no preconditions.
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return format!("the test binary could not be found: {e}"),
    };
    let mut command = std::process::Command::new(exe);
    command.args(arguments);
    // The report is printed by the child: the test harness must not swallow it.
    command.arg("--nocapture");
    command.env("PIME_IME_CHILD", "1");
    command.env("PIME_IME_LIVE", "1");
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return format!("the child could not be started: {e}"),
    };
    match child.wait_with_output() {
        Ok(output) => format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
        Err(e) => format!("the child could not be waited for: {e}"),
    }
}

// ---------------------------------------------------------------- the real notepad

/// A host that is the **real Notepad** of Windows: the application the user types in.
///
/// The evidence is the file itself: the Notepad is opened with a file, the keys are typed on the real keyboard, the
/// file is saved, and the text of the file says what the input method committed. Nothing of TSF is done by the test
/// here - the Notepad is the application, exactly like in the manual acceptance.
pub struct NotepadHost {
    file: std::path::PathBuf,
    window: HWND,
    child: std::process::Child,
}

impl NotepadHost {
    /// Opens the Notepad with a file of its own and puts it in front.
    pub fn open() -> windows::core::Result<Self> {
        let file = std::env::temp_dir().join("pime-ime-live-notepad.txt");
        // The file starts with a known text: the Notepad saves it without asking anything.
        std::fs::write(&file, "start:").ok();
        // SAFETY: the call has no preconditions.
        let child = std::process::Command::new("C:\\Windows\\System32\\notepad.exe")
            .arg(&file)
            .spawn()
            .map_err(|e| windows::core::Error::from_win32())?;
        // The Notepad needs a moment; its window carries the name of the file.
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let window = wait_for_a_window_whose_title_has(&name, 20_000)
            .ok_or_else(windows::core::Error::from_win32)?;
        let host = Self { file, window, child };
        host.put_in_front();
        Ok(host)
    }

    /// Brings the Notepad in front and gives it the keyboard focus.
    ///
    /// `SetForegroundWindow` is refused for a process that does not own the foreground (the foreground lock). A **real
    /// mouse click** on the window of the application is what gives the foreground to a background process, so the
    /// harness clicks on the text area of the Notepad.
    pub fn put_in_front(&self) {
        // The window is raised above everything: a window that is behind another one never receives the click.
        // SAFETY: the window is live.
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowPos(
                self.window,
                windows::Win32::UI::WindowsAndMessaging::HWND_TOPMOST,
                0,
                0,
                0,
                0,
                windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                    | windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE
                    | windows::Win32::UI::WindowsAndMessaging::SWP_SHOWWINDOW,
            );
            let other = thread_of_window(self.window);
            let mine = GetCurrentThreadId();
            if other != 0 && other != mine {
                let _ = AttachThreadInput(mine, other, true);
            }
            let _ = SetForegroundWindow(self.window);
            let _ = SetFocus(self.window);
            if other != 0 && other != mine {
                let _ = AttachThreadInput(mine, other, false);
            }
        }
        pump_for(200);
        // the click: into the text area of the Notepad (below the tab bar of the window)
        let mut rect = windows::Win32::Foundation::RECT::default();
        // SAFETY: the window is live; the rectangle is a live local.
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::GetWindowRect(self.window, &mut rect);
        }
        let x = rect.left + 200;
        let y = rect.top + 140;
        click_at(x, y);
        pump_for(400);
    }

    /// Whether the Notepad really is the window in front (the keys only go there).
    pub fn is_in_front(&self) -> bool {
        // SAFETY: the call has no preconditions.
        unsafe { GetForegroundWindow() == self.window }
    }

    /// The process of the Notepad.
    pub fn process_id(&self) -> u32 {
        process_of_window(self.window)
    }

    /// Where the window is, and how big the screen is (the click needs both).
    pub fn geometry(&self) -> (i32, i32, i32, i32, i32, i32) {
        let mut rect = windows::Win32::Foundation::RECT::default();
        // SAFETY: the window is live; the rectangle is a live local.
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::GetWindowRect(self.window, &mut rect);
        }
        // SAFETY: the calls have no preconditions.
        let (width, height) = unsafe {
            (
                windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
                    windows::Win32::UI::WindowsAndMessaging::SM_CXSCREEN,
                ),
                windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
                    windows::Win32::UI::WindowsAndMessaging::SM_CYSCREEN,
                ),
            )
        };
        (rect.left, rect.top, rect.right, rect.bottom, width, height)
    }

    /// Types `text` on the real keyboard (the Notepad has the focus).
    pub fn type_real(&self, text: &str) {
        for ch in text.chars() {
            let (vk, scan) = key_of(ch);
            if vk == 0 {
                continue;
            }
            send_real_key(vk, scan);
            pump_for(200);
        }
    }

    /// Presses `Ctrl+S` (save) and waits a moment.
    pub fn save(&self) {
        send_virtual_key(0x11, false); // the control key
        send_real_key(0x53, 0x1D); // S
        send_virtual_key(0x11, true);
        pump_for(1200);
    }

    /// Presses `Ctrl+A` (select everything).
    pub fn select_all(&self) {
        send_virtual_key(0x11, false); // the control key
        send_real_key(0x41, 0x1E); // A
        send_virtual_key(0x11, true);
        pump_for(400);
    }

    /// The text of the file: what the Notepad saved.
    pub fn what_was_saved(&self) -> String {
        std::fs::read_to_string(&self.file).unwrap_or_default()
    }

    /// Closes the Notepad.
    pub fn close(&mut self) {
        // SAFETY: the window is live.
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                self.window,
                WM_CLOSE,
                WPARAM(0),
                LPARAM(0),
            );
        }
        let _ = self.child.kill();
        pump_for(300);
    }
}

/// The thread that owns a window (its input queue is the one that gets the keys).
fn thread_of_window(hwnd: HWND) -> u32 {
    // SAFETY: the call has no preconditions.
    unsafe { GetWindowThreadProcessId(hwnd, None) }
}

/// The first top level window whose title has `part` in it.
fn wait_for_a_window_whose_title_has(part: &str, timeout_ms: u64) -> Option<HWND> {
    let end = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    while std::time::Instant::now() < end {
        if let Some(hwnd) = find_a_window_whose_title_has(part) {
            return Some(hwnd);
        }
        pump_for(200);
    }
    None
}

fn find_a_window_whose_title_has(part: &str) -> Option<HWND> {
    let mut found: Option<HWND> = None;
    // the callback reads the part out of this thread's memory
    TITLE_PART.with(|p| *p.borrow_mut() = part.to_string());
    // SAFETY: the pointer is a live local for the whole enumeration.
    unsafe {
        let _ = EnumWindows(
            Some(find_by_title),
            LPARAM((&mut found as *mut Option<HWND>) as isize),
        );
    }
    found
}

thread_local! {
    static TITLE_PART: RefCell<String> = const { RefCell::new(String::new()) };
}

unsafe extern "system" fn find_by_title(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: the pointer was made from a live local in `find_a_window_whose_title_has`.
    let slot = unsafe { &mut *(lparam.0 as *mut Option<HWND>) };
    let mut buffer = [0u16; 512];
    // SAFETY: the buffer is live and large enough for the title.
    let len = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    if len > 0 {
        let title = String::from_utf16_lossy(&buffer[..len as usize]);
        let wanted = TITLE_PART.with(|p| p.borrow().clone());
        if !wanted.is_empty() && title.contains(&wanted) {
            *slot = Some(hwnd);
            return BOOL(0);
        }
    }
    BOOL(1)
}

/// A window of the host that only receives the answer of the bridge.
///
/// It is needed when the input method lives in **another process** (the real Notepad): the answer of the bridge comes
/// to the window the request names.
pub struct ReplyWindow {
    window: FocusWindow,
}

impl ReplyWindow {
    /// Opens the window.
    pub fn open() -> Option<Self> {
        FocusWindow::open().ok().map(|window| Self { window })
    }

    /// The handle of the window.
    pub fn hwnd(&self) -> HWND {
        self.window.hwnd
    }

    /// Asks the bridge of the input method (wherever it is) and returns its answer.
    pub fn ask(&self, payload: &str) -> Option<String> {
        ask_the_bridge(self.window.hwnd, payload)
    }
}

/// The process that owns a window.
pub fn process_of_window(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: the call has no preconditions; the out parameter is a live local.
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    pid
}

/// Clicks the mouse at a point of the screen: the click is what gives the foreground to a background process.
pub fn click_at(x: i32, y: i32) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEINPUT,
    };
    // The cursor goes to the point in **pixels** (the normalized coordinates of SendInput would need the size of the
    // screen, and the virtual screen may start somewhere else).
    // SAFETY: the call has no preconditions.
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::SetCursorPos(x, y);
    }
    pump_for(100);
    let press = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_LEFTDOWN,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let release = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_LEFTUP,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    // SAFETY: the inputs are live locals of the right size.
    unsafe {
        let _ = SendInput(
            &[press, release],
            std::mem::size_of::<INPUT>() as i32,
        );
    }
    pump_for(200);
}

/// The window procedure of the host: it asks TSF about every key, the way every application does.
unsafe extern "system" fn host_window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_COPYDATA {
        // The answer of the bridge of the input method (the protocol of the agents).
        // SAFETY: the message carries a structure that lives while it is handled.
        let data = unsafe { &*(lparam.0 as *const CopyDataStruct) };
        if data.dw_data == COPYDATA_REPLY as usize && !data.lp_data.is_null() {
            // SAFETY: Windows points at `cbData` bytes of payload that live while the message is handled.
            let bytes = unsafe { std::slice::from_raw_parts(data.lp_data as *const u8, data.cb_data as usize) };
            let text = String::from_utf8_lossy(bytes);
            let text = text.trim_end_matches('\0').to_string();
            REPLY.with(|slot| *slot.borrow_mut() = Some(text));
            return LRESULT(1);
        }
        return LRESULT(0);
    }
    if msg == WM_SETFOCUS {
        // The window has the keyboard focus now: this is where a real application tells TSF that the context of
        // this document is the active one.
        HOST.with(|slot| {
            let borrowed = slot.borrow();
            let Some((_, _, thread_mgr, doc_mgr)) = borrowed.as_ref() else {
                return;
            };
            // SAFETY: the thread manager and the document manager are live.
            unsafe {
                let _ = thread_mgr.AssociateFocus(hwnd, doc_mgr);
                let _ = thread_mgr.SetFocus(Some(doc_mgr));
            }
        });
        eprintln!("    the window procedure got the focus");
        return LRESULT(0);
    }
    if msg == WM_KEYDOWN {
        let eaten = HOST.with(|slot| {
            let borrowed = slot.borrow();
            let Some((keys, ctx, _, _)) = borrowed.as_ref() else {
                return false;
            };
            // SAFETY: plain COM calls on live objects.
            let eaten = unsafe { keys.TestKeyDown(wparam, lparam) }
                .map(|b| b.as_bool())
                .unwrap_or(false);
            if !eaten {
                // the application writes what the user typed: the input method did not take the key
                if let Some(ch) = char_of(wparam, lparam) {
                    let _ = insert_text(ctx, &ch.to_string());
                }
            }
            // SAFETY: plain COM call on a live object.
            let _ = unsafe { keys.KeyDown(wparam, lparam) };
            eaten
        });
        RECEIVED.with(|slot| slot.borrow_mut().push(eaten));
        return LRESULT(0);
    }
    // SAFETY: the call has no preconditions beyond a live window.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// A real top level window: TSF only hands the keys to a text service when the window owns the keyboard focus.
struct FocusWindow {
    hwnd: HWND,
}

impl FocusWindow {
    fn open() -> windows::core::Result<Self> {
        let class_name: Vec<u16> = HOST_CLASS
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: the module of this process is live.
        let module = unsafe { GetModuleHandleW(None) }?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(host_window_proc),
            hInstance: module.into(),
            hbrBackground: HBRUSH(std::ptr::null_mut()),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        // SAFETY: the class is a live local; the name outlives the call.
        let atom = unsafe { RegisterClassW(&class) };
        // A second host in the same process (the next test) finds the class already there: that is not an error.
        if atom == 0
            && unsafe { windows::Win32::Foundation::GetLastError() }
                != windows::Win32::Foundation::ERROR_CLASS_ALREADY_EXISTS
        {
            return Err(windows::core::Error::from_win32());
        }
        let title: Vec<u16> = "IME1.0 live host"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: the class name and the title are live wide strings; the window belongs to this thread.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                PCWSTR(class_name.as_ptr()),
                PCWSTR(title.as_ptr()),
                WS_OVERLAPPEDWINDOW,
                40,
                40,
                320,
                120,
                HWND(std::ptr::null_mut()),
                windows::Win32::UI::WindowsAndMessaging::HMENU(std::ptr::null_mut()),
                module,
                None,
            )
        }?;
        // The window asks for the Chinese layout explicitly: a window that is in front with the English layout makes
        // msctf pick the English profile of the input method, not the Chinese one the user asked for.
        // SAFETY: the window is live; the message carries the layout of the thread.
        unsafe {
            let _ = PostMessageW(
                hwnd,
                WM_INPUTLANGCHANGEREQUEST,
                WPARAM(1),
                LPARAM(unsafe { GetKeyboardLayout(0) }.0 as isize),
            );
        }
        Ok(Self { hwnd })
    }

    /// Shows the window and gives it the keyboard focus.
    ///
    /// A process that is not the foreground one may not take the focus on its own: the thread of the window that is in
    /// front is attached to ours for a moment, the window is brought to the front, and the attachment is undone.
    pub fn focus(&self) {
        // SAFETY: the window is live; the calls have no other preconditions.
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOW);
            let other = GetWindowThreadProcessId(GetForegroundWindow(), None);
            let mine = GetCurrentThreadId();
            if other != 0 && other != mine {
                let _ = AttachThreadInput(mine, other, true);
            }
            let _ = SetForegroundWindow(self.hwnd);
            let _ = SetFocus(self.hwnd);
            if other != 0 && other != mine {
                let _ = AttachThreadInput(mine, other, false);
            }
        }
    }
}

impl Drop for FocusWindow {
    fn drop(&mut self) {
        // SAFETY: the window is live and belongs to this thread.
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(self.hwnd);
        }
    }
}

// ---------------------------------------------------------------- the drive

/// The client id of this thread (the thread manager hands it out).
static CLIENT_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn client_id() -> u32 {
    let known = CLIENT_ID.load(std::sync::atomic::Ordering::Relaxed);
    if known != 0 {
        return known;
    }
    // SAFETY: the call has no preconditions.
    unsafe { GetCurrentThreadId() }
}

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

fn pump_for(ms: u64) {
    let end = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    while std::time::Instant::now() < end {
        pump();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    pump();
}

/// Whether a module with this file name is loaded in this process.
fn module_loaded(name: &str) -> bool {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: the name is a null-terminated wide string that outlives the call.
    unsafe { GetModuleHandleW(PCWSTR(wide.as_ptr())) }.is_ok()
}

/// The window of the floating panel of the input method (the candidate list) and its rectangle on the screen.
///
/// The panel is the list: the candidates are drawn in it, and a click on a row takes the candidate (the user
/// reported on 2026-11-20 that the floating window did not react). `None` when the panel is not on the screen.
pub fn the_panel_of_the_input_method() -> Option<(HWND, i32, i32, i32, i32)> {
    let panel = find_window_by_class("ProgrammingIME.LaserPanel")?;
    let mut rect = windows::Win32::Foundation::RECT::default();
    // SAFETY: the window is live and the rectangle is a live local.
    if unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowRect(panel, &mut rect) }.is_err() {
        return None;
    }
    Some((
        panel,
        rect.left,
        rect.top,
        rect.right - rect.left,
        rect.bottom - rect.top,
    ))
}

/// The first window of this thread with the class of the floating panel.
fn find_window_by_class(class: &str) -> Option<HWND> {
    let mut found: Option<HWND> = None;
    // SAFETY: the call has no preconditions.
    let tid = unsafe { GetCurrentThreadId() };
    // SAFETY: the pointer is a live local for the whole enumeration.
    unsafe {
        let _ = EnumThreadWindows(
            tid,
            Some(collect_class_window),
            LPARAM((&mut found as *mut Option<HWND>).cast::<isize>() as isize),
        );
    }
    let _ = class;
    found
}

unsafe extern "system" fn collect_class_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: the pointer was made from a live local in `find_window_by_class`.
    let found = unsafe { &mut *(lparam.0 as *mut Option<HWND>) };
    let mut buffer = [0u16; 256];
    // SAFETY: the buffer is live and large enough for the class name.
    let len = unsafe { GetClassNameW(hwnd, &mut buffer) };
    if len > 0 {
        let name = String::from_utf16_lossy(&buffer[..len as usize]);
        if name == "ProgrammingIME.LaserPanel" {
            *found = Some(hwnd);
            return BOOL(0); // one is enough
        }
    }
    BOOL(1)
}

/// The class names of the windows of this thread: the input method opens its own windows when it is activated.
pub fn thread_window_classes() -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    // SAFETY: the call has no preconditions.
    let tid = unsafe { GetCurrentThreadId() };
    // SAFETY: the pointer is a live local for the whole enumeration.
    unsafe {
        let _ = EnumThreadWindows(
            tid,
            Some(collect_class),
            LPARAM(&mut names as *mut Vec<String> as isize),
        );
    }
    names
}

unsafe extern "system" fn collect_class(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: the pointer was made from a live local in `thread_window_classes`.
    let names = unsafe { &mut *(lparam.0 as *mut Vec<String>) };
    let mut buffer = [0u16; 256];
    // SAFETY: the buffer is live and large enough for the class name.
    let len = unsafe { GetClassNameW(hwnd, &mut buffer) };
    if len > 0 {
        names.push(String::from_utf16_lossy(&buffer[..len as usize]));
    }
    BOOL(1)
}

/// The keys of the text on an English layout: the virtual key and the scan code.
pub fn key_of(ch: char) -> (u32, u32) {
    match ch {
        'a'..='z' => {
            let code = ch as u32 - 'a' as u32 + 0x41;
            // the scan codes of the letters (the way the BIOS numbers them)
            const SCANS: [u32; 26] = [
                0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31,
                0x18, 0x19, 0x10, 0x13, 0x1F, 0x14, 0x16, 0x2F, 0x11, 0x2D, 0x15, 0x2B,
            ];
            (code, SCANS[(ch as usize) - ('a' as usize)])
        }
        // the scan codes of the digits: '1' is 0x02, ..., '9' is 0x0A, '0' is 0x0B. The scan code decides which key the
        // system sees: 0x0C is the minus key, so a wrong number here types a minus instead of a digit.
        '1'..='9' => (
            ch as u32 - '0' as u32 + 0x30,
            0x02 + (ch as u32 - '0' as u32),
        ),
        '0' => (0x30, 0x0B),
        ' ' => (0x20, 0x39),
        _ => (0, 0),
    }
}

/// The virtual key and the scan code of F12.
pub const KEY_F12: (u32, u32) = (0x7B, 0x58);
/// The virtual key and the scan code of the space.
pub const KEY_SPACE: (u32, u32) = (0x20, 0x39);

// ---------------------------------------------------------------- the host

/// A key event sink of the host. It is only used to find out whether msctf would accept a **second** sink for the
/// client id of the thread: the text service already holds that sink, and msctf refuses a second one.
#[implement(ITfKeyEventSink)]
struct ProbeSink;

impl ITfKeyEventSink_Impl for ProbeSink_Impl {
    fn OnSetFocus(&self, _fforeground: BOOL) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnTestKeyDown(
        &self,
        _pic: Option<&ITfContext>,
        _wparam: WPARAM,
        _lparam: LPARAM,
    ) -> windows::core::Result<BOOL> {
        Ok(BOOL(0))
    }
    fn OnTestKeyUp(
        &self,
        _pic: Option<&ITfContext>,
        _wparam: WPARAM,
        _lparam: LPARAM,
    ) -> windows::core::Result<BOOL> {
        Ok(BOOL(0))
    }
    fn OnKeyDown(
        &self,
        _pic: Option<&ITfContext>,
        _wparam: WPARAM,
        _lparam: LPARAM,
    ) -> windows::core::Result<BOOL> {
        Ok(BOOL(0))
    }
    fn OnKeyUp(
        &self,
        _pic: Option<&ITfContext>,
        _wparam: WPARAM,
        _lparam: LPARAM,
    ) -> windows::core::Result<BOOL> {
        Ok(BOOL(0))
    }
    fn OnPreservedKey(
        &self,
        _pic: Option<&ITfContext>,
        _rguid: *const GUID,
    ) -> windows::core::Result<BOOL> {
        Ok(BOOL(0))
    }
}

// ---------------------------------------------------------------- the host with a plain edit control

/// A host that is **an ordinary Win32 window with an ordinary EDIT control**.
///
/// Nothing of TSF is done here on purpose: the keys go into the control, Windows hands them to the input processor
/// of the thread, and the input processor writes its composition into the control. That is the path of every classic
/// Windows application - the one that does not implement TSF itself - so it is the closest an automated test gets to
/// "what the user sees in a real application".
pub struct EditHost {
    window: FocusWindow,
    edit: HWND,
}

impl EditHost {
    /// Opens the window with the edit control in it and gives the control the keyboard focus.
    pub fn open() -> windows::core::Result<Self> {
        // SAFETY: the module of this process is live.
        let module = unsafe { GetModuleHandleW(None) }?;
        let window = FocusWindow::open()?;
        let class: Vec<u16> = "EDIT".encode_utf16().chain(std::iter::once(0)).collect();
        // The text of an edit control **is** its content, so it starts empty (one NUL is the empty string).
        let title: Vec<u16> = vec![0];
        // SAFETY: the class name and the title are live wide strings; the control is a child of our window.
        let edit = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class.as_ptr()),
                PCWSTR(title.as_ptr()),
                WINDOW_STYLE(
                    (windows::Win32::UI::WindowsAndMessaging::ES_MULTILINE as u32)
                        | (windows::Win32::UI::WindowsAndMessaging::ES_AUTOVSCROLL as u32)
                        | windows::Win32::UI::WindowsAndMessaging::WS_CHILD.0
                        | windows::Win32::UI::WindowsAndMessaging::WS_VISIBLE.0,
                ),
                0,
                0,
                320,
                80,
                window.hwnd,
                windows::Win32::UI::WindowsAndMessaging::HMENU(std::ptr::null_mut()),
                module,
                None,
            )
        }?;
        // The control has the keyboard focus, and the window is in front.
        // SAFETY: both windows are live and belong to this thread.
        unsafe {
            window.focus();
            let _ = SetFocus(edit);
        }
        pump_for(150);
        Ok(Self { window, edit })
    }

    /// Activates the profile of this product (the zh-CN one) for this thread.
    ///
    /// The system only makes the text service the input processor of a thread when the profile of its language is
    /// active. Without this the measurement depends on the state of the desktop of the user - the language bar may
    /// have another input method in front - and the drive measures nothing at all.
    pub fn activate_our_profile(&self) -> windows::core::Result<()> {
        let profiles: ITfInputProcessorProfileMgr = unsafe {
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)
        }?;
        // SAFETY: the profile manager is live; the ids are valid references.
        unsafe {
            profiles.ActivateProfile(
                TF_PROFILETYPE_INPUTPROCESSOR,
                LANG_ZH_CN,
                &CLSID_PIME,
                &PROFILE_ZH,
                None,
                TF_IPSINK_FLAG_ACTIVE,
            )
        }
    }

    /// Takes the keyboard focus of the edit control, and says whether it got it.
    ///
    /// The focus of the desktop is a scarce resource: Windows does not hand it to every process that asks. The test
    /// **must not** type into somebody else's window, so the focus is checked before a single key is sent.
    pub fn take_the_focus(&self) -> bool {
        for _ in 0..30 {
            // SAFETY: both windows are live and belong to this thread.
            unsafe {
                self.window.focus();
                let _ = SetFocus(self.edit);
            }
            pump_for(60);
            // SAFETY: the calls have no preconditions.
            let (foreground, focus) = unsafe { (GetForegroundWindow(), GetFocus()) };
            if foreground == self.window.hwnd && focus == self.edit {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(120));
        }
        false
    }

    /// Waits until the input method is alive (it opens its own windows when it is activated).
    ///
    /// The system activates the input processor of the thread when the focused window has the keyboard focus - that
    /// takes a moment. Keys typed before it are plain text in the control, keys typed after it are the input
    /// method's, so the test has to wait for it.
    pub fn wait_for_the_input_method(&self, ms: u64) -> bool {
        let end = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        while std::time::Instant::now() < end {
            if thread_window_classes().iter().any(|w| w == GLOW_CLASS) {
                return true;
            }
            pump_for(100);
        }
        false
    }

    /// The caret of the application: the start and the end of its selection (the way the application sees it).
    pub fn read_selection(&self) -> String {
        let mut start = 0u32;
        let mut end = 0u32;
        // EM_GETSEL (0x00B0): the start and the end of the selection of the control, in characters.
        // SAFETY: the control is live; the two numbers are live locals for the call.
        let ok = unsafe {
            SendMessageW(
                self.edit,
                0x00B0,
                WPARAM((&mut start as *mut u32).cast::<usize>() as usize),
                LPARAM((&mut end as *mut u32).cast::<usize>() as isize),
            )
        };
        if ok.0 == 0 {
            return "<the control did not answer>".to_string();
        }
        format!("{start}..{end}")
    }

    /// Whether the edit control has the keyboard focus right now.
    pub fn has_the_focus(&self) -> bool {
        // SAFETY: the calls have no preconditions.
        unsafe { GetForegroundWindow() == self.window.hwnd && GetFocus() == self.edit }
    }

    /// Presses `Ctrl+A` (select everything).
    pub fn select_all(&self) {
        send_virtual_key(0x11, false); // the control key
        send_real_key(0x41, 0x1E); // A
        send_virtual_key(0x11, true);
        pump_for(400);
    }

    /// The text of the edit control, the way an application reads it.
    pub fn read(&self) -> String {
        // SAFETY: the control is live.
        let length = unsafe { GetWindowTextLengthW(self.edit) };
        if length <= 0 {
            return String::new();
        }
        let mut buffer = vec![0u16; length as usize + 1];
        // SAFETY: the buffer is live and large enough for the text of the control.
        let written = unsafe { GetWindowTextW(self.edit, &mut buffer) };
        String::from_utf16_lossy(&buffer[..written as usize])
    }

    /// Types `text` on the real keyboard.
    pub fn type_real(&self, text: &str) {
        for ch in text.chars() {
            let (vk, scan) = key_of(ch);
            if vk == 0 {
                continue;
            }
            send_real_key(vk, scan);
            pump_for(150);
        }
    }

    /// The state of the modifier keys of the keyboard right now.
    ///
    /// A modifier that is down makes every key a chord: the measurement would type `Ctrl+W` instead of `w`, and the
    /// document would stay empty - a measurement of the modifier, not of the input method.
    pub fn modifier_state(&self) -> String {
        let mut out = String::new();
        for (name, vk) in [
            ("shift", 0x10),
            ("ctrl", 0x11),
            ("alt", 0x12),
            ("lwin", 0x5B),
            ("rwin", 0x5C),
        ] {
            // SAFETY: the call has no preconditions.
            let down = unsafe { GetAsyncKeyState(vk as i32) } < 0;
            out.push_str(&format!("{name}={} ", if down { "down" } else { "up" }));
        }
        out
    }

    /// Presses `Ctrl+F12` on the real keyboard: the chord that switches the input mode of the product.
    pub fn press_ctrl_f12(&self) {
        // The modifier goes in as a virtual key (a modifier has no extended scan code), the key itself with its scan
        // code.
        send_virtual_key(0x11, false);
        send_real_key(0x7B, 0x58); // F12
        send_virtual_key(0x11, true);
        pump_for(400);
    }

    /// The window of the host (for the diagnostics).
    pub fn window(&self) -> HWND {
        self.window.hwnd
    }
}

/// The host of the real input method.
pub struct LiveHost {
    window: Option<FocusWindow>,
    thread_mgr: Option<ITfThreadMgr>,
    doc_mgr: Option<ITfDocumentMgr>,
    context: Option<ITfContext>,
    keys: Option<ITfKeystrokeMgr>,
    /// The text service of this build, when the host activated it itself.
    our_own_text_service: Option<ITfTextInputProcessorEx>,
    tid: u32,
    apartment: bool,
}

/// `ITfThreadMgr2::ActivateEx(TF_PID pid, REFCLSID rclsid)`: "this thread uses this text service".
///
/// The binding of the `windows` crate models this entry point with its two arguments **in the wrong order** (it
/// passes a pointer where the `TF_PID` belongs), and the legacy `ITfThreadMgr::Activate` is modelled with another
/// wrong shape - calling either of them through the binding crashes msctf. So the call is made through the vtable
/// with the shape of the header.
fn activate_ex(thread_mgr2: &ITfThreadMgr2, pid: u32, clsid: &GUID) -> HRESULT {
    type ActivateExFn = unsafe extern "system" fn(*mut c_void, u32, *const GUID) -> HRESULT;
    let vtable = ITfThreadMgr2::vtable(thread_mgr2);
    // SAFETY: the vtable of the live interface; the slot is the one of the header.
    let call: ActivateExFn = unsafe { std::mem::transmute(vtable.ActivateEx) };
    // SAFETY: the thread manager is live; the class id is a valid reference.
    unsafe { call(windows::core::Interface::as_raw(thread_mgr2), pid, clsid) }
}

/// `TF_PID_INPUTPROCESSOR`: the input processor of the thread (the keyboard one).
const TF_PID_INPUTPROCESSOR: u32 = 0x1;

/// What the host activates when it opens.
#[derive(Clone, Copy)]
pub enum What {
    /// The registered profile of this product: msctf loads the installed DLL and activates it.
    RegisteredProfile,
    /// The text service of this build, activated by the host through `ITfThreadMgr2::ActivateEx`.
    OurOwnThroughActivateEx,
    /// Nothing: the test activates a profile itself (the control experiment).
    Nothing,
}

impl LiveHost {
    /// Opens the host: the window, the TSF objects, the focus, and the registered profile of this product.
    pub fn open() -> Result<Self, String> {
        // SAFETY: the apartment is initialised once and uninitialised in `Drop`.
        let apartment = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
        Self::open_inner(apartment, What::RegisteredProfile, None)
    }

    /// Opens the host with the document context created under **another client id**.
    ///
    /// The keystroke manager of TSF hands the key to the key event sink that belongs to the **owner of the focused
    /// context**. The text service advises its sink under the client id that msctf gave it (measured: 21 on this
    /// machine), while `ITfThreadMgr2::Activate` hands the application another one (measured: 32). When the two do
    /// not match, the keystroke manager finds no sink for the context of the application and the key goes nowhere.
    /// This constructor exists to measure that: it creates the context under the id that was measured.
    pub fn open_with_the_context_of_another_client(id: u32) -> Result<Self, String> {
        // SAFETY: the apartment is initialised once and uninitialised in `Drop`.
        let apartment = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
        Self::open_inner(apartment, What::RegisteredProfile, Some(id))
    }

    /// Opens the host and activates **the text service of this build** (not the installed one) through
    /// `ITfThreadMgr2::ActivateEx` - the modern "this thread uses this text service".
    pub fn open_with_our_own_text_service() -> Result<Self, String> {
        // SAFETY: the apartment is initialised once and uninitialised in `Drop`.
        let apartment = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
        Self::open_inner(apartment, What::OurOwnThroughActivateEx, None)
    }

    /// Opens the host **without** activating any profile: the test then activates the profile of another text
    /// service (the control experiment of D-170).
    pub fn open_without_any_profile() -> Result<Self, String> {
        // SAFETY: the apartment is initialised once and uninitialised in `Drop`.
        let apartment = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
        Self::open_inner(apartment, What::Nothing, None)
    }

    /// Activates the profile of a text service that is registered on this machine.
    pub fn activate_the_profile_of(&self, clsid: &GUID, profile: &GUID) -> windows::core::Result<()> {
        let profiles: ITfInputProcessorProfileMgr = unsafe {
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)
        }?;
        // SAFETY: the profile manager is live; the ids are valid references.
        unsafe {
            profiles.ActivateProfile(
                TF_PROFILETYPE_INPUTPROCESSOR,
                LANG_ZH_CN,
                clsid,
                profile,
                None,
                TF_IPSINK_FLAG_ACTIVE,
            )
        }
    }

    fn open_inner(apartment: bool, what: What, context_client_id: Option<u32>) -> Result<Self, String> {
        // The layout of the thread comes first: the window inherits it when it is created.
        let layout = use_the_chinese_layout();
        let window = FocusWindow::open().map_err(|e| format!("the window: {e:?}"))?;
        // SAFETY: the call has no preconditions.
        let thread_layout = unsafe { GetKeyboardLayout(0) };
        eprintln!("the keyboard layout of the thread: {thread_layout:?} (asked for {layout:?})");
        // SAFETY: COM is initialised on this thread; the class is the real msctf thread manager.
        let thread_mgr: ITfThreadMgr = unsafe { CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER) }
            .map_err(|e| format!("the thread manager: {e:?}"))?;
        let thread_mgr2: ITfThreadMgr2 = thread_mgr
            .cast()
            .map_err(|e| format!("the thread manager of the second generation: {e:?}"))?;
        // SAFETY: the thread manager is live.
        let tid = unsafe { thread_mgr2.Activate() }.map_err(|e| format!("the thread: {e:?}"))?;
        CLIENT_ID.store(tid, std::sync::atomic::Ordering::Relaxed);

        // SAFETY: the thread manager is live.
        let doc_mgr: ITfDocumentMgr = unsafe { thread_mgr.CreateDocumentMgr() }
            .map_err(|e| format!("the document manager: {e:?}"))?;
        // msctf accepts only 0 or TF_ES_SYNC for the creation of a context (measured: 0x2 and 0x6 are refused with
        // E_INVALIDARG); the write session is the one that asks for TF_ES_READWRITE.
        // The client id that owns the context. It has to be the one that owns the key event sink, otherwise the
        // keystroke manager of TSF finds no sink for the context of the application and the key goes nowhere.
        let context_client_id = context_client_id.unwrap_or(tid);
        eprintln!(
            "the client id of the thread: {tid}; the client id of the context: {context_client_id}; the thread id \
             of Windows: {:?}",
            // SAFETY: the call has no preconditions.
            unsafe { GetCurrentThreadId() }
        );
        let mut context: Option<ITfContext> = None;
        for flags in [TF_ES_SYNC.0, 0u32] {
            let mut candidate: Option<ITfContext> = None;
            let mut cookie = 0u32;
            // SAFETY: the document manager is live; the out parameters are live locals.
            let hr = unsafe {
                doc_mgr.CreateContext(context_client_id, flags, None, &mut candidate, &mut cookie)
            };
            if hr.is_ok() && candidate.is_some() {
                context = candidate;
                break;
            }
        }
        let context = context.ok_or_else(|| "the context".to_string())?;
        // SAFETY: the context is live.
        unsafe { doc_mgr.Push(&context) }.map_err(|e| format!("the context: {e:?}"))?;
        // SAFETY: the thread manager is live; the window is live.
        let _ = unsafe { thread_mgr.AssociateFocus(window.hwnd, &doc_mgr) };

        // The window gets the keyboard focus **after** the association: the focus of a window is what tells msctf that
        // the context of this document is the active one.
        window.focus();
        pump_for(150);
        // SAFETY: the document manager is live.
        unsafe { thread_mgr.SetFocus(Some(&doc_mgr)) }.map_err(|e| format!("the focus: {e:?}"))?;

        // The text service of this build, activated by the host itself, or the profile of this product that is
        // registered on this machine (msctf then loads the installed DLL and activates it).
        let mut our_own: Option<ITfTextInputProcessorEx> = None;
        match what {
            // the profile of this product that is registered on this machine: msctf loads the installed DLL
            What::RegisteredProfile => {
                let profiles: ITfInputProcessorProfileMgr = unsafe {
                    CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)
                }
                .map_err(|e| format!("the profile manager: {e:?}"))?;
                // SAFETY: the profile manager is live; the ids are valid references.
                unsafe {
                    profiles.ActivateProfile(
                        TF_PROFILETYPE_INPUTPROCESSOR,
                        LANG_ZH_CN,
                        &CLSID_PIME,
                        &PROFILE_ZH,
                        None,
                        TF_IPSINK_FLAG_ACTIVE,
                    )
                }
                .map_err(|e| format!("the profile: {e:?}"))?;
            }
            // no profile at all: the test activates one itself (the control experiment)
            What::Nothing => {}
            // the text service of this build, activated by the host itself
            What::OurOwnThroughActivateEx => {
                // The modern entry point: "this thread uses this text service". The binding of the `windows` crate
                // has the wrong shape for the legacy `ITfThreadMgr::Activate`, but this one is right.
                let hr = activate_ex(&thread_mgr2, TF_PID_INPUTPROCESSOR, &CLSID_PIME);
                eprintln!("  ITfThreadMgr2::ActivateEx(our class) -> {hr:?}");
                // The class object of this build, straight out of the crate.
                let mut raw: *mut c_void = std::ptr::null_mut();
                // SAFETY: the ids are valid references; the out parameter is a live local.
                let class = unsafe { pime_tsf::DllGetClassObject(&CLSID_PIME, &IClassFactory::IID, &mut raw) };
                if class.is_err() || raw.is_null() {
                    return Err(format!("the class object of this build: {class:?}"));
                }
                // SAFETY: the factory is a live COM object the crate handed over.
                let factory: IClassFactory = unsafe { windows::core::Interface::from_raw(raw) };
                // SAFETY: the factory creates the text service for our class.
                let tip: ITfTextInputProcessorEx = unsafe { factory.CreateInstance(None) }
                    .map_err(|e| format!("the text service of this build: {e:?}"))?;
                // SAFETY: the thread manager is live; the client id belongs to this thread.
                let activated = unsafe { tip.ActivateEx(Some(&thread_mgr), tid, 0) };
                eprintln!("  the text service of this build -> ActivateEx {activated:?}");
                if activated.is_err() {
                    return Err(format!("the text service of this build: {activated:?}"));
                }
                our_own = Some(tip);
            }
        }
        pump_for(600);

        // SAFETY: the thread manager is the one the profile was activated on.
        let keys: ITfKeystrokeMgr = thread_mgr
            .cast()
            .map_err(|e| format!("the keystroke manager: {e:?}"))?;
        HOST.with(|slot| {
            *slot.borrow_mut() = Some((
                keys.clone(),
                context.clone(),
                thread_mgr.clone(),
                doc_mgr.clone(),
            ));
        });
        Ok(Self {
            window: Some(window),
            thread_mgr: Some(thread_mgr),
            doc_mgr: Some(doc_mgr),
            context: Some(context),
            keys: Some(keys),
            our_own_text_service: our_own,
            tid,
            apartment,
        })
    }

    /// Whether the zh-CN profile of this product is registered on this machine.
    pub fn profile_registered() -> bool {
        // SAFETY: the apartment is initialised once and uninitialised at the end of the function.
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        // The COM object has to go before the apartment is uninitialised: an apartment that is uninitialised while a
        // COM object of msctf is still alive crashes msctf itself (measured: 0xC0000005).
        let found = {
            // SAFETY: COM is initialised on this thread; the class is the real msctf profile manager.
            let profiles: ITfInputProcessorProfileMgr = match unsafe {
                CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)
            } {
                Ok(profiles) => profiles,
                Err(_) => {
                    // SAFETY: balances the CoInitializeEx above.
                    unsafe { CoUninitialize() };
                    return false;
                }
            };
            let mut profile = unsafe { std::mem::zeroed() };
            // SAFETY: the profile manager is live; the out parameter is a live local.
            let found = unsafe {
                profiles.GetProfile(
                    TF_PROFILETYPE_INPUTPROCESSOR,
                    LANG_ZH_CN,
                    &CLSID_PIME,
                    &PROFILE_ZH,
                    None,
                    &mut profile,
                )
            };
            found.is_ok()
        };
        // SAFETY: balances the CoInitializeEx above.
        unsafe { CoUninitialize() };
        found
    }

    /// The text of the document of the host, read back through a read session.
    pub fn document(&self) -> String {
        read_text(self.context.as_ref().expect("the context")).unwrap_or_default()
    }

    /// The class names of the windows of this thread.
    pub fn windows(&self) -> Vec<String> {
        thread_window_classes()
    }

    /// Whether the installed DLL of this product is loaded in this process (msctf loaded it).
    pub fn installed_dll_loaded(&self) -> bool {
        module_loaded("pime_tsf.dll")
    }

    /// Whether the text service holds the key event sink of this thread: a second advise of a sink of the host is
    /// refused by msctf with `E_INVALIDARG` (one key event sink per client id).
    pub fn key_sink_is_taken(&self) -> bool {
        // SAFETY: the thread manager is live.
        let keys: ITfKeystrokeMgr = match self.thread_mgr.as_ref().and_then(|m| m.cast().ok()) {
            Some(keys) => keys,
            None => return false,
        };
        let probe: ITfKeyEventSink = ProbeSink.into();
        // SAFETY: plain COM call on a live object; the sink is a live object of ours.
        let refused =
            unsafe { keys.AdviseKeyEventSink(self.tid, &probe, BOOL(1)) };
        refused.is_err()
    }

    /// Whether msctf accepts a **second** key event sink for another client id of this thread.
    ///
    /// One sink per client id means the text service's sink is on the client id of the thread (the one the context was
    /// created with); one sink per thread means it is somewhere else.
    pub fn another_client_id_may_have_a_sink(&self) -> bool {
        let keys: ITfKeystrokeMgr = match self.thread_mgr.as_ref().and_then(|m| m.cast().ok()) {
            Some(keys) => keys,
            None => return false,
        };
        let probe: ITfKeyEventSink = ProbeSink.into();
        // SAFETY: plain COM call on a live object; the sink is a live object of ours.
        let accepted = unsafe { keys.AdviseKeyEventSink(0x9999, &probe, BOOL(1)) }.is_ok();
        if accepted {
            // SAFETY: undoes the advise above.
            let _ = unsafe { keys.UnadviseKeyEventSink(0x9999) };
        }
        accepted
    }

    /// Whether the document manager of the host is the one that has the focus of the thread manager.
///
/// The keystroke manager of TSF hands the key to the sink of the **focused context**; if the document manager of the
/// host is not the focused one, the context of the host is not the one that is asked.
pub fn focus_is_our_document_manager(&self) -> bool {
    let (Some(doc_mgr), Some(thread_mgr)) = (self.doc_mgr.as_ref(), self.thread_mgr.as_ref()) else {
        return false;
    };
    // SAFETY: the thread manager is live.
    let focused = match unsafe { thread_mgr.GetFocus() } {
        Ok(focused) => focused,
        Err(_) => return false,
    };
    let ours = doc_mgr.cast::<windows::core::IUnknown>();
    let theirs = focused.cast::<windows::core::IUnknown>();
    match (ours, theirs) {
        (Ok(ours), Ok(theirs)) => {
            windows::core::Interface::as_raw(&ours) == windows::core::Interface::as_raw(&theirs)
        }
        _ => false,
    }
}

/// Which client id holds the key event sink of this thread.
///
/// msctf allows **one** key event sink per thread (measured: a second one is refused for every client id), and it
/// hands the key to the sink of the **focused context** - which belongs to the client id that created that context.
/// If the text service advised its sink under a different id, the keystroke manager cannot find it: the key goes
/// nowhere. This probe walks the ids that could be the one and reports which of them can be unadvised.
pub fn client_id_that_holds_the_sink(&self) -> Option<u32> {
    let keys: ITfKeystrokeMgr = match self.thread_mgr.as_ref().and_then(|m| m.cast().ok()) {
        Some(keys) => keys,
        None => return None,
    };
    // SAFETY: the call has no preconditions.
    let thread_id = unsafe { GetCurrentThreadId() };
    // The scan is wide on purpose: the keystroke manager of TSF hands the key to the sink that belongs to the **owner
    // of the focused context**, so the id under which the text service advised its sink decides whether the keys ever
    // arrive. `UnadviseKeyEventSink` answers an error when there is no sink for that id, so the scan finds it.
    let mut candidates: Vec<u32> = (0u32..512).collect();
    candidates.push(self.tid);
    candidates.push(thread_id);
    for id in candidates {
        // SAFETY: plain COM call on a live object; the id is a plain number.
        let removed = unsafe { keys.UnadviseKeyEventSink(id) }.is_ok();
        if removed {
            return Some(id);
        }
    }
    None
}

/// Asks TSF about one key, the way the window procedure of an application does. Returns whether the text service
    /// ate the key. A key that is not eaten is written into the document by the host.
    pub fn ask_about_key(&self, vk: u32, scan: u32, text: Option<&str>) -> bool {
        let state = [0u8; 256];
        // SAFETY: the table is a live local of the right size.
        unsafe { SetKeyboardState(&state) }.ok();
        let lparam = LPARAM(1 | ((scan as isize) << 16));
        // SAFETY: plain COM calls on live objects.
        let keys = self.keys.as_ref().expect("the keystroke manager");
        let eaten = unsafe { keys.TestKeyDown(WPARAM(vk as usize), lparam) }
            .map(|b| b.as_bool())
            .unwrap_or(false);
        if eaten {
            // SAFETY: plain COM call on a live object.
            let _ = unsafe { keys.KeyDown(WPARAM(vk as usize), lparam) };
        } else if let Some(text) = text
            && let Err(e) = insert_text(self.context.as_ref().expect("the context"), text)
        {
            eprintln!("    the host could not write {text:?} into the store: {e:?}");
        }
        // SAFETY: plain COM call on a live object.
        let _ = unsafe { keys.KeyUp(WPARAM(vk as usize), lparam) };
        // SAFETY: the table is a live local of the right size; it restores the empty state.
        unsafe { SetKeyboardState(&[0u8; 256]) }.ok();
        pump();
        eaten
    }

    /// Asks TSF about the chord `Ctrl+F12` (the input mode of the product). The modifier is the state of the thread
    /// for the moment of the key: the text service reads it with `GetKeyState`.
    pub fn ask_about_ctrl_f12(&self) -> bool {
        let mut state = [0u8; 256];
        state[VK_CONTROL.0 as usize] = 0x80;
        // SAFETY: the table is a live local of the right size.
        unsafe { SetKeyboardState(&state) }.ok();
        let (vk, scan) = KEY_F12;
        let lparam = LPARAM(1 | ((scan as isize) << 16));
        // SAFETY: plain COM calls on live objects.
        let keys = self.keys.as_ref().expect("the keystroke manager");
        let eaten = unsafe { keys.TestKeyDown(WPARAM(vk as usize), lparam) }
            .map(|b| b.as_bool())
            .unwrap_or(false);
        if eaten {
            // SAFETY: plain COM call on a live object.
            let _ = unsafe { keys.KeyDown(WPARAM(vk as usize), lparam) };
        }
        // SAFETY: plain COM call on a live object.
        let _ = unsafe { keys.KeyUp(WPARAM(vk as usize), lparam) };
        // SAFETY: the table is a live local of the right size; it restores the empty state.
        unsafe { SetKeyboardState(&[0u8; 256]) }.ok();
        pump();
        eaten
    }

    /// Types `text` on the **real keyboard**: the keys go through the system into the window procedure of the host,
    /// which asks TSF about them. Returns what the window procedure saw (true = the text service ate the key).
    pub fn type_on_the_real_keyboard(&self, text: &str) -> Vec<bool> {
        RECEIVED.with(|slot| slot.borrow_mut().clear());
        for ch in text.chars() {
            let (vk, scan) = key_of(ch);
            if vk == 0 {
                continue;
            }
            send_real_key(vk, scan);
            pump_for(250);
        }
        RECEIVED.with(|slot| slot.borrow().clone())
    }

    /// The client id of this thread (the thread manager hands it out).
pub fn client_id(&self) -> u32 {
    self.tid
}

/// The window of the host (its handle), for the tests that need it.
    pub fn window(&self) -> HWND {
        self.window.as_ref().expect("the window").hwnd
    }
}

impl Drop for LiveHost {
    fn drop(&mut self) {
        // The order matters: the window procedure must not touch TSF any more, the window goes first (the input
        // method leaves the screen with it), then the COM objects, and only then the apartment. Uninitialising the
        // apartment while a COM object of msctf is still alive crashes msctf itself (measured: 0xC0000005).
        HOST.with(|slot| *slot.borrow_mut() = None);
        self.keys = None;
        self.context = None;
        self.our_own_text_service = None;
        self.doc_mgr = None;
        self.thread_mgr = None;
        self.window = None;
        if self.apartment {
            // SAFETY: balances the CoInitializeEx of `open`.
            unsafe { CoUninitialize() };
        }
    }
}

/// Releases every modifier key of the keyboard.
///
/// The drives press chords (Ctrl+F12, Ctrl+A, Ctrl+S) with `SendInput`. A modifier that goes down and never comes up
/// - because the drive was interrupted, or because the process died in between - stays down for **the whole machine**:
/// every key of the user then behaves as if that key were held (the user of the machine sees it as "the input is
/// broken"). Nothing of that kind may survive a drive.
pub fn release_every_modifier() {
    for vk in [0x10, 0x11, 0x12, 0x5B, 0x5C] {
        send_virtual_key(vk, true);
    }
    // the modifiers that have a key of their own on the keyboard
    for vk in [0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5] {
        send_virtual_key(vk, true);
    }
}

/// The guard that releases the modifiers of the keyboard when the drive ends, whatever the way it ends.
pub struct ModifiersReleased;

impl Drop for ModifiersReleased {
    fn drop(&mut self) {
        release_every_modifier();
    }
}

/// Sends a virtual key (without a scan code): the modifiers go this way.
///
/// **No `KEYEVENTF_EXTENDEDKEY`**: that flag means "the right-hand key of the pair". A modifier that goes down as the
/// extended key and comes up as the normal one leaves the *right* Ctrl (or Shift, or Alt) pressed **for the whole
/// machine**: every key of the user then behaves as a chord (the user sees it as "the input is broken"), and the
/// measurement types `Ctrl+W` instead of `w`.
fn send_virtual_key(vk: u32, up: bool) -> bool {
    let flags = if up {
        KEYEVENTF_KEYUP
    } else {
        KEYBD_EVENT_FLAGS(0)
    };
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk as u16),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    // SAFETY: the input is a live local of the right size.
    let sent = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    sent == 1
}

/// Sends a real key through the keyboard of the system: the window procedure of the host gets the WM_KEYDOWN and
/// asks TSF about the key, the way every application does.
fn send_real_key(vk: u32, scan: u32) -> bool {
    let down = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk as u16),
                wScan: scan as u16,
                dwFlags: KEYEVENTF_SCANCODE,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let up = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk as u16),
                wScan: scan as u16,
                dwFlags: KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    // SAFETY: the inputs are live locals of the right size.
    let sent = unsafe { SendInput(&[down, up], std::mem::size_of::<INPUT>() as i32) };
    sent == 2
}
