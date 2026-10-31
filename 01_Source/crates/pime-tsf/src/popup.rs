//! The native Win32 candidate window (a no-activate, topmost tool popup drawn with GDI) implementing
//! [`crate::ui::CandidateUi`]. It must never take focus (WS_EX_NOACTIVATE, MA_NOACTIVATE, SW_SHOWNOACTIVATE) and never
//! touch the host's text.
//!
//! Threading: a [`PopupWindow`] belongs to the thread that first shows it (a window has thread affinity); that thread's
//! message loop, which the host owns, delivers its paint messages. Every failure (no monitor, no font, window creation,
//! a layout that leaves nothing to draw) ends as "not shown": `show` answers `false` so that the TIP knows nobody sees
//! the list (it then lets every key through, see `controller`). A panic must not unwind out of the window procedure
//! (an `extern "system"` frame: it would abort the host), so every message runs inside `contained`.
//!
//! Colours are system colours (`GetSysColor`), so high-contrast themes apply; the selected row is additionally marked
//! by a text marker (see `layout::draw_ops`). DPI comes from the monitor under the caret.
//!
//! Verified only with `examples/popup_demo.rs` on one monitor at 100 %: DPI scaling, several monitors, high-contrast
//! themes and hosting inside real applications have not been run (the placement and DPI arithmetic is unit-tested in
//! `layout`).

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
    BeginPaint, BitBlt, COLOR_3DSHADOW, COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT,
    COLOR_WINDOW, COLOR_WINDOWFRAME, COLOR_WINDOWTEXT, CreateCompatibleBitmap, CreateCompatibleDC,
    CreateFontIndirectW, DeleteDC, DeleteObject, ETO_CLIPPED, EndPaint, ExtTextOutW, FillRect,
    GetDC, GetMonitorInfoW, GetSysColor, GetSysColorBrush, GetTextExtentPoint32W, GetTextMetricsW,
    HBRUSH, HDC, HFONT, HGDIOBJ, HMONITOR, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint, PAINTSTRUCT, ReleaseDC, SRCCOPY, SYS_COLOR_INDEX, SelectObject, SetBkMode,
    SetTextColor, TEXTMETRICW, TRANSPARENT, UpdateWindow,
};
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    GetModuleHandleExW, GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, SystemParametersInfoForDpi};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW,
    GetWindowRect, HMENU, HWND_TOPMOST, IDC_ARROW, IsWindow, LoadCursorW, MA_NOACTIVATE,
    NONCLIENTMETRICSW, RegisterClassExW, SPI_GETNONCLIENTMETRICS, SW_HIDE, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SWP_NOOWNERZORDER, SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow,
    WM_ERASEBKGND, WM_MOUSEACTIVATE, WM_NCDESTROY, WM_PAINT, WNDCLASSEXW, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{HRESULT, PCWSTR, s, w};

use crate::layout::{self, DrawOp, FillTone, Layout, Rect, TextMetrics, TextTone};
use crate::ui::{Anchor, CandidateUi, CandidateView, guard};

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
}

pub struct PopupWindow {
    hwnd: HWND,
    /// Shared with the window through GWLP_USERDATA (one strong count is owned by the window until WM_NCDESTROY).
    painted: Rc<RefCell<Painted>>,
    /// The message font for one DPI; owned here, selected into a DC only while painting or measuring.
    font: Option<(u32, GdiObject)>,
    accessible_name: String,
    visible: bool,
}

impl Default for PopupWindow {
    fn default() -> PopupWindow {
        PopupWindow::new()
    }
}

impl PopupWindow {
    /// Creates nothing yet: the window is created on the first `show`, on the calling thread.
    pub fn new() -> PopupWindow {
        PopupWindow {
            hwnd: HWND::default(),
            painted: Rc::new(RefCell::new(Painted {
                layout: None,
                selected: 0,
                font: HFONT::default(),
            })),
            font: None,
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
        // The borrow ends before any window call: SetWindowPos/UpdateWindow re-enter `wnd_proc` synchronously.
        {
            let mut painted = self.painted.try_borrow_mut().map_err(|_| NotShown)?;
            painted.layout = Some(layout);
            painted.selected = view.selected_in_page;
            painted.font = font;
        }
        set_pos(hwnd, window)?;
        let name = layout::accessible_name(view);
        if name != self.accessible_name {
            set_name(hwnd, &name);
            self.accessible_name = name;
        }
        if !self.visible {
            // SAFETY: plain call on our own window; SW_SHOWNOACTIVATE shows it without activating it.
            let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
            self.visible = true;
        }
        // SAFETY: plain calls on our own window.
        unsafe {
            let _ = InvalidateRect(hwnd, None, false);
            let _ = UpdateWindow(hwnd);
        }
        Ok(())
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
                WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
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

fn set_pos(hwnd: HWND, window: Rect) -> Result<(), NotShown> {
    // SAFETY: plain call on our own window; no pointers.
    unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            window.left,
            window.top,
            window.width(),
            window.height(),
            SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        )
    }
    .map_err(|_| NotShown)
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
fn module_instance() -> Option<HINSTANCE> {
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

fn on_paint(hwnd: HWND) {
    let mut scope = PaintScope {
        hwnd,
        ps: PAINTSTRUCT::default(),
    };
    // SAFETY: `scope.ps` is a live local that BeginPaint fills in.
    let hdc = unsafe { BeginPaint(hwnd, &mut scope.ps) };
    if hdc.is_invalid() {
        return;
    }
    // SAFETY: called from `wnd_proc`, the reference is dropped before returning.
    let Some(cell) = (unsafe { painted_of(hwnd) }) else {
        return;
    };
    // `show` never holds the borrow across a window call, so this only fails if something is badly wrong: draw nothing.
    let Ok(painted) = cell.try_borrow() else {
        return;
    };
    let Some(layout) = painted.layout.as_ref() else {
        return;
    };
    draw(hdc, layout, painted.selected, painted.font);
}

// ----- GDI helpers (small RAII wrappers so every handle is released on every path) -----

/// A GDI object (font, bitmap) deleted on drop.
struct GdiObject(HGDIOBJ);

impl GdiObject {
    fn as_font(&self) -> HFONT {
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
struct MemDc(HDC);

impl Drop for MemDc {
    fn drop(&mut self) {
        // SAFETY: created by CreateCompatibleDC, with nothing selected into it any more (see `Selection`).
        let _ = unsafe { DeleteDC(self.0) };
    }
}

/// An object selected into a DC; the previous object is selected back on drop.
struct Selection {
    hdc: HDC,
    previous: HGDIOBJ,
}

impl Selection {
    fn new(hdc: HDC, object: HGDIOBJ) -> Selection {
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
struct GdiMetrics {
    // field order = drop order: restore the previous font before the DC is released
    _font: Selection,
    dc: ScreenDc,
    line_height: i32,
}

impl GdiMetrics {
    fn new(font: HFONT) -> Option<GdiMetrics> {
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
fn message_font(dpi: u32) -> Option<GdiObject> {
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

fn colour(index: SYS_COLOR_INDEX) -> COLORREF {
    // SAFETY: a plain query.
    COLORREF(unsafe { GetSysColor(index) })
}

fn brush(index: SYS_COLOR_INDEX) -> HBRUSH {
    // SAFETY: returns a shared system brush that must not be deleted (we do not).
    unsafe { GetSysColorBrush(index) }
}

fn to_rect(r: Rect) -> RECT {
    RECT {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    }
}

/// Paints `layout` into `target` through an off-screen bitmap (no flicker).
fn draw(target: HDC, layout: &Layout, selected: usize, font: HFONT) {
    let (w, h) = (layout.window.width(), layout.window.height());
    if w <= 0 || h <= 0 {
        return;
    }
    // SAFETY: `target` is the live paint DC.
    let mem = MemDc(unsafe { CreateCompatibleDC(target) });
    if mem.0.is_invalid() {
        return;
    }
    // SAFETY: `target` is the live paint DC.
    let bitmap = unsafe { CreateCompatibleBitmap(target, w, h) };
    if bitmap.is_invalid() {
        return;
    }
    // Declared in the order of release: the selections go back first, then the bitmap and the DC are deleted.
    let bitmap = GdiObject(HGDIOBJ(bitmap.0));
    let _bitmap_selected = Selection::new(mem.0, bitmap.0);
    let _font_selected = Selection::new(mem.0, HGDIOBJ(font.0));
    // SAFETY: plain state change on our memory DC.
    let _ = unsafe { SetBkMode(mem.0, TRANSPARENT) };
    for op in layout::draw_ops(layout, selected) {
        draw_op(mem.0, &op);
    }
    // SAFETY: both DCs are live and the bitmap covers `w` x `h`.
    let _ = unsafe { BitBlt(target, 0, 0, w, h, mem.0, 0, 0, SRCCOPY) };
}

fn draw_op(hdc: HDC, op: &DrawOp) {
    match op {
        DrawOp::Fill { rect, tone } => {
            let index = match tone {
                FillTone::Window => COLOR_WINDOW,
                FillTone::Frame => COLOR_WINDOWFRAME,
                FillTone::Selection => COLOR_HIGHLIGHT,
                FillTone::Separator => COLOR_3DSHADOW,
            };
            // SAFETY: `hdc` is live and the RECT is a local.
            let _ = unsafe { FillRect(hdc, &to_rect(*rect), brush(index)) };
        }
        DrawOp::Text {
            text,
            x,
            y,
            clip,
            tone,
        } => {
            let index = match tone {
                TextTone::Normal => COLOR_WINDOWTEXT,
                TextTone::Dim => COLOR_GRAYTEXT,
                TextTone::OnSelection => COLOR_HIGHLIGHTTEXT,
            };
            let wide: Vec<u16> = text.encode_utf16().collect();
            let Ok(count) = u32::try_from(wide.len()) else {
                return;
            };
            // SAFETY: `hdc` is live; `wide` holds `count` UTF-16 units and outlives the call; the clip RECT is a local.
            unsafe {
                SetTextColor(hdc, colour(index));
                let _ = ExtTextOutW(
                    hdc,
                    *x,
                    *y,
                    ETO_CLIPPED,
                    Some(&to_rect(*clip)),
                    PCWSTR(wide.as_ptr()),
                    count,
                    None,
                );
            }
        }
    }
}

// ----- monitor and DPI -----

/// The monitor nearest to the caret and its work area (the desktop minus taskbars), in screen coordinates.
fn work_area_at(anchor: Anchor) -> Option<(HMONITOR, Rect)> {
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

/// Effective DPI of `monitor` (what the user chose in Settings), else of the window, else 96.
fn dpi_for(monitor: HMONITOR, hwnd: HWND) -> u32 {
    if let Some(get_dpi) = monitor_dpi_fn() {
        let (mut x, mut y) = (0u32, 0u32);
        // MDT_EFFECTIVE_DPI = 0
        // SAFETY: both out-pointers are live locals.
        if unsafe { get_dpi(monitor, 0, &mut x, &mut y) }.is_ok() && x > 0 {
            return x;
        }
    }
    // SAFETY: a plain query; an invalid handle yields 0.
    match unsafe { GetDpiForWindow(hwnd) } {
        0 => 96,
        dpi => dpi,
    }
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
}
