//! The client side of the bridge: how an agent asks the input method to write into the focused window.
//!
//! The text service lives inside the process of the application that has the focus, so the agent cannot write into
//! the document by itself. It sends one `WM_COPYDATA` message to the bridge window of that process (the DLL side
//! creates one hidden window per process, of the class `PIME1AgentBridge`), and the text service answers with the
//! result of the write.
//!
//! Two things make the call safe to make from a host process:
//! * the window is found by the **foreground process** (the process that has the focus is the one that owns the text
//!   service), and the choice is a pure function ([`select_bridge_window`]) so it can be tested without a desktop;
//! * every step has a deadline: a text service that does not answer never blocks the caller for ever.
//!
//! The reply comes back as a second `WM_COPYDATA` message, to a hidden message-only window that the client creates
//! for the call. The request is sent on a thread of its own: a reply that arrives as a *sent* message (the text
//! service waits for it) must be dispatched by the thread that owns the reply window, and that thread must not be
//! the one that waits inside `SendMessageTimeoutW` - otherwise the two processes wait for each other.

use crate::{AgentError, ImeBridge};
use pime_core::agent_proto::{
    BRIDGE_CLASS, BRIDGE_TITLE_PREFIX, Reply, Request,
};
use std::time::Duration;

/// The deadline of one call of the bridge.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(4000);

/// What the client knows about one top-level window of the desktop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowInfo {
    /// The handle of the window.
    pub hwnd: isize,
    /// The process that owns the window.
    pub pid: u32,
    /// The class of the window.
    pub class: String,
    /// The title of the window.
    pub title: String,
}

/// The bridge window of the focused process, or `None` when the input method is not running there.
///
/// The rules are the ones of the DLL side: the class is `PIME1AgentBridge`, the title starts with `PIME1.` (the
/// nonce that makes the title unique) and the window belongs to the process that has the focus - the text service
/// lives inside that process, nowhere else. When several windows match, the one with the smallest handle is taken, so
/// the choice does not depend on the order the enumeration happens to return.
pub fn select_bridge_window(candidates: &[WindowInfo], foreground_pid: u32) -> Option<WindowInfo> {
    candidates
        .iter()
        .filter(|window| {
            window.pid == foreground_pid
                && window.class == BRIDGE_CLASS
                && window.title.starts_with(BRIDGE_TITLE_PREFIX)
        })
        .min_by_key(|window| window.hwnd)
        .cloned()
}

/// The payload of an answer, without the zero that terminates it.
///
/// The text service sends a zero-terminated copy of the answer (so that the other side may read it as a C string);
/// the zero is not part of the JSON and the strict parser of the core would refuse it.
pub fn trim_payload(bytes: &[u8]) -> &[u8] {
    let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(bytes.len());
    &bytes[..end]
}

/// The client of the bridge of the input method.
pub struct BridgeClient {
    timeout: Duration,
}

impl Default for BridgeClient {
    fn default() -> Self {
        BridgeClient::new()
    }
}

impl BridgeClient {
    /// A client with the default deadline.
    pub fn new() -> BridgeClient {
        BridgeClient {
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// A client with a deadline of its own.
    pub fn with_timeout(timeout: Duration) -> BridgeClient {
        BridgeClient { timeout }
    }

    /// The deadline of one call.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

impl ImeBridge for BridgeClient {
    fn send(&self, request: &Request) -> Result<Reply, AgentError> {
        #[cfg(windows)]
        {
            windows_impl::send(self.timeout, request)
        }
        #[cfg(not(windows))]
        {
            let _ = request;
            Err(AgentError::protocol(
                pime_core::agent_proto::Error::NoIme,
                "the bridge of the input method needs Windows",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pime_core::agent_proto::{BRIDGE_CLASS, BRIDGE_TITLE_PREFIX};

    fn window(hwnd: isize, pid: u32, class: &str, title: &str) -> WindowInfo {
        WindowInfo {
            hwnd,
            pid,
            class: class.to_string(),
            title: title.to_string(),
        }
    }

    #[test]
    fn the_window_of_the_focused_process_is_the_one_of_the_bridge() {
        let windows = vec![
            window(10, 100, BRIDGE_CLASS, "PIME1.abc"),
            window(20, 200, BRIDGE_CLASS, "PIME1.def"),
        ];
        let chosen = select_bridge_window(&windows, 200).expect("the window of the second process");
        assert_eq!(chosen.hwnd, 20);
        assert!(select_bridge_window(&windows, 300).is_none());
    }

    #[test]
    fn a_window_of_another_class_or_of_another_title_is_not_the_bridge() {
        let windows = vec![
            window(10, 100, "SomeOtherClass", "PIME1.abc"),
            window(11, 100, BRIDGE_CLASS, "Something else"),
            window(12, 100, BRIDGE_CLASS, "PIME1.abc"),
        ];
        let chosen = select_bridge_window(&windows, 100).expect("the bridge window");
        assert_eq!(chosen.hwnd, 12);
    }

    #[test]
    fn the_smallest_handle_wins_when_there_are_several() {
        let windows = vec![
            window(30, 100, BRIDGE_CLASS, "PIME1.abc"),
            window(10, 100, BRIDGE_CLASS, "PIME1.def"),
            window(20, 100, BRIDGE_CLASS, "PIME1.ghi"),
        ];
        let chosen = select_bridge_window(&windows, 100).expect("a window");
        assert_eq!(chosen.hwnd, 10);
    }

    #[test]
    fn an_empty_desktop_has_no_bridge() {
        assert!(select_bridge_window(&[], 100).is_none());
    }

    #[test]
    fn the_names_of_the_bridge_are_the_ones_of_the_contract() {
        assert_eq!(BRIDGE_CLASS, "PIME1AgentBridge");
        assert!(BRIDGE_TITLE_PREFIX.starts_with("PIME1."));
    }

    /// The text service sends a zero-terminated copy of its answer: the zero is not part of the JSON.
    #[test]
    fn the_zero_that_terminates_the_answer_is_not_part_of_it() {
        let mut reply = Reply::ok();
        reply.inserted = 4;
        reply.language = Some("zh-CN".to_string());
        let mut bytes = reply.to_json().to_json_string().into_bytes();
        bytes.push(0);
        let trimmed = trim_payload(&bytes);
        assert_eq!(trimmed.len(), bytes.len() - 1);
        let back = Reply::from_bytes(trimmed).expect("the answer is read");
        assert_eq!(back, reply);
    }
}

/// The Win32 side of the client.
#[cfg(windows)]
#[allow(unsafe_code)]
mod windows_impl {
    use super::{select_bridge_window, WindowInfo};
    use crate::AgentError;
    use pime_core::agent_proto::{
        COPYDATA_REQUEST, Error, Reply, Request, MAX_PAYLOAD_BYTES,
    };
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::mpsc::{Sender, TryRecvError, channel};
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::{
        BOOL, ERROR_CLASS_ALREADY_EXISTS, GetLastError, HWND, HINSTANCE, LPARAM, LRESULT, WPARAM,
    };
    use windows::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows, GetClassNameW,
        GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId, HMENU, HWND_MESSAGE, MSG,
        PeekMessageW, PM_REMOVE, RegisterClassW, SMTO_ABORTIFHUNG, TranslateMessage, WINDOW_EX_STYLE,
        WINDOW_STYLE, WM_COPYDATA, WM_NCDESTROY, WNDCLASSW,
    };
    use windows::core::PCWSTR;

    /// The Win32 error number inside a `windows::core::Error` (the `HRESULT_FROM_WIN32` values carry it in the low
    /// word of the HRESULT).
    fn win32_code(error: &windows::core::Error) -> u32 {
        let hr = error.code().0 as u32;
        if hr >> 16 == 0x8007 { hr & 0xFFFF } else { hr }
    }

    // The reply channels of the message-only windows of this thread, by the handle of the window.
    //
    // The window procedure of a window always runs on the thread that owns the window, so the state of the call
    // never leaves the thread that made it.
    thread_local! {
        static REPLY_CHANNELS: RefCell<HashMap<isize, Sender<Vec<u8>>>> = RefCell::new(HashMap::new());
    }

    // The windows of the desktop, collected by the enumeration.
    thread_local! {
        static COLLECTED: RefCell<Vec<WindowInfo>> = const { RefCell::new(Vec::new()) };
    }

    /// The class of the reply window, registered once per process.
    fn reply_class() -> Result<Vec<u16>, AgentError> {
        static CLASS: std::sync::OnceLock<std::result::Result<Vec<u16>, String>> = std::sync::OnceLock::new();
        let value = CLASS.get_or_init(|| {
            let pid = unsafe { GetCurrentProcessId() };
            let name = format!("PIME1AgentReply-{pid}");
            let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            // SAFETY: the instance of the module is always valid; the class name is a NUL-terminated string that
            // `wide` keeps alive until the call is over.
            let instance = match unsafe { GetModuleHandleW(None) } {
                Ok(instance) => instance,
                Err(error) => {
                    return Err(format!(
                        "the module handle is not available: os error {}",
                        win32_code(&error)
                    ));
                }
            };
            let class = WNDCLASSW {
                lpfnWndProc: Some(reply_window_proc),
                hInstance: HINSTANCE(instance.0),
                lpszClassName: PCWSTR(wide.as_ptr()),
                ..Default::default()
            };
            // SAFETY: `class` is a valid structure and `wide` outlives the call (it is returned and kept alive).
            let atom = unsafe { RegisterClassW(&class) };
            if atom == 0 {
                let error = unsafe { GetLastError() };
                // the class exists already (a second call in the same process) - that is not a failure
                if error != ERROR_CLASS_ALREADY_EXISTS {
                    return Err(format!(
                        "the reply window class could not be registered: {}",
                        error.0
                    ));
                }
            }
            Ok(wide)
        });
        match value {
            Ok(wide) => Ok(wide.clone()),
            Err(error) => Err(AgentError::protocol(Error::Refused, error)),
        }
    }

    /// The window procedure of the reply window.
    unsafe extern "system" fn reply_window_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_COPYDATA => {
                // SAFETY: the pointer is the one of the sender and is read during the message only.
                if let Some(bytes) = unsafe { copy_payload(lparam) } {
                    let key = hwnd.0 as isize;
                    let sender = REPLY_CHANNELS.with(|channels| channels.borrow().get(&key).cloned());
                    if let Some(sender) = sender {
                        let _ = sender.send(bytes);
                    }
                }
                LRESULT(1)
            }
            WM_NCDESTROY => {
                let key = hwnd.0 as isize;
                REPLY_CHANNELS.with(|channels| {
                    channels.borrow_mut().remove(&key);
                });
                // SAFETY: the window is a real window of this thread and the message is passed on unchanged.
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
            _ => {
                // SAFETY: the window is a real window of this thread and the message is passed on unchanged.
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
        }
    }

    /// The payload of a `WM_COPYDATA` message, copied out (the pointer is only valid during the message).
    unsafe fn copy_payload(lparam: LPARAM) -> Option<Vec<u8>> {
        if lparam.0 == 0 {
            return None;
        }
        // SAFETY: the message carries the pointer to the COPYDATASTRUCT of the sender; it is valid until the message
        // is handled, and it is read (not written) here.
        let data = unsafe { *(lparam.0 as *const COPYDATASTRUCT) };
        if data.cbData == 0 || data.cbData as usize > MAX_PAYLOAD_BYTES || data.lpData.is_null() {
            return None;
        }
        // SAFETY: the sender guarantees `cbData` readable bytes at `lpData` for the duration of the message.
        let bytes = unsafe { std::slice::from_raw_parts(data.lpData.cast::<u8>(), data.cbData as usize) };
        // the text service sends a zero-terminated copy of the answer: the zero is not part of the JSON
        Some(super::trim_payload(bytes).to_vec())
    }

    /// Every top-level window of the desktop.
    fn enum_windows() -> Vec<WindowInfo> {
        COLLECTED.with(|collected| collected.borrow_mut().clear());
        // SAFETY: the callback only reads the windows and collects them in the thread-local list of this thread.
        unsafe {
            let _ = EnumWindows(Some(collect_window), LPARAM(0));
        }
        COLLECTED.with(|collected| collected.borrow().clone())
    }

    /// The callback of the enumeration.
    unsafe extern "system" fn collect_window(hwnd: HWND, _lparam: LPARAM) -> BOOL {
        let mut pid = 0u32;
        // SAFETY: the handle is a real window and the out pointer is valid.
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        let mut class = [0u16; 256];
        // SAFETY: the buffer is valid and the length of the class name is far below its size.
        let class_len = unsafe { GetClassNameW(hwnd, &mut class) };
        let class = if class_len > 0 {
            String::from_utf16_lossy(&class[..class_len as usize])
        } else {
            String::new()
        };
        let mut title = [0u16; 512];
        // SAFETY: the buffer is valid and the length of the title is far below its size.
        let title_len = unsafe { GetWindowTextW(hwnd, &mut title) };
        let title = if title_len > 0 {
            String::from_utf16_lossy(&title[..title_len as usize])
        } else {
            String::new()
        };
        let info = WindowInfo {
            hwnd: hwnd.0 as isize,
            pid,
            class,
            title,
        };
        COLLECTED.with(|collected| collected.borrow_mut().push(info));
        BOOL(1)
    }

    /// Creates the hidden message-only window that receives the reply.
    fn create_reply_window(sender: Sender<Vec<u8>>) -> Result<HWND, AgentError> {
        let class = reply_class()?;
        // SAFETY: the instance of the module is always valid.
        let instance = unsafe { GetModuleHandleW(None) }
            .map_err(|error| AgentError::protocol(Error::Refused, error.to_string()))?;
        // SAFETY: the class is registered, the window is message-only (it never appears on the desktop) and the
        // strings are NUL-terminated and alive across the call.
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class.as_ptr()),
                PCWSTR::null(),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                HMENU::default(),
                HINSTANCE(instance.0),
                None,
            )
        }
        .map_err(|error| {
            AgentError::protocol(
                Error::Refused,
                format!(
                    "the reply window could not be created: os error {}",
                    win32_code(&error)
                ),
            )
        })?;
        REPLY_CHANNELS.with(|channels| {
            channels.borrow_mut().insert(hwnd.0 as isize, sender);
        });
        Ok(hwnd)
    }

    /// The process that has the focus.
    fn foreground_pid() -> u32 {
        // SAFETY: both calls only read the state of the desktop.
        let foreground = unsafe { GetForegroundWindow() };
        let mut pid = 0u32;
        // SAFETY: the out pointer is valid.
        unsafe { GetWindowThreadProcessId(foreground, Some(&mut pid)) };
        pid
    }

    /// Sends the request to the text service and waits for its answer.
    pub fn send(timeout: Duration, request: &Request) -> Result<Reply, AgentError> {
        let pid = foreground_pid();
        if pid == 0 {
            return Err(AgentError::protocol(
                Error::NoIme,
                "there is no focused window",
            ));
        }
        let Some(target) = select_bridge_window(&enum_windows(), pid) else {
            return Err(AgentError::protocol(
                Error::NoIme,
                "the input method is not running in the focused window",
            ));
        };
        send_to(target.hwnd, timeout, request)
    }

    /// Sends the request to the window that is named and waits for its answer.
    ///
    /// The part of the call that does not depend on the desktop: the window is found by [`send`], but the message
    /// itself can be driven against any window, which is what the test of the channel does.
    pub fn send_to(target_hwnd: isize, timeout: Duration, request: &Request) -> Result<Reply, AgentError> {
        let deadline = Instant::now() + timeout;

        let (sender, receiver) = channel();
        let reply_window = create_reply_window(sender)?;

        let mut wire = request.clone();
        wire.reply = reply_window.0 as u64;
        let payload = wire.to_json().to_json_string();
        if payload.len() > MAX_PAYLOAD_BYTES {
            destroy(reply_window);
            return Err(AgentError::protocol(
                Error::TooLarge,
                "the request is larger than the bridge accepts",
            ));
        }

        // The request is sent on a thread of its own: the answer may arrive as a sent message, and the thread that
        // owns the reply window must keep pumping its queue while it waits.
        let sender_thread = std::thread::spawn(move || send_copydata(target_hwnd, payload, timeout));

        let answer = wait_for_reply(&receiver, deadline);
        destroy(reply_window);
        // the thread of the request ends by itself when the timeout of the send is over
        let _ = sender_thread;
        let bytes = answer?;
        Reply::from_bytes(&bytes).map_err(|error| {
            AgentError::protocol(error, "the answer of the input method is not one of ours")
        })
    }

    /// Waits for the answer, pumping the messages of this thread while it waits.
    fn wait_for_reply(
        receiver: &std::sync::mpsc::Receiver<Vec<u8>>,
        deadline: Instant,
    ) -> Result<Vec<u8>, AgentError> {
        loop {
            pump_messages();
            match receiver.try_recv() {
                Ok(bytes) => return Ok(bytes),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    return Err(AgentError::protocol(
                        Error::Busy,
                        "the input method did not answer",
                    ));
                }
            }
            if Instant::now() >= deadline {
                return Err(AgentError::protocol(
                    Error::Busy,
                    "the input method did not answer in time",
                ));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Dispatches the messages that are waiting for this thread (the answer of the text service).
    fn pump_messages() {
        let mut msg = MSG::default();
        loop {
            // SAFETY: the message structure is valid and the window handle is the "all windows of this thread" one.
            let has_message = unsafe { PeekMessageW(&mut msg, HWND::default(), 0, 0, PM_REMOVE) };
            if !has_message.as_bool() {
                break;
            }
            // SAFETY: the message was taken from the queue of this thread.
            unsafe {
                let _ = TranslateMessage(&msg);
                let _ = DispatchMessageW(&msg);
            }
        }
    }

    /// Destroys the reply window.
    fn destroy(hwnd: HWND) {
        // SAFETY: the window belongs to this thread and is destroyed exactly once.
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }

    /// Sends the request to the bridge window and waits for the text service to handle it.
    fn send_copydata(target: isize, payload: String, timeout: Duration) -> Result<(), AgentError> {
        let target = HWND(target as *mut core::ffi::c_void);
        let mut bytes = payload.into_bytes();
        let data = COPYDATASTRUCT {
            dwData: COPYDATA_REQUEST as usize,
            cbData: bytes.len() as u32,
            lpData: bytes.as_mut_ptr().cast(),
        };
        let mut result = 0usize;
        let milliseconds = timeout.as_millis().min(u32::MAX as u128) as u32;
        // SAFETY: the structure lives until the call is over (the receiver copies the data during the call), the
        // handle is a real window and the timeout bounds the wait.
        let sent = unsafe {
            windows::Win32::UI::WindowsAndMessaging::SendMessageTimeoutW(
                target,
                WM_COPYDATA,
                WPARAM(0),
                LPARAM((&data as *const COPYDATASTRUCT) as isize),
                SMTO_ABORTIFHUNG,
                milliseconds,
                Some(&mut result),
            )
        };
        if sent.0 == 0 {
            return Err(AgentError::protocol(
                Error::Busy,
                "the input method did not handle the request in time",
            ));
        }
        Ok(())
    }
}

/// The channel itself: a real `WM_COPYDATA` round trip on a real Windows.
///
/// The tests above prove the **rules** (which window is the bridge, how an answer is read). This one proves the
/// **channel**: a hidden window of the class `PIME1AgentBridge` answers exactly like the text service does - it reads
/// the request, and it sends a zero-terminated answer back to the window that the request names - and the client
/// finds it, sends to it and reads the answer. Without it, a mistake in the message plumbing (the reply window, the
/// message pump, the thread that sends) would only show up in a real editor.
#[cfg(all(windows, test))]
#[allow(unsafe_code)]
mod channel_tests {
    use super::*;
    use pime_core::agent_proto::{COPYDATA_REPLY, Error, Op, Reply, Request};
    use std::cell::RefCell;
    use std::sync::mpsc::{Sender, channel};
    use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, HMENU, RegisterClassExW, SendMessageW,
        WINDOW_EX_STYLE, WINDOW_STYLE, WM_COPYDATA, WNDCLASSEXW,
    };
    use windows::core::PCWSTR;

    thread_local! {
        /// The request the fake bridge received: the test wants to see it.
        static RECEIVED: RefCell<Option<Sender<Request>>> = const { RefCell::new(None) };
    }

    /// The window procedure of the fake bridge: it answers the way the text service does.
    unsafe extern "system" fn fake_bridge_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_COPYDATA {
            // SAFETY: the message carries the structure of the sender; it is valid while the message is handled.
            let data = unsafe { *(lparam.0 as *const COPYDATASTRUCT) };
            // SAFETY: the sender guarantees `cbData` readable bytes for the duration of the message.
            let bytes = unsafe { std::slice::from_raw_parts(data.lpData.cast::<u8>(), data.cbData as usize) };
            let Ok(request) = Request::from_bytes(bytes) else {
                return LRESULT(0);
            };
            RECEIVED.with(|slot| {
                if let Some(sender) = slot.borrow().as_ref() {
                    let _ = sender.send(request.clone());
                }
            });
            let mut reply = Reply::ok();
            reply.inserted = request.text.chars().count();
            reply.language = Some("zh-CN".to_string());
            let mut payload = reply.to_json().to_json_string().into_bytes();
            payload.push(0); // the text service sends a zero-terminated copy
            let out = COPYDATASTRUCT {
                dwData: COPYDATA_REPLY as usize,
                cbData: payload.len() as u32,
                lpData: payload.as_ptr() as *mut core::ffi::c_void,
            };
            // SAFETY: the target is the window the request named; the payload lives until the call returns.
            unsafe {
                SendMessageW(
                    HWND(request.reply as *mut core::ffi::c_void),
                    WM_COPYDATA,
                    WPARAM(0),
                    LPARAM((&out as *const COPYDATASTRUCT) as isize),
                );
            }
            return LRESULT(1);
        }
        // SAFETY: the plain default handling of the window.
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    /// The window procedure of the window that answers nothing.
    unsafe extern "system" fn silent_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        // SAFETY: the plain default handling of the window.
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    /// A hidden window that looks like the bridge of the text service.
    fn fake_bridge_window() -> HWND {
        let class_name: Vec<u16> = BRIDGE_CLASS.encode_utf16().chain(Some(0)).collect();
        // SAFETY: the module of the caller is always valid.
        let instance = unsafe { GetModuleHandleW(None) }.expect("the module handle");
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(fake_bridge_proc),
            hInstance: HINSTANCE(instance.0),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        // SAFETY: the class is described by a live local; Windows copies the name.
        unsafe { RegisterClassExW(&class) };
        let title: Vec<u16> = format!("{BRIDGE_TITLE_PREFIX}test")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // SAFETY: the class is registered and the strings live across the call.
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class_name.as_ptr()),
                PCWSTR(title.as_ptr()),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                HWND(std::ptr::null_mut()),
                HMENU::default(),
                HINSTANCE(instance.0),
                None,
            )
        }
        .expect("the fake bridge window")
    }

    /// The request travels to the bridge and the answer comes back.
    #[test]
    fn a_request_travels_to_the_bridge_and_the_answer_comes_back() {
        let (sender, receiver) = channel();
        RECEIVED.with(|slot| *slot.borrow_mut() = Some(sender));

        let window = fake_bridge_window();
        let mut request = Request::new(Op::Insert);
        request.text = "你好，世界".to_string();

        let answer = super::windows_impl::send_to(
            window.0 as isize,
            Duration::from_secs(5),
            &request,
        )
        .expect("the bridge answers");

        // the answer of the fake bridge
        assert!(answer.ok, "the answer says the write worked");
        assert_eq!(answer.inserted, 5, "five characters were written");
        assert_eq!(answer.language.as_deref(), Some("zh-CN"));

        // and the bridge really received the request of the client
        let received = receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("the bridge received the request");
        assert_eq!(received.op, Op::Insert);
        assert_eq!(received.text, "你好，世界");
        assert_ne!(received.reply, 0, "the request names the window of the answer");

        // SAFETY: the window belongs to this thread and is destroyed exactly once.
        unsafe {
            let _ = DestroyWindow(window);
        }
    }

    /// A bridge that does not answer must not block the caller for ever.
    #[test]
    fn a_bridge_that_does_not_answer_gives_up_in_time() {
        // a window of a class that nothing answers on
        let class_name: Vec<u16> = "PIME1AgentSilent".encode_utf16().chain(Some(0)).collect();
        // SAFETY: the module of the caller is always valid.
        let instance = unsafe { GetModuleHandleW(None) }.expect("the module handle");
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            // a window without a procedure would crash the process when the message arrives
            lpfnWndProc: Some(silent_proc),
            hInstance: HINSTANCE(instance.0),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        // SAFETY: the class is described by a live local; Windows copies the name.
        unsafe { RegisterClassExW(&class) };
        // SAFETY: the class is registered and the strings live across the call.
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class_name.as_ptr()),
                PCWSTR::null(),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                HWND(std::ptr::null_mut()),
                HMENU::default(),
                HINSTANCE(instance.0),
                None,
            )
        }
        .expect("the silent window");

        let request = Request::new(Op::Status);
        let started = std::time::Instant::now();
        let answer = super::windows_impl::send_to(
            window.0 as isize,
            Duration::from_millis(600),
            &request,
        );
        let elapsed = started.elapsed();
        assert!(answer.is_err(), "a bridge that does not answer is an error");
        // the error says that the input method did not answer in time, and it is the one that can be tried again
        let error = answer.unwrap_err();
        assert!(
            error.code() == Error::Busy.as_str() || error.code() == Error::NoIme.as_str(),
            "the error says that the input method did not answer: {error:?}"
        );
        assert!(
            elapsed < Duration::from_secs(3),
            "the caller is not blocked for ever ({elapsed:?})"
        );

        // SAFETY: the window belongs to this thread and is destroyed exactly once.
        unsafe {
            let _ = DestroyWindow(window);
        }
    }
}
