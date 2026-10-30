//! Pure geometry of the candidate window: sizes, row rectangles, placement relative to the caret and the monitor
//! work area (flip above when it would not fit, clamp horizontally). No Win32 calls: testable with synthetic metrics.
//!
//! OWNER: the "POPUP" work item. Stub.

use crate::ui::{Anchor, CandidateView};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// Text measurements supplied by the caller (from GDI in production, synthetic in tests).
pub trait TextMetrics {
    fn text_width(&self, text: &str) -> i32;
    fn line_height(&self) -> i32;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub window: Rect,
    pub rows: Vec<Rect>,
    pub footer: Option<Rect>,
}

pub fn compute(
    _view: &CandidateView,
    _anchor: Anchor,
    _work_area: Rect,
    _metrics: &dyn TextMetrics,
) -> Layout {
    Layout {
        window: Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
        rows: Vec::new(),
        footer: None,
    }
}
