//! The plumbing of the windows that are painted pixel by pixel: the glows (one per mouth of light), the beam and the
//! laser panel.
//!
//! All of them are **layered windows**: a 32-bit picture with an alpha channel per pixel is handed to
//! `UpdateLayeredWindow`, which puts it on the screen with that alpha. That is the only way to draw light: GDI has
//! no transparency and no soft edges.
//!
//! They share one window procedure and one picture buffer; what makes them different is their role and the painter
//! that draws them. The windows never take the focus (`WM_MOUSEACTIVATE` answers "no"), so a click on the glow or
//! on the panel never takes the caret away from the editor.

use std::cell::RefCell;
use std::rc::Rc;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{HGDIOBJ, HFONT};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW, HMENU,
    MA_NOACTIVATE, RegisterClassExW, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
    SWP_NOOWNERZORDER, SetWindowLongPtrW, SetWindowPos, ShowWindow, HWND_TOPMOST, WNDCLASSEXW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use crate::laser::{Beam, Mouth, PanelLayout};
use crate::layout::Rect;
use crate::paint::LayeredSurface;
use pime_core::style::Rgb;

/// The class of the glow on the left of the screen.
pub const GLOW_CLASS: PCWSTR = w!("ProgrammingIME.Glow");
/// The class of the beam that shoots out of the glow.
pub const BEAM_CLASS: PCWSTR = w!("ProgrammingIME.Beam");
/// The class of the panel that opens on the mouth of the glow.
pub const PANEL_CLASS: PCWSTR = w!("ProgrammingIME.LaserPanel");

/// `WM_MOUSELEAVE` is not in the bindings of this version of the `windows` crate; the value is fixed by Windows.
const WM_MOUSELEAVE: u32 = 0x02A3;

/// The bit of a class in the table of the registered classes (every class has its own).
fn class_bit(role: Role) -> u8 {
    match role {
        Role::Glow => 0,
        Role::Beam => 1,
        Role::Panel => 2,
    }
}

/// The sink of a window: what the user did in it.
///
/// The key of the source is told with the event: every mouth of light has a window of its own, and the mouse that is
/// on the mouth of an agent is not the mouse that is on the mouth of the human.
pub type Sink = Rc<dyn Fn(Role, &str, GlassEvent)>;

/// What a glass window is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// A glow on the edge of the screen: there is one per mouth of light.
    Glow,
    /// The beam that shoots out of a mouth.
    Beam,
    /// The panel with the settings.
    Panel,
}

/// What the user did in a glass window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlassEvent {
    /// The mouse moved to a point of the window (client coordinates).
    Move { x: i32, y: i32 },
    /// The mouse left the window.
    Leave,
    /// The mouse was pressed at a point of the window (client coordinates).
    Press { x: i32, y: i32 },
    /// The mouse is being dragged (the position on the screen).
    Drag { x: i32, y: i32 },
    /// The button was let go (the point of the window and the position on the screen).
    Release {
        x: i32,
        y: i32,
        screen_x: i32,
        screen_y: i32,
    },
    /// The animation timer of the window ticked.
    Tick,
}

/// The state of the panel.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelState {
    /// Whether the panel is open.
    pub open: bool,
    /// The rectangle of the panel on the screen.
    pub rect: Rect,
    /// The rows of the panel.
    pub layout: PanelLayout,
    /// The row under the mouse.
    pub hot: Option<usize>,
    /// How much of the panel is drawn (the light wipes it open).
    pub reveal: f64,
}

impl PanelState {
    /// A closed panel.
    pub fn closed() -> PanelState {
        PanelState {
            open: false,
            rect: Rect::new(0, 0, 0, 0),
            layout: PanelLayout::compute(&[], 30, 8, 260),
            hot: None,
            reveal: 0.0,
        }
    }
}

/// What a window is: the class it is registered under, its role and the source it belongs to.
pub struct WindowKind {
    /// The class of the window.
    pub class: PCWSTR,
    /// What the window is for.
    pub role: Role,
    /// The key of the source the window belongs to (empty for the beam and the panel).
    pub key: String,
}

impl WindowKind {
    /// The window of the glow of a mouth of light.
    pub fn glow(key: &str) -> WindowKind {
        WindowKind {
            class: GLOW_CLASS,
            role: Role::Glow,
            key: key.to_string(),
        }
    }

    /// The window of the beam.
    pub fn beam() -> WindowKind {
        WindowKind {
            class: BEAM_CLASS,
            role: Role::Beam,
            key: String::new(),
        }
    }

    /// The window of the panel.
    pub fn panel() -> WindowKind {
        WindowKind {
            class: PANEL_CLASS,
            role: Role::Panel,
            key: String::new(),
        }
    }
}

/// One mouth on the edge of the screen.
///
/// There is one of these for every source of light: the human at the keyboard and every agent that writes into the
/// document. The colour of the mouth is the colour of its source, so that the user sees at once who is writing.
#[derive(Clone, Debug, PartialEq)]
pub struct MouthState {
    /// The key of the source (the identity of the speaker).
    pub key: String,
    /// The name of the source that is shown to the user.
    pub label: String,
    /// The colour of the slit of light.
    pub colour: Rgb,
    /// The colour of the halo around the slit.
    pub halo: Rgb,
    /// The mouth itself, in screen coordinates.
    pub mouth: Mouth,
    /// The rectangle of the window of the glow of this mouth, in screen coordinates.
    pub window: Rect,
}

/// What the windows draw and what the mouse works on.
pub struct Model {
    /// The mouths of light, one per source (the human and every agent).
    pub mouths: Vec<MouthState>,
    /// The beam that is alive, if any.
    pub beam: Option<Beam>,
    /// The rectangle of the window of the beam, in screen coordinates (the beam is drawn relative to it).
    pub beam_window: Rect,
    /// The panel.
    pub panel: PanelState,
    /// The clock of the animation, in milliseconds.
    pub now_ms: u64,
    /// The font the text is drawn with.
    pub font: Option<HFONT>,
    /// The mouse over the window (client coordinates).
    pub mouse: Option<(i32, i32)>,
}

impl Model {
    /// A model with nothing shown.
    pub fn empty() -> Model {
        Model {
            mouths: Vec::new(),
            beam: None,
            beam_window: Rect::new(0, 0, 0, 0),
            panel: PanelState::closed(),
            now_ms: 0,
            font: None,
            mouse: None,
        }
    }

    /// The mouth of a source, if it has one.
    pub fn mouth_of(&self, key: &str) -> Option<&MouthState> {
        self.mouths.iter().find(|mouth| mouth.key == key)
    }
}

/// The painter of a role: it draws the window into its picture.
///
/// The painter gets the whole picture and not just its pixels, because the text of the panel is drawn with GDI
/// (there is no other way to draw a glyph) and GDI does not write the alpha byte: the picture has to be told which
/// pixels the text touched.
///
/// The painter also gets the **key of the source** the window belongs to (the mouth of light it draws; the beam and
/// the panel have none), so that the glow of one source is never drawn in the window of another one.
pub type Painter = fn(&mut LayeredSurface, &Model, &str);

/// The state a glass window shares with its window procedure.
pub struct Shared {
    /// What the window is for.
    pub role: Role,
    /// The key of the source the window belongs to (empty for the beam and the panel).
    pub key: String,
    /// Whether the left button is down in this window right now.
    pub dragging: std::cell::Cell<bool>,
    /// What it draws and what the mouse works on. **All the windows share one model**, so what the mouse does in
    /// the glow is seen by the painter of the panel in the very same frame.
    pub model: Rc<RefCell<Model>>,
    /// The picture that is put on the screen.
    pub surface: RefCell<Option<LayeredSurface>>,
    /// The painter of this role.
    pub painter: Painter,
    /// What the user asked for.
    pub sink: Rc<dyn Fn(Role, GlassEvent)>,
}

/// A window that is painted pixel by pixel and shown with its own alpha.
pub struct GlassWindow {
    hwnd: HWND,
    shared: Rc<RefCell<Shared>>,
    width: i32,
    height: i32,
}

impl GlassWindow {
    /// Create the window (nothing is shown yet).
    ///
    /// The **key** is the identity of the source the window belongs to: the mouth of light whose glow this window
    /// draws. The beam and the panel have none (the key of theirs is empty).
    pub fn create(
        kind: WindowKind,
        width: i32,
        height: i32,
        model: Rc<RefCell<Model>>,
        painter: Painter,
        sink: Sink,
    ) -> Option<GlassWindow> {
        let instance = crate::popup::module_instance()?;
        register_class(kind.class, class_bit(kind.role), instance)?;
        // The window knows its source: the painter draws the mouth of that source, and the events of the mouse are
        // told with the key of it.
        let own_key = kind.key.clone();
        let event_key = own_key.clone();
        let sink: Rc<dyn Fn(Role, GlassEvent)> =
            Rc::new(move |role, event| sink(role, &event_key, event));
        let shared = Rc::new(RefCell::new(Shared {
            role: kind.role,
            key: own_key,
            dragging: std::cell::Cell::new(false),
            model,
            surface: RefCell::new(None),
            painter,
            sink,
        }));
        // SAFETY: the class is registered; no pointers are passed except the static class name.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                kind.class,
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
        .ok()?;
        let raw = Rc::into_raw(Rc::clone(&shared));
        // SAFETY: `hwnd` is our live window; USERDATA is ours alone.
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize) };
        Some(GlassWindow {
            hwnd,
            shared,
            width: width.max(1),
            height: height.max(1),
        })
    }

    /// The handle of the window.
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// How wide the picture of the window is.
    pub fn width(&self) -> i32 {
        self.width
    }

    /// How tall the picture of the window is.
    pub fn height(&self) -> i32 {
        self.height
    }

    /// The shared state of the window.
    pub fn shared(&self) -> &Rc<RefCell<Shared>> {
        &self.shared
    }

    /// Move the window (without activating it).
    ///
    /// A layered window is a **surface of its own**: the system does not carry that surface along when the window is
    /// moved with `SetWindowPos`, so the picture stays where it was while the window has already moved. What the user
    /// sees then is the picture of the old place at the old place and an empty window at the new one - the residue at
    /// the edges of the light.
    ///
    /// The picture is therefore uploaded again at the new place: `UpdateLayeredWindow` hands the new picture to the
    /// compositor **and** moves the window in the same call, so the old picture is gone before the new one appears and
    /// nothing can stay behind.
    pub fn move_to(&self, x: i32, y: i32) {
        // The rule of the layered windows: a window that has a picture is moved by that picture.
        match move_way(self.has_picture()) {
            Move::ByPicture => {
                if self.move_by_picture(x, y) {
                    return;
                }
            }
            Move::ByWindow => {}
        }
        // There is no picture yet (the window was only created), or the compositor refused it: the window is placed,
        // and the picture comes with the next paint.
        // SAFETY: plain call on our own window; SWP_NOACTIVATE keeps the focus in the editor.
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                x,
                y,
                self.width,
                self.height,
                SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            );
        }
    }

    /// Whether the window has a picture at the moment.
    fn has_picture(&self) -> bool {
        let Ok(shared) = self.shared.try_borrow() else {
            return false;
        };
        let Ok(surface) = shared.surface.try_borrow() else {
            return false;
        };
        surface.is_some()
    }

    /// Move the window by handing its picture to the compositor at the new place.
    ///
    /// Returns whether the picture could be moved. False when the window is busy painting itself (the next paint
    /// puts the picture where it belongs).
    fn move_by_picture(&self, x: i32, y: i32) -> bool {
        let Ok(shared) = self.shared.try_borrow() else {
            return false;
        };
        let Ok(surface) = shared.surface.try_borrow() else {
            return false;
        };
        let Some(surface) = surface.as_ref() else {
            return false;
        };
        // SAFETY: the surface is the picture of this window; the call moves the window and the surface together,
        // so the old place cannot keep a ghost of the light.
        surface.upload(self.hwnd, x, y)
    }

    /// Give the window a new size (the picture is made again on the next paint).
    pub fn resize(&mut self, width: i32, height: i32) {
        self.width = width.max(1);
        self.height = height.max(1);
        // the picture of the old size does not fit any more
        self.shared.borrow_mut().surface.borrow_mut().take();
    }

    /// Show the window (it never takes the focus).
    pub fn show(&self) {
        // SAFETY: plain call on our own window.
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
    }

    /// Hide the window (idempotent).
    pub fn hide(&self) {
        // SAFETY: plain call on our own window.
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    /// Draw the window and put it on the screen.
    pub fn repaint(&self) {
        paint_window(self.hwnd);
    }
}

impl Drop for GlassWindow {
    fn drop(&mut self) {
        if !self.hwnd.is_invalid() {
            // SAFETY: destroys our own window.
            unsafe {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}

/// Draw the window of `hwnd` and put it on the screen.
pub fn paint_window(hwnd: HWND) {
    // SAFETY: the window was created by `GlassWindow::create`, so USERDATA holds the live pointer.
    let Some(shared) = (unsafe { shared_of(hwnd) }) else {
        return;
    };
    let Ok(shared) = shared.try_borrow_mut() else {
        return;
    };
    let Some(mut surface) = shared.surface.borrow_mut().take() else {
        return;
    };
    surface.clear();
    {
        let model = shared.model.borrow();
        (shared.painter)(&mut surface, &model, &shared.key);
    }
    let position = window_position(hwnd);
    surface.upload(hwnd, position.0, position.1);
    shared.surface.borrow_mut().replace(surface);
}

/// The picture of a window, created on demand (the screen DC is the reference).
pub fn ensure_surface(shared: &Rc<RefCell<Shared>>, width: i32, height: i32) -> bool {
    if shared.borrow().surface.borrow().is_some() {
        return true;
    }
    // SAFETY: a plain call that borrows the desktop DC for a moment.
    let screen = unsafe { windows::Win32::Graphics::Gdi::GetDC(HWND::default()) };
    let Some(surface) = LayeredSurface::new(screen, width, height) else {
        // SAFETY: the DC was borrowed above and is given back here.
        unsafe {
            windows::Win32::Graphics::Gdi::ReleaseDC(HWND::default(), screen);
        }
        return false;
    };
    // SAFETY: the DC was borrowed above and is given back here.
    unsafe {
        windows::Win32::Graphics::Gdi::ReleaseDC(HWND::default(), screen);
    }
    shared.borrow_mut().surface.borrow_mut().replace(surface);
    true
}

/// The screen position of a window.
pub fn window_position(hwnd: HWND) -> (i32, i32) {
    let mut rect = windows::Win32::Foundation::RECT::default();
    // SAFETY: a plain read of the window rectangle.
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut rect) }.ok();
    (rect.left, rect.top)
}

/// Register a window class (once per class).
///
/// Every class has its own bit: the windows of the laser interface are three different classes (the glow of the
/// mouths, the beam and the panel), and a class that was never registered makes `CreateWindowExW` fail with "class
/// not found". Every mouth of light has its own window but they all share the class of the glow.
fn register_class(class: PCWSTR, bit: u8, instance: windows::Win32::Foundation::HINSTANCE) -> Option<()> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static REGISTERED: AtomicU32 = AtomicU32::new(0);
    if REGISTERED.load(Ordering::Acquire) & (1 << bit) != 0 {
        return Some(());
    }
    // SAFETY: a stock cursor id; no instance is needed for it.
    let cursor = unsafe {
        windows::Win32::UI::WindowsAndMessaging::LoadCursorW(
            windows::Win32::Foundation::HINSTANCE::default(),
            windows::Win32::UI::WindowsAndMessaging::IDC_ARROW,
        )
    }
    .unwrap_or_default();
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wnd_proc),
        hInstance: instance,
        hCursor: cursor,
        lpszClassName: class,
        ..Default::default()
    };
    // SAFETY: `wc` is fully initialised and its strings are static.
    let atom = unsafe { RegisterClassExW(&wc) };
    if atom != 0
        || unsafe { windows::Win32::Foundation::GetLastError() }
            == windows::Win32::Foundation::ERROR_CLASS_ALREADY_EXISTS
    {
        REGISTERED.fetch_or(1 << bit, Ordering::Release);
        Some(())
    } else {
        None
    }
}

/// The shared state of `hwnd`, if it has one.
///
/// # Safety
/// Only call from `wnd_proc` of a window created by [`GlassWindow::create`]: USERDATA then holds the pointer made by
/// `Rc::into_raw`, which stays valid until `WM_NCDESTROY` on this same thread, and the result must not outlive the
/// message.
unsafe fn shared_of<'a>(hwnd: HWND) -> Option<&'a RefCell<Shared>> {
    // SAFETY: a plain read of the window's USERDATA.
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const RefCell<Shared>;
    // SAFETY: null or the live pointer described above.
    unsafe { ptr.as_ref() }
}

/// The window procedure of every glass window.
///
/// Everything runs inside a panic guard: a panic that unwound out of this `extern "system"` frame would abort the
/// host process.
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    crate::ui::guard(None, || Some(window_message(hwnd, msg, wparam, lparam))).unwrap_or_else(|| {
        match msg {
            windows::Win32::UI::WindowsAndMessaging::WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            windows::Win32::UI::WindowsAndMessaging::WM_ERASEBKGND => LRESULT(1),
            windows::Win32::UI::WindowsAndMessaging::WM_PAINT => LRESULT(0),
            // SAFETY: forwarding the message unchanged.
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    })
}

fn window_message(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{
        HTCLIENT, HTTRANSPARENT, WM_ERASEBKGND, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE,
        WM_MOUSEMOVE, WM_NCDESTROY, WM_NCHITTEST, WM_PAINT, WM_TIMER,
    };
    match msg {
        // A click on the glow or on the panel must not move the focus away from the host's editor.
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_NCHITTEST => {
            // The windows of the light are only there where the light is: everywhere else the click belongs to the
            // program underneath. The beam in particular must never swallow a click.
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            if hit_at(hwnd, x, y) {
                LRESULT(HTCLIENT as isize)
            } else {
                LRESULT(HTTRANSPARENT as isize)
            }
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            paint_window(hwnd);
            LRESULT(0)
        }
        WM_TIMER => {
            notify(hwnd, GlassEvent::Tick);
            paint_window(hwnd);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            if is_dragging(hwnd) {
                // The window may have moved while the mouse was down, so the position on the screen is what counts.
                let (screen_x, screen_y) = cursor_position();
                notify(hwnd, GlassEvent::Drag { x: screen_x, y: screen_y });
            } else {
                notify(hwnd, GlassEvent::Move { x, y });
            }
            paint_window(hwnd);
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            notify(hwnd, GlassEvent::Leave);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            // The window keeps the mouse while the button is down: the drag of the mouth must not stop when the
            // mouse leaves the small window of the mouth.
            set_dragging(hwnd, true);
            // SAFETY: our own window; the capture is given back when the button is let go.
            unsafe {
                let _ = SetCapture(hwnd);
            }
            notify(hwnd, GlassEvent::Press { x, y });
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let (screen_x, screen_y) = cursor_position();
            set_dragging(hwnd, false);
            // SAFETY: the capture is ours; giving it back is what Windows expects.
            unsafe {
                let _ = ReleaseCapture();
            }
            notify(
                hwnd,
                GlassEvent::Release {
                    x,
                    y,
                    screen_x,
                    screen_y,
                },
            );
            LRESULT(0)
        }
        WM_NCDESTROY => {
            // SAFETY: the last message of this window; takes back the count given at creation.
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

/// Whether the point (in screen coordinates) is on the part of the window that is really there.
fn hit_at(hwnd: HWND, x: i32, y: i32) -> bool {
    // SAFETY: the window was created by `GlassWindow::create`, so USERDATA holds the live pointer.
    let Some(shared) = (unsafe { shared_of(hwnd) }) else {
        return false;
    };
    let role = shared.borrow().role;
    match role {
        // The glow is the mouth: the light around it is a picture, not a thing the mouse can touch. Every glow has
        // the mouth of its own source, so the light of an agent is not a handle for the user.
        Role::Glow => {
            let shared = shared.borrow();
            let key = shared.key.clone();
            let model = shared.model.borrow();
            model
                .mouth_of(&key)
                .map(|mouth| mouth.mouth.contains(x, y))
                .unwrap_or(false)
        }
        // The beam is light in the air: a click belongs to the program under it.
        Role::Beam => false,
        // The panel is a solid thing.
        Role::Panel => true,
    }
}

/// Whether the left button is down in the window.
fn is_dragging(hwnd: HWND) -> bool {
    // SAFETY: the window was created by `GlassWindow::create`, so USERDATA holds the live pointer.
    let Some(shared) = (unsafe { shared_of(hwnd) }) else {
        return false;
    };
    shared.borrow().dragging.get()
}

/// Remembers that the left button is down (or not).
fn set_dragging(hwnd: HWND, dragging: bool) {
    // SAFETY: the window was created by `GlassWindow::create`, so USERDATA holds the live pointer.
    let Some(shared) = (unsafe { shared_of(hwnd) }) else {
        return;
    };
    shared.borrow_mut().dragging.set(dragging);
}

/// Tell the owner that the user did something in the window.
fn notify(hwnd: HWND, event: GlassEvent) {
    // SAFETY: the window was created by `GlassWindow::create`, so USERDATA holds the live pointer.
    let Some(shared) = (unsafe { shared_of(hwnd) }) else {
        return;
    };
    let (role, sink) = {
        let Ok(shared) = shared.try_borrow() else {
            return;
        };
        (shared.role, shared.sink.clone())
    };
    sink(role, event);
}

/// The GDI objects of a window are released when the window goes; this keeps the compiler happy about the unused
/// import in builds where the font is not drawn.
#[allow(dead_code)]
fn _font_is_a_gdi_object(font: HFONT) -> HGDIOBJ {
    HGDIOBJ(font.0)
}

/// How a layered window is moved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    /// The picture is handed to the compositor at the new place: the window and the surface move in the same call,
    /// so nothing of the old place can stay behind.
    ByPicture,
    /// The window is only placed: there is no picture yet, so there is nothing that could stay behind either.
    ByWindow,
}

/// The way a layered window is moved.
///
/// The rule is a function of one thing, so it can be stated in a test without a screen: **a window that has a picture
/// is moved by that picture**. Moving such a window with `SetWindowPos` alone would leave the picture behind - the
/// residue at the edges of the light.
pub fn move_way(has_picture: bool) -> Move {
    if has_picture {
        Move::ByPicture
    } else {
        Move::ByWindow
    }
}

/// The point of a click, in client coordinates.
pub fn click_point(lparam: LPARAM) -> (i32, i32) {
    (
        (lparam.0 & 0xFFFF) as i16 as i32,
        ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
    )
}

/// The point of a mouse move, in client coordinates.
pub fn mouse_point(lparam: LPARAM) -> (i32, i32) {
    click_point(lparam)
}

/// Where the mouse is on the screen.
pub fn cursor_position() -> (i32, i32) {
    let mut point = POINT::default();
    // SAFETY: a plain read of the cursor position.
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut point) }.ok();
    (point.x, point.y)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A window that has a picture is moved by that picture: the surface and the window move in the same call, so the
    /// old place cannot keep a ghost of the light (the residue at the edges the user asked for).
    ///
    /// A window that has no picture yet is only placed: there is nothing on the screen that could stay behind.
    #[test]
    fn a_window_with_a_picture_is_moved_by_its_picture() {
        assert_eq!(
            move_way(true),
            Move::ByPicture,
            "the picture moves the window: the surface and the window go together"
        );
        assert_eq!(
            move_way(false),
            Move::ByWindow,
            "no picture, nothing to leave behind: the window is only placed"
        );
    }
}
