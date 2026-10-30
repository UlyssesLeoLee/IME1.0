//! The native Win32 candidate window (a layered, no-activate, topmost popup drawn with GDI) implementing
//! [`crate::ui::CandidateUi`]. It must never take focus (WS_EX_NOACTIVATE) and never touch the host's text.
//!
//! OWNER: the "POPUP" work item. Stub: shows nothing.

use crate::ui::{Anchor, CandidateUi, CandidateView};

#[derive(Default)]
pub struct PopupWindow;

impl PopupWindow {
    pub fn new() -> PopupWindow {
        PopupWindow
    }
}

impl CandidateUi for PopupWindow {
    fn show(&mut self, _view: &CandidateView, _anchor: Anchor) {}

    fn hide(&mut self) {}
}
