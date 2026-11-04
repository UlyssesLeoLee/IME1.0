//! The settings window: one group of basic settings per language profile, with a save button.
//!
//! The window is an ordinary top-level window (it takes the focus while it is open, so that the user can use the
//! keyboard) that is created on the thread of the host application and destroyed when it is closed. The controls are
//! standard Windows controls (a group of checkboxes and a drop-down list per language); their geometry is computed by
//! the pure [`compute_layout`], so that "what is where" is testable without a screen.
//!
//! What the window does with the values is deliberately small: it reads the controls, writes the file and tells the
//! text service that something changed. The text service then re-reads the file and applies it (see `tip`).

use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;

use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BN_CLICKED, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_PUSHBUTTON,
    CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, CBS_DROPDOWNLIST, CreateWindowExW, DefWindowProcW,
    DestroyWindow, GWLP_USERDATA, GetForegroundWindow, GetWindowLongPtrW, HMENU, HWND_TOPMOST,
    IDC_ARROW, LWA_ALPHA, LoadCursorW, RegisterClassExW, SW_SHOW, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, SendMessageW, SetForegroundWindow, SetLayeredWindowAttributes, SetWindowLongPtrW,
    SetWindowPos, SetWindowTextW, ShowWindow, WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_CREATE,
    WM_DESTROY, WNDCLASSEXW, WS_CAPTION, WS_CHILD, WS_CLIPCHILDREN, WS_EX_APPWINDOW,
    WS_EX_LAYERED, WS_OVERLAPPEDWINDOW, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};
use windows::core::{PCWSTR, w};

use crate::imesettings::{self, ImeSettings, LanguageSettings};
use crate::layout::Rect;
use crate::popup::module_instance;
use crate::ui::guard;

/// The window class is process-wide: a distinctive name, registered once per process.
const CLASS_NAME: PCWSTR = w!("ProgrammingIME.SettingsWindow");

/// How opaque the window is: **80%**, the same material as every panel of the product (see `LaserTheme::MATERIAL_ALPHA`).
///
/// The value is the one Windows takes for a layered window: 0 is invisible, 255 is opaque. 80 % of 255 is 204.
const MATERIAL_ALPHA: u8 = 204;

/// The check box of a button: the value `BM_GETCHECK` answers for a checked box.
const BST_CHECKED: i32 = 1;

/// The control id of the save button.
const ID_SAVE: usize = 9001;
/// The control id of the cancel button.
const ID_CANCEL: usize = 9002;
/// The control id of the "back to the defaults" button.
const ID_DEFAULTS: usize = 9003;

/// The buttons at the bottom of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsButton {
    /// Write the settings and close the window.
    Save,
    /// Close the window without writing anything.
    Cancel,
    /// Put the controls back to the values the product ships with.
    Defaults,
}

/// The check boxes of one language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toggle {
    /// The Tab key walks the candidates instead of confirming the highlighted one.
    TabCycles,
    /// The Tab key belongs to the host even while the candidate list is on screen.
    TabToHost,
    /// The space key takes the highlighted candidate.
    SpaceConfirms,
    /// A composition is opened as soon as candidates appear.
    AutoCompose,
    /// The input method starts in input mode.
    InputMode,
    /// The floating bar is shown.
    FloatingBar,
}

/// The kind of one control of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlKind {
    /// The name of the language (a static text).
    GroupTitle,
    /// The label of the drop-down list ("候选页大小").
    PageSizeLabel,
    /// The drop-down list with the page sizes.
    PageSizeCombo,
    /// One check box.
    Check(Toggle),
}

/// One control of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlSlot {
    pub kind: ControlKind,
    /// The rectangle, relative to the client area of the window.
    pub rect: Rect,
    /// The language the control belongs to.
    pub language: Option<usize>,
}

/// The geometry of the settings window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsLayout {
    /// The size of the client area.
    pub width: i32,
    pub height: i32,
    /// The controls, in the order they are created.
    pub controls: Vec<ControlSlot>,
    /// The buttons at the bottom.
    pub buttons: Vec<(Rect, SettingsButton)>,
}

/// The margin around the content.
const MARGIN: i32 = 12;
/// The height of one row of controls.
const ROW: i32 = 22;
/// The height of the title of a language group.
const GROUP_TITLE: i32 = 20;
/// The gap between two language groups.
const GROUP_GAP: i32 = 10;
/// The width of a check box (the label is drawn by the control itself).
const CHECK_WIDTH: i32 = 290;
/// The width of the drop-down list.
const COMBO_WIDTH: i32 = 80;
/// The height of the button row.
const BUTTON_ROW: i32 = 30;

/// The geometry of the settings window for the three languages of the product.
pub fn compute_layout() -> SettingsLayout {
    let mut controls = Vec::new();
    let mut y = MARGIN;
    for (index, _language) in imesettings::LANGUAGES.iter().enumerate() {
        controls.push(ControlSlot {
            kind: ControlKind::GroupTitle,
            rect: Rect::new(MARGIN, y, MARGIN + 240, y + GROUP_TITLE),
            language: Some(index),
        });
        y += GROUP_TITLE;
        controls.push(ControlSlot {
            kind: ControlKind::PageSizeLabel,
            rect: Rect::new(MARGIN + 12, y, MARGIN + 12 + 90, y + ROW),
            language: Some(index),
        });
        controls.push(ControlSlot {
            kind: ControlKind::PageSizeCombo,
            rect: Rect::new(MARGIN + 110, y, MARGIN + 110 + COMBO_WIDTH, y + ROW),
            language: Some(index),
        });
        y += ROW;
        for toggle in [
            Toggle::TabCycles,
            Toggle::TabToHost,
            Toggle::SpaceConfirms,
            Toggle::AutoCompose,
            Toggle::InputMode,
            Toggle::FloatingBar,
        ] {
            controls.push(ControlSlot {
                kind: ControlKind::Check(toggle),
                rect: Rect::new(MARGIN + 12, y, MARGIN + 12 + CHECK_WIDTH, y + ROW),
                language: Some(index),
            });
            y += ROW;
        }
        y += GROUP_GAP;
    }
    let height = y + BUTTON_ROW + MARGIN;
    let mut buttons = Vec::new();
    let mut x = MARGIN;
    for (kind, label_width) in [
        (SettingsButton::Save, 90),
        (SettingsButton::Cancel, 90),
        (SettingsButton::Defaults, 120),
    ] {
        buttons.push((Rect::new(x, y, x + label_width, y + BUTTON_ROW - 6), kind));
        x += label_width + 8;
    }
    SettingsLayout {
        width: (MARGIN * 2 + 300).max(x),
        height,
        controls,
        buttons,
    }
}

/// The label of a check box.
pub fn toggle_label(toggle: Toggle) -> &'static str {
    match toggle {
        Toggle::TabCycles => "Tab 确认候选上屏（否则 Tab 只移动选择）",
        Toggle::TabToHost => "候选窗显示时 Tab 交给宿主（IDE 保留缩进）",
        Toggle::SpaceConfirms => "空格上屏（否则空格就是空格）",
        Toggle::AutoCompose => "自动开组合",
        Toggle::InputMode => "默认进入输入模式",
        Toggle::FloatingBar => "显示悬浮窗",
    }
}

/// The label of the page size drop-down.
pub const PAGE_SIZE_LABEL: &str = "候选页大小";

/// What happened in the settings window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsEvent {
    /// The user saved the settings: the file was written.
    Saved,
    /// The window was closed without saving.
    Cancelled,
}

/// How the settings window tells the text service what happened.
pub type SettingsSink = Rc<dyn Fn(SettingsEvent)>;

/// The state the window procedure needs.
struct SettingsState {
    settings: ImeSettings,
    layout: SettingsLayout,
    /// The window of every control, in the order of the layout.
    handles: Vec<HWND>,
    sink: SettingsSink,
    /// Whether the user saved the settings (then the window does not report a cancellation).
    saved: bool,
    /// The window that had the focus before the settings window opened.
    owner: HWND,
}

impl SettingsState {
    /// Read the controls into the settings.
    fn collect(&self) -> ImeSettings {
        let mut settings = self.settings.clone();
        for (slot, handle) in self.layout.controls.iter().zip(self.handles.iter()) {
            let Some(language) = slot.language else {
                continue;
            };
            let key = imesettings::LANGUAGES[language];
            let mut value = settings.get(key);
            match slot.kind {
                ControlKind::PageSizeCombo => {
                    // SAFETY: a plain call on our own combo box.
                    let index =
                        unsafe { SendMessageW(*handle, CB_GETCURSEL, WPARAM(0), LPARAM(0)) }.0;
                    if index >= 0 {
                        value.page_size = (index as usize + 1).clamp(1, 9);
                    }
                }
                ControlKind::Check(toggle) => {
                    // SAFETY: a plain call on our own check box.
                    let checked =
                        unsafe { SendMessageW(*handle, BM_GETCHECK, WPARAM(0), LPARAM(0)) }.0
                            == BST_CHECKED as isize;
                    match toggle {
                        Toggle::TabCycles => value.tab_cycles = checked,
                        Toggle::TabToHost => value.tab_to_host = checked,
                        Toggle::SpaceConfirms => value.space_confirms = checked,
                        Toggle::AutoCompose => value.auto_compose = checked,
                        Toggle::InputMode => value.input_mode = checked,
                        Toggle::FloatingBar => value.floating_bar = checked,
                    }
                }
                _ => {}
            }
            settings.set(key, value);
        }
        settings
    }

    /// Put the controls back to the values the product ships with.
    fn show_defaults(&mut self) {
        let defaults = LanguageSettings::default();
        for (slot, handle) in self.layout.controls.iter().zip(self.handles.iter()) {
            match slot.kind {
                ControlKind::PageSizeCombo => {
                    // SAFETY: a plain call on our own combo box.
                    unsafe {
                        let _ = SendMessageW(
                            *handle,
                            CB_SETCURSEL,
                            WPARAM(defaults.page_size - 1),
                            LPARAM(0),
                        );
                    }
                }
                ControlKind::Check(toggle) => {
                    let value = match toggle {
                        Toggle::TabCycles => defaults.tab_cycles,
                        Toggle::TabToHost => defaults.tab_to_host,
                        Toggle::SpaceConfirms => defaults.space_confirms,
                        Toggle::AutoCompose => defaults.auto_compose,
                        Toggle::InputMode => defaults.input_mode,
                        Toggle::FloatingBar => defaults.floating_bar,
                    };
                    // SAFETY: a plain call on our own check box.
                    unsafe {
                        let _ = SendMessageW(
                            *handle,
                            BM_SETCHECK,
                            WPARAM(i32::from(value) as usize),
                            LPARAM(0),
                        );
                    }
                }
                _ => {}
            }
        }
    }
}

/// The settings window of one thread.
///
/// The window is created when the user asks for it (the settings button of the floating bar) and destroyed when it is
/// closed. It takes the focus while it is open.
pub struct SettingsWindow {
    hwnd: HWND,
    /// Kept alive: the window holds the other strong count.
    _shared: Rc<RefCell<SettingsState>>,
}

impl SettingsWindow {
    /// Open the window with the settings the user has right now. `None` when the window could not be created.
    pub fn open(settings: ImeSettings, sink: SettingsSink) -> Option<SettingsWindow> {
        let instance = module_instance()?;
        register_class(instance).ok()?;
        let layout = compute_layout();
        let (window_width, window_height) = (layout.width + 40, layout.height + 60);
        // SAFETY: a plain query; the handle is only used to give the focus back later.
        let owner = unsafe { GetForegroundWindow() };
        let shared = Rc::new(RefCell::new(SettingsState {
            settings,
            layout,
            handles: Vec::new(),
            sink,
            saved: false,
            owner,
        }));
        let raw = Rc::into_raw(Rc::clone(&shared));
        // SAFETY: the class is registered; the only pointer is the one we hand over in USERDATA.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_APPWINDOW | WS_EX_LAYERED,
                CLASS_NAME,
                w!("IME1.0 设置"),
                WS_OVERLAPPEDWINDOW | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN,
                0,
                0,
                window_width,
                window_height,
                HWND::default(),
                HMENU::default(),
                instance,
                None,
            )
        }
        .ok()?;
        // The window is made of the same material as the panels of the product: **80% opaque**, so the page below it
        // shows through a little.
        //
        // SAFETY: our own window; the call only sets how the window is composited.
        unsafe {
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), MATERIAL_ALPHA, LWA_ALPHA);
        };
        // SAFETY: `hwnd` is our live window; USERDATA is ours alone.
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize) };
        // SAFETY: plain calls on our own window.
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
            );
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
        Some(SettingsWindow {
            hwnd,
            _shared: shared,
        })
    }

    /// The window of the settings.
    pub fn handle(&self) -> HWND {
        self.hwnd
    }
}

impl Drop for SettingsWindow {
    fn drop(&mut self) {
        if !self.hwnd.is_invalid() {
            // SAFETY: destroys our own window; on another thread than the creator it fails, which only leaks a window.
            let _ = unsafe { DestroyWindow(self.hwnd) };
        }
    }
}

fn register_class(instance: HINSTANCE) -> Result<(), ()> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static CLASS_REGISTERED: AtomicBool = AtomicBool::new(false);
    if CLASS_REGISTERED.load(Ordering::Acquire) {
        return Ok(());
    }
    // SAFETY: a stock cursor id; no instance is needed for it.
    let cursor = unsafe { LoadCursorW(HINSTANCE::default(), IDC_ARROW) }.unwrap_or_default();
    let class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wnd_proc),
        hInstance: instance,
        hCursor: cursor,
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    // SAFETY: `class` is fully initialised and its strings are static.
    let atom = unsafe { RegisterClassExW(&class) };
    if atom != 0
        || unsafe { windows::Win32::Foundation::GetLastError() }
            == windows::Win32::Foundation::ERROR_CLASS_ALREADY_EXISTS
    {
        CLASS_REGISTERED.store(true, Ordering::Release);
        Ok(())
    } else {
        Err(())
    }
}

/// The shared state of `hwnd`, if it has one.
///
/// # Safety
/// Only call from `wnd_proc` of a window created by `SettingsWindow::open`.
unsafe fn shared_of<'a>(hwnd: HWND) -> Option<&'a RefCell<SettingsState>> {
    // SAFETY: a plain read of the window's USERDATA.
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const RefCell<SettingsState>;
    // SAFETY: null or the live pointer described above.
    unsafe { ptr.as_ref() }
}

/// The window procedure. Everything runs inside a panic guard: a panic that unwound out of this `extern "system"` frame
/// would abort the host process.
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    guard(None, || Some(window_message(hwnd, msg, wparam, lparam))).unwrap_or_else(|| {
        // SAFETY: forwarding the message unchanged.
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    })
}

fn window_message(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            if !on_create(hwnd) {
                return LRESULT(-1);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            on_command(hwnd, wparam);
            LRESULT(0)
        }
        WM_CLOSE => {
            // SAFETY: our own window.
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            on_destroy(hwnd);
            // SAFETY: forwarding the message unchanged.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        _ => {
            let _ = lparam;
            // SAFETY: forwarding the message unchanged.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
    }
}

/// A null-terminated UTF-16 string, ready for `CreateWindowExW`.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Create the controls of the window. Returns whether they are all there.
fn on_create(hwnd: HWND) -> bool {
    let Some(cell) = (unsafe { shared_of(hwnd) }) else {
        return false;
    };
    let Ok(mut shared) = cell.try_borrow_mut() else {
        return false;
    };
    let Some(instance) = module_instance() else {
        return false;
    };
    let layout = shared.layout.clone();
    let mut handles: Vec<HWND> = Vec::with_capacity(layout.controls.len());
    for (index, slot) in layout.controls.iter().enumerate() {
        let (class, text, style): (PCWSTR, Vec<u16>, WINDOW_STYLE) = match slot.kind {
            ControlKind::GroupTitle => (
                w!("Static"),
                wide(imesettings::display_name(
                    imesettings::LANGUAGES[slot.language.unwrap_or(0)],
                )),
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0),
            ),
            ControlKind::PageSizeLabel => (
                w!("Static"),
                wide(PAGE_SIZE_LABEL),
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0),
            ),
            ControlKind::PageSizeCombo => (
                w!("ComboBox"),
                wide(""),
                WINDOW_STYLE(CBS_DROPDOWNLIST as u32 | WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0),
            ),
            ControlKind::Check(toggle) => (
                w!("Button"),
                wide(toggle_label(toggle)),
                WINDOW_STYLE(BS_AUTOCHECKBOX as u32 | WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0),
            ),
        };
        // SAFETY: the class is a standard control; the parameters are plain values.
        let control = unsafe {
            CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WS_EX_CONTROLPARENT,
                class,
                PCWSTR(text.as_ptr()),
                style,
                slot.rect.left,
                slot.rect.top,
                slot.rect.width(),
                slot.rect.height(),
                hwnd,
                HMENU(index as *mut c_void),
                instance,
                None,
            )
        };
        let Ok(control) = control else {
            return false;
        };
        handles.push(control);
    }
    shared.handles = handles;
    // The drop-down lists get their items and their value.
    for (index, slot) in layout.controls.iter().enumerate() {
        if slot.kind != ControlKind::PageSizeCombo {
            continue;
        }
        let handle = shared.handles[index];
        for size in 1..=9 {
            let text = wide(&size.to_string());
            // SAFETY: a plain call on our own combo box; the string outlives the call.
            unsafe {
                let _ = SendMessageW(
                    handle,
                    CB_ADDSTRING,
                    WPARAM(0),
                    LPARAM(text.as_ptr() as isize),
                );
            }
        }
        let language = slot.language.unwrap_or(0);
        let value = shared.settings.get(imesettings::LANGUAGES[language]);
        // SAFETY: a plain call on our own combo box.
        unsafe {
            let _ = SendMessageW(
                handle,
                CB_SETCURSEL,
                WPARAM(value.page_size.clamp(1, 9) - 1),
                LPARAM(0),
            );
        }
    }
    // The check boxes get their value.
    for (index, slot) in layout.controls.iter().enumerate() {
        let ControlKind::Check(toggle) = slot.kind else {
            continue;
        };
        let language = slot.language.unwrap_or(0);
        let value = shared.settings.get(imesettings::LANGUAGES[language]);
        let checked = match toggle {
            Toggle::TabCycles => value.tab_cycles,
            Toggle::TabToHost => value.tab_to_host,
            Toggle::SpaceConfirms => value.space_confirms,
            Toggle::AutoCompose => value.auto_compose,
            Toggle::InputMode => value.input_mode,
            Toggle::FloatingBar => value.floating_bar,
        };
        // SAFETY: a plain call on our own check box.
        unsafe {
            let _ = SendMessageW(
                shared.handles[index],
                BM_SETCHECK,
                WPARAM(i32::from(checked) as usize),
                LPARAM(0),
            );
        }
    }
    // The buttons at the bottom.
    for (rect, kind) in layout.buttons.iter() {
        let id = match kind {
            SettingsButton::Save => ID_SAVE,
            SettingsButton::Cancel => ID_CANCEL,
            SettingsButton::Defaults => ID_DEFAULTS,
        };
        let label = match kind {
            SettingsButton::Save => "保存",
            SettingsButton::Cancel => "取消",
            SettingsButton::Defaults => "恢复默认",
        };
        let base = if matches!(kind, SettingsButton::Save) {
            BS_DEFPUSHBUTTON
        } else {
            BS_PUSHBUTTON
        };
        let text = wide(label);
        // SAFETY: the class is a standard control; the parameters are plain values.
        let button = unsafe {
            CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WS_EX_CONTROLPARENT,
                w!("Button"),
                PCWSTR(text.as_ptr()),
                WINDOW_STYLE(base as u32 | WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0),
                rect.left,
                rect.top,
                rect.width(),
                rect.height(),
                hwnd,
                HMENU(id as *mut c_void),
                instance,
                None,
            )
        };
        if button.is_err() {
            return false;
        }
    }
    true
}

/// A click on one of the buttons.
fn on_command(hwnd: HWND, wparam: WPARAM) {
    let id = wparam.0 & 0xFFFF;
    let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
    if code != BN_CLICKED {
        return;
    }
    match id {
        ID_SAVE => save_and_close(hwnd),
        ID_CANCEL => {
            // SAFETY: our own window.
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
        ID_DEFAULTS => {
            let Some(cell) = (unsafe { shared_of(hwnd) }) else {
                return;
            };
            let Ok(mut shared) = cell.try_borrow_mut() else {
                return;
            };
            shared.show_defaults();
        }
        _ => {}
    }
}

/// Write the settings and close the window. A file that cannot be written is reported in the title bar of the window
/// (there is no other place to say it): the window stays open.
fn save_and_close(hwnd: HWND) {
    let Some(cell) = (unsafe { shared_of(hwnd) }) else {
        return;
    };
    let Ok(mut shared) = cell.try_borrow_mut() else {
        return;
    };
    let settings = shared.collect();
    let Some(path) = imesettings::settings_path() else {
        set_title(hwnd, "IME1.0 设置 - 没有 LOCALAPPDATA，设置无法保存");
        return;
    };
    if let Err(e) = settings.save(&path) {
        set_title(hwnd, &format!("IME1.0 设置 - 保存失败：{e}"));
        return;
    }
    shared.settings = settings;
    shared.saved = true;
    let sink = shared.sink.clone();
    sink(SettingsEvent::Saved);
    // SAFETY: our own window.
    unsafe {
        let _ = DestroyWindow(hwnd);
    };
}

/// The window is gone: tell the text service whether the settings were saved, and give the focus back to the editor.
fn on_destroy(hwnd: HWND) {
    let Some(cell) = (unsafe { shared_of(hwnd) }) else {
        return;
    };
    let Ok(shared) = cell.try_borrow() else {
        return;
    };
    // SAFETY: the last message of this window; takes back the count given in `open`.
    unsafe {
        let ptr = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *const RefCell<SettingsState>;
        if !ptr.is_null() {
            drop(Rc::from_raw(ptr));
        }
    }
    let saved = shared.saved;
    let sink = shared.sink.clone();
    let owner = shared.owner;
    if !saved {
        sink(SettingsEvent::Cancelled);
    }
    if !owner.is_invalid() {
        // SAFETY: the window that had the focus before; it is still alive (it is the host's editor).
        unsafe {
            let _ = SetForegroundWindow(owner);
        }
    }
}

fn set_title(hwnd: HWND, text: &str) {
    let text = wide(text);
    // SAFETY: the string is a live local with its terminator.
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(text.as_ptr()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_gets_the_same_rows() {
        let layout = compute_layout();
        for (index, language) in imesettings::LANGUAGES.iter().enumerate() {
            let title = layout
                .controls
                .iter()
                .filter(|slot| slot.language == Some(index) && slot.kind == ControlKind::GroupTitle)
                .count();
            assert_eq!(title, 1, "{language}");
            let combos = layout
                .controls
                .iter()
                .filter(|slot| {
                    slot.language == Some(index) && slot.kind == ControlKind::PageSizeCombo
                })
                .count();
            assert_eq!(combos, 1, "{language}");
            let checks = layout
                .controls
                .iter()
                .filter(|slot| {
                    slot.language == Some(index) && matches!(slot.kind, ControlKind::Check(_))
                })
                .count();
            assert_eq!(checks, 6, "{language}");
        }
    }

    #[test]
    fn the_controls_do_not_overlap_each_other() {
        let layout = compute_layout();
        let rects: Vec<Rect> = layout.controls.iter().map(|slot| slot.rect).collect();
        for (i, a) in rects.iter().enumerate() {
            for b in rects.iter().skip(i + 1) {
                let overlap =
                    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom;
                assert!(!overlap, "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn the_buttons_are_at_the_bottom_and_next_to_each_other() {
        let layout = compute_layout();
        assert_eq!(layout.buttons.len(), 3);
        let kinds: Vec<SettingsButton> = layout.buttons.iter().map(|(_, kind)| *kind).collect();
        assert_eq!(
            kinds,
            vec![
                SettingsButton::Save,
                SettingsButton::Cancel,
                SettingsButton::Defaults
            ]
        );
        for (rect, _) in &layout.buttons {
            assert!(rect.bottom <= layout.height);
            assert!(rect.top > 0);
        }
        for pair in layout.buttons.windows(2) {
            assert!(pair[0].0.right <= pair[1].0.left);
        }
    }

    #[test]
    fn the_window_is_big_enough_for_its_content() {
        let layout = compute_layout();
        for slot in &layout.controls {
            assert!(slot.rect.right <= layout.width, "{slot:?}");
            assert!(slot.rect.bottom <= layout.height, "{slot:?}");
        }
        assert!(layout.height > 400, "three groups of settings need room");
    }

    #[test]
    fn every_toggle_has_a_label() {
        for toggle in [
            Toggle::TabCycles,
            Toggle::TabToHost,
            Toggle::SpaceConfirms,
            Toggle::AutoCompose,
            Toggle::InputMode,
            Toggle::FloatingBar,
        ] {
            assert!(!toggle_label(toggle).is_empty());
        }
    }
}
