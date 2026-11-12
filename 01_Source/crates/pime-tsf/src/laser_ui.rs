//! The laser interface: the mouths of light on the edges of the screen, the beams that shoot out of them when a
//! character is written, and the panel that opens when the mouse rests on the mouth of the human.
//!
//! There is **one mouth per source of input**: the human at the keyboard has the pale blue one the user asked for,
//! and every agent that writes into the document has a mouth of its own, in a colour that tells the agents apart.
//! Whoever writes, the beam comes out of **that** mouth and has **that** colour, so the user sees at once which agent
//! is working and which one is waiting.
//!
//! The windows are the ones of `glass`; this module owns them, draws them and runs the small state machine that
//! connects them:
//!
//! * the mouths are always there while the input method has a document;
//! * a character that is written makes the mouth of its source fire a **beam** at it, which lights the character up
//!   for half a second, so the eye sees where the text appeared;
//! * the mouse that rests on the mouth of the **human** opens the **panel** with the settings that are used most
//!   often (the panel belongs to the human: the switches in it are the ones of the keyboard).
//!
//! There is **no logo window**: the user asked for the spherical rainbow in the corner to be gone and for the light
//! on the edge of the screen to be the only sign of the input method (the mouth is the handle, and it opens the same
//! panel).
//!
//! Nothing here takes the focus: the user is typing in another program, and the input method must not take the
//! caret away from it.
//!
//! The work is split in three files: this one is the interface itself (the state, the windows and the events), the
//! painters live in [`paint`] (the glow, the beam and the panel) and the hit tests live in [`hit`] (what is under
//! the mouse).

mod hit;
mod paint;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::glass::{
    GlassEvent, GlassWindow, Model, MouthState, Role, Sink, WindowKind, ensure_surface,
    window_position,
};
use crate::laser::{
    Beam, Edge, HOVER_CLOSE_MS, HOVER_OPEN_MS, Mouth, MouthPlace, PanelAction, PanelCandidate, PanelItem,
    PanelLayout, mouth_at, panel_beside, panel_place, place_at,
};
use crate::layout::Rect;
use crate::mouths::{MOUTH_RADIUS, MouthRegistry, MouthSource, USER_KEY, USER_LABEL};
use crate::popup::GdiObject;
use crate::ui::Anchor;

/// How wide the window of the mouth is.
///
/// It is a square around the lens and not a tall band any more: the mouth can be on the left, on the top or on the
/// right, and the light around it follows it there. It only has to hold the slit of light (26 pixels) and the halo
/// around it (5 pixels on each side), so it is small: every pixel of it is painted again on every tick.
const GLOW_SIZE: i32 = 40;

/// How tall the band at the top of the panel is. It is the handle of the panel: the panel is dragged by it.
const PANEL_HANDLE: i32 = 18;

/// How wide the panel is.
const PANEL_WIDTH: i32 = 268;

/// How many rows the panel of the laser interface has.
///
/// The panel is the quick way into the settings: the rows are the ones of `tip::laser_items` (the mode, the space
/// key, the composition, the scheme of the keys, the language, the effort, the skin, the symbol table, the date and
/// the time, the clipboard, the demonstration and the settings). The height of the panel follows this number, so
/// the two cannot drift apart.
pub const PANEL_ROWS: usize = 12;

/// How tall the panel is (the header, the twelve rows and the room the light needs around them).
const PANEL_HEIGHT: i32 = 416;

/// The room under the last row of the panel.
const PANEL_BOTTOM: i32 = 38;

/// How tall the panel is for the rows it shows.
///
/// The rows decide the height: the settings are nine of them, the list of the candidates is as many as the
/// page holds (up to nine), and the panel must not be taller than what it shows.
fn panel_height(rows: usize) -> i32 {
    PANEL_HANDLE + rows as i32 * PANEL_ROW_HEIGHT + PANEL_BOTTOM
}

/// What the panel shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PanelMode {
    /// The settings that are used most often.
    Settings,
    /// The candidates of the list.
    Candidates,
}

/// How tall a row of the panel is.
const PANEL_ROW_HEIGHT: i32 = 30;

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
    /// The row of the demonstration was clicked: ten agents write their own domain into the document.
    Demo,
    /// The mouth of the glow was clicked: the input method switches (the user is in another one and wants to
    /// write code, or wants to go back to the one they came from).
    Switch,
    /// The mouth of the light was dragged to another place on the screen.
    MouthMoved { edge: Edge, offset: i32 },
    /// A candidate of the list was clicked in the panel (the index is the one of the row).
    Candidate { index: usize },
}

/// The laser interface of one thread.
pub struct LaserUi {
    /// The handle to itself, so that the windows can call back into it (the handle is weak, so the windows do not
    /// keep the interface alive).
    weak: std::rc::Weak<RefCell<LaserUi>>,
    /// The state the windows share.
    model: Rc<RefCell<Model>>,
    /// The windows of the glow: one per mouth of light (the human's and the ones of the agents).
    glows: Vec<Glow>,
    /// The window of the beam.
    beam: Option<GlassWindow>,
    /// The window of the panel.
    panel: Option<GlassWindow>,
    /// The registry of the mouths: every source gets its own colour and its own place.
    registry: MouthRegistry,
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
    /// The rows of the panel (the settings that are used most often).
    items: Vec<PanelItem>,
    /// The one line the panel shows under its rows (what the last action did).
    status: Option<String>,
    /// The candidates of the list (the panel shows them instead of the settings while they are there).
    candidates: Vec<PanelCandidate>,
    /// What the panel shows.
    panel_mode: PanelMode,
    /// How tall the panel is (the rows decide it).
    panel_height: i32,
    /// The work area the windows live in.
    work: Rect,
    /// Whether the interface is on the screen.
    visible: bool,
    /// Where the mouth of the human is on the screen (the edge it is on and how far along it). `None` is the
    /// default place: the left edge, in the middle of the screen.
    place: Option<MouthPlace>,
    /// The drag that is going on right now.
    drag: Option<Drag>,

    /// Whether the picture of the panel has to be drawn again.
    ///
    /// The panel is the expensive window (268 by 326 pixels and an 18 pixel shadow blur), so its picture is only
    /// drawn again when something in it really changed. D-27 measured a whole core of CPU before this.
    panel_dirty: bool,
    /// Whether the picture of the beam has to be drawn again.
    beam_dirty: bool,

    /// That the interface has to go off the screen, asked for while it was busy.
    ///
    /// The switch of the input method happens in the callback of the mouth, and TSF deactivates the text service
    /// inside that very call: the interface is borrowed at that moment, so it cannot be asked to hide itself. The
    /// flag is set instead, and the windows go off the screen as soon as the callback returns.
    hide_requested: Rc<Cell<bool>>,
}

/// One window of the glow: the window of the mouth of one source.
struct Glow {
    /// The key of the source whose mouth this window draws.
    key: String,
    /// The window itself.
    window: GlassWindow,
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
                glows: Vec::new(),
                beam: None,
                panel: None,
                registry: MouthRegistry::new(),
                font: None,
                sink,
                hover_since: None,
                on_mouth: false,
                on_panel: false,
                left_at: None,
                items: Vec::new(),
                status: None,
                candidates: Vec::new(),
                panel_mode: PanelMode::Settings,
                panel_height: PANEL_HEIGHT,
                work: Rect::new(0, 0, 0, 0),
                visible: false,
                place: None,
                drag: None,
                panel_dirty: true,
                beam_dirty: true,
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

    /// The candidates the panel is showing right now (the rows of the list).
    pub fn candidate_rows(&self) -> Vec<PanelCandidate> {
        self.candidates.clone()
    }

    /// Whether the panel is on the screen right now.
    pub fn panel_is_open(&self) -> bool {
        self.model.borrow().panel.open
    }

    /// Close the panel unless it is the list of candidates.
    ///
    /// This is what the user asked for on 2026-11-11: the floating window goes away when he clicks somewhere
    /// else. The click itself is not ours to see (it belongs to the window under the panel), but the next thing
    /// the user does with the keyboard is: the panel is then in the way of the code, so it goes.
    pub fn close_panel_unless_list(&mut self) {
        if self.panel_mode == PanelMode::Candidates {
            return;
        }
        if self.model.borrow().panel.open {
            self.close_panel();
        }
    }

    /// The panel and the list go away; the light stays.
    ///
    /// This is what a switch of the **language** does (the user reported on 2026-11-12: 「切换语言和输入法也没有
    /// 正确随之消失」): the candidates of the old language must not stay on the screen, but the input method is
    /// still the one the user is in, so the mouth of the light stays.
    pub fn close_panel_and_list(&mut self) {
        self.candidates.clear();
        self.panel_mode = PanelMode::Settings;
        self.refresh_panel_layout();
        self.close_panel();
        self.repaint_all();
    }

    /// Show the candidates of the list in the panel.
    ///
    /// The panel is the list: it opens by itself while there are candidates (the user asked for the options to
    /// show in the code input too — 2026-11-12: 「代码输入模式现在不显示备选选项」). **What decides whether the
    /// list takes the keys is the mode** (the natural language input takes the digits, the Tab key and Space; the
    /// code input leaves them to the editor), not whether the panel is open.
    pub fn set_candidates(&mut self, candidates: Vec<PanelCandidate>) {
        self.candidates = candidates;
        self.panel_mode = PanelMode::Candidates;
        self.refresh_panel_layout();
        if self.panel.is_some() {
            self.open_panel();
        }
    }

    /// The list is gone: the panel shows the settings again.
    pub fn clear_candidates(&mut self) {
        if self.candidates.is_empty() && self.panel_mode == PanelMode::Settings {
            return;
        }
        self.candidates.clear();
        self.panel_mode = PanelMode::Settings;
        self.refresh_panel_layout();
        // the panel was opened by the list: the mouse is not on it, so it goes away
        if !self.on_panel && !self.on_mouth {
            self.close_panel();
        }
        self.repaint_all();
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
            self.sync_mouths();
            self.repaint_all();
        }
    }

    /// Put the interface on the screen (the work area of the monitor the caret is on).
    pub fn show(&mut self, work: Rect) {
        self.hide_requested.set(false);
        self.work = work;
        self.visible = true;
        self.ensure_windows();
        self.sync_mouths();
        self.start_timer();
        self.ensure_font_for(work);
        self.repaint_all();
    }

    /// The font of the panel, for the DPI of the monitor the interface is on.
    fn ensure_font_for(&mut self, work: Rect) {
        let Some(glow) = self.glows.first() else {
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
        let dpi = crate::popup::dpi_for(monitor, glow.window.hwnd());
        self.ensure_font(dpi);
    }

    /// The rectangle of a row of the panel, in screen coordinates (the menu of that row opens under it).
    pub fn row_rect(&self, key: &str) -> Option<Rect> {
        let model = self.model.borrow();
        if !model.panel.open {
            return None;
        }
        let origin = model.panel.rect;
        hit::row_rect(&model.panel.layout, origin, key)
    }

    /// Start the timer that drives the animation (the window procedure of the glow asks for the tick).
    fn start_timer(&self) {
        let Some(glow) = self.glows.first() else {
            return;
        };
        // SAFETY: a timer on our own window; the id is ours.
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SetTimer(
                glow.window.hwnd(),
                TIMER_ID,
                TICK_MS,
                None,
            );
        }
    }

    /// Stop the timer.
    fn stop_timer(&self) {
        let Some(glow) = self.glows.first() else {
            return;
        };
        // SAFETY: a timer on our own window; the id is ours.
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(glow.window.hwnd(), TIMER_ID);
        }
    }

    /// Take the interface off the screen.
    ///
    /// The mouths of the agents go away with it: the input method is not there any more, so nobody is writing and
    /// the light of the agents has nothing to say. The mouth of the human comes back with the interface.
    ///
    /// The windows of the glow are only **hidden** here: this can happen inside the callback of one of them (the
    /// switch of the input method happens in the callback of the mouth), and a window must not be destroyed while it
    /// is answering. The windows of the mouths that are gone are destroyed when the interface comes back.
    pub fn hide(&mut self) {
        self.hide_requested.set(false);
        self.visible = false;
        self.stop_timer();
        self.close_panel();
        self.registry.clear();
        self.model.borrow_mut().mouths.clear();
        for glow in &self.glows {
            glow.window.hide();
        }
        for window in [self.beam.as_ref(), self.panel.as_ref()]
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

    /// Fire the beam at a character that the **human** just wrote.
    ///
    /// The beam comes out of the mouth of the human (the pale blue one) and has its colour.
    pub fn fire(&mut self, anchor: Anchor) {
        self.fire_for(USER_KEY, USER_LABEL, anchor);
    }

    /// Fire the beam at a character that **a source** just wrote.
    ///
    /// The beam comes out of the mouth of that source and has the colour of that source: the user sees at once
    /// which agent wrote the character. A source that has no mouth yet gets one (the registry gives it a colour and
    /// a place).
    pub fn fire_for(&mut self, source_key: &str, source_label: &str, anchor: Anchor) {
        if !self.visible {
            return;
        }
        let work = self.work;
        let place = self.place.unwrap_or_else(|| MouthPlace::default_in(work));
        // the mouth of the source (it is made here when the source is new)
        let source = self.registry.obtain(source_key, source_label, work, place);
        self.fire_from(source, anchor);
    }

    /// Fire the beam of a source whose mouth is at a given point of the screen.
    ///
    /// The demonstration puts the mouths of its agents in the middle of the screen: their beams fly across
    /// it instead of along its border.
    pub fn fire_for_at(&mut self, source_key: &str, source_label: &str, anchor: Anchor, x: i32, y: i32) {
        if !self.visible {
            return;
        }
        let work = self.work;
        let place = self.place.unwrap_or_else(|| MouthPlace::default_in(work));
        // the mouth of the source, at the point the demonstration chose for it
        let source = self
            .registry
            .obtain_at(source_key, source_label, work, place, x, y);
        self.fire_from(source, anchor);
    }

    /// The beam from the mouth of a source to the character it wrote.
    fn fire_from(&mut self, source: MouthSource, anchor: Anchor) {
        // the windows of the mouths follow the registry
        self.sync_mouths();
        let now = now_ms();
        let beam = Beam::new(
            (source.mouth.x, source.mouth.y),
            (anchor.x, anchor.y),
            now,
            source.colour,
        );
        // the window of the beam covers the whole way of the beam
        let left = source.mouth.x.min(anchor.x) - BEAM_MARGIN;
        let top = source.mouth.y.min(anchor.y) - BEAM_MARGIN;
        let right = source.mouth.x.max(anchor.x) + BEAM_MARGIN;
        let bottom = source.mouth.y.max(anchor.y) + BEAM_MARGIN;
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
        // only the beam (and the mouths) changed: the panel keeps its picture
        self.beam_dirty = true;
        self.repaint_changed();
    }

    /// The mouths of light that are on the screen right now (the human and every agent).
    pub fn mouths(&self) -> Vec<MouthState> {
        self.model.borrow().mouths.clone()
    }

    /// The work area the interface lives in (the monitor the caret is on).
    pub fn work_area(&self) -> Rect {
        self.work
    }

    /// The one line the panel shows under its rows: what the last action did.
    ///
    /// The panel is the only place the text service can speak to the user while it works, so the result of
    /// the demonstration is told there.
    pub fn set_status(&mut self, status: Option<&str>) {
        let status = status.map(|text| text.to_string());
        if self.status == status {
            return;
        }
        self.status = status;
        self.refresh_panel_layout();
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
                // the beam is gone: its picture has to be wiped once
                self.beam_dirty = true;
            }
            // the panel wipes itself open
            if model.panel.open && model.panel.reveal < 1.0 {
                model.panel.reveal = (model.panel.reveal + 0.14).min(1.0);
                // the wipe is still running: its picture has to be drawn again
                self.panel_dirty = true;
            }
        }
        // the mouse rested on the mouth long enough: the panel opens
        if self.on_mouth && !self.model.borrow().panel.open {
            let since = self.hover_since.unwrap_or(now);
            if now.saturating_sub(since) >= HOVER_OPEN_MS {
                self.open_panel();
            }
        }
        // the mouse left the panel: it closes after a moment - but not while the list is there, for the panel
        // **is** the list
        if !self.on_panel
            && !self.on_mouth
            && self.panel_mode == PanelMode::Settings
            && self.model.borrow().panel.open
        {
            let left_at = self.left_at.unwrap_or(now);
            if now.saturating_sub(left_at) >= HOVER_CLOSE_MS {
                self.close_panel();
            }
        }
        self.repaint_changed();
    }

    /// What the user did in one of the windows.
    ///
    /// The **key** is the source the window belongs to: every mouth of light has a window of its own, and the mouse
    /// that is on the mouth of an agent is not the mouse that is on the mouth of the human.
    pub fn handle(&mut self, role: Role, key: &str, event: GlassEvent) {
        match event {
            GlassEvent::Move { x, y } => self.on_move(role, key, x, y),
            GlassEvent::Leave => self.on_leave(role),
            GlassEvent::Press { x, y } => self.on_press(role, key, x, y),
            GlassEvent::Drag { x, y } => self.on_drag(role, key, x, y),
            GlassEvent::Release {
                x,
                y,
                screen_x,
                screen_y,
            } => self.on_release(role, key, x, y, screen_x, screen_y),
            GlassEvent::Tick => self.tick(),
        }
    }

    /// The mouse was pressed in one of the windows.
    fn on_press(&mut self, role: Role, key: &str, x: i32, y: i32) {
        match role {
            Role::Glow => {
                // Only the mouth of the human is a handle: the mouths of the agents are light, not something the
                // user drags.
                if key != USER_KEY {
                    return;
                }
                let origin = self.window_origin(role, key);
                let Some(mouth) = self.mouth_of(key) else {
                    return;
                };
                let (sx, sy) = (origin.0 + x, origin.1 + y);
                if hit::on_the_lens(&mouth, sx, sy) {
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
                    let origin = self.window_origin(role, key);
                    self.drag = Some(Drag::Panel {
                        dx: origin.0 + x,
                        dy: origin.1 + y,
                    });
                }
            }
            Role::Beam => {}
        }
    }

    /// The mouse is being dragged: the mouth or the panel follows it.
    fn on_drag(&mut self, role: Role, key: &str, x: i32, y: i32) {
        let Some(drag) = self.drag else {
            return;
        };
        let work = self.work;
        match drag {
            Drag::Mouth { dx, dy, .. } => {
                if role != Role::Glow || key != USER_KEY {
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
                    // the slit of light follows the edge the mouth sits on
                    edge: self.place.map(|place| place.edge).unwrap_or(Edge::Left),
                };
                self.set_user_mouth(mouth);
                self.repaint_all();
            }
            Drag::Panel { dx, dy } => {
                if role != Role::Panel {
                    return;
                }
                let low_x = work.left;
                let high_x = (work.right - PANEL_WIDTH).max(low_x);
                let low_y = work.top;
                let high_y = (work.bottom - self.panel_height).max(low_y);
                let nx = (x - dx).clamp(low_x, high_x);
                let ny = (y - dy).clamp(low_y, high_y);
                self.model.borrow_mut().panel.rect =
                    Rect::new(nx, ny, nx + PANEL_WIDTH, ny + self.panel_height);
                if let Some(panel) = &self.panel {
                    panel.move_to(nx, ny);
                }
                self.repaint_all();
            }
        }
    }

    /// The button was let go: a drag ends, or a click does what it does.
    fn on_release(&mut self, role: Role, key: &str, x: i32, y: i32, screen_x: i32, screen_y: i32) {
        let Some(drag) = self.drag.take() else {
            // No drag went on: this is a click.
            self.on_click(role, key, x, y);
            return;
        };
        match drag {
            Drag::Mouth {
                dx,
                dy,
                start_x,
                start_y,
            } => {
                let Some(mouth) = self.mouth_of(USER_KEY) else {
                    return;
                };
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
                self.set_user_mouth(mouth);
                // The panel that hangs on the mouth goes with it: otherwise it would stay behind on the old edge.
                let panel_open = self.model.borrow().panel.open;
                if panel_open {
                    self.place_panel();
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
    fn on_move(&mut self, role: Role, key: &str, x: i32, y: i32) {
        match role {
            Role::Glow => {
                // Only the mouth of the human opens the panel: the switches in it belong to the keyboard.
                if key != USER_KEY {
                    return;
                }
                let origin = self.window_origin(role, key);
                let Some(mouth) = self.mouth_of(key) else {
                    return;
                };
                let on = hit::on_the_light(&mouth, origin.0 + x, origin.1 + y);
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
                let hot = hit::row_under(&layout, x, y);
                self.model.borrow_mut().panel.hot = hot;
                // the highlighted row changed: the picture of the panel has to be drawn again
                self.panel_dirty = true;
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
            Role::Beam => {}
        }
    }

    /// The user clicked in one of the windows.
    fn on_click(&mut self, role: Role, key: &str, x: i32, y: i32) {
        match role {
            Role::Panel => {
                let action = {
                    let layout = self.model.borrow().panel.layout.clone();
                    hit::action_under(&layout, x, y)
                };
                if let Some(action) = action {
                    self.apply(action);
                }
            }
            Role::Glow => {
                // The mouth itself is handled by the drag (a click on it is the switch, a drag of it moves it).
                // A click on the light around the mouth opens the panel - the panel of the human, so only the mouth
                // of the human opens it.
                if key == USER_KEY {
                    self.open_panel();
                }
            }
            Role::Beam => {}
        }
    }

    /// Tell the owner that a row of the panel was used.
    fn apply(&mut self, action: PanelAction) {
        match action {
            PanelAction::Toggle { key, on } => (self.sink)(LaserEvent::Toggle { key, on }),
            PanelAction::Menu { key } => (self.sink)(LaserEvent::Menu { key }),
            PanelAction::Demo => (self.sink)(LaserEvent::Demo),
            PanelAction::Settings => (self.sink)(LaserEvent::Settings),
            PanelAction::Candidate { index } => (self.sink)(LaserEvent::Candidate { index }),
        }
    }

    /// Open the panel next to the mouth.
    pub fn open_panel(&mut self) {
        if self.panel.is_none() {
            return;
        }
        {
            let mut model = self.model.borrow_mut();
            model.panel.open = true;
            model.panel.reveal = 0.0;
        }
        self.place_panel();
        self.on_panel = true;
        self.left_at = None;
        if let Some(panel) = &self.panel {
            panel.show();
        }
        self.panel_dirty = true;
        self.repaint_changed();
    }

    /// Close the panel.
    fn close_panel(&mut self) {
        self.model.borrow_mut().panel.open = false;
        if let Some(panel) = &self.panel {
            panel.hide();
        }
        self.panel_dirty = true;
    }

    /// The screen position of one of the windows.
    ///
    /// Every mouth of light has a window of its own, so the key of the source is needed to find the window of a
    /// glow.
    fn window_origin(&self, role: Role, key: &str) -> (i32, i32) {
        let window = match role {
            Role::Glow => self
                .glows
                .iter()
                .find(|glow| glow.key == key)
                .map(|glow| &glow.window),
            Role::Beam => self.beam.as_ref(),
            Role::Panel => self.panel.as_ref(),
        };
        match window {
            Some(window) => window_position(window.hwnd()),
            None => (0, 0),
        }
    }

    /// The mouth of a source, in screen coordinates.
    fn mouth_of(&self, key: &str) -> Option<Mouth> {
        self.model.borrow().mouth_of(key).map(|mouth| mouth.mouth)
    }

    /// Create the windows (once).
    ///
    /// The windows of the glow are not made here: there is one per mouth of light, and a mouth appears when its
    /// source writes for the first time. They are made by `sync_mouths`.
    fn ensure_windows(&mut self) {
        if self.beam.is_some() && self.panel.is_some() {
            return;
        }
        // The windows call back into the interface through a weak handle: the interface owns the windows, so a strong
        // handle here would be a cycle that nothing would ever break.
        let weak = self.weak.clone();
        let hide_requested = Rc::clone(&self.hide_requested);
        let sink: Sink = Rc::new(move |role, key, event| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if let Ok(mut ui) = ui.try_borrow_mut() {
                ui.handle(role, key, event);
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
        self.beam = GlassWindow::create(
            WindowKind::beam(),
            16,
            16,
            Rc::clone(&model),
            paint::paint_beam,
            Rc::clone(&sink),
        );
        self.panel = GlassWindow::create(
            WindowKind::panel(),
            PANEL_WIDTH,
            PANEL_HEIGHT,
            Rc::clone(&model),
            paint::paint_panel,
            Rc::clone(&sink),
        );
        self.refresh_panel_layout();
    }

    /// Make the window of a mouth of light (once per source).
    fn create_glow(&mut self, key: &str) -> Option<Glow> {
        let weak = self.weak.clone();
        let hide_requested = Rc::clone(&self.hide_requested);
        let sink: Sink = Rc::new(move |role, key, event| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if let Ok(mut ui) = ui.try_borrow_mut() {
                ui.handle(role, key, event);
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
        let own = key.to_string();
        let window = GlassWindow::create(
            WindowKind::glow(&own),
            GLOW_SIZE,
            GLOW_SIZE,
            model,
            paint::paint_glow,
            sink,
        )?;
        Some(Glow { key: own, window })
    }

    /// Put the window of the glow on the mouth and remember where the window is on the screen.
    ///
    /// The painter of the glow works with the position of the mouth **inside** the window, so the place of the window
    /// has to be known. The window follows the mouth, and the mouth sits in its middle.
    fn move_glow(&mut self, key: &str, mouth: Mouth) {
        let left = mouth.x - GLOW_SIZE / 2;
        let top = mouth.y - GLOW_SIZE / 2;
        {
            let mut model = self.model.borrow_mut();
            if let Some(state) = model.mouths.iter_mut().find(|state| state.key == key) {
                state.mouth = mouth;
                state.window = Rect::new(left, top, left + GLOW_SIZE, top + GLOW_SIZE);
            }
        }
        if let Some(glow) = self.glows.iter().find(|glow| glow.key == key) {
            glow.window.move_to(left, top);
        }
    }

    /// The mouths of light, and the windows that draw them.
    ///
    /// The human always has a mouth (it is the handle of the panel); an agent gets one when it writes for the
    /// first time. The registry decides the colour and the place of every one of them.
    fn sync_mouths(&mut self) {
        let work = self.work;
        if work.width() <= 0 || work.height() <= 0 {
            return;
        }
        let place = self.place.unwrap_or_else(|| MouthPlace::default_in(work));
        // the mouth of the human: the place the user put it
        let user = self.registry.obtain(USER_KEY, USER_LABEL, work, place);
        let mut states = vec![mouth_state(&user)];
        for source in self.registry.sources() {
            if source.is_user {
                continue;
            }
            states.push(mouth_state(&source));
        }
        // the windows of the mouths that are gone go with them
        self.glows
            .retain(|glow| states.iter().any(|state| state.key == glow.key));
        // the windows of the new mouths
        for state in &states {
            if self.glows.iter().any(|glow| glow.key == state.key) {
                continue;
            }
            if let Some(glow) = self.create_glow(&state.key) {
                self.glows.push(glow);
            }
        }
        {
            let mut model = self.model.borrow_mut();
            model.mouths = states.clone();
        }
        // every window of a mouth follows its mouth
        for state in &states {
            self.move_glow(&state.key, state.mouth);
            if let Some(glow) = self.glows.iter().find(|glow| glow.key == state.key) {
                glow.window.show();
            }
        }
        self.refresh_panel_layout();
    }

    /// Put the mouth of the human where it is now (the user dragged it).
    fn set_user_mouth(&mut self, mouth: Mouth) {
        self.move_glow(USER_KEY, mouth);
    }

    /// The rows of the panel, with their places.
    fn refresh_panel_layout(&mut self) {
        let items: Vec<PanelItem> = match self.panel_mode {
            PanelMode::Settings => self.items.clone(),
            PanelMode::Candidates => self
                .candidates
                .iter()
                .cloned()
                .map(PanelItem::Candidate)
                .collect(),
        };
        let mut layout = PanelLayout::compute(&items, PANEL_ROW_HEIGHT, PANEL_HANDLE, PANEL_WIDTH - 24);
        let height = panel_height(layout.rows.len());
        self.panel_height = height;
        layout.status = self.status.clone();
        self.model.borrow_mut().panel.layout = layout;
        // the rows changed: the picture of the panel has to be drawn again
        self.panel_dirty = true;
        // the rows changed: the picture of the panel has to be drawn again
        self.panel_dirty = true;
        if height != PANEL_HEIGHT {
            // the window follows the picture: the picture is made again at the new size
            if let Some(panel) = &mut self.panel {
                panel.resize(PANEL_WIDTH, height);
            }
        }
        if self.model.borrow().panel.open {
            self.place_panel();
        }
    }

    /// Put the panel where it belongs: next to the mouth of the human.
    fn place_panel(&mut self) {
        let height = self.panel_height;
        let Some(mouth) = self.mouth_of(USER_KEY) else {
            return;
        };
        let place = self.place.unwrap_or_else(|| MouthPlace::default_in(self.work));
        let (x, y) = panel_beside(place, mouth, (PANEL_WIDTH, height), self.work);
        self.model.borrow_mut().panel.rect = Rect::new(x, y, x + PANEL_WIDTH, y + height);
        if let Some(panel) = &self.panel {
            panel.move_to(x, y);
        }
    }

    /// Draw every window that is on the screen (everything is marked as changed first).
    fn repaint_all(&mut self) {
        self.panel_dirty = true;
        self.beam_dirty = true;
        self.repaint_changed();
    }

    /// Draw the windows whose picture changed since the last time.
    ///
    /// The mouths are small (40 by 40) and they breathe, so they are drawn on every tick. The beam only lives for
    /// half a second and the panel only changes when it opens, when the list changes or when the mouse moves over
    /// a row. Before this, the panel was drawn 60 times a second (and once more for every character that was
    /// written), which cost a whole core: D-27.
    fn repaint_changed(&mut self) {
        for glow in &self.glows {
            ensure_surface(glow.window.shared(), glow.window.width(), glow.window.height());
            glow.window.repaint();
        }
        if self.beam_dirty {
            self.beam_dirty = false;
            if let Some(beam) = &self.beam {
                ensure_surface(beam.shared(), beam.width(), beam.height());
                beam.repaint();
            }
        }
        if self.panel_dirty {
            self.panel_dirty = false;
            if let Some(panel) = &self.panel {
                ensure_surface(panel.shared(), panel.width(), panel.height());
                panel.repaint();
            }
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

/// The state of a mouth of light, in the shape the painters read.
fn mouth_state(source: &MouthSource) -> MouthState {
    let left = source.mouth.x - GLOW_SIZE / 2;
    let top = source.mouth.y - GLOW_SIZE / 2;
    MouthState {
        key: source.key.clone(),
        label: source.label.clone(),
        colour: source.colour,
        halo: source.halo,
        mouth: source.mouth,
        window: Rect::new(left, top, left + GLOW_SIZE, top + GLOW_SIZE),
    }
}

/// The clock of the animation.
fn now_ms() -> u64 {
    // SAFETY: a plain read of the tick count.
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pime_core::style::{Canvas, LaserTheme, Rgb};

    /// The slit of light of the mouth has to be drawn **inside** the window of the glow.
    ///
    /// The window is small and it follows the mouth, so the picture is drawn in the coordinates of the window. When
    /// the position of the mouth on the screen was used, the light was drawn outside of the picture and the mouth was
    /// not seen at all (the user wrote "现在发光口看不见").
    ///
    /// The user then asked for the mouth to be "barely there, a small piece of pale blue, half transparent, not loud
    /// but visible" ("若隐若现有一小截淡蓝色半透明激光"), so the light is a hairline: the test pins down that it
    /// is lit, that it is **thin** (the pixel next to it is much fainter), that it is blue and that the rest of the
    /// window stays dark.
    #[test]
    fn the_mouth_is_a_thin_pale_line_inside_the_window_of_the_glow() {
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
            paint::draw_glow(
                &mut canvas,
                mouth,
                origin,
                0,
                LaserTheme::MOUTH_SLIT,
                LaserTheme::MOUTH_HALO,
            );
        }
        // the light is at the edge of the screen (2026-11-12), so inside the picture it is at the edge of the
        // window: the pixel of the line is the one of the slit, not the middle of the picture
        let slit = crate::laser::mouth_slit(&mouth);
        let local_x = (slit.x - origin.left) as usize;
        let local_y = (slit.y - origin.top) as usize;
        let middle = (local_y * GLOW_SIZE as usize + local_x) * 4;
        let alpha = pixels[middle + 3];
        assert!(alpha > 90, "the line of light must be lit, it was {alpha}");
        // the light is blue: the blue channel is the strongest one (this picture is red first)
        assert!(
            pixels[middle + 2] > pixels[middle],
            "the light is blue: b={} r={}",
            pixels[middle + 2],
            pixels[middle]
        );
        // and it is thin: the pixel next to the line (across it, not along it) is much fainter than the line itself
        let above = if slit.vertical {
            middle - 4
        } else {
            middle - (GLOW_SIZE as usize) * 4
        };
        assert!(
            pixels[above + 3] * 2 < alpha,
            "the light must be a hairline: the line is {alpha}, the pixel above it is {}",
            pixels[above + 3]
        );
        // and the corner of the picture is far away from the mouth: there is no light there
        let corner = 3;
        assert_eq!(
            pixels[corner + 3],
            0,
            "the corner of the window must stay dark, it was {}",
            pixels[corner + 3]
        );
    }

    /// The mouth of an agent is drawn in the colour of that agent: the user sees at once who is writing.
    ///
    /// The slit of light stays a hairline in the colour of its source (the human keeps the pale blue the user asked
    /// for), and the rest of the window stays dark.
    #[test]
    fn the_mouth_of_an_agent_is_drawn_in_the_colour_of_that_agent() {
        let work = Rect::new(0, 0, 1920, 1040);
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
        // the colour of an agent: a red that is far away from the pale blue of the human
        let colour = Rgb::from_hsv(20.0, 0.90, 0.95);
        let mut pixels = vec![0u8; (GLOW_SIZE * GLOW_SIZE * 4) as usize];
        {
            let mut canvas = Canvas::new(GLOW_SIZE, GLOW_SIZE, &mut pixels)
                .expect("the picture of the window fits into the buffer");
            paint::draw_glow(&mut canvas, mouth, origin, 0, colour, colour);
        }
        // the light is at the edge of the screen (2026-11-12), so inside the picture it is at the edge of the
        // window, not at its middle
        let slit = crate::laser::mouth_slit(&mouth);
        let local_x = (slit.x - origin.left) as usize;
        let local_y = (slit.y - origin.top) as usize;
        let middle = (local_y * GLOW_SIZE as usize + local_x) * 4;
        assert!(pixels[middle + 3] > 90, "the slit of light must be lit");
        // the light is in the colour of the agent: the red channel is the strongest one (this picture is red first)
        assert!(
            pixels[middle] > pixels[middle + 2],
            "the light is in the colour of the agent: r={} b={}",
            pixels[middle],
            pixels[middle + 2]
        );
        // and the corner of the picture is far away from the mouth: there is no light there
        let corner = 3;
        assert_eq!(
            pixels[corner + 3],
            0,
            "the corner of the window must stay dark, it was {}",
            pixels[corner + 3]
        );
    }

    /// The panel has to be tall enough for the rows it shows, and the number of rows is written down in one place.
    #[test]
    fn the_panel_is_tall_enough_for_the_rows_it_shows() {
        let items: Vec<PanelItem> = (0..PANEL_ROWS)
            .map(|index| PanelItem::Toggle {
                key: "k",
                label: "row",
                on: index % 2 == 0,
            })
            .collect();
        let layout = PanelLayout::compute(&items, PANEL_ROW_HEIGHT, PANEL_HANDLE, PANEL_WIDTH - 24);
        assert_eq!(layout.rows.len(), PANEL_ROWS);
        assert!(
            layout.height <= PANEL_HEIGHT,
            "the rows need {} px, the panel is {} px",
            layout.height,
            PANEL_HEIGHT
        );
        for (_, rect) in &layout.rows {
            assert!(rect.bottom <= PANEL_HEIGHT, "{rect:?}");
        }
    }

    /// An interface on the screen (the windows are real: the panel needs them to be able to open).
    fn on_screen() -> Rc<RefCell<LaserUi>> {
        let ui = LaserUi::create(Rc::new(|_: LaserEvent| {}));
        ui.borrow_mut().show(Rect::new(0, 0, 1920, 1040));
        ui
    }

    /// A row of the list.
    fn row() -> PanelCandidate {
        PanelCandidate {
            label: "1".to_string(),
            text: "fn".to_string(),
            detail: "rule".to_string(),
            selected: true,
        }
    }

    /// The list of candidates opens the panel: the options are shown while the user types (the user asked for
    /// that in the code input too — 2026-11-12: 「代码输入模式现在不显示备选选项」).
    #[test]
    fn the_list_of_candidates_opens_the_panel() {
        let ui = on_screen();
        ui.borrow_mut().set_candidates(vec![row()]);
        assert!(ui.borrow().panel_is_open(), "the panel is the list");
    }

    /// A switch of the language takes the panel and the list away (the user reported it on 2026-11-12): the
    /// candidates of the old language must not stay on the screen.
    #[test]
    fn a_switch_of_the_language_takes_the_panel_and_the_list_away() {
        let ui = on_screen();
        ui.borrow_mut().set_candidates(vec![row()]);
        assert!(ui.borrow().panel_is_open());
        ui.borrow_mut().close_panel_and_list();
        assert!(!ui.borrow().panel_is_open(), "the panel went away");
        assert!(ui.borrow().candidate_rows().is_empty(), "the list of the old language went away");
    }

    /// The panel is closed by default: it opens when the mouse rests on the mouth or when the light around it is
    /// clicked, and not before that.
    #[test]
    fn the_panel_is_closed_until_the_user_asks_for_it() {
        let ui = on_screen();
        assert!(!ui.borrow().panel_is_open(), "the panel does not show itself");
    }

    /// The user works in the application again (he clicked somewhere else and types): the panel of the settings goes
    /// away. The list of candidates stays, because it is the list.
    #[test]
    fn the_panel_goes_away_when_the_user_works_in_the_application_again() {
        let ui = on_screen();
        ui.borrow_mut().open_panel();
        assert!(ui.borrow().panel_is_open(), "the panel is open");
        ui.borrow_mut().close_panel_unless_list();
        assert!(!ui.borrow().panel_is_open(), "the panel went away");
        // the list is the panel: it stays
        let ui = on_screen();
        ui.borrow_mut().set_candidates(vec![row()]);
        assert!(ui.borrow().panel_is_open());
        ui.borrow_mut().close_panel_unless_list();
        assert!(ui.borrow().panel_is_open(), "the list stays");
    }
}
