//! The resident tray of the input method.
//!
//! The input method may stay resident: a small program lives in the notification area, keeps the logo of the brand
//! there (where Sogou keeps it too) and owns the gestures that must work even when IME1.0 is **not** the input method
//! that is active:
//!
//! * Ctrl+F12 switches between IME1.0 and the input method the user came from. The user is writing in the Japanese
//!   input method, wants to write code, presses the key, writes code, presses it again and is back in Japanese.
//! * Ctrl+Alt+V starts the voice input: the microphone is recorded, the speech is recognised and the text is written
//!   into the program the user is in.
//!
//! The tray never takes the focus: the user is typing in another program and the input method must not take the
//! caret away from it.

mod autostart;
mod icon;
mod tray;
mod voice;

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{HOT_KEY_MODIFIERS, RegisterHotKey, UnregisterHotKey};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetMessageW, PostQuitMessage, RegisterClassW, TranslateMessage, TrackPopupMenu, MF_CHECKED,
    MF_STRING, MENU_ITEM_FLAGS, TPM_RETURNCMD, TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_COMMAND, WM_DESTROY, WM_HOTKEY, WNDCLASSW,
};

use crate::tray::{WM_TRAY, cursor_position};
use pime_tsf::imesettings::ImeSettings;
use pime_tsf::settings_window::{SettingsEvent, SettingsSink, SettingsWindow};

/// The identifier of the icon in the notification area.
const TRAY_ID: u32 = 1;

/// The hot key that switches the input method.
const HOTKEY_SWITCH: i32 = 1;
/// The hot key that starts the voice input.
const HOTKEY_VOICE: i32 = 2;

/// The menu item that switches the input method.
const MENU_SWITCH: usize = 100;
/// The menu item that starts the voice input.
const MENU_VOICE: usize = 101;
/// The menu item that opens the settings.
const MENU_SETTINGS: usize = 102;
/// The menu item that ends the tray.
const MENU_QUIT: usize = 103;
/// The menu item that starts the tray with Windows (a check box).
const MENU_AUTOSTART: usize = 104;

/// `MOD_CONTROL`.
const MOD_CONTROL: HOT_KEY_MODIFIERS = HOT_KEY_MODIFIERS(0x0002);
/// `MOD_ALT`.
const MOD_ALT: HOT_KEY_MODIFIERS = HOT_KEY_MODIFIERS(0x0001);

/// The class of the message window.
const CLASS_NAME: &str = "ProgrammingIME.Tray";

/// What the tray is doing right now.
struct State {
    /// The voice input, if it is running.
    voice: Option<voice::Voice>,
    /// The settings window, while it is open.
    settings: Option<SettingsWindow>,
    /// The settings window was closed and may be dropped. Dropping it inside the window procedure would destroy the
    /// window a second time, so it waits until the message is over.
    settings_closed: bool,
}

impl State {
    fn new() -> State {
        State {
            voice: None,
            settings: None,
            settings_closed: false,
        }
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_HOTKEY => {
            match wparam.0 as i32 {
                HOTKEY_SWITCH => {
                    // The switch: IME1.0 and the input method the user came from.
                    pime_tsf::ime_switch::switch_now();
                }
                HOTKEY_VOICE => {
                    // The voice: start it, or stop it and write the text.
                    start_or_stop_voice(hwnd);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_TRAY => {
            // The left button opens the menu of the settings, the right button the same one.
            let low = lparam.0 as u32 & 0xffff;
            if low == 0x0201 || low == 0x0202 || low == 0x0203 || low == 0x0205 {
                show_menu(hwnd);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = wparam.0 & 0xffff;
            run_command(hwnd, id);
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: the message ends the loop of this thread.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => {
            // SAFETY: a plain call on our own window.
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
    }
}

/// Do what the user chose in the menu of the tray.
///
/// Both the menu (which returns the command) and a `WM_COMMAND` message end up here, so that the two can never
/// drift apart.
fn run_command(hwnd: HWND, id: usize) {
    match id {
        MENU_SWITCH => {
            pime_tsf::ime_switch::switch_now();
        }
        MENU_VOICE => {
            start_or_stop_voice(hwnd);
        }
        MENU_SETTINGS => {
            open_settings(hwnd);
        }
        MENU_AUTOSTART => {
            toggle_autostart();
        }
        MENU_QUIT => {
            // SAFETY: the window is live and the message ends the loop.
            unsafe { PostQuitMessage(0) };
        }
        _ => {}
    }
}

/// Open the settings of the input method, with the settings the user has right now.
fn open_settings(hwnd: HWND) {
    let state = window_state(hwnd);
    if state.borrow().settings.is_some() {
        return; // already open
    }
    let settings = match pime_tsf::imesettings::settings_path() {
        Some(path) => ImeSettings::load(&path),
        None => ImeSettings::default(),
    };
    // The window writes the file itself; the text service reads it again the next time it needs it.
    let sink: SettingsSink = {
        let weak = std::rc::Rc::downgrade(&state);
        std::rc::Rc::new(move |_event: SettingsEvent| {
            if let Some(state) = weak.upgrade() {
                state.borrow_mut().settings_closed = true;
            }
        })
    };
    match SettingsWindow::open(settings, sink) {
        Some(window) => state.borrow_mut().settings = Some(window),
        None => notify("打不开输入法设置窗口。"),
    }
}

/// Start the tray with Windows, or stop doing that: the user decides.
fn toggle_autostart() {
    let enabled = autostart::is_enabled();
    let result = if enabled {
        autostart::disable()
    } else {
        match autostart::own_exe() {
            Some(exe) => autostart::enable(&exe),
            None => Err("找不到自己的程序路径".to_string()),
        }
    };
    if let Err(reason) = result {
        notify(&reason);
    }
}

/// Show a message to the user.
///
/// The tray has no window of its own, so a balloon of the icon cannot be shown; a message box is the one thing every
/// Windows program can do.
fn notify(text: &str) {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONWARNING, MB_OK, MessageBoxW};
    let body: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let title: Vec<u16> = "IME1.0 编程输入法"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: both strings are NUL-terminated and outlive the call; a null window means "no owner".
    let _ = unsafe {
        MessageBoxW(
            HWND::default(),
            PCWSTR(body.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONWARNING,
        )
    };
}

/// Start the voice input, or stop it and write what was said.
fn start_or_stop_voice(hwnd: HWND) {
    // The state lives with the window, so that the menu and the hot key share it.
    let state = window_state(hwnd);
    if let Some(voice) = state.borrow_mut().voice.take() {
        // The user pressed the key again: stop, recognise, write.
        voice::finish(voice);
    } else {
        match voice::Voice::start() {
            Ok(voice) => {
                state.borrow_mut().voice = Some(voice);
            }
            Err(reason) => {
                // No microphone, no permission, no recording device: the tray says so and stays out of the way.
                voice::report(reason);
            }
        }
    }
}

/// The state of the tray, kept with the window.
fn window_state(hwnd: HWND) -> std::rc::Rc<std::cell::RefCell<State>> {
    // SAFETY: the pointer was put there by `WM_NCCREATE` and is only read here.
    unsafe {
        let raw = windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(
            hwnd,
            windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
        );
        if raw == 0 {
            return std::rc::Rc::new(std::cell::RefCell::new(State::new()));
        }
        let raw = raw as *const std::rc::Rc<std::cell::RefCell<State>>;
        (*raw).clone()
    }
}

/// Open the menu of the tray under the mouse.
fn show_menu(hwnd: HWND) {
    // SAFETY: a plain call that creates a menu.
    let Ok(menu) = (unsafe { CreatePopupMenu() }) else {
        return;
    };
    // The check box of "start with Windows" shows the state that is really there, not the state the last click left.
    let autostart = autostart::is_enabled();
    let items: [(usize, &str, windows::Win32::UI::WindowsAndMessaging::MENU_ITEM_FLAGS); 5] = [
        (MENU_SWITCH, "切换输入法\tCtrl+F12", MENU_ITEM_FLAGS(0)),
        (MENU_VOICE, "语音输入\tCtrl+Alt+V", MENU_ITEM_FLAGS(0)),
        (MENU_SETTINGS, "设置…", MENU_ITEM_FLAGS(0)),
        (
            MENU_AUTOSTART,
            "开机自动启动",
            if autostart {
                MENU_ITEM_FLAGS(MF_CHECKED.0)
            } else {
                MENU_ITEM_FLAGS(0)
            },
        ),
        (MENU_QUIT, "退出", MENU_ITEM_FLAGS(0)),
    ];
    for (id, label, flags) in items {
        let text: Vec<u16> = label.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: the text is NUL-terminated and outlives the call; the menu is live.
        let _ = unsafe { AppendMenuW(menu, flags | MF_STRING, id, windows::core::PCWSTR(text.as_ptr())) };
    }
    let point: POINT = cursor_position();
    // SAFETY: the menu is live and the window is live; the command is returned instead of sent.
    let command = unsafe {
        TrackPopupMenu(
            menu,
            TPM_RIGHTBUTTON | TPM_RETURNCMD,
            point.x,
            point.y,
            0,
            hwnd,
            None,
        )
    };
    tray::close_menu(menu);
    if command.0 == 0 {
        return;
    }
    run_command(hwnd, command.0 as usize);
}

/// The commands that set the voice input up and then end.
///
/// * `pime-tray --set-key` reads the API key from the standard input and keeps it in the Windows Credential Manager.
/// * `pime-tray --set-voice <address>|<model>|<language>` keeps the speech service and its language.
/// * `pime-tray --forget-key` takes the API key out of the Credential Manager.
fn settings_command() -> Option<std::process::ExitCode> {
    use std::io::Read;
    let args: Vec<String> = std::env::args().collect();
    let has = |name: &str| args.iter().any(|a| a == name);
    let value_of = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    if has("--set-key") {
        let mut key = String::new();
        if std::io::stdin().read_to_string(&mut key).is_err() {
            eprintln!("IME1.0: 读不到密钥");
            return Some(std::process::ExitCode::FAILURE);
        }
        let settings = voice::load_settings();
        return Some(match voice::store_key(&key, &settings.endpoint) {
            Ok(()) => {
                println!("语音输入的密钥已经保存在凭据管理器里。");
                std::process::ExitCode::SUCCESS
            }
            Err(reason) => {
                eprintln!("IME1.0: {}", reason.message());
                std::process::ExitCode::FAILURE
            }
        });
    }
    if has("--forget-key") {
        return Some(match voice::forget_key() {
            Ok(()) => {
                println!("语音输入的密钥已经从凭据管理器里取出。");
                std::process::ExitCode::SUCCESS
            }
            Err(reason) => {
                eprintln!("IME1.0: {}", reason.message());
                std::process::ExitCode::FAILURE
            }
        });
    }
    if has("--set-voice") {
        let Some(value) = value_of("--set-voice") else {
            eprintln!("IME1.0: --set-voice 需要 <地址>|<模型>|<语言>");
            return Some(std::process::ExitCode::FAILURE);
        };
        let mut parts = value.split('|');
        let mut settings = voice::Settings::default();
        if let Some(endpoint) = parts.next() {
            settings.endpoint = endpoint.trim().to_string();
        }
        if let Some(model) = parts.next() {
            settings.model = model.trim().to_string();
        }
        if let Some(language) = parts.next() {
            settings.language = language.trim().to_string();
        }
        voice::save_settings(&settings);
        println!("语音服务：{}", settings.endpoint);
        return Some(std::process::ExitCode::SUCCESS);
    }
    None
}

/// The program.
fn main() -> std::process::ExitCode {
    // The commands that set the voice input up and then end.
    if let Some(code) = settings_command() {
        return code;
    }
    // SAFETY: a plain call; the apartment is released at the end of this function.
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();

    // SAFETY: the instance handle is only read.
    let instance = unsafe { GetModuleHandleW(None) }.unwrap_or_default();
    let class_name: Vec<u16> = CLASS_NAME.encode_utf16().chain(std::iter::once(0)).collect();
    let class = WNDCLASSW {
        lpfnWndProc: Some(wnd_proc),
        hInstance: HINSTANCE(instance.0),
        lpszClassName: windows::core::PCWSTR(class_name.as_ptr()),
        ..Default::default()
    };
    // SAFETY: the class is a live local and the window procedure is a plain function.
    if unsafe { RegisterClassW(&class) } == 0 {
        if com {
            // SAFETY: the apartment was initialised above.
            unsafe { CoUninitialize() };
        }
        return std::process::ExitCode::FAILURE;
    }

    let state = std::rc::Rc::new(std::cell::RefCell::new(State::new()));
    // SAFETY: the window is created without a parent and without a menu.
    let window = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            windows::core::PCWSTR(class_name.as_ptr()),
            windows::core::PCWSTR::null(),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            HWND::default(),
            None,
            HINSTANCE(instance.0),
            None,
        )
    };
    let Ok(window) = window else {
        if com {
            // SAFETY: the apartment was initialised above.
            unsafe { CoUninitialize() };
        }
        return std::process::ExitCode::FAILURE;
    };
    // SAFETY: the window is live and the pointer outlives it (the state is dropped with the program).
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(
            window,
            windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
            std::rc::Rc::as_ptr(&state) as isize,
        );
    }

    // The logo in the corner.
    let _tray = tray::Tray::add(window, TRAY_ID, "IME1.0 编程输入法", &icon::icon_resource());

    // The keys that must work even when IME1.0 is not the active input method.
    // SAFETY: the window is live; the ids are ours.
    let _ = unsafe { RegisterHotKey(window, HOTKEY_SWITCH, MOD_CONTROL, 0x7B) }; // Ctrl+F12
    let _ = unsafe { RegisterHotKey(window, HOTKEY_VOICE, MOD_CONTROL | MOD_ALT, 0x56) }; // Ctrl+Alt+V

    // The loop.
    let mut message = windows::Win32::UI::WindowsAndMessaging::MSG::default();
    // SAFETY: the message is a live local.
    unsafe {
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
            // The settings window is dropped here and not inside its own window procedure: dropping it there would
            // destroy the window a second time, while it is being destroyed.
            if state.borrow().settings_closed {
                let mut borrowed = state.borrow_mut();
                borrowed.settings_closed = false;
                borrowed.settings = None;
            }
        }
    }

    // SAFETY: the hot keys were registered for this window.
    unsafe {
        let _ = UnregisterHotKey(window, HOTKEY_SWITCH);
        let _ = UnregisterHotKey(window, HOTKEY_VOICE);
        let _ = DestroyWindow(window);
    }
    if com {
        // SAFETY: the apartment was initialised above.
        unsafe { CoUninitialize() };
    }
    std::process::ExitCode::SUCCESS
}
