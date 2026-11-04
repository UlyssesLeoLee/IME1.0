//! The contract between the text input processor (`tip`) and any candidate-window implementation (`popup`).
//! Pure data, one trait and the panic guard: no Windows API here, so the TIP logic is testable with [`RecordingUi`].
//!
//! Rules (design DD-02 / SRS NFR accessibility): a row is described by text (source, validation), never by colour
//! alone; the window never steals focus and never changes the host's text; the host is only changed by the TIP through
//! an edit session.

/// One visible candidate row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateRow {
    /// The key that picks it on this page: "1".."9".
    pub label: String,
    /// The text that would be inserted (single line preview; the TIP truncates very long snippets).
    pub text: String,
    /// Short description in words: source and validation, e.g. "rule · valid". Never empty.
    pub detail: String,
}

/// What the user (a click in our own window) or the host (a UI element that draws the list itself) asks the text
/// service to do with the candidate list.
///
/// The host asks through `ITfCandidateListUIElementBehavior` (`SetSelection`, `Finalize`, `Abort`); without that
/// interface a host that draws the list has no way to tell the text service what the user picked, and the click does
/// nothing. The user asks through a click in the native window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListAction {
    /// Move the highlight to this row of the current page (the host selected it; nothing is written).
    Select(usize),
    /// Take this row of the current page (the user clicked it): it is written to the document.
    Take(usize),
    /// Take the highlighted candidate (the host confirmed the selection).
    Finalize,
    /// Drop the list without taking anything (the host cancelled it).
    Abort,
}

/// How a window or a host hands an action to the text service. The sink runs on the thread that owns the candidate
/// window (the host's own thread), so it must not block and must not unwind into the host.
pub type ActionSink = std::rc::Rc<dyn Fn(ListAction)>;

/// A sink that drops every action: for the parts of the code that have no text service to talk to (the demo).
pub fn no_sink() -> ActionSink {
    std::rc::Rc::new(|_| {})
}

/// What the window shows right now (exactly one page).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateView {
    pub rows: Vec<CandidateRow>,
    /// Index of the highlighted row within `rows`.
    pub selected_in_page: usize,
    /// Zero-based page and total page count (page_count >= 1).
    pub page: usize,
    pub page_count: usize,
}

/// Where the caret is, in screen coordinates (device pixels). The window is placed below `y + line_height`
/// and flipped above the caret when it would leave the monitor work area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub x: i32,
    pub y: i32,
    pub line_height: i32,
}

/// Run a callback so that a panic cannot unwind out of it: it answers `fallback` instead. Every COM method of the text
/// service and the popup's window procedure go through this, because a panic that unwinds out of a COM method or an
/// `extern "system"` frame takes the host application down with it. Plain `std`, so the popup demo can share this file.
pub(crate) fn guard<T>(fallback: T, f: impl FnOnce() -> T) -> T {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or(fallback)
}

pub trait CandidateUi {
    /// Show or update the window. Must not take focus. Returns whether the list is VISIBLE to the user afterwards:
    /// false when it could not be drawn (no window, no monitor, no font, nothing to lay out) or when the implementation
    /// cannot tell. The caller treats false like a hidden window: nothing may be eaten behind a list nobody sees.
    fn show(&mut self, view: &CandidateView, anchor: Anchor) -> bool;
    /// Hide it (idempotent).
    fn hide(&mut self);
    /// Whether the list is visible to the user RIGHT NOW. It can change without a `show` or `hide`: a host that draws the
    /// list itself may hand it back (`ITfUIElement::Show(TRUE)`) to a popup that then cannot be drawn. The controller asks
    /// before it eats a key.
    fn is_visible(&self) -> bool;
    /// An action the user or the host asked for, waiting to be applied. `None` when there is none. The controller asks
    /// on every key event and after every `show`: an action that arrived while the text service was busy (a host that
    /// calls back into the element from inside `UpdateUIElement`) waits here instead of being lost.
    fn take_action(&mut self) -> Option<ListAction> {
        None
    }
}

/// Test double: remembers what it was asked to show.
#[derive(Default, Debug)]
pub struct RecordingUi {
    pub last: Option<(CandidateView, Anchor)>,
    pub visible: bool,
    pub shows: usize,
    pub hides: usize,
    /// Set by a test: `show` records the request but reports "not visible", like a popup that could not be drawn.
    pub refuse: bool,
}

impl CandidateUi for RecordingUi {
    fn show(&mut self, view: &CandidateView, anchor: Anchor) -> bool {
        self.last = Some((view.clone(), anchor));
        self.visible = !self.refuse;
        self.shows += 1;
        self.visible
    }

    fn hide(&mut self) {
        self.visible = false;
        self.hides += 1;
    }

    fn is_visible(&self) -> bool {
        self.visible
    }
}
