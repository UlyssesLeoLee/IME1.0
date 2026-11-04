//! The native Win32 candidate window (a no-activate, topmost tool popup) implementing
//! [`crate::ui::CandidateUi`]. It must never take focus (WS_EX_NOACTIVATE, MA_NOACTIVATE, SW_SHOWNOACTIVATE) and never
//! touch the host's text.
//!
//! The window is a **layered window** (`WS_EX_LAYERED`): the list is drawn pixel by pixel into a 32-bit picture with an
//! alpha channel and handed to `UpdateLayeredWindow`, so the panel has soft corners, a shadow and a rim of the brand
//! rainbow — the laser-rainbow style of the product (see [`crate::candidate_skin`]). GDI cannot draw that: a window
//! that is painted with GDI has hard edges and no transparency.
//!
//! Threading: a [`PopupWindow`] belongs to the thread that first shows it (a window has thread affinity); that thread's
//! message loop, which the host owns, delivers its paint messages. Every failure (no monitor, no font, window creation,
//! a layout that leaves nothing to draw) ends as "not shown": `show` answers `false` so that the TIP knows nobody sees
//! the list (it then lets every key through, see `controller`). A panic must not unwind out of the window procedure
//! (an `extern "system"` frame: it would abort the host), so every message runs inside `contained`.
//!
//! The picture is made when the list is shown, not in `WM_PAINT`: the content of a layered window is not painted by the
//! system. `WM_PAINT` is still answered (the paint cycle has to be ended), but it paints nothing.
//!
//! DPI comes from the monitor under the caret.

use std::cell::RefCell;
use std::ffi::c_void;
use std::mem::size_of;
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{
    COLORREF, ERROR_CLASS_ALREADY_EXISTS, GetLastError, HANDLE, HINSTANCE, HMODULE, HWND, LPARAM,
    LRESULT, POINT, RECT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontIndirectW, DeleteDC, DeleteObject, EndPaint, GetDC, GetMonitorInfoW,
    GetSysColor, GetTextExtentPoint32W, GetTextMetricsW, HDC, HFONT, HGDIOBJ, HMONITOR,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, PAINTSTRUCT, ReleaseDC, SelectObject,
    SYS_COLOR_INDEX, TEXTMETRICW,
};
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    GetModuleHandleExW, GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::HiDpi::{
    AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, GetThreadDpiAwarenessContext,
    SystemParametersInfoForDpi,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_HWNDPARENT, GWLP_USERDATA,
    GetForegroundWindow, GetSystemMetrics, GetWindowLongPtrW, GetWindowRect,
    GetWindowThreadProcessId, HMENU, IDC_ARROW, IsWindow, LoadCursorW, MA_NOACTIVATE,
    NONCLIENTMETRICSW, RegisterClassExW, SM_CXSCREEN, SM_CYSCREEN, SPI_GETNONCLIENTMETRICS,
    SW_HIDE, SW_SHOWNOACTIVATE, SetWindowLongPtrW, SetWindowTextW, ShowWindow, WM_ERASEBKGND,
    WM_LBUTTONDOWN, WM_MOUSEACTIVATE, WM_NCDESTROY, WM_PAINT, WNDCLASSEXW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{HRESULT, PCWSTR, s, w};

use crate::candidate_skin;
use crate::layout::{self, Layout, Rect, TextMetrics};
use crate::paint::LayeredSurface;
use crate::ui::{ActionSink, Anchor, CandidateUi, CandidateView, ListAction, guard, no_sink};

/// The window class is process-wide: a distinctive name, registered once per process.
const CLASS_NAME: PCWSTR = w!("ProgrammingIME.CandidateWindow");
static CLASS_REGISTERED: AtomicBool = AtomicBool::new(false);

/// Marker error for "the popup cannot be shown now"; nothing above `show` ever sees it.
struct NotShown;

/// What the window procedure needs to repaint; written by `show`, read by `WM_PAINT`.
struct Painted {
    layout: Option<Layout>,
    selected: usize,
    font: HFONT,
    /// How a click in the window reaches the text service (the same channel a host that draws the list uses).
    sink: ActionSink,
}

pub struct PopupWindow {
    hwnd: HWND,
    /// Shared with the window through GWLP_USERDATA (one strong count is owned by the window until WM_NCDESTROY).
    painted: Rc<RefCell<Painted>>,
    /// The message font for one DPI; owned here, selected into a DC only while painting or measuring.
    font: Option<(u32, GdiObject)>,
    /// The picture of the window (the list plus the padding around it). It is made again when the size changes.
    surface: Option<LayeredSurface>,
    accessible_name: String,
    visible: bool,
}

impl Default for PopupWindow {
    fn default() -> PopupWindow {
        PopupWindow::new()
    }
}

impl PopupWindow {
    /// Creates nothing yet: the window is created on the first `show`, on the calling thread. Clicks are dropped.
    pub fn new() -> PopupWindow {
        PopupWindow::with_sink(no_sink())
    }

    /// The same window, but a click on a row is reported to the text service through `sink`.
    pub fn with_sink(sink: ActionSink) -> PopupWindow {
        PopupWindow {
            hwnd: HWND::default(),
            painted: Rc::new(RefCell::new(Painted {
                layout: None,
                selected: 0,
                font: HFONT::default(),
                sink,
            })),
            font: None,
            surface: None,
            accessible_name: String::new(),
            visible: false,
        }
    }

    /// The window rectangle in screen coordinates while it is shown (for tests and the demo).
    pub fn window_rect(&self) -> Option<Rect> {
        if !self.visible {
            return None;
        }
        let mut r = RECT::default();
        // SAFETY: `r` is a live local; an invalid handle only makes the call fail.
        unsafe { GetWindowRect(self.hwnd, &mut r) }.ok()?;
        Some(Rect::new(r.left, r.top, r.right, r.bottom))
    }

    fn try_show(&mut self, view: &CandidateView, anchor: Anchor) -> Result<(), NotShown> {
        let hwnd = self.ensure_window()?;
        // The host application may have changed since the window was created (the user switched windows): the owner
        // is refreshed every time the list appears.
        set_owner(hwnd);
        let (monitor, work) = work_area_at(anchor).ok_or(NotShown)?;
        let dpi = dpi_for(monitor, hwnd);
        let font = self.font_for(dpi)?;
        let layout = {
            let metrics = GdiMetrics::new(font).ok_or(NotShown)?;
            layout::compute_scaled(view, anchor, work, &metrics, dpi as f32 / 96.0)
        };
        if !layout.is_visible() {
            return Err(NotShown); // `show` hides the window and reports "not visible"
        }
        let window = layout.window;
        // The borrow ends before any window call: the window calls re-enter `wnd_proc` synchronously.
        {
            let mut painted = self.painted.try_borrow_mut().map_err(|_| NotShown)?;
            painted.layout = Some(layout.clone());
            painted.selected = view.selected_in_page;
            painted.font = font;
        }
        let name = layout::accessible_name(view);
        if name != self.accessible_name {
            set_name(hwnd, &name);
            self.accessible_name = name;
        }
        // The picture of the window: the list plus the padding the shadow and the soft corners need.
        let pad = candidate_skin::PADDING;
        self.ensure_surface(window.width() + pad * 2, window.height() + pad * 2)?;
        {
            let Some(surface) = self.surface.as_mut() else {
                return Err(NotShown);
            };
            candidate_skin::paint(surface, &layout, view.selected_in_page, font);
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

    /// The window of this popup, created on first use (or again if the host destroyed it).
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
        // The window owns one strong count; `wnd_proc` gives it back on WM_NCDESTROY.
        let raw = Rc::into_raw(Rc::clone(&self.painted));
        // SAFETY: `hwnd` is our live window; USERDATA is ours alone (the class has no other use for it).
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize) };
        set_owner(hwnd);
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
}

impl CandidateUi for PopupWindow {
    fn show(&mut self, view: &CandidateView, anchor: Anchor) -> bool {
        if self.try_show(view, anchor).is_err() {
            self.hide();
            return false;
        }
        true
    }

    fn hide(&mut self) {
        if self.visible {
            self.visible = false;
            // SAFETY: plain call on our own window; a destroyed handle only makes it a no-op.
            let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
        }
    }

    fn is_visible(&self) -> bool {
        self.visible
    }
}

impl Drop for PopupWindow {
    fn drop(&mut self) {
        // From here on a stray WM_PAINT has nothing to draw (and the font below is about to go away).
        if let Ok(mut painted) = self.painted.try_borrow_mut() {
            painted.layout = None;
        }
        if !self.hwnd.is_invalid() {
            // SAFETY: destroys our own window; on another thread than the creator it fails, which only leaks a hidden window.
            let _ = unsafe { DestroyWindow(self.hwnd) };
        }
    }
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

/// The window name is what the default accessibility proxy reports for the popup.
fn set_name(hwnd: HWND, name: &str) {
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call.
    let _ = unsafe { SetWindowTextW(hwnd, PCWSTR(wide.as_ptr())) };
}

// ----- module, class and window procedure -----

/// The module that contains this code: the DLL when loaded into a host, the executable in the demo. Never
/// `GetModuleHandle(NULL)`, which would be the host application.
pub(crate) fn module_instance() -> Option<HINSTANCE> {
    let proc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT = wnd_proc;
    let mut module = HMODULE::default();
    // SAFETY: with FROM_ADDRESS the "name" is an address inside the module (our window procedure); the reference count is
    // left unchanged, so no FreeLibrary is owed.
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(proc as usize as *const u16),
            &mut module,
        )
    }
    .ok()?;
    Some(HINSTANCE(module.0))
}

fn register_class(instance: HINSTANCE) -> Result<(), NotShown> {
    if CLASS_REGISTERED.load(Ordering::Acquire) {
        return Ok(());
    }
    // SAFETY: a stock cursor id; no instance is needed for it.
    let cursor = unsafe { LoadCursorW(HINSTANCE::default(), IDC_ARROW) }.unwrap_or_default();
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wnd_proc),
        hInstance: instance,
        hCursor: cursor,
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    // SAFETY: `class` is fully initialised and its strings are static.
    let atom = unsafe { RegisterClassExW(&class) };
    // Registration is process-wide: another thread (or an earlier load of this code) may have got there first.
    // SAFETY: reads the calling thread's last error right after the failed call.
    if atom != 0 || unsafe { GetLastError() } == ERROR_CLASS_ALREADY_EXISTS {
        CLASS_REGISTERED.store(true, Ordering::Release);
        Ok(())
    } else {
        Err(NotShown)
    }
}

/// The paint state of `hwnd`, if it has one.
///
/// # Safety
/// Only call from `wnd_proc` of a window created by `ensure_window`: USERDATA then holds the pointer made by
/// `Rc::into_raw`, which stays valid until `WM_NCDESTROY` on this same thread, and the result must not outlive the
/// message.
unsafe fn painted_of<'a>(hwnd: HWND) -> Option<&'a RefCell<Painted>> {
    // SAFETY: a plain read of the window's USERDATA.
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const RefCell<Painted>;
    // SAFETY: null or the live pointer described above.
    unsafe { ptr.as_ref() }
}

/// The window procedure. Everything runs inside [`contained`]: a panic that unwound out of this `extern "system"` frame
/// would abort the host process (this is the one entry point of the DLL that Windows calls without a COM guard).
/// `on_paint` is only ever reached from here, so it is covered too.
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    contained(
        msg,
        || window_message(hwnd, msg, wparam, lparam),
        // SAFETY: forwarding the message unchanged.
        || unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    )
}

/// What a message answers when its handler panicked. Never "activate" (the popup must not take the focus) and never an
/// unpainted `WM_PAINT` (`PaintScope` has already ended the paint cycle while unwinding); everything else gets the
/// default processing.
fn answer_after_panic(msg: u32, default: impl FnOnce() -> LRESULT) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => LRESULT(0),
        _ => default(),
    }
}

/// Run the handler of one window message; a panic becomes [`answer_after_panic`]. A pure wrapper, so that the
/// containment is tested without a window.
fn contained(
    msg: u32,
    handle: impl FnOnce() -> LRESULT,
    default: impl FnOnce() -> LRESULT,
) -> LRESULT {
    guard(None, || Some(handle())).unwrap_or_else(|| answer_after_panic(msg, default))
}

fn window_message(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // A click on the popup must not move the focus away from the host's editor.
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        // A click takes the row it is on: the same as pressing the digit of that row.
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
                let ptr = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *const RefCell<Painted>;
                if !ptr.is_null() {
                    drop(Rc::from_raw(ptr));
                }
            }
            // SAFETY: forwarding the message unchanged.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        // SAFETY: forwarding the message unchanged.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
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

/// A click in the candidate window takes the row it is on. The row is found under the client point of the message; the
/// borrow of the painted state is released before the sink runs, because the sink writes to the document and may ask
/// for the window to be drawn again.
fn on_click(hwnd: HWND, lparam: LPARAM) {
    let Some(cell) = (unsafe { painted_of(hwnd) }) else {
        return;
    };
    let x = (lparam.0 & 0xFFFF) as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
    // The picture of the window is bigger than the list: the padding around it belongs to the shadow.
    let (x, y) = (x - candidate_skin::PADDING, y - candidate_skin::PADDING);
    let (row, sink) = {
        let Ok(painted) = cell.try_borrow() else {
            return;
        };
        let Some(layout) = painted.layout.as_ref() else {
            return;
        };
        let row = layout.rows.iter().position(|r| r.contains(x, y));
        (row, painted.sink.clone())
    };
    if let Some(row) = row {
        sink(ListAction::Take(row));
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

// ----- GDI helpers (small RAII wrappers so every handle is released on every path) -----

/// A GDI object (font, bitmap) deleted on drop.
pub(crate) struct GdiObject(pub(crate) HGDIOBJ);

impl GdiObject {
    pub(crate) fn as_font(&self) -> HFONT {
        HFONT(self.0.0)
    }
}

impl Drop for GdiObject {
    fn drop(&mut self) {
        // SAFETY: the object is ours and no longer selected into any DC (see `Selection`).
        let _ = unsafe { DeleteObject(self.0) };
    }
}

/// A DC from `GetDC(NULL)` (the screen), released on drop.
struct ScreenDc(HDC);

impl ScreenDc {
    fn new() -> Option<ScreenDc> {
        // SAFETY: a null window means the screen DC.
        let hdc = unsafe { GetDC(HWND::default()) };
        (!hdc.is_invalid()).then_some(ScreenDc(hdc))
    }
}

impl Drop for ScreenDc {
    fn drop(&mut self) {
        // SAFETY: pairs with `GetDC(NULL)`.
        let _ = unsafe { ReleaseDC(HWND::default(), self.0) };
    }
}

/// A memory DC, deleted on drop.
pub(crate) struct MemDc(pub(crate) HDC);

impl Drop for MemDc {
    fn drop(&mut self) {
        // SAFETY: created by CreateCompatibleDC, with nothing selected into it any more (see `Selection`).
        let _ = unsafe { DeleteDC(self.0) };
    }
}

/// An object selected into a DC; the previous object is selected back on drop.
pub(crate) struct Selection {
    hdc: HDC,
    previous: HGDIOBJ,
}

impl Selection {
    pub(crate) fn new(hdc: HDC, object: HGDIOBJ) -> Selection {
        // SAFETY: `hdc` and `object` are live handles owned by the caller for longer than this guard.
        let previous = unsafe { SelectObject(hdc, object) };
        Selection { hdc, previous }
    }
}

impl Drop for Selection {
    fn drop(&mut self) {
        // SAFETY: restores what was selected before.
        let _ = unsafe { SelectObject(self.hdc, self.previous) };
    }
}

/// Text measurement with the message font on the screen DC; implements the layout's [`TextMetrics`].
pub(crate) struct GdiMetrics {
    // field order = drop order: restore the previous font before the DC is released
    _font: Selection,
    dc: ScreenDc,
    line_height: i32,
}

impl GdiMetrics {
    pub(crate) fn new(font: HFONT) -> Option<GdiMetrics> {
        let dc = ScreenDc::new()?;
        let selection = Selection::new(dc.0, HGDIOBJ(font.0));
        let mut tm = TEXTMETRICW::default();
        // SAFETY: `tm` is a live local.
        unsafe { GetTextMetricsW(dc.0, &mut tm) }
            .as_bool()
            .then_some(())?;
        Some(GdiMetrics {
            _font: selection,
            dc,
            line_height: tm.tmHeight,
        })
    }
}

impl TextMetrics for GdiMetrics {
    fn text_width(&self, text: &str) -> i32 {
        let wide: Vec<u16> = text.encode_utf16().collect();
        let mut size = SIZE::default();
        // SAFETY: `wide` and `size` are live locals.
        let ok = unsafe { GetTextExtentPoint32W(self.dc.0, &wide, &mut size) }.as_bool();
        if ok { size.cx } else { 0 }
    }

    fn line_height(&self) -> i32 {
        self.line_height
    }
}

/// The system message font (what menus and message boxes use), already scaled for `dpi`.
pub(crate) fn message_font(dpi: u32) -> Option<GdiObject> {
    let mut metrics = NONCLIENTMETRICSW {
        cbSize: size_of::<NONCLIENTMETRICSW>() as u32,
        ..Default::default()
    };
    // SAFETY: `metrics` is a live local of the size announced in `cbSize`.
    unsafe {
        SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS.0,
            metrics.cbSize,
            Some(&mut metrics as *mut NONCLIENTMETRICSW as *mut c_void),
            0,
            dpi,
        )
    }
    .ok()?;
    // SAFETY: `lfMessageFont` was just filled in by the system.
    let font = unsafe { CreateFontIndirectW(&metrics.lfMessageFont) };
    (!font.is_invalid()).then_some(GdiObject(HGDIOBJ(font.0)))
}

pub(crate) fn colour(index: SYS_COLOR_INDEX) -> COLORREF {
    // SAFETY: a plain query.
    COLORREF(unsafe { GetSysColor(index) })
}

pub(crate) fn to_rect(r: Rect) -> RECT {
    RECT {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    }
}

// ----- monitor and DPI -----

/// The monitor nearest to the caret and its work area (the desktop minus taskbars), in screen coordinates.
/// The work area of the monitor that holds the rectangle (the screen minus the taskbars), in screen coordinates.
pub(crate) fn work_area_of(rect: Rect) -> Rect {
    let anchor = Anchor {
        x: rect.left,
        y: rect.top,
        line_height: 0,
    };
    if let Some((_, work)) = work_area_at(anchor) {
        return work;
    }
    // No monitor (should not happen): the whole screen.
    // SAFETY: plain queries.
    let width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    Rect::new(0, 0, width, height)
}

pub(crate) fn work_area_at(anchor: Anchor) -> Option<(HMONITOR, Rect)> {
    // SAFETY: a plain query; DEFAULTTONEAREST always yields a monitor.
    let monitor = unsafe {
        MonitorFromPoint(
            POINT {
                x: anchor.x,
                y: anchor.y,
            },
            MONITOR_DEFAULTTONEAREST,
        )
    };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is a live local with `cbSize` set.
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return None;
    }
    let r = info.rcWork;
    Some((monitor, Rect::new(r.left, r.top, r.right, r.bottom)))
}

type GetDpiForMonitorFn = unsafe extern "system" fn(HMONITOR, i32, *mut u32, *mut u32) -> HRESULT;

/// `shcore!GetDpiForMonitor`, loaded on demand: linking it statically would add a SHCORE.dll import to the text
/// service, and the DLL imports only the long-standing system libraries.
fn monitor_dpi_fn() -> Option<GetDpiForMonitorFn> {
    static FUNCTION: OnceLock<Option<GetDpiForMonitorFn>> = OnceLock::new();
    *FUNCTION.get_or_init(|| {
        // SAFETY: loads a system DLL from System32 only; it is never freed, so the pointer stays valid.
        let module = unsafe {
            LoadLibraryExW(
                w!("shcore.dll"),
                HANDLE::default(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        }
        .ok()?;
        // SAFETY: `module` is the loaded shcore.
        let proc = unsafe { GetProcAddress(module, s!("GetDpiForMonitor")) }?;
        // SAFETY: GetDpiForMonitor has exactly this signature (shellscalingapi.h).
        Some(unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, GetDpiForMonitorFn>(proc)
        })
    })
}

/// Effective DPI of the candidate window.
///
/// The window lives in the coordinate space of the host application. When that application is per-monitor aware, the
/// caret rectangle is in physical pixels of the monitor under the caret and the window inherits that monitor's
/// DPI. When it is only system-DPI aware, the rectangle (and our window) are in system-scaled coordinates and the
/// window's own DPI is the system DPI; scaling by the raw monitor DPI would then draw the list at the wrong size.
///
/// The text service must not change the awareness of the host process: `SetProcessDpiAwarenessContext` decides how
/// every window of the application is rendered, and that is the application's business, not the input method's. So
/// the window adapts to the awareness the application already has.
pub(crate) fn dpi_for(monitor: HMONITOR, hwnd: HWND) -> u32 {
    let window_dpi = match unsafe { GetDpiForWindow(hwnd) } {
        0 => 96,
        dpi => dpi,
    };
    let monitor_dpi = monitor_dpi(monitor);
    choose_dpi(window_dpi, monitor_dpi, per_monitor_aware())
}

/// The DPI of `monitor` (what the user chose in Settings), if it can be read.
fn monitor_dpi(monitor: HMONITOR) -> Option<u32> {
    let get_dpi = monitor_dpi_fn()?;
    let (mut x, mut y) = (0u32, 0u32);
    // MDT_EFFECTIVE_DPI = 0
    // SAFETY: both out-pointers are live locals.
    if unsafe { get_dpi(monitor, 0, &mut x, &mut y) }.is_ok() && x > 0 {
        Some(x)
    } else {
        None
    }
}

/// The DPI to draw at, separated from the queries so that the decision itself can be tested.
///
/// A per-monitor aware process puts the candidate list on the monitor under the caret, so the list is drawn at that
/// monitor's DPI. A system-DPI aware process keeps every coordinate in the system scale, so the list is drawn at the
/// window's own DPI - the raw monitor DPI would make it the wrong size.
fn choose_dpi(window_dpi: u32, monitor_dpi: Option<u32>, per_monitor: bool) -> u32 {
    if per_monitor
        && let Some(dpi) = monitor_dpi
        && dpi > 0
    {
        return dpi;
    }
    if window_dpi > 0 { window_dpi } else { 96 }
}

/// Whether this process is per-monitor aware (either generation).
fn per_monitor_aware() -> bool {
    // SAFETY: a plain query about the calling thread; it never fails.
    let context = unsafe { GetThreadDpiAwarenessContext() };
    // SAFETY: comparing two awareness contexts is a plain call with two live values.
    unsafe { AreDpiAwarenessContextsEqual(context, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE) }
        .as_bool()
        // SAFETY: the same, with the second generation of per-monitor awareness.
        || unsafe {
            AreDpiAwarenessContextsEqual(context, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
        }
        .as_bool()
}

/// The window that may own the candidate list: the foreground window of our own application.
///
/// Only a window of our own process may be the owner. An owner from another application would place our window
/// above a foreign window and would hide the candidate list when that application closes.
fn owner_window() -> Option<HWND> {
    // SAFETY: a plain query; an invalid handle yields a null window.
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_invalid() {
        return None;
    }
    let mut process = 0u32;
    // SAFETY: `process` is a live local the call only writes to.
    unsafe { GetWindowThreadProcessId(foreground, Some(&mut process)) };
    // SAFETY: a plain query about the calling process.
    if process != unsafe { GetCurrentProcessId() } {
        return None;
    }
    Some(foreground)
}

/// Make `hwnd` owned by the host application, so the candidate list disappears with it and never floats over another
/// application. A window without an owner stays on the screen until it is hidden, which is how an orphan popup
/// survives the window it belonged to.
fn set_owner(hwnd: HWND) {
    let Some(owner) = owner_window() else {
        return;
    };
    if owner == hwnd {
        return;
    }
    // SAFETY: `hwnd` is our own window; GWLP_HWNDPARENT of a top-level window is its owner.
    unsafe { SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, owner.0 as isize) };
}

// Only the pure parts are tested: nothing here creates a window (the real popup is verified by `examples/popup_demo.rs`).
#[cfg(test)]
mod tests {
    use super::*;

    fn panics() -> LRESULT {
        panic!("message handler panicked (test)")
    }

    fn default_must_not_run() -> LRESULT {
        panic!("the default processing must not run for this message (test)")
    }

    #[test]
    fn a_panic_in_a_message_handler_becomes_that_messages_safe_answer() {
        // review finding 9: wnd_proc is an `extern "system"` frame; a panic unwinding out of it aborts the host
        assert_eq!(
            contained(WM_MOUSEACTIVATE, panics, default_must_not_run),
            LRESULT(MA_NOACTIVATE as isize),
            "a panic must never let a click activate the popup"
        );
        assert_eq!(
            contained(WM_ERASEBKGND, panics, default_must_not_run),
            LRESULT(1)
        );
        assert_eq!(
            contained(WM_PAINT, panics, default_must_not_run),
            LRESULT(0)
        );
        assert_eq!(
            contained(WM_NCDESTROY, panics, || LRESULT(77)),
            LRESULT(77),
            "every other message gets the default processing"
        );
    }

    #[test]
    fn a_handler_that_does_not_panic_is_passed_through_and_the_default_stays_unused() {
        assert_eq!(
            contained(WM_PAINT, || LRESULT(5), default_must_not_run),
            LRESULT(5)
        );
        assert_eq!(
            contained(WM_NCDESTROY, || LRESULT(6), default_must_not_run),
            LRESULT(6)
        );
        // the wrapper is reusable after a panic
        assert_eq!(contained(WM_PAINT, panics, || LRESULT(1)), LRESULT(0));
        assert_eq!(
            contained(WM_PAINT, || LRESULT(9), || LRESULT(1)),
            LRESULT(9)
        );
    }

    /// The real procedure, called without a window: the messages it answers itself never reach `DefWindowProc`.
    #[test]
    fn the_window_procedure_answers_its_own_messages_without_a_window() {
        let none = HWND::default();
        // SAFETY: these messages are answered without touching the (null) window.
        unsafe {
            assert_eq!(
                wnd_proc(none, WM_MOUSEACTIVATE, WPARAM(0), LPARAM(0)),
                LRESULT(MA_NOACTIVATE as isize)
            );
            assert_eq!(
                wnd_proc(none, WM_ERASEBKGND, WPARAM(0), LPARAM(0)),
                LRESULT(1)
            );
        }
    }

    /// The DPI the candidate list is drawn at.
    ///
    /// A per-monitor aware process draws the list at the DPI of the monitor under the caret; a system-DPI aware one
    /// keeps every coordinate in the system scale and has to draw at the window's own DPI.
    #[test]
    fn the_candidate_list_is_drawn_at_the_dpi_of_the_coordinate_space_it_lives_in() {
        // per-monitor aware: the monitor under the caret decides
        assert_eq!(choose_dpi(96, Some(144), true), 144);
        // system-DPI aware: the window's own DPI decides, the raw monitor DPI would be the wrong size
        assert_eq!(choose_dpi(96, Some(144), false), 96);
        // an unreadable monitor falls back to the window
        assert_eq!(choose_dpi(120, None, true), 120);
        // a monitor that reports 0 is no answer
        assert_eq!(choose_dpi(120, Some(0), true), 120);
        // no window and no monitor: the system default
        assert_eq!(choose_dpi(0, None, true), 96);
        assert_eq!(choose_dpi(0, Some(144), false), 96);
    }

    /// The owner of the candidate list: only a window of our own application may own it.
    ///
    /// The test process has no window of its own in the foreground, so there is no owner - and the function must
    /// say so instead of pointing at a foreign window.
    #[test]
    fn the_owner_of_the_candidate_list_is_never_a_foreign_window() {
        if let Some(owner) = owner_window() {
            let mut process = 0u32;
            // SAFETY: `process` is a live local the call only writes to.
            unsafe { GetWindowThreadProcessId(owner, Some(&mut process)) };
            assert_eq!(
                process,
                unsafe { GetCurrentProcessId() },
                "only a window of our own process may own the candidate list"
            );
        }
    }
}
