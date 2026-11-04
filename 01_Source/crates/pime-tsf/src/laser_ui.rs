//! The laser interface: the blue glow on the left of the screen, the beam that shoots out of it when a character is
//! written, the panel that opens when the mouse rests on the mouth of the glow, and the logo in the corner.
//!
//! The four windows are the ones of `glass`; this module owns them, draws them and runs the small state machine
//! that connects them:
//!
//! * the glow is always there while the input method has a document;
//! * a character that is written makes the mouth fire a **beam** at it, which lights the character up for half a
//!   second, so the eye sees where the text appeared;
//! * the mouse that rests on the mouth opens the **panel** with the settings that are used most often;
//! * the **logo** in the corner is the way into the same panel without moving the mouse to the edge of the screen.
//!
//! Nothing here takes the focus: the user is typing in another program, and the input method must not take the
//! caret away from it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use pime_core::style::{
    BRAND_RAINBOW, Canvas, LaserTheme, PixelRect, Rgb, coverage, glow_profile, mix, spline_sample,
};

use crate::glass::{
    BEAM_CLASS, GLOW_CLASS, GlassEvent, GlassWindow, LOGO_CLASS, Model, PANEL_CLASS, Role,
    ensure_surface, window_position,
};
use crate::laser::{
    Beam, Edge, HOVER_CLOSE_MS, HOVER_OPEN_MS, Mouth, MouthPlace, PanelAction, PanelItem, PanelLayout,
    PanelOrigin, mouth_at, panel_beside, panel_place, place_at,
};
use crate::layout::Rect;
use crate::layout::TextMetrics;
use crate::paint::LayeredSurface;
use crate::popup::GdiObject;
use crate::ui::Anchor;

/// The work area of the screen: the area without the taskbar.
///
/// Windows draws the taskbar over the work area of the monitor in some arrangements, so the work area of the screen
/// is asked for directly: the logo of the input method must never cover the clock.
fn desktop_work_area() -> Option<Rect> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETWORKAREA, SPIF_UPDATEINIFILE, SystemParametersInfoW,
    };
    let mut area = RECT::default();
    // SAFETY: the rectangle is a live local and the action only writes into it.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut area as *mut RECT as *mut core::ffi::c_void),
            SPIF_UPDATEINIFILE,
        )
    };
    if ok.is_err() {
        return None;
    }
    Some(Rect::new(area.left, area.top, area.right, area.bottom))
}

/// How wide the window of the mouth is.
///
/// It is a square around the lens and not a tall band any more: the mouth can be on the left, on the top or on the
/// right, and the light around it follows it there.
const GLOW_SIZE: i32 = 64;

/// The radius of the lens of the mouth.
const MOUTH_RADIUS: i32 = 15;

/// How tall the band at the top of the panel is. It is the handle of the panel: the panel is dragged by it.
const PANEL_HANDLE: i32 = 18;

/// How wide the logo in the corner is.
const LOGO_SIZE: i32 = 34;

/// How wide the panel is.
const PANEL_WIDTH: i32 = 268;

/// How tall the panel is (the header, the nine rows and the room the light needs around them).
const PANEL_HEIGHT: i32 = 326;

/// How tall a row of the panel is.
const PANEL_ROW_HEIGHT: i32 = 30;

/// How deep the gradient of the brand rainbow along the top of the panel reaches.
const PANEL_RAINBOW_BAND: f64 = 9.0;

/// How much room the beam has around its way.
const BEAM_MARGIN: i32 = 26;

/// The id of the timer that drives the animation.
const TIMER_ID: usize = 1;

/// How often the animation is drawn (in milliseconds).
const TICK_MS: u32 = 16;

/// What the user asked the laser interface to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaserEvent {
    /// A switch of the panel was turned around.
    Toggle { key: &'static str, on: bool },
    /// A list of the panel was opened.
    Menu { key: &'static str },
    /// The settings window was asked for.
    Settings,
    /// The mouth of the glow was clicked: the input method switches (the user is in another one and wants to
    /// write code, or wants to go back to the one they came from).
    Switch,
    /// The mouth of the light was dragged to another place on the screen.
    MouthMoved { edge: Edge, offset: i32 },
}

/// The laser interface of one thread.
pub struct LaserUi {
    /// The handle to itself, so that the windows can call back into it (the handle is weak, so the windows do not
    /// keep the interface alive).
    weak: std::rc::Weak<RefCell<LaserUi>>,
    /// The state the four windows share.
    model: Rc<RefCell<Model>>,
    glow: Option<GlassWindow>,
    beam: Option<GlassWindow>,
    panel: Option<GlassWindow>,
    logo: Option<GlassWindow>,
    /// The font, kept alive for as long as the interface lives.
    font: Option<(u32, GdiObject)>,
    /// What the user asked for.
    sink: Rc<dyn Fn(LaserEvent)>,
    /// When the mouse came to rest on the mouth (the panel opens after `HOVER_OPEN_MS`).
    hover_since: Option<u64>,
    /// Whether the mouse is on the mouth right now.
    on_mouth: bool,
    /// Whether the mouse is on the panel right now.
    on_panel: bool,
    /// When the mouse left the panel (it closes after `HOVER_CLOSE_MS`).
    left_at: Option<u64>,
    /// The rows of the panel.
    items: Vec<PanelItem>,
    /// The work area the windows live in.
    work: Rect,
    /// Whether the interface is on the screen.
    visible: bool,
    /// Where the mouth is on the screen (the edge it is on and how far along it). `None` is the default place: the
    /// left edge, in the middle of the screen.
    place: Option<MouthPlace>,
    /// The drag that is going on right now.
    drag: Option<Drag>,
    /// That the interface has to go off the screen, asked for while it was busy.
    ///
    /// The switch of the input method happens in the callback of the mouth, and TSF deactivates the text service
    /// inside that very call: the interface is borrowed at that moment, so it cannot be asked to hide itself. The
    /// flag is set instead, and the windows go off the screen as soon as the callback returns.
    hide_requested: Rc<Cell<bool>>,
}

/// What is being dragged right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drag {
    /// The mouth of the light. The numbers are the offset of the mouse from the middle of the lens, and the place
    /// the lens was at when the drag began.
    Mouth {
        dx: i32,
        dy: i32,
        start_x: i32,
        start_y: i32,
    },
    /// The panel. The numbers are the offset of the mouse from the top left corner of the panel.
    Panel { dx: i32, dy: i32 },
}

impl LaserUi {
    /// Create the interface and the handle the owner keeps.
    ///
    /// The handle is a `Rc<RefCell<LaserUi>>`: the windows call back into the interface (the timer of the glow
    /// drives the animation), and the interface owns the windows. The call back is a **weak** handle, so the windows
    /// do not keep the interface alive.
    pub fn create(sink: Rc<dyn Fn(LaserEvent)>) -> Rc<RefCell<LaserUi>> {
        Rc::new_cyclic(|weak| {
            let mut ui = LaserUi {
                weak: weak.clone(),
                model: Rc::new(RefCell::new(Model::empty())),
                glow: None,
                beam: None,
                panel: None,
                logo: None,
                font: None,
                sink,
                hover_since: None,
                on_mouth: false,
                on_panel: false,
                left_at: None,
                items: Vec::new(),
                work: Rect::new(0, 0, 0, 0),
                visible: false,
                place: None,
                drag: None,
                hide_requested: Rc::new(Cell::new(false)),
            };
            ui.ensure_windows();
            RefCell::new(ui)
        })
    }

    /// The rows the panel shows.
    pub fn set_items(&mut self, items: Vec<PanelItem>) {
        self.items = items;
        self.refresh_panel_layout();
    }

    /// The flag that says the interface has to go off the screen.
    ///
    /// The owner of the interface keeps a copy of it, so that it can ask the interface to go away even while one
    /// of its own windows is calling back (the switch of the input method happens exactly then).
    pub fn hide_flag(&self) -> Rc<Cell<bool>> {
        Rc::clone(&self.hide_requested)
    }

    /// Put the mouth where the user put it the last time.
    pub fn set_place(&mut self, place: Option<MouthPlace>) {
        self.place = place;
        if self.visible {
            self.place_windows();
            self.repaint_all();
        }
    }

    /// Put the interface on the screen (the work area of the monitor the caret is on).
    pub fn show(&mut self, work: Rect) {
        self.hide_requested.set(false);
        self.work = work;
        self.visible = true;
        self.ensure_windows();
        self.place_windows();
        self.start_timer();
        self.ensure_font_for(work);
        self.repaint_all();
    }

    /// The font of the panel, for the DPI of the monitor the interface is on.
    fn ensure_font_for(&mut self, work: Rect) {
        let Some(glow) = &self.glow else {
            return;
        };
        let anchor = Anchor {
            x: work.left + 1,
            y: work.top + 1,
            line_height: 0,
        };
        let Some((monitor, _)) = crate::popup::work_area_at(anchor) else {
            return;
        };
        let dpi = crate::popup::dpi_for(monitor, glow.hwnd());
        self.ensure_font(dpi);
    }

    /// The rectangle of a row of the panel, in screen coordinates (the menu of that row opens under it).
    pub fn row_rect(&self, key: &str) -> Option<Rect> {
        let model = self.model.borrow();
        if !model.panel.open {
            return None;
        }
        let origin = model.panel.rect;
        model.panel.layout.rows.iter().find_map(|(item, rect)| {
            let matches = match item {
                PanelItem::Toggle { key: k, .. } => *k == key,
                PanelItem::Menu { key: k, .. } => *k == key,
                PanelItem::Settings => key == "settings",
            };
            if !matches {
                return None;
            }
            Some(Rect::new(
                origin.left + rect.left,
                origin.top + rect.top,
                origin.left + rect.right,
                origin.top + rect.bottom,
            ))
        })
    }

    /// Start the timer that drives the animation (the window procedure of the glow asks for the tick).
    fn start_timer(&self) {
        let Some(glow) = &self.glow else {
            return;
        };
        // SAFETY: a timer on our own window; the id is ours.
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SetTimer(
                glow.hwnd(),
                TIMER_ID,
                TICK_MS,
                None,
            );
        }
    }

    /// Stop the timer.
    fn stop_timer(&self) {
        let Some(glow) = &self.glow else {
            return;
        };
        // SAFETY: a timer on our own window; the id is ours.
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(glow.hwnd(), TIMER_ID);
        }
    }

    /// Take the interface off the screen.
    pub fn hide(&mut self) {
        self.hide_requested.set(false);
        self.visible = false;
        self.stop_timer();
        self.close_panel();
        for window in [
            self.glow.as_ref(),
            self.beam.as_ref(),
            self.panel.as_ref(),
            self.logo.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            window.hide();
        }
    }

    /// Whether the interface is on the screen.
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Fire the beam at a character that was just written.
    pub fn fire(&mut self, anchor: Anchor) {
        if !self.visible {
            return;
        }
        let now = now_ms();
        let mouth = self.model.borrow().mouth;
        let beam = Beam::new((mouth.x, mouth.y), (anchor.x, anchor.y), now);
        // the window of the beam covers the whole way of the beam
        let left = mouth.x.min(anchor.x) - BEAM_MARGIN;
        let top = mouth.y.min(anchor.y) - BEAM_MARGIN;
        let right = mouth.x.max(anchor.x) + BEAM_MARGIN;
        let bottom = mouth.y.max(anchor.y) + BEAM_MARGIN;
        {
            let mut model = self.model.borrow_mut();
            model.beam = Some(beam);
            model.beam_window = Rect::new(left, top, right, bottom);
        }
        if let Some(beam_window) = &mut self.beam {
            beam_window.resize(right - left, bottom - top);
            beam_window.move_to(left, top);
            beam_window.show();
        }
        self.repaint_all();
    }

    /// Advance the animation (the timer of the glow window calls this).
    pub fn tick(&mut self) {
        if !self.visible {
            return;
        }
        let now = now_ms();
        {
            let mut model = self.model.borrow_mut();
            model.now_ms = now;
            if let Some(beam) = model.beam
                && beam.is_finished(now)
            {
                model.beam = None;
            }
            // the panel wipes itself open
            if model.panel.open && model.panel.reveal < 1.0 {
                model.panel.reveal = (model.panel.reveal + 0.14).min(1.0);
            }
            // the logo breathes
            model.logo.pulse = ((now % 2400) as f64 / 2400.0).min(1.0);
        }
        // the mouse rested on the mouth long enough: the panel opens
        if self.on_mouth && !self.model.borrow().panel.open {
            let since = self.hover_since.unwrap_or(now);
            if now.saturating_sub(since) >= HOVER_OPEN_MS {
                self.open_panel(PanelOrigin::Glow);
            }
        }
        // the mouse left the panel: it closes after a moment
        if !self.on_panel && !self.on_mouth && self.model.borrow().panel.open {
            let left_at = self.left_at.unwrap_or(now);
            if now.saturating_sub(left_at) >= HOVER_CLOSE_MS {
                self.close_panel();
            }
        }
        self.repaint_all();
    }

    /// What the user did in one of the windows.
    pub fn handle(&mut self, role: Role, event: GlassEvent) {
        match event {
            GlassEvent::Move { x, y } => self.on_move(role, x, y),
            GlassEvent::Leave => self.on_leave(role),
            GlassEvent::Press { x, y } => self.on_press(role, x, y),
            GlassEvent::Drag { x, y } => self.on_drag(role, x, y),
            GlassEvent::Release {
                x,
                y,
                screen_x,
                screen_y,
            } => self.on_release(role, x, y, screen_x, screen_y),
            GlassEvent::LogoClick => self.on_logo_click(),
            GlassEvent::Tick => self.tick(),
        }
    }

    /// The mouse was pressed in one of the windows.
    fn on_press(&mut self, role: Role, x: i32, y: i32) {
        match role {
            Role::Glow => {
                let origin = self.window_origin(Role::Glow);
                let mouth = self.model.borrow().mouth;
                let (sx, sy) = (origin.0 + x, origin.1 + y);
                if mouth.contains(sx, sy) {
                    // The mouth is the handle of the light: it can be dragged along the edge of the screen.
                    self.drag = Some(Drag::Mouth {
                        dx: sx - mouth.x,
                        dy: sy - mouth.y,
                        start_x: mouth.x,
                        start_y: mouth.y,
                    });
                }
            }
            Role::Panel => {
                // The rows are the buttons; the band at the top is the handle of the panel.
                if y < PANEL_HANDLE {
                    let origin = self.window_origin(Role::Panel);
                    self.drag = Some(Drag::Panel {
                        dx: origin.0 + x,
                        dy: origin.1 + y,
                    });
                }
            }
            Role::Logo => {}
            Role::Beam => {}
        }
    }

    /// The mouse is being dragged: the mouth or the panel follows it.
    fn on_drag(&mut self, role: Role, x: i32, y: i32) {
        let Some(drag) = self.drag else {
            return;
        };
        let work = self.work;
        match drag {
            Drag::Mouth { dx, dy, .. } => {
                if role != Role::Glow {
                    return;
                }
                // The mouth follows the mouse, but it never leaves the screen.
                let low_x = work.left + MOUTH_RADIUS;
                let high_x = (work.right - MOUTH_RADIUS).max(low_x);
                let low_y = work.top + MOUTH_RADIUS;
                let high_y = (work.bottom - MOUTH_RADIUS).max(low_y);
                let mouth = Mouth {
                    x: (x - dx).clamp(low_x, high_x),
                    y: (y - dy).clamp(low_y, high_y),
                    radius: MOUTH_RADIUS,
                };
                self.model.borrow_mut().mouth = mouth;
                self.move_glow(mouth);
                self.repaint_all();
            }
            Drag::Panel { dx, dy } => {
                if role != Role::Panel {
                    return;
                }
                let low_x = work.left;
                let high_x = (work.right - PANEL_WIDTH).max(low_x);
                let low_y = work.top;
                let high_y = (work.bottom - PANEL_HEIGHT).max(low_y);
                let nx = (x - dx).clamp(low_x, high_x);
                let ny = (y - dy).clamp(low_y, high_y);
                self.model.borrow_mut().panel.rect =
                    Rect::new(nx, ny, nx + PANEL_WIDTH, ny + PANEL_HEIGHT);
                if let Some(panel) = &self.panel {
                    panel.move_to(nx, ny);
                }
                self.repaint_all();
            }
        }
    }

    /// The button was let go: a drag ends, or a click does what it does.
    fn on_release(&mut self, role: Role, x: i32, y: i32, screen_x: i32, screen_y: i32) {
        let Some(drag) = self.drag.take() else {
            // No drag went on: this is a click.
            self.on_click(role, x, y);
            return;
        };
        match drag {
            Drag::Mouth {
                dx,
                dy,
                start_x,
                start_y,
            } => {
                let mouth = self.model.borrow().mouth;
                let moved = (mouth.x - start_x).abs() + (mouth.y - start_y).abs();
                if moved < 4 {
                    // A click on the mouth and not a drag: the mouth is the switch between the input methods.
                    (self.sink)(LaserEvent::Switch);
                    return;
                }
                // The mouth goes to the edge that is nearest, and it stays on the screen.
                let work = self.work;
                let place = place_at(work, screen_x - dx, screen_y - dy, MOUTH_RADIUS);
                self.place = Some(place);
                let mouth = mouth_at(work, place, MOUTH_RADIUS);
                self.model.borrow_mut().mouth = mouth;
                self.move_glow(mouth);
                // The panel that hangs on the mouth goes with it: otherwise it would stay behind on the old edge.
                let panel_open = self.model.borrow().panel.open;
                let panel_from_glow = self.model.borrow().panel.origin == PanelOrigin::Glow;
                if panel_open && panel_from_glow {
                    let (x, y) = panel_beside(place, mouth, (PANEL_WIDTH, PANEL_HEIGHT), work);
                    self.model.borrow_mut().panel.rect =
                        Rect::new(x, y, x + PANEL_WIDTH, y + PANEL_HEIGHT);
                    if let Some(panel) = &self.panel {
                        panel.move_to(x, y);
                    }
                }
                // The owner remembers the place, so the mouth is still there the next time.
                (self.sink)(LaserEvent::MouthMoved {
                    edge: place.edge,
                    offset: place.offset,
                });
                self.repaint_all();
            }
            Drag::Panel { dx, dy } => {
                let work = self.work;
                let (nx, ny) =
                    panel_place(work, screen_x - dx, screen_y - dy, PANEL_WIDTH, PANEL_HEIGHT);
                self.model.borrow_mut().panel.rect =
                    Rect::new(nx, ny, nx + PANEL_WIDTH, ny + PANEL_HEIGHT);
                if let Some(panel) = &self.panel {
                    panel.move_to(nx, ny);
                }
                self.repaint_all();
            }
        }
    }

    /// The mouse moved in one of the windows.
    fn on_move(&mut self, role: Role, x: i32, y: i32) {
        match role {
            Role::Glow => {
                let origin = self.window_origin(Role::Glow);
                let mouth = self.model.borrow().mouth;
                let on = mouth.contains(origin.0 + x, origin.1 + y);
                if on && !self.on_mouth {
                    self.on_mouth = true;
                    self.hover_since = Some(now_ms());
                } else if !on && self.on_mouth {
                    self.on_mouth = false;
                    self.hover_since = None;
                }
                self.model.borrow_mut().mouse = Some((x, y));
            }
            Role::Panel => {
                self.on_panel = true;
                self.left_at = None;
                let layout = self.model.borrow().panel.layout.clone();
                let hot = layout.rows.iter().position(|(_, rect)| rect.contains(x, y));
                self.model.borrow_mut().panel.hot = hot;
            }
            Role::Logo => {
                self.model.borrow_mut().logo.hover = true;
            }
            Role::Beam => {}
        }
    }

    /// The mouse left one of the windows.
    fn on_leave(&mut self, role: Role) {
        match role {
            Role::Glow => {
                self.on_mouth = false;
                self.hover_since = None;
            }
            Role::Panel => {
                self.on_panel = false;
                self.left_at = Some(now_ms());
            }
            Role::Logo => {
                self.model.borrow_mut().logo.hover = false;
            }
            Role::Beam => {}
        }
    }

    /// The user clicked in one of the windows.
    fn on_click(&mut self, role: Role, x: i32, y: i32) {
        match role {
            Role::Panel => {
                let action = {
                    let layout = self.model.borrow().panel.layout.clone();
                    layout.action_at(x, y)
                };
                if let Some(action) = action {
                    self.apply(action);
                }
            }
            Role::Logo => self.on_logo_click(),
            Role::Glow => {
                // The mouth itself is handled by the drag (a click on it is the switch, a drag of it moves it).
                // A click on the light around the mouth opens the panel.
                self.open_panel(PanelOrigin::Glow);
            }
            Role::Beam => {}
        }
    }

    /// The logo in the corner was clicked: the panel opens next to it.
    fn on_logo_click(&mut self) {
        self.open_panel(PanelOrigin::Logo);
    }

    /// Tell the owner that a row of the panel was used.
    fn apply(&mut self, action: PanelAction) {
        match action {
            PanelAction::Toggle { key, on } => (self.sink)(LaserEvent::Toggle { key, on }),
            PanelAction::Menu { key } => (self.sink)(LaserEvent::Menu { key }),
            PanelAction::Settings => (self.sink)(LaserEvent::Settings),
        }
    }

    /// Open the panel next to the mouth (or next to the logo).
    pub fn open_panel(&mut self, origin: PanelOrigin) {
        if self.panel.is_none() {
            return;
        }
        let (x, y) = match origin {
            PanelOrigin::Glow => {
                let mouth = self.model.borrow().mouth;
                let place = self.place.unwrap_or_else(|| MouthPlace::default_in(self.work));
                panel_beside(place, mouth, (PANEL_WIDTH, PANEL_HEIGHT), self.work)
            }
            PanelOrigin::Logo => {
                let logo = self.model.borrow().logo.rect;
                (logo.left - PANEL_WIDTH - 8, logo.top - PANEL_HEIGHT - 8)
            }
        };
        {
            let mut model = self.model.borrow_mut();
            model.panel.open = true;
            model.panel.origin = origin;
            model.panel.reveal = 0.0;
            model.panel.rect = Rect::new(x, y, x + PANEL_WIDTH, y + PANEL_HEIGHT);
        }
        self.on_panel = true;
        self.left_at = None;
        if let Some(panel) = &self.panel {
            panel.move_to(x, y);
            panel.show();
        }
        self.repaint_all();
    }

    /// Close the panel.
    fn close_panel(&mut self) {
        self.model.borrow_mut().panel.open = false;
        if let Some(panel) = &self.panel {
            panel.hide();
        }
    }

    /// The screen position of one of the windows.
    fn window_origin(&self, role: Role) -> (i32, i32) {
        let window = match role {
            Role::Glow => self.glow.as_ref(),
            Role::Beam => self.beam.as_ref(),
            Role::Panel => self.panel.as_ref(),
            Role::Logo => self.logo.as_ref(),
        };
        match window {
            Some(window) => window_position(window.hwnd()),
            None => (0, 0),
        }
    }

    /// Create the windows (once).
    fn ensure_windows(&mut self) {
        if self.glow.is_some() {
            return;
        }
        // The windows call back into the interface through a weak handle: the interface owns the windows, so a strong
        // handle here would be a cycle that nothing would ever break.
        let weak = self.weak.clone();
        let hide_requested = Rc::clone(&self.hide_requested);
        let sink: Rc<dyn Fn(Role, GlassEvent)> = Rc::new(move |role, event| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if let Ok(mut ui) = ui.try_borrow_mut() {
                ui.handle(role, event);
            }
            // The interface is free again: a hide that was asked for during the callback (the switch of the input
            // method happens in the callback of the mouth) happens now, and the windows go off the screen at once.
            if hide_requested.get()
                && let Ok(mut ui) = ui.try_borrow_mut()
            {
                ui.hide();
            }
        });
        let model = Rc::clone(&self.model);
        self.glow = GlassWindow::create(
            GLOW_CLASS,
            Role::Glow,
            GLOW_SIZE,
            GLOW_SIZE,
            Rc::clone(&model),
            paint_glow,
            Rc::clone(&sink),
        );
        self.beam = GlassWindow::create(
            BEAM_CLASS,
            Role::Beam,
            16,
            16,
            Rc::clone(&model),
            paint_beam,
            Rc::clone(&sink),
        );
        self.panel = GlassWindow::create(
            PANEL_CLASS,
            Role::Panel,
            PANEL_WIDTH,
            PANEL_HEIGHT,
            Rc::clone(&model),
            paint_panel,
            Rc::clone(&sink),
        );
        self.logo = GlassWindow::create(
            LOGO_CLASS,
            Role::Logo,
            LOGO_SIZE,
            LOGO_SIZE,
            model,
            paint_logo,
            sink,
        );
        self.refresh_panel_layout();
    }

    /// Put the window of the glow on the mouth and remember where the window is on the screen.
    ///
    /// The painter of the glow works with the position of the mouth **inside** the window, so the place of the window
    /// has to be known. The window follows the mouth, and the mouth sits in its middle.
    fn move_glow(&mut self, mouth: Mouth) {
        let left = mouth.x - GLOW_SIZE / 2;
        let top = mouth.y - GLOW_SIZE / 2;
        self.model.borrow_mut().glow_window =
            Rect::new(left, top, left + GLOW_SIZE, top + GLOW_SIZE);
        if let Some(glow) = &self.glow {
            glow.move_to(left, top);
        }
    }

    /// Put the windows where they belong.
    fn place_windows(&mut self) {
        let work = self.work;
        if work.width() <= 0 || work.height() <= 0 {
            return;
        }
        let place = self.place.unwrap_or_else(|| MouthPlace::default_in(work));
        let mouth = mouth_at(work, place, MOUTH_RADIUS);
        self.model.borrow_mut().mouth = mouth;
        self.move_glow(mouth);
        if let Some(glow) = &self.glow {
            glow.show();
        }
        // The logo sits in the corner, but it must not cover the clock of the taskbar. The work area of the screen
        // is the area without the taskbar, so the logo goes to the bottom of that one - not to the bottom of the
        // screen, where the taskbar is drawn over it.
        let corner = desktop_work_area().unwrap_or(work);
        let logo = Rect::new(
            corner.right - LOGO_SIZE - 8,
            corner.bottom - LOGO_SIZE - 8,
            corner.right - 8,
            corner.bottom - 8,
        );
        self.model.borrow_mut().logo.rect = logo;
        if let Some(logo_window) = &self.logo {
            logo_window.move_to(logo.left, logo.top);
            logo_window.show();
        }
        self.refresh_panel_layout();
    }

    /// The rows of the panel, with their places.
    fn refresh_panel_layout(&mut self) {
        let layout = PanelLayout::compute(&self.items, PANEL_ROW_HEIGHT, PANEL_HANDLE, PANEL_WIDTH - 24);
        self.model.borrow_mut().panel.layout = layout;
    }

    /// Draw every window that is on the screen.
    fn repaint_all(&mut self) {
        let windows: Vec<&GlassWindow> = [
            self.glow.as_ref(),
            self.beam.as_ref(),
            self.panel.as_ref(),
            self.logo.as_ref(),
        ]
        .into_iter()
        .flatten()
        .collect();
        for window in windows {
            ensure_surface(window.shared(), window.width(), window.height());
            window.repaint();
        }
    }

    /// The font of the interface, created for a DPI.
    pub fn ensure_font(&mut self, dpi: u32) {
        if let Some((current, _font)) = &self.font
            && *current == dpi
        {
            return;
        }
        if let Some(font) = crate::popup::message_font(dpi) {
            let handle = font.as_font();
            self.model.borrow_mut().font = Some(handle);
            self.font = Some((dpi, font));
        }
    }
}

/// The clock of the animation.
fn now_ms() -> u64 {
    // SAFETY: a plain read of the tick count.
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() }
}
/// The glow on the left of the screen: a band of blue light with a lens in the middle.
fn paint_glow(surface: &mut LayeredSurface, model: &Model) {
    let mouth = model.mouth;
    let origin = model.glow_window;
    let Some(mut canvas) = surface.canvas() else {
        return;
    };
    draw_glow(&mut canvas, mouth, origin);
}

/// The picture of the glow: the band of light around the mouth and the lens in the middle of it.
///
/// The mouth is drawn **inside** the window: the position of the mouth on the screen is turned into a position inside
/// the picture first. Without that the lens would be drawn outside of the window, and the mouth would not be seen.
fn draw_glow(canvas: &mut Canvas, mouth: Mouth, origin: Rect) {
    let centre_x = (mouth.x - origin.left) as f64;
    let centre_y = (mouth.y - origin.top) as f64;
    let radius = mouth.radius as f64;
    {
        // the band of light: every pixel is a little further from the mouth than the one above it
        for y in 0..canvas.height {
            for x in 0..canvas.width {
                let dx = x as f64 + 0.5 - centre_x;
                let dy = y as f64 + 0.5 - centre_y;
                let distance = (dx * dx + dy * dy).sqrt();
                let profile = glow_profile(distance, radius * 3.4);
                if profile <= 0.0 {
                    continue;
                }
                let t = (distance / (radius * 3.4)).min(1.0);
                let colour = mix(
                    LaserTheme::GLOW_CORE,
                    mix(LaserTheme::GLOW_MID, LaserTheme::GLOW_EDGE, t),
                    t,
                );
                canvas.blend(x, y, colour, profile * 0.5);
            }
        }
        // the lens: a bright ring of the brand rainbow around the mouth. The user said the mouth could not be seen
        // ("现在发光口看不见"), so the ring and the core are drawn strong: the mouth is the handle of the light, and
        // a handle that cannot be seen cannot be found.
        for y in 0..canvas.height {
            for x in 0..canvas.width {
                let dx = (x as f64 + 0.5 - centre_x) / radius;
                let dy = (y as f64 + 0.5 - centre_y) / radius;
                let distance = (dx * dx + dy * dy).sqrt();
                let band = (1.0 - (distance - 1.0).abs() * 1.6).clamp(0.0, 1.0);
                if band <= 0.0 {
                    continue;
                }
                let angle = dy.atan2(dx).rem_euclid(std::f64::consts::TAU)
                    / std::f64::consts::TAU;
                let colour = spline_sample(&BRAND_RAINBOW, angle);
                canvas.blend(x, y, colour, band);
            }
        }
        // the core of the lens: the pale blue of the light, bright enough to be found in a dark page
        canvas.fill_circle(centre_x, centre_y, radius * 0.62, LaserTheme::GLOW_CORE, 0.85);
        // and a small bright point in the middle of it, so that the eye finds it at once
        canvas.fill_circle(centre_x, centre_y, radius * 0.26, LaserTheme::BEAM_CORE, 0.95);
    }
}

/// The beam that shoots out of the mouth.
fn paint_beam(surface: &mut LayeredSurface, model: &Model) {
    let Some(beam) = model.beam else {
        return;
    };
    let origin = model.beam_window;
    let from_x = (beam.from.0 - origin.left) as f64;
    let from_y = (beam.from.1 - origin.top) as f64;
    let target_x = (beam.to.0 - origin.left) as f64;
    let target_y = (beam.to.1 - origin.top) as f64;
    let progress = beam.progress(model.now_ms);
    let strength = beam.strength(model.now_ms);
    if strength <= 0.0 {
        return;
    }
    let Some(mut canvas) = surface.canvas() else {
        return;
    };
    canvas.fill_beam(from_x, from_y, target_x, target_y, progress, strength);
    // the light that hits the character
    if progress >= 1.0 {
        canvas.fill_glow(target_x, target_y, 26.0, strength * 0.8);
    }
}

/// The panel with the settings.
fn paint_panel(surface: &mut LayeredSurface, model: &Model) {
    let panel = &model.panel;
    if !panel.open {
        return;
    }
    let reveal = panel.reveal;
    let font = model.font;
    {
        let Some(mut canvas) = surface.canvas() else {
            return;
        };
        let height = canvas.height as f64;
        // the glass of the panel
        let rect = PixelRect::new(0, 0, canvas.width, canvas.height);
        canvas.fill_shadow(
            rect,
            16,
            LaserTheme::PAGE,
            pime_core::style::Shadow {
                blur: 18,
                offset_x: 0,
                offset_y: 6,
                strength: 0.45,
            },
        );
        canvas.fill_rounded(rect, 16, LaserTheme::PANEL, LaserTheme::MATERIAL_ALPHA);
        // The rim of the brand rainbow along the top: a **gradient**, not a stripe. The whole rainbow runs across
        // the edge of the panel and fades out downwards, clipped by the rounded outline of the panel.
        canvas.fill_rainbow_edge(rect, 16, PANEL_RAINBOW_BAND, &BRAND_RAINBOW, 0.8);
        // the rows: the light wipes them open from the top
        let scan = reveal * height;
        for (index, (item, rect)) in panel.layout.rows.iter().enumerate() {
            let row_y = rect.top as f64 + rect.height() as f64 * 0.5;
            if row_y > scan {
                break;
            }
            let hot = panel.hot == Some(index);
            if hot {
                canvas.fill_rounded(
                    PixelRect::new(
                        rect.left - 6,
                        rect.top - 2,
                        rect.right + 6,
                        rect.bottom + 2,
                    ),
                    8,
                    LaserTheme::PANEL_LIT,
                    0.9,
                );
            }
            // the switch of a toggle
            if let PanelItem::Toggle { on, .. } = item {
                let switch_right = rect.right - 4;
                let switch_left = switch_right - 34;
                let centre_y = row_y;
                let shape = PixelRect::new(
                    switch_left,
                    centre_y as i32 - 9,
                    switch_right,
                    centre_y as i32 + 9,
                );
                canvas.fill_rounded(shape, 9, LaserTheme::PAGE, 0.85);
                let knob_x = if *on { switch_right - 9 } else { switch_left + 9 };
                let knob = if *on {
                    LaserTheme::ACCENT
                } else {
                    LaserTheme::DIM
                };
                canvas.fill_circle(knob_x as f64, centre_y, 6.5, knob, 0.95);
                if *on {
                    canvas.fill_glow(knob_x as f64, centre_y, 14.0, 0.5);
                }
            }
        }
    }
    // the text of the rows (GDI: there is no other way to draw a glyph)
    if let Some(font) = font {
        let layout = panel.layout.clone();
        for (index, (item, rect)) in layout.rows.iter().enumerate() {
            let row_y = rect.top as f64 + rect.height() as f64 * 0.5;
            let scan = reveal * PANEL_HEIGHT as f64;
            if row_y > scan {
                break;
            }
            let hot = panel.hot == Some(index);
            let colour = if hot {
                LaserTheme::ACCENT
            } else {
                LaserTheme::TEXT
            };
            surface.draw_text(font, colour, rect.left, rect.top + 6, item.label());
            if let PanelItem::Toggle { .. } = item {
                // the switch carries its own state; the text is only the name of it
            } else {
                let value = item.value();
                let width = crate::popup::GdiMetrics::new(font)
                    .map(|metrics| metrics.text_width(&value))
                    .unwrap_or(0);
                surface.draw_text(
                    font,
                    LaserTheme::DIM,
                    rect.right - width,
                    rect.top + 6,
                    &value,
                );
            }
            let _ = index;
        }
    }
}

/// The logo in the corner: the rainbow of the brand, breathing.
fn paint_logo(surface: &mut LayeredSurface, model: &Model) {
    let pulse = model.logo.pulse;
    let breathe = 0.85 + 0.15 * (pulse * std::f64::consts::TAU).sin();
    let Some(mut canvas) = surface.canvas() else {
        return;
    };
    let centre = canvas.width as f64 / 2.0;
    // the halo
    canvas.fill_glow(centre, centre, canvas.width as f64 * 0.55, 0.35 * breathe);
    // the disc of the brand rainbow
    let radius = canvas.width as f64 * 0.30;
    for y in 0..canvas.height {
        for x in 0..canvas.width {
            let dx = x as f64 + 0.5 - centre;
            let dy = y as f64 + 0.5 - centre;
            let distance = (dx * dx + dy * dy).sqrt();
            let cover = coverage(distance - radius);
            if cover <= 0.0 {
                continue;
            }
            let angle = dy.atan2(dx).rem_euclid(std::f64::consts::TAU) / std::f64::consts::TAU;
            let colour = spline_sample(&BRAND_RAINBOW, angle);
            canvas.blend(x, y, colour, cover * breathe);
        }
    }
    // the bright core
    canvas.fill_circle(centre, centre, radius * 0.42, LaserTheme::GLOW_CORE, 0.65 * breathe);
}

/// The colour of the light, in one place (the core has the mix already).
#[allow(dead_code)]
fn light_colour(t: f64) -> Rgb {
    mix(LaserTheme::GLOW_MID, LaserTheme::GLOW_EDGE, t)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lens of the mouth has to be drawn **inside** the window of the glow.
    ///
    /// The window is small and it follows the mouth, so the picture is drawn in the coordinates of the window. When
    /// the position of the mouth on the screen was used, the lens was drawn outside of the picture and the mouth was
    /// not seen at all (the user wrote "现在发光口看不见").
    #[test]
    fn the_lens_of_the_mouth_is_drawn_inside_the_window_of_the_glow() {
        let work = Rect::new(0, 0, 1920, 1040);
        // the mouth on the top edge, far away from the left edge: the case in which it was not seen
        let place = MouthPlace {
            edge: Edge::Top,
            offset: 960,
        };
        let mouth = mouth_at(work, place, MOUTH_RADIUS);
        let origin = Rect::new(
            mouth.x - GLOW_SIZE / 2,
            mouth.y - GLOW_SIZE / 2,
            mouth.x + GLOW_SIZE / 2,
            mouth.y + GLOW_SIZE / 2,
        );
        let mut pixels = vec![0u8; (GLOW_SIZE * GLOW_SIZE * 4) as usize];
        {
            let mut canvas = Canvas::new(GLOW_SIZE, GLOW_SIZE, &mut pixels)
                .expect("the picture of the window fits into the buffer");
            draw_glow(&mut canvas, mouth, origin);
        }
        // the middle of the picture is the middle of the lens: it must be bright there
        let middle = ((GLOW_SIZE / 2) * GLOW_SIZE + GLOW_SIZE / 2) as usize * 4;
        let alpha = pixels[middle + 3];
        assert!(alpha > 200, "the middle of the lens must be bright, it was {alpha}");
        // and the corner of the picture is far away from the mouth: there is no light there
        let corner = 3;
        assert!(
            pixels[corner] < 80,
            "the corner of the window must stay dark, it was {}",
            pixels[corner]
        );
    }
}
