//! The floating bar: the strip that follows the caret and carries the things the user needs from the mouse - the input
//! mode, the effort of the AI, the skin, the programming switches, the language and the settings window.
//!
//! The bar is a no-activate topmost tool window (`WS_EX_NOACTIVATE`, `SW_SHOWNOACTIVATE`): a click on it is handled by
//! our own window procedure and the editor keeps the focus, so the user never loses the caret to press a button. It is
//! drawn with GDI like the candidate popup, in the colours of the chosen skin, so a high-contrast theme applies.
//!
//! The geometry is pure ([`compute_layout`], [`place`], [`hit_test`]) and therefore testable without Windows; the Win32
//! part only measures the text, places the window and paints what the pure part decided.

use std::cell::RefCell;
use std::rc::Rc;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, COLOR_BTNFACE, COLOR_BTNTEXT, COLOR_GRAYTEXT, COLOR_HIGHLIGHT,
    COLOR_HIGHLIGHTTEXT, COLOR_WINDOWFRAME, EndPaint, GetDC, HFONT, PAINTSTRUCT, ReleaseDC,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW, HMENU,
    IsWindow, LoadCursorW, MA_NOACTIVATE, RegisterClassExW, SW_HIDE, SW_SHOWNOACTIVATE,
    SetWindowLongPtrW, ShowWindow, WM_ERASEBKGND, WM_LBUTTONDOWN, WM_MOUSEACTIVATE, WM_NCDESTROY,
    WM_PAINT, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use crate::bar_skin;
use crate::layout::{Rect, TextMetrics};
use crate::paint::LayeredSurface;
use crate::popup::{
    GdiMetrics, GdiObject, colour, dpi_for, message_font, module_instance, work_area_at,
};
use crate::ui::{Anchor, guard};
use pime_core::style::LaserTheme;

/// The window class is process-wide: a distinctive name, registered once per process.
const CLASS_NAME: PCWSTR = w!("ProgrammingIME.FloatingBar");

/// The product name in the title of the bar.
pub const PRODUCT: &str = "IME1.0";

/// The label of the settings button.
pub const SETTINGS_LABEL: &str = "设置";

/// The gap between the bar and the caret line, in pixels.
const CARET_GAP: i32 = 4;

/// The buttons of the bar, in the order they are drawn.
pub const BAR_BUTTONS: [BarButtonKind; 6] = [
    BarButtonKind::Mode,
    BarButtonKind::Effort,
    BarButtonKind::Skin,
    BarButtonKind::Convenience,
    BarButtonKind::Language,
    BarButtonKind::Settings,
];

/// One button of the bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarButtonKind {
    /// Switch between direct mode and input mode (the same thing Alt+F1 does).
    Mode,
    /// The effort of the AI features.
    Effort,
    /// The skin of the bar.
    Skin,
    /// The programming convenience switches.
    Convenience,
    /// The input language (a searchable menu).
    Language,
    /// The settings window.
    Settings,
}

/// The menus the bar can open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarMenu {
    Effort,
    Skin,
    Convenience,
    Language,
    /// The settings window (not a menu, but it is opened from the bar).
    Settings,
}

/// The button that opens a menu.
pub fn button_of_menu(menu: BarMenu) -> BarButtonKind {
    match menu {
        BarMenu::Effort => BarButtonKind::Effort,
        BarMenu::Skin => BarButtonKind::Skin,
        BarMenu::Convenience => BarButtonKind::Convenience,
        BarMenu::Language => BarButtonKind::Language,
        BarMenu::Settings => BarButtonKind::Settings,
    }
}

/// What the user asks the bar to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarAction {
    /// Switch between direct mode and input mode.
    ToggleInputMode,
    /// Open one of the menus.
    OpenMenu(BarMenu),
}

/// How a click reaches the text service. The sink runs on the thread that owns the bar (the host's thread), so it must
/// not block and must not unwind into the host.
pub type BarSink = Rc<dyn Fn(BarAction)>;

/// A sink that drops every action.
pub fn no_bar_sink() -> BarSink {
    Rc::new(|_| {})
}

/// The colours of the bar and of its menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkinColors {
    pub background: COLORREF,
    pub text: COLORREF,
    pub dim: COLORREF,
    pub highlight: COLORREF,
    pub highlight_text: COLORREF,
    pub frame: COLORREF,
}

/// The colours of a skin: "laser" is the look of the product, "system" follows the theme of Windows, the other two are
/// fixed.
pub fn skin_colors(skin: &str) -> SkinColors {
    match skin {
        // 0x00BBGGRR
        "laser" => SkinColors {
            background: COLORREF(LaserTheme::PANEL.to_bgr()),
            text: COLORREF(LaserTheme::TEXT.to_bgr()),
            dim: COLORREF(LaserTheme::DIM.to_bgr()),
            highlight: COLORREF(LaserTheme::ACCENT.to_bgr()),
            highlight_text: COLORREF(LaserTheme::ON_LIGHT.to_bgr()),
            frame: COLORREF(LaserTheme::RIM.to_bgr()),
        },
        "light" => SkinColors {
            background: COLORREF(0x00F7F3EC),
            text: COLORREF(0x00201A15),
            dim: COLORREF(0x00706860),
            highlight: COLORREF(0x00C8DCF0),
            highlight_text: COLORREF(0x00201A15),
            frame: COLORREF(0x00A8A196),
        },
        "dark" => SkinColors {
            background: COLORREF(0x00262320),
            text: COLORREF(0x00EDE7DF),
            dim: COLORREF(0x00A79E93),
            highlight: COLORREF(0x005A6E82),
            highlight_text: COLORREF(0x00EDE7DF),
            frame: COLORREF(0x00555048),
        },
        _ => SkinColors {
            background: colour(COLOR_BTNFACE),
            text: colour(COLOR_BTNTEXT),
            dim: colour(COLOR_GRAYTEXT),
            highlight: colour(COLOR_HIGHLIGHT),
            highlight_text: colour(COLOR_HIGHLIGHTTEXT),
            frame: colour(COLOR_WINDOWFRAME),
        },
    }
}

/// What the bar shows right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarState {
    /// The language profile that is active ("zh-CN", "en-US", "ja-JP").
    pub language: String,
    /// Whether the input method is in input mode (typing opens a composition) or in direct mode (nothing is eaten).
    pub input_mode: bool,
    /// The skin of the bar ("system", "light", "dark").
    pub skin: String,
}

impl BarState {
    /// The title of the bar: the product and the language.
    pub fn title(&self) -> String {
        format!(
            "{PRODUCT} · {}",
            crate::imesettings::display_name(&self.language)
        )
    }

    /// The state of the input method, in words.
    pub fn status(&self) -> &'static str {
        if self.input_mode {
            "输入模式"
        } else {
            "直接模式"
        }
    }

    /// The label of a button.
    pub fn label(&self, kind: BarButtonKind) -> &'static str {
        match kind {
            // "中" is the input mode, "英" the direct one: the same two characters the language bar uses.
            BarButtonKind::Mode => {
                if self.input_mode {
                    "中"
                } else {
                    "英"
                }
            }
            BarButtonKind::Effort => "AI",
            BarButtonKind::Skin => match self.skin.as_str() {
                "laser" => "激",
                "light" => "浅",
                "dark" => "深",
                _ => "随",
            },
            BarButtonKind::Convenience => "便捷",
            BarButtonKind::Language => crate::imesettings::display_name(&self.language),
            BarButtonKind::Settings => SETTINGS_LABEL,
        }
    }
}

/// The measurements the layout needs; the Win32 part measures the text, the tests invent them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BarMetrics {
    /// The height of one line of text.
    pub line_height: i32,
    pub title_width: i32,
    /// The width of every button, in the order of [`BAR_BUTTONS`].
    pub button_widths: [i32; 6],
    /// The margin around the content.
    pub padding: i32,
    /// The gap between two parts.
    pub gap: i32,
}

/// One button of the bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BarButton {
    /// The rectangle, relative to the top-left of the window.
    pub rect: Rect,
    pub kind: BarButtonKind,
}

/// The geometry of the bar, relative to the top-left of its window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarLayout {
    pub width: i32,
    pub height: i32,
    /// Where the title is drawn.
    pub title: Rect,
    /// The buttons, in the order they are drawn.
    pub buttons: Vec<BarButton>,
}

/// The geometry of the bar for a state and the measurements of its texts.
pub fn compute_layout(_state: &BarState, metrics: &BarMetrics) -> BarLayout {
    let top = metrics.padding;
    let bottom = top + metrics.line_height;
    let mut x = metrics.padding;
    let title = Rect::new(x, top, x + metrics.title_width, bottom);
    x = title.right + metrics.gap;
    let mut buttons = Vec::with_capacity(BAR_BUTTONS.len());
    for (kind, width) in BAR_BUTTONS.iter().zip(metrics.button_widths.iter()) {
        let rect = Rect::new(x, top, x + width, bottom);
        buttons.push(BarButton { rect, kind: *kind });
        x = rect.right + metrics.gap;
    }
    x -= metrics.gap; // no gap after the last button
    BarLayout {
        width: x + metrics.padding,
        height: bottom + metrics.padding,
        title,
        buttons,
    }
}

/// Where the window goes on the screen: above the caret line when there is room (the candidate popup is drawn below
/// it, so the two do not cover each other), below the caret line otherwise. The result is always inside the work area.
pub fn place(layout: &BarLayout, anchor: Anchor, work: Rect) -> Rect {
    let above = anchor.y - layout.height - CARET_GAP;
    let below = anchor.y + anchor.line_height + CARET_GAP;
    let top = if above >= work.top {
        above
    } else if below + layout.height <= work.bottom {
        below
    } else {
        work.top
    };
    let left = anchor
        .x
        .clamp(work.left, (work.right - layout.width).max(work.left));
    Rect::new(left, top, left + layout.width, top + layout.height)
}

/// The button under a client point, if any.
pub fn hit_test(layout: &BarLayout, x: i32, y: i32) -> Option<BarButtonKind> {
    layout
        .buttons
        .iter()
        .find(|button| button.rect.contains(x, y))
        .map(|button| button.kind)
}

/// The state the window procedure needs to paint and to answer a click.
struct Shared {
    state: BarState,
    layout: BarLayout,
    font: HFONT,
    skin: SkinColors,
    sink: BarSink,
}

/// The floating bar of one thread.
///
/// Threading: like the candidate popup, the window belongs to the thread that created it (the host's thread), and that
/// thread's message loop delivers its messages.
pub struct FloatingBar {
    hwnd: HWND,
    /// Shared with the window through GWLP_USERDATA (one strong count is owned by the window until WM_NCDESTROY).
    shared: Rc<RefCell<Shared>>,
    /// The message font for one DPI; owned here, selected into a DC only while painting or measuring.
    font: Option<(u32, GdiObject)>,
    /// The picture of the window (the bar plus the padding around it). It is made again when the size changes.
    surface: Option<LayeredSurface>,
    visible: bool,
}

impl FloatingBar {
    /// Creates nothing yet: the window is created on the first `show`, on the calling thread.
    pub fn new(sink: BarSink) -> FloatingBar {
        FloatingBar {
            hwnd: HWND::default(),
            shared: Rc::new(RefCell::new(Shared {
                state: BarState {
                    language: "zh-CN".to_string(),
                    input_mode: false,
                    skin: "laser".to_string(),
                },
                layout: BarLayout {
                    width: 0,
                    height: 0,
                    title: Rect::EMPTY,
                    buttons: Vec::new(),
                },
                font: HFONT::default(),
                skin: skin_colors("laser"),
                sink,
            })),
            font: None,
            surface: None,
            visible: false,
        }
    }

    /// Whether the bar is on the screen right now.
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// The rectangle of one button, in screen coordinates (the menus are placed under it). `None` when the bar is not
    /// on the screen.
    pub fn button_rect(&self, kind: BarButtonKind) -> Option<Rect> {
        if !self.visible {
            return None;
        }
        let shared = self.shared.try_borrow().ok()?;
        let button = shared.layout.buttons.iter().find(|b| b.kind == kind)?;
        // SAFETY: a plain read of the window rectangle.
        let mut r = windows::Win32::Foundation::RECT::default();
        unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowRect(self.hwnd, &mut r) }
            .ok()?;
        // The picture of the window is bigger than the bar: the padding belongs to the shadow.
        let pad = bar_skin::PADDING;
        Some(Rect::new(
            r.left + pad + button.rect.left,
            r.top + pad + button.rect.top,
            r.left + pad + button.rect.right,
            r.top + pad + button.rect.bottom,
        ))
    }

    /// Show the bar at the caret, or move it there. Returns whether it is on the screen afterwards.
    pub fn show(&mut self, state: &BarState, anchor: Anchor) -> bool {
        if self.try_show(state, anchor).is_err() {
            self.hide();
            return false;
        }
        true
    }

    /// Update what the bar shows (the language, the mode, the skin) without moving it.
    pub fn set_state(&mut self, state: &BarState) {
        {
            let Ok(mut shared) = self.shared.try_borrow_mut() else {
                return;
            };
            shared.state = state.clone();
            shared.skin = skin_colors(&state.skin);
        }
        if self.visible && !self.hwnd.is_invalid() {
            self.repaint();
        }
    }

    /// Hide it (idempotent).
    pub fn hide(&mut self) {
        if self.visible {
            self.visible = false;
            // SAFETY: plain call on our own window; a destroyed handle only makes it a no-op.
            let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
        }
    }

    fn try_show(&mut self, state: &BarState, anchor: Anchor) -> Result<(), NotShown> {
        let hwnd = self.ensure_window()?;
        let (monitor, work) = work_area_at(anchor).ok_or(NotShown)?;
        let dpi = dpi_for(monitor, hwnd);
        let font = self.font_for(dpi)?;
        let metrics = self.metrics_for(font, state)?;
        let layout = compute_layout(state, &metrics);
        let window = place(&layout, anchor, work);
        {
            let mut shared = self.shared.try_borrow_mut().map_err(|_| NotShown)?;
            shared.state = state.clone();
            shared.layout = layout.clone();
            shared.font = font;
            shared.skin = skin_colors(&state.skin);
        }
        // The picture of the window: the bar plus the padding the shadow needs.
        let pad = bar_skin::PADDING;
        self.ensure_surface(layout.width + pad * 2, layout.height + pad * 2)?;
        {
            let Some(surface) = self.surface.as_mut() else {
                return Err(NotShown);
            };
            let shared = self.shared.try_borrow().map_err(|_| NotShown)?;
            bar_skin::paint(surface, &layout, state, font, &shared.skin);
            drop(shared);
            // `UpdateLayeredWindow` puts the picture on the screen and moves the window to it.
            surface.upload(hwnd, window.left - pad, window.top - pad);
        }
        if !self.visible {
            // SAFETY: plain call on our own window; SW_SHOWNOACTIVATE shows it without activating it.
            let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
            self.visible = true;
        }
        Ok(())
    }

    /// Draw the bar again and put it on the screen (the content of a layered window is not painted by the system).
    fn repaint(&mut self) {
        if self.hwnd.is_invalid() {
            return;
        }
        let Ok(shared) = self.shared.try_borrow() else {
            return;
        };
        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        bar_skin::paint(surface, &shared.layout, &shared.state, shared.font, &shared.skin);
        // SAFETY: a plain read of the window rectangle.
        let mut r = windows::Win32::Foundation::RECT::default();
        let ok = unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowRect(self.hwnd, &mut r) }.is_ok();
        drop(shared);
        if ok {
            surface.upload(self.hwnd, r.left, r.top);
        }
    }

    /// The picture of the window, made again when the size changes.
    fn ensure_surface(&mut self, width: i32, height: i32) -> Result<(), NotShown> {
        if let Some(surface) = &self.surface
            && surface.width() == width
            && surface.height() == height
        {
            return Ok(());
        }
        self.surface = new_surface(width, height);
        match self.surface {
            Some(_) => Ok(()),
            None => Err(NotShown),
        }
    }

    fn ensure_window(&mut self) -> Result<HWND, NotShown> {
        // SAFETY: IsWindow accepts any handle value.
        if !self.hwnd.is_invalid() && unsafe { IsWindow(self.hwnd) }.as_bool() {
            return Ok(self.hwnd);
        }
        self.hwnd = HWND::default();
        self.visible = false;
        let instance = module_instance().ok_or(NotShown)?;
        register_class(instance)?;
        // SAFETY: the class is registered; no pointers are passed except the static class name.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                CLASS_NAME,
                w!(""),
                WS_POPUP,
                0,
                0,
                1,
                1,
                HWND::default(),
                HMENU::default(),
                instance,
                None,
            )
        }
        .map_err(|_| NotShown)?;
        let raw = Rc::into_raw(Rc::clone(&self.shared));
        // SAFETY: `hwnd` is our live window; USERDATA is ours alone (the class has no other use for it).
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize) };
        self.hwnd = hwnd;
        Ok(hwnd)
    }

    fn font_for(&mut self, dpi: u32) -> Result<HFONT, NotShown> {
        if let Some((current, font)) = &self.font
            && *current == dpi
        {
            return Ok(font.as_font());
        }
        let font = message_font(dpi).ok_or(NotShown)?;
        let handle = font.as_font();
        self.font = Some((dpi, font));
        Ok(handle)
    }

    fn metrics_for(&self, font: HFONT, state: &BarState) -> Result<BarMetrics, NotShown> {
        let metrics = GdiMetrics::new(font).ok_or(NotShown)?;
        let mut button_widths = [0i32; 6];
        for (slot, kind) in button_widths.iter_mut().zip(BAR_BUTTONS.iter()) {
            // room for the text and for the border of the button
            *slot = metrics.text_width(state.label(*kind)) + 14;
        }
        Ok(BarMetrics {
            line_height: metrics.line_height(),
            title_width: metrics.text_width(&state.title()),
            button_widths,
            padding: 6,
            gap: 4,
        })
    }
}

impl Drop for FloatingBar {
    fn drop(&mut self) {
        if let Ok(mut shared) = self.shared.try_borrow_mut() {
            shared.layout = BarLayout {
                width: 0,
                height: 0,
                title: Rect::EMPTY,
                buttons: Vec::new(),
            };
        }
        if !self.hwnd.is_invalid() {
            // SAFETY: destroys our own window; on another thread than the creator it fails, which only leaks a hidden window.
            let _ = unsafe { DestroyWindow(self.hwnd) };
        }
    }
}

/// Marker error for "the bar cannot be shown now"; nothing above `show` ever sees it.
struct NotShown;

fn register_class(instance: windows::Win32::Foundation::HINSTANCE) -> Result<(), NotShown> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static CLASS_REGISTERED: AtomicBool = AtomicBool::new(false);
    if CLASS_REGISTERED.load(Ordering::Acquire) {
        return Ok(());
    }
    // SAFETY: a stock cursor id; no instance is needed for it.
    let cursor = unsafe {
        LoadCursorW(
            windows::Win32::Foundation::HINSTANCE::default(),
            windows::Win32::UI::WindowsAndMessaging::IDC_ARROW,
        )
    }
    .unwrap_or_default();
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
        Err(NotShown)
    }
}

/// The shared state of `hwnd`, if it has one.
///
/// # Safety
/// Only call from `wnd_proc` of a window created by `ensure_window`: USERDATA then holds the pointer made by
/// `Rc::into_raw`, which stays valid until `WM_NCDESTROY` on this same thread, and the result must not outlive the
/// message.
unsafe fn shared_of<'a>(hwnd: HWND) -> Option<&'a RefCell<Shared>> {
    // SAFETY: a plain read of the window's USERDATA.
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const RefCell<Shared>;
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
    guard(None, || Some(window_message(hwnd, msg, wparam, lparam))).unwrap_or_else(|| match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => LRESULT(0),
        // SAFETY: forwarding the message unchanged.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    })
}

fn window_message(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // A click on the bar must not move the focus away from the host's editor.
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        // A click takes the button it is on.
        WM_LBUTTONDOWN => {
            on_click(hwnd, lparam);
            LRESULT(0)
        }
        // Everything is painted in WM_PAINT, double-buffered.
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            on_paint(hwnd);
            LRESULT(0)
        }
        WM_NCDESTROY => {
            // SAFETY: the last message of this window; takes back the count given in `ensure_window`.
            unsafe {
                let ptr = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *const RefCell<Shared>;
                if !ptr.is_null() {
                    drop(Rc::from_raw(ptr));
                }
            }
            // SAFETY: forwarding the message unchanged.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        _ => {
            let _ = wparam;
            // SAFETY: forwarding the message unchanged.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
    }
}

/// A click in the bar takes the button it is on. The borrow of the shared state is released before the sink runs,
/// because the sink writes to the document and may ask for the bar to be drawn again.
fn on_click(hwnd: HWND, lparam: LPARAM) {
    let Some(cell) = (unsafe { shared_of(hwnd) }) else {
        return;
    };
    let x = (lparam.0 & 0xFFFF) as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
    // The picture of the window is bigger than the bar: the padding around it belongs to the shadow.
    let (x, y) = (x - bar_skin::PADDING, y - bar_skin::PADDING);
    let (kind, sink) = {
        let Ok(shared) = cell.try_borrow() else {
            return;
        };
        (hit_test(&shared.layout, x, y), shared.sink.clone())
    };
    let Some(kind) = kind else {
        return;
    };
    let action = match kind {
        BarButtonKind::Mode => BarAction::ToggleInputMode,
        BarButtonKind::Effort => BarAction::OpenMenu(BarMenu::Effort),
        BarButtonKind::Skin => BarAction::OpenMenu(BarMenu::Skin),
        BarButtonKind::Convenience => BarAction::OpenMenu(BarMenu::Convenience),
        BarButtonKind::Language => BarAction::OpenMenu(BarMenu::Language),
        BarButtonKind::Settings => BarAction::OpenMenu(BarMenu::Settings),
    };
    sink(action);
}

/// Ends the paint cycle even on early return (a WM_PAINT that is not validated repeats forever).
struct PaintScope {
    hwnd: HWND,
    ps: PAINTSTRUCT,
}

impl Drop for PaintScope {
    fn drop(&mut self) {
        // SAFETY: pairs with the BeginPaint in `on_paint`.
        let _ = unsafe { EndPaint(self.hwnd, &self.ps) };
    }
}

/// End the paint cycle of the window.
///
/// The content of a layered window is not painted by the system: it is the picture that was handed to
/// `UpdateLayeredWindow`. The paint cycle still has to be ended, or the window is sent `WM_PAINT` again and again.
fn on_paint(hwnd: HWND) {
    let mut scope = PaintScope {
        hwnd,
        ps: PAINTSTRUCT::default(),
    };
    // SAFETY: `scope.ps` is a live local that BeginPaint fills in.
    let _ = unsafe { BeginPaint(hwnd, &mut scope.ps) };
}

/// A new picture of the given size (the screen DC is the reference).
fn new_surface(width: i32, height: i32) -> Option<LayeredSurface> {
    // SAFETY: a plain call that borrows the desktop DC for a moment.
    let screen = unsafe { GetDC(HWND::default()) };
    let surface = LayeredSurface::new(screen, width, height);
    // SAFETY: the DC was borrowed above and is given back here.
    unsafe { ReleaseDC(HWND::default(), screen) };
    surface
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> BarMetrics {
        BarMetrics {
            line_height: 16,
            title_width: 80,
            button_widths: [30, 30, 30, 40, 60, 40],
            padding: 6,
            gap: 4,
        }
    }

    fn state() -> BarState {
        BarState {
            language: "zh-CN".to_string(),
            input_mode: false,
            skin: "system".to_string(),
        }
    }

    #[test]
    fn the_bar_shows_the_product_the_language_and_the_mode() {
        let state = state();
        assert_eq!(state.title(), "IME1.0 · 中文");
        assert_eq!(state.status(), "直接模式");
        assert_eq!(state.label(BarButtonKind::Mode), "英");
        let input = BarState {
            input_mode: true,
            ..state.clone()
        };
        assert_eq!(input.status(), "输入模式");
        assert_eq!(input.label(BarButtonKind::Mode), "中");
    }

    #[test]
    fn every_button_has_a_label() {
        let state = state();
        for kind in BAR_BUTTONS {
            assert!(!state.label(kind).is_empty(), "{kind:?}");
        }
        assert_eq!(state.label(BarButtonKind::Language), "中文");
        assert_eq!(state.label(BarButtonKind::Settings), "设置");
        assert_eq!(state.label(BarButtonKind::Effort), "AI");
    }

    #[test]
    fn the_skin_is_shown_on_its_button() {
        let mut state = state();
        assert_eq!(state.label(BarButtonKind::Skin), "随");
        state.skin = "light".to_string();
        assert_eq!(state.label(BarButtonKind::Skin), "浅");
        state.skin = "dark".to_string();
        assert_eq!(state.label(BarButtonKind::Skin), "深");
    }

    #[test]
    fn the_layout_puts_the_parts_next_to_each_other_without_overlap() {
        let layout = compute_layout(&state(), &metrics());
        assert!(layout.width > 0 && layout.height > 0);
        assert_eq!(layout.title.left, metrics().padding);
        assert!(layout.buttons[0].rect.left >= layout.title.right);
        for pair in layout.buttons.windows(2) {
            assert!(pair[0].rect.right <= pair[1].rect.left);
        }
        assert_eq!(layout.buttons.len(), BAR_BUTTONS.len());
        assert_eq!(
            layout.width,
            layout.buttons.last().unwrap().rect.right + metrics().padding
        );
    }

    #[test]
    fn a_click_finds_the_button_under_it_and_nothing_else() {
        let layout = compute_layout(&state(), &metrics());
        let language = layout
            .buttons
            .iter()
            .find(|b| b.kind == BarButtonKind::Language)
            .unwrap();
        assert_eq!(
            hit_test(&layout, language.rect.left + 1, language.rect.top + 1),
            Some(BarButtonKind::Language)
        );
        assert_eq!(hit_test(&layout, 0, 0), None, "the margin is not a button");
    }

    #[test]
    fn the_bar_lands_above_the_caret_when_there_is_room() {
        let layout = compute_layout(&state(), &metrics());
        let work = Rect::new(0, 0, 1920, 1080);
        let anchor = Anchor {
            x: 100,
            y: 400,
            line_height: 20,
        };
        let window = place(&layout, anchor, work);
        assert!(window.bottom <= anchor.y, "the bar is above the caret line");
        assert!(window.top >= work.top);
        assert_eq!(window.left, anchor.x);
    }

    #[test]
    fn the_bar_falls_below_the_caret_when_there_is_no_room_above() {
        let layout = compute_layout(&state(), &metrics());
        let work = Rect::new(0, 0, 1920, 1080);
        let anchor = Anchor {
            x: 100,
            y: 10,
            line_height: 20,
        };
        let window = place(&layout, anchor, work);
        assert!(
            window.top >= anchor.y + anchor.line_height,
            "the bar is below the caret"
        );
        assert!(window.bottom <= work.bottom);
    }

    #[test]
    fn the_bar_stays_inside_the_work_area() {
        let layout = compute_layout(&state(), &metrics());
        let work = Rect::new(100, 100, 1200, 300);
        let anchor = Anchor {
            x: 1190,
            y: 295,
            line_height: 20,
        };
        let window = place(&layout, anchor, work);
        assert!(window.left >= work.left);
        assert!(window.right <= work.right);
        assert!(window.top >= work.top);
        assert!(window.bottom <= work.bottom);
    }

    #[test]
    fn the_skins_have_readable_colours() {
        for skin in ["system", "light", "dark"] {
            let colors = skin_colors(skin);
            assert_ne!(colors.background, colors.text, "{skin}");
            assert_ne!(colors.highlight, colors.highlight_text, "{skin}");
        }
    }
}
