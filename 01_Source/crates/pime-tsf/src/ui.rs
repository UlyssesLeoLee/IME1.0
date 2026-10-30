//! The contract between the text input processor (`tip`) and any candidate-window implementation (`popup`).
//! Pure data and one trait: no Windows API here, so the TIP logic is testable with [`RecordingUi`].
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

pub trait CandidateUi {
    /// Show or update the window. Must not take focus.
    fn show(&mut self, view: &CandidateView, anchor: Anchor);
    /// Hide it (idempotent).
    fn hide(&mut self);
}

/// Test double: remembers what it was asked to show.
#[derive(Default, Debug)]
pub struct RecordingUi {
    pub last: Option<(CandidateView, Anchor)>,
    pub visible: bool,
    pub shows: usize,
    pub hides: usize,
}

impl CandidateUi for RecordingUi {
    fn show(&mut self, view: &CandidateView, anchor: Anchor) {
        self.last = Some((view.clone(), anchor));
        self.visible = true;
        self.shows += 1;
    }

    fn hide(&mut self) {
        self.visible = false;
        self.hides += 1;
    }
}
