//! The bridge window: how an AI agent asks this input method to write into the window that has the focus.
//!
//! The text service lives **inside** the process of the application that has the focus, and only there does it know
//! the document, the caret and the selection. An agent that runs somewhere else (an MCP client, a CLI, a script) can
//! therefore not write into that document by itself: it has to ask the text service of the focused application to do
//! it, and it has to learn whether the text really landed.
//!
//! The channel is `WM_COPYDATA`. The agent finds the bridge window of the focused process and sends one message; the
//! window procedure of that window runs on the **UI thread of the focused application** - the thread that owns the
//! text service and the TSF context - so the write happens where the document is. The answer travels back the same way,
//! to the window the agent named in the request.
//!
//! The format is the one of [`pime_core::agent_proto`]: a JSON payload, bounded in size before it is parsed, and an
//! answer that says either what happened or why it did not happen.
//!
//! The trust model is the one of the clipboard: a program of the user that runs in the same session can ask the input
//! method to write into the focused window. That is no more than what simulated keystrokes can do, and a program that
//! can do that can already type into the window.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use pime_core::agent_proto::{
    Error, Reply, Request, BRIDGE_CLASS, BRIDGE_TITLE_PREFIX, COPYDATA_REPLY, COPYDATA_REQUEST,
    MAX_PAYLOAD_BYTES,
};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowLongPtrW, HMENU, RegisterClassExW,
    SendMessageW, SetWindowLongPtrW, GWLP_USERDATA, WINDOW_EX_STYLE, WINDOW_STYLE, WM_COPYDATA,
    WNDCLASSEXW,
};
use windows::core::PCWSTR;

/// What the bridge does with a request.
type Handler = Box<dyn Fn(Request) -> Reply>;

/// The `CopyDataStruct` of Windows, declared here instead of taken from the `windows` crate.
///
/// The bindings of the crate carry the same fields, but the layout is declared here so that it is visible and pinned by
/// a test: the struct travels between two processes, and a layout that is one byte off would make the message carry
/// nonsense. The Windows definition is `ULONG_PTR dwData; DWORD cbData; PVOID lpData;` - on x64 that is 24 bytes with
/// the data pointer at 16.
#[repr(C)]
#[derive(Clone, Copy)]
struct CopyDataStruct {
    dw_data: usize,
    cb_data: u32,
    lp_data: *mut c_void,
}

/// The state the window points at. It lives in the [`Bridge`], so it is alive as long as the window is.
struct Shared {
    handler: Handler,
}

/// The bridge window of this process.
///
/// The window is created when the text service is activated and destroyed when it is deactivated: exactly while the
/// input method of this application is the one that is running.
pub struct Bridge {
    hwnd: HWND,
    /// Keeps the handler alive while the window points at it.
    _shared: Box<Shared>,
}

impl Bridge {
    /// Open the bridge window. `handler` answers the requests of the agents.
    ///
    /// `None` when the window could not be created: the input method then works as before, without the bridge (an
    /// agent gets `NO_IME` and says so).
    pub fn create(handler: Handler) -> Option<Bridge> {
        register_class()?;
        let class: Vec<u16> = BRIDGE_CLASS.encode_utf16().chain(std::iter::once(0)).collect();
        let title = format!("{BRIDGE_TITLE_PREFIX}{}", nonce());
        let wide_title: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        let instance = module_instance();
        // SAFETY: the class is registered, the two strings live until the call returns (Windows copies them), and the
        // window is a hidden top-level window of this process.
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class.as_ptr()),
                PCWSTR(wide_title.as_ptr()),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                HWND(std::ptr::null_mut()),
                HMENU(std::ptr::null_mut()),
                HINSTANCE(instance),
                None,
            )
        }
        .ok()?;
        let shared = Box::new(Shared { handler });
        // SAFETY: the window is ours and the pointer stays valid as long as `shared` is alive (it is owned by the bridge).
        unsafe {
            SetWindowLongPtrW(
                hwnd,
                GWLP_USERDATA,
                (&*shared as *const Shared) as isize,
            )
        };
        Some(Bridge {
            hwnd,
            _shared: shared,
        })
    }

    /// Close the bridge window.
    ///
    /// The window must be destroyed on the thread that created it: the text service is activated and deactivated on the
    /// thread of the host, and the bridge lives exactly as long as it does.
    pub fn close(&mut self) {
        if self.hwnd.0.is_null() {
            return;
        }
        // SAFETY: the window is ours; the handler is cleared first so that no message can reach a freed handler.
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
        }
        self.hwnd = HWND(std::ptr::null_mut());
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.close();
    }
}

/// A value that makes every window of the process different: it is not a secret, it only identifies the window.
fn nonce() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}{:x}", now, count)
}

/// The module of the DLL: the class of the window belongs to it, so the window dies with the DLL.
fn module_instance() -> *mut c_void {
    // SAFETY: a null name asks for the module of the caller, which is this DLL.
    unsafe { GetModuleHandleW(None) }
        .map(|m| m.0)
        .unwrap_or(std::ptr::null_mut())
}

/// Register the class of the bridge window once per process.
///
/// The result of the registration has to survive the call: the text service is activated and deactivated over and over
/// (every switch of the input method does it), and the bridge is opened again every time. A local variable would be
/// `false` on the second call, and the bridge would never open again.
fn register_class() -> Option<()> {
    static REGISTERED: OnceLock<bool> = OnceLock::new();
    let registered = REGISTERED.get_or_init(|| {
        let name: Vec<u16> = BRIDGE_CLASS.encode_utf16().chain(std::iter::once(0)).collect();
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(bridge_wnd_proc),
            hInstance: HINSTANCE(module_instance()),
            lpszClassName: PCWSTR(name.as_ptr()),
            ..Default::default()
        };
        // SAFETY: the class is described by a live local; Windows copies the name.
        (unsafe { RegisterClassExW(&class) }) != 0
    });
    registered.then_some(())
}

/// The window procedure of the bridge window.
unsafe extern "system" fn bridge_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_COPYDATA {
        // SAFETY: Windows hands the address of a CopyDataStruct that is valid while the message is handled.
        let data = unsafe { &*(lparam.0 as *const CopyDataStruct) };
        // SAFETY: the handler is alive as long as the window points at it (the bridge owns it).
        return unsafe { handle_copy_data(hwnd, data, wparam.0 as u64) };
    }
    // SAFETY: the plain default handling of the window.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Answer one request of an agent.
unsafe fn handle_copy_data(hwnd: HWND, data: &CopyDataStruct, fallback: u64) -> LRESULT {
    if data.dw_data != COPYDATA_REQUEST as usize {
        return LRESULT(0); // a message of somebody else: not ours, not answered
    }
    // The window the answer goes to: the request names it, and `wparam` is the fallback for a request that cannot be
    // read (a malformed payload still deserves an answer).
    let mut target = fallback;
    let reply = match unsafe { request_of(data) } {
        Ok(request) => {
            if request.reply != 0 {
                target = request.reply;
            }
            match unsafe { shared_of(hwnd) } {
                Some(shared) => (shared.handler)(request),
                None => Reply::failed(Error::Busy, "the bridge has no handler"),
            }
        }
        Err(error) => Reply::failed(error, "the request could not be read"),
    };
    if target != 0 {
        // SAFETY: the target is a window of the agent; the payload lives until the call returns.
        unsafe { send_reply(HWND(target as *mut c_void), &reply) };
    }
    LRESULT(1)
}

/// The request of a payload.
unsafe fn request_of(data: &CopyDataStruct) -> Result<Request, Error> {
    if data.cb_data as usize > MAX_PAYLOAD_BYTES {
        return Err(Error::TooLarge);
    }
    if data.lp_data.is_null() {
        return Err(Error::Malformed);
    }
    // SAFETY: Windows points at `cbData` bytes of payload that live while the message is handled.
    let bytes = unsafe { std::slice::from_raw_parts(data.lp_data as *const u8, data.cb_data as usize) };
    Request::from_bytes(bytes)
}

/// The state of the window.
unsafe fn shared_of(hwnd: HWND) -> Option<&'static Shared> {
    // SAFETY: the pointer was set by `Bridge::create` and is cleared when the bridge closes.
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Shared;
    if ptr.is_null() {
        return None;
    }
    // SAFETY: the bridge owns the state and keeps it alive while the window points at it.
    Some(unsafe { &*ptr })
}

/// Send the answer to the window of the agent.
unsafe fn send_reply(target: HWND, reply: &Reply) {
    let mut bytes = reply.to_json().to_json_string().into_bytes();
    bytes.push(0); // a zero-terminated copy: the receiver may read it as a C string as well
    let data = CopyDataStruct {
        dw_data: COPYDATA_REPLY as usize,
        cb_data: bytes.len() as u32,
        lp_data: bytes.as_ptr() as *mut c_void,
    };
    // SAFETY: the payload lives until the call returns; the target is the window of the agent.
    unsafe {
        SendMessageW(
            target,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM((&data as *const CopyDataStruct) as isize),
        );
    }
}

/// Whether a write may go into the document that has the focus.
///
/// A password field never gets anything from an agent either: the rule of the product is that a secure field collects
/// nothing, and an agent is not more trusted than the keyboard.
pub fn may_write(secure_field: bool) -> Result<(), Error> {
    if secure_field {
        return Err(Error::Secure);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A password field never gets a word from an agent.
    #[test]
    fn a_password_field_is_never_written_into() {
        assert_eq!(may_write(true).err(), Some(Error::Secure));
        assert_eq!(may_write(false), Ok(()));
    }

    /// The class and the title are what the agent looks for: they must not change without the protocol changing.
    #[test]
    fn the_window_is_found_by_its_class_and_its_title() {
        assert_eq!(BRIDGE_CLASS, "PIME1AgentBridge");
        assert!(BRIDGE_TITLE_PREFIX.starts_with("PIME1."));
        let title = format!("{BRIDGE_TITLE_PREFIX}{}", nonce());
        assert!(title.starts_with(BRIDGE_TITLE_PREFIX));
        assert!(title.len() > BRIDGE_TITLE_PREFIX.len());
    }

    /// Two windows of the same process must not have the same title (the agent finds one of them).
    #[test]
    fn every_window_has_its_own_title() {
        let first = format!("{BRIDGE_TITLE_PREFIX}{}", nonce());
        let second = format!("{BRIDGE_TITLE_PREFIX}{}", nonce());
        assert_ne!(first, second);
    }

    /// The codes of the two messages are the ones of the protocol.
    #[test]
    fn the_codes_of_the_messages_are_fixed() {
        assert_eq!(COPYDATA_REQUEST, 0x5049_4D01);
        assert_eq!(COPYDATA_REPLY, 0x5049_4D02);
    }

    /// The struct travels between two processes: its layout must be the one of Windows, byte for byte.
    #[test]
    fn the_layout_of_the_message_struct_is_the_one_of_windows() {
        let size = std::mem::size_of::<CopyDataStruct>();
        let data = std::mem::offset_of!(CopyDataStruct, dw_data);
        let count = std::mem::offset_of!(CopyDataStruct, cb_data);
        let payload = std::mem::offset_of!(CopyDataStruct, lp_data);
        assert_eq!(data, 0, "dwData comes first");
        assert_eq!(count, 8, "cbData follows the pointer-sized field");
        assert_eq!(payload, 16, "lpData is the third field");
        assert_eq!(size, 24, "the struct of Windows on x64");
    }

    /// The text service is activated and deactivated over and over (every switch of the input method does it), and the
    /// bridge is opened again every time. The class of the window is registered once, but the **answer** of the
    /// registration has to be remembered: a bridge that cannot be opened the second time means that no agent can reach
    /// the input method any more.
    #[test]
    fn the_class_is_still_there_when_the_bridge_is_opened_again() {
        assert!(register_class().is_some(), "the class is registered");
        assert!(
            register_class().is_some(),
            "the second activation of the text service must find the class again"
        );
    }
}

/// The channel itself: a real `WM_COPYDATA` round trip on a real Windows.
///
/// The text service is the side that answers, so this is where the round trip has to be proven: the bridge opens a
/// real hidden window, a real `WM_COPYDATA` message arrives at it, the window procedure runs on the thread of the
/// host, the handler is called and the answer travels back to the window that the request named. Without this test a
/// mistake in the message plumbing would only show up in a real editor, with a real agent.
#[cfg(all(windows, test))]
mod channel_tests {
    use super::*;
    use pime_core::agent_proto::{Op, Request};
    use std::cell::RefCell;
    use std::sync::mpsc::{Sender, channel};
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, HMENU, HWND_MESSAGE, MSG,
        PeekMessageW, PM_REMOVE, RegisterClassExW, SendMessageTimeoutW, SMTO_ABORTIFHUNG,
        TranslateMessage, WNDCLASSEXW,
    };

    thread_local! {
        /// The request the bridge received: the test wants to see it.
        static RECEIVED: RefCell<Option<Sender<Request>>> = const { RefCell::new(None) };
        /// The answer the bridge sent: the test wants to read it.
        static ANSWER: RefCell<Option<Sender<Vec<u8>>>> = const { RefCell::new(None) };
    }

    /// The window procedure of the window of the agent: it keeps the answer of the bridge.
    unsafe extern "system" fn answer_window_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_COPYDATA {
            // SAFETY: the message carries the structure of the sender; it is valid while the message is handled.
            let data = unsafe { &*(lparam.0 as *const CopyDataStruct) };
            if !data.lp_data.is_null() {
                // SAFETY: the sender guarantees `cb_data` readable bytes for the duration of the message.
                let bytes =
                    unsafe { std::slice::from_raw_parts(data.lp_data as *const u8, data.cb_data as usize) };
                let bytes = bytes.to_vec();
                ANSWER.with(|slot| {
                    if let Some(sender) = slot.borrow().as_ref() {
                        let _ = sender.send(bytes);
                    }
                });
            }
            return LRESULT(1);
        }
        // SAFETY: the plain default handling of the window.
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    /// The window of the agent: a hidden message-only window that waits for the answer.
    fn answer_window() -> HWND {
        let class_name: Vec<u16> = "PIME1AgentTestAnswer"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(answer_window_proc),
            hInstance: HINSTANCE(module_instance()),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        // SAFETY: the class is described by a live local; Windows copies the name.
        unsafe { RegisterClassExW(&class) };
        // SAFETY: the class is registered and the strings live across the call.
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class_name.as_ptr()),
                PCWSTR::null(),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                HMENU(std::ptr::null_mut()),
                HINSTANCE(module_instance()),
                None,
            )
        }
        .expect("the window of the agent")
    }

    /// The messages of this thread are dispatched while the test waits for the answer.
    fn pump() {
        let mut msg = MSG::default();
        loop {
            // SAFETY: the message structure is valid.
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

    /// The request reaches the handler and the answer comes back.
    #[test]
    fn a_request_reaches_the_handler_and_the_answer_comes_back() {
        let (request_sender, request_receiver) = channel();
        let (answer_sender, answer_receiver) = channel();
        RECEIVED.with(|slot| *slot.borrow_mut() = Some(request_sender));
        ANSWER.with(|slot| *slot.borrow_mut() = Some(answer_sender));

        let agent_window = answer_window();

        let bridge = Bridge::create(Box::new(move |request: Request| {
            RECEIVED.with(|slot| {
                if let Some(sender) = slot.borrow().as_ref() {
                    let _ = sender.send(request.clone());
                }
            });
            let mut reply = Reply::ok();
            reply.inserted = request.text.chars().count();
            reply.language = Some("ja-JP".to_string());
            reply
        }))
        .expect("the bridge window");

        // the request, as the agent sends it
        let mut request = Request::new(Op::Insert);
        request.text = "日本語".to_string();
        request.reply = agent_window.0 as u64;
        let payload = request.to_json().to_json_string().into_bytes();

        // The message is sent from a thread of its own: the bridge answers with `SendMessageW`, and that answer is a
        // sent message that the thread of the bridge has to dispatch. The handle travels as a number: a window handle
        // is a pointer, and a pointer may not be sent between threads.
        let bridge_handle = bridge.hwnd.0 as isize;
        let sender = std::thread::spawn(move || {
            let target = HWND(bridge_handle as *mut c_void);
            let data = CopyDataStruct {
                dw_data: COPYDATA_REQUEST as usize,
                cb_data: payload.len() as u32,
                lp_data: payload.as_ptr() as *mut c_void,
            };
            let mut result = 0usize;
            // SAFETY: the structure lives until the call is over; the handle is the bridge window.
            let sent = unsafe {
                SendMessageTimeoutW(
                    target,
                    WM_COPYDATA,
                    WPARAM(0),
                    LPARAM((&data as *const CopyDataStruct) as isize),
                    SMTO_ABORTIFHUNG,
                    4000,
                    Some(&mut result),
                )
            };
            sent.0 != 0
        });

        // the answer is waited for while the messages of this thread are dispatched
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut answer = None;
        while std::time::Instant::now() < deadline {
            pump();
            match answer_receiver.try_recv() {
                Ok(bytes) => {
                    answer = Some(bytes);
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(sender.join().expect("the sender thread"), "the message was sent");

        // the handler really ran
        let received = request_receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("the handler was called");
        assert_eq!(received.op, Op::Insert);
        assert_eq!(received.text, "日本語");

        // and the answer came back through the window the request named
        let bytes = answer.expect("the answer of the bridge");
        // the answer is a zero-terminated copy: the zero is not part of the JSON
        let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(bytes.len());
        let reply = Reply::from_bytes(&bytes[..end]).expect("the answer is ours");
        assert!(reply.ok, "the answer says the write worked");
        assert_eq!(reply.inserted, 3, "three characters were written");
        assert_eq!(reply.language.as_deref(), Some("ja-JP"));

        // SAFETY: the window of the agent belongs to this thread and is destroyed exactly once.
        unsafe {
            let _ = DestroyWindow(agent_window);
        }
    }

    /// The name of the test that plays the part of the agent in another process.
    const CHILD_TEST: &str = "agent_bridge::channel_tests::the_agent_of_another_process_sends_the_request";

    /// The agent in another process: it sends the request to the bridge of this process and reads the answer.
    ///
    /// The test that plays this part is the one that is named [`CHILD_TEST`]; it does nothing when the environment
    /// does not tell it to play it, so a normal test run does not start a process of its own.
    #[test]
    fn the_agent_of_another_process_sends_the_request() {
        let Ok(handle) = std::env::var("PIME_AGENT_BRIDGE_HWND") else {
            return; // not the child: nothing to do
        };
        let Ok(handle) = handle.parse::<isize>() else {
            panic!("the handle of the bridge is not a number");
        };
        // the handle travels as a number: a window handle is a pointer, and a pointer may not be sent between threads
        let target_handle = handle;

        // the window of the agent: it waits for the answer of the bridge
        let (answer_sender, answer_receiver) = channel();
        ANSWER.with(|slot| *slot.borrow_mut() = Some(answer_sender));
        let class_name: Vec<u16> = "PIME1AgentTestChild"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(answer_window_proc),
            hInstance: HINSTANCE(module_instance()),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        // SAFETY: the class is described by a live local; Windows copies the name.
        unsafe { RegisterClassExW(&class) };
        // SAFETY: the class is registered and the strings live across the call.
        let agent_window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class_name.as_ptr()),
                PCWSTR::null(),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                HMENU(std::ptr::null_mut()),
                HINSTANCE(module_instance()),
                None,
            )
        }
        .expect("the window of the agent");

        // the request
        let mut request = Request::new(Op::Insert);
        request.text = "跨进程".to_string();
        request.reply = agent_window.0 as u64;
        let payload = request.to_json().to_json_string().into_bytes();

        // the request is sent from a thread of its own: the answer of the bridge is a sent message, and the thread
        // that owns the window of the agent has to dispatch it
        let sender = std::thread::spawn(move || {
            let target = HWND(target_handle as *mut c_void);
            let data = CopyDataStruct {
                dw_data: COPYDATA_REQUEST as usize,
                cb_data: payload.len() as u32,
                lp_data: payload.as_ptr() as *mut c_void,
            };
            let mut result = 0usize;
            // SAFETY: the structure lives until the call is over; the handle is the bridge of the other process.
            let sent = unsafe {
                SendMessageTimeoutW(
                    target,
                    WM_COPYDATA,
                    WPARAM(0),
                    LPARAM((&data as *const CopyDataStruct) as isize),
                    SMTO_ABORTIFHUNG,
                    8000,
                    Some(&mut result),
                )
            };
            sent.0 != 0
        });

        // the answer is waited for while the messages of this thread are dispatched
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut answer = None;
        while std::time::Instant::now() < deadline {
            pump();
            match answer_receiver.try_recv() {
                Ok(bytes) => {
                    answer = Some(bytes);
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(sender.join().expect("the sender thread"), "the message was sent");
        let bytes = answer.expect("the answer of the bridge of the other process");
        let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(bytes.len());
        let reply = Reply::from_bytes(&bytes[..end]).expect("the answer is ours");
        assert!(reply.ok, "the answer says the write worked");
        assert_eq!(reply.inserted, 3, "three characters were written");
    }

    /// The bridge of this process answers an agent that runs in another process.
    ///
    /// In the product the agent (an MCP client, a CLI) and the editor are always **two processes**: the text service
    /// lives inside the editor. This test is the one that proves that the message crosses the process boundary: the
    /// child process sends the request and waits for the answer, this process answers.
    #[test]
    fn the_bridge_answers_an_agent_of_another_process() {
        let (request_sender, request_receiver) = channel();
        RECEIVED.with(|slot| *slot.borrow_mut() = Some(request_sender));

        let bridge = Bridge::create(Box::new(move |request: Request| {
            RECEIVED.with(|slot| {
                if let Some(sender) = slot.borrow().as_ref() {
                    let _ = sender.send(request.clone());
                }
            });
            let mut reply = Reply::ok();
            reply.inserted = request.text.chars().count();
            reply
        }))
        .expect("the bridge window");

        // the agent of another process: the same test program, told to play the part of the agent
        let exe = std::env::current_exe().expect("the path of this program");
        let mut child = std::process::Command::new(exe)
            .args([CHILD_TEST, "--exact", "--nocapture", "--test-threads=1"])
            .env("PIME_AGENT_BRIDGE_HWND", (bridge.hwnd.0 as isize).to_string())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("the agent of another process");

        // the messages of this thread are dispatched while the agent works: the bridge answers on this thread
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut status = None;
        while std::time::Instant::now() < deadline {
            pump();
            match child.try_wait() {
                Ok(Some(result)) => {
                    status = Some(result);
                    break;
                }
                Ok(None) => {}
                Err(error) => panic!("the agent of another process failed: {error}"),
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let Some(status) = status else {
            let _ = child.kill();
            panic!("the agent of another process did not answer in time");
        };
        let output = child.wait_with_output().expect("the output of the agent");
        assert!(
            status.success(),
            "the agent of another process failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        // the handler of this process really ran, with the text of the other process
        let received = request_receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("the handler was called");
        assert_eq!(received.op, Op::Insert);
        assert_eq!(received.text, "跨进程");
    }
}
