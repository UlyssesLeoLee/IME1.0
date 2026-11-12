//! The candidate window at a high DPI, and the independence of the interface language, the language of the code and
//! the language of the operating system.
//!
//! The design case is IT-037 ("DPI + UI 语言互操作"). Its inputs are "2 monitors DPI 200 %" and "a ja-JP operating
//! system with a zh-CN interface, writing Rust", and its criterion is "the candidates are on the screen, they can be
//! used, and the interface, the code and the operating system are independent of each other".
//!
//! What is measured here is the part that can be measured without a GUI: the geometry of the window at 200 % (it must
//! stay inside the work area of the monitor, it must not cover the caret line, and every row must be reachable) and the
//! independence of the language of the code from the language of the interface.
//!
//! What this does NOT prove: that two monitors at 200 % and an operating system in another language behave the same
//! on a real desktop. That is the GUI run of the case (the runbook is in
//! `02_Verification/REAL_MACHINE_CHECKLIST.md`).

mod common;

use common::{FakeHost, Harness};
use pime_tsf::controller::TipConfig;
use pime_tsf::layout::{self, Placement, Rect, TextMetrics};
use pime_tsf::ui::{Anchor, CandidateRow, CandidateView};

/// The work area of a 4K monitor at 200 %, in device pixels.
fn work_area() -> Rect {
    Rect::new(0, 0, 3840, 2160)
}

/// The measurements of a font at 200 % (the metrics of the DPI the window is drawn at).
struct Metrics {
    char_width: i32,
    line_height: i32,
}

impl TextMetrics for Metrics {
    fn text_width(&self, text: &str) -> i32 {
        text.chars()
            .map(|c| {
                if c.is_ascii() {
                    self.char_width
                } else {
                    self.char_width * 2
                }
            })
            .sum()
    }
    fn line_height(&self) -> i32 {
        self.line_height
    }
}

/// The metrics at 200 %.
fn metrics_200() -> Metrics {
    Metrics {
        char_width: 16,
        line_height: 32,
    }
}

/// A page of five candidates, as the window shows it.
fn page() -> CandidateView {
    CandidateView {
        rows: (0..5)
            .map(|i| CandidateRow {
                label: (i + 1).to_string(),
                text: format!("c{i}:fn"),
                detail: "rule \u{b7} valid".to_string(),
            })
            .collect(),
        selected_in_page: 0,
        page: 0,
        page_count: 3,
    }
}

/// The window is inside the work area of the monitor, whichever corner the caret is in.
#[test]
fn the_candidates_stay_inside_the_work_area_at_two_hundred_percent() {
    let corners = [
        // the caret at the bottom right: the window must go above the caret and stay inside the screen
        Anchor {
            x: 3700,
            y: 2100,
            line_height: 36,
        },
        // the caret at the bottom left
        Anchor {
            x: 20,
            y: 2100,
            line_height: 36,
        },
        // the caret at the top right
        Anchor {
            x: 3700,
            y: 20,
            line_height: 36,
        },
        // the caret in the middle
        Anchor {
            x: 1800,
            y: 1000,
            line_height: 36,
        },
    ];
    for anchor in corners {
        let laid = layout::compute_scaled(&page(), anchor, work_area(), &metrics_200(), 2.0);
        assert!(laid.is_visible(), "the window is not shown at {anchor:?}");
        let window = laid.window;
        assert!(
            window.left >= work_area().left
                && window.right <= work_area().right
                && window.top >= work_area().top
                && window.bottom <= work_area().bottom,
            "the window leaves the work area at {anchor:?}: {window:?}"
        );
        assert!(
            window.width() > 0 && window.height() > 0,
            "the window has no size at {anchor:?}: {window:?}"
        );
        // the window never covers the caret line
        assert!(
            window.bottom <= anchor.y || window.top >= anchor.y + anchor.line_height,
            "the window covers the caret line at {anchor:?}: {window:?}"
        );
        // every row is inside the window, so every candidate can be picked (the rows are in client coordinates)
        assert_eq!(laid.rows.len(), 5, "the rows of the page at {anchor:?}");
        let client = Rect::new(0, 0, window.width(), window.height());
        for row in &laid.rows {
            assert!(
                row.left >= client.left
                    && row.right <= client.right
                    && row.top >= client.top
                    && row.bottom <= client.bottom,
                "the row {row:?} is not inside the window {client:?} at {anchor:?}"
            );
        }
    }
}

/// There is no room below the caret: the window goes above it.
#[test]
fn the_window_goes_above_the_caret_when_there_is_no_room_below() {
    let anchor = Anchor {
        x: 1800,
        y: 2100,
        line_height: 36,
    };
    let laid = layout::compute_scaled(&page(), anchor, work_area(), &metrics_200(), 2.0);
    assert_eq!(laid.placement, Placement::Above, "{:?}", laid.window);
    assert!(laid.window.bottom <= anchor.y, "{:?}", laid.window);
}

/// The language of the code is the one of the file; the interface does not follow it.
#[test]
fn the_language_of_the_code_does_not_follow_the_language_of_the_interface() {
    let mut rust = Harness::with(
        TipConfig {
            language: "rust".to_string(),
            ..TipConfig::default()
        },
        FakeHost::with_doc(""),
    );
    rust.type_text("fn");
    let mut sql = Harness::with(
        TipConfig {
            language: "plsql".to_string(),
            ..TipConfig::default()
        },
        FakeHost::with_doc(""),
    );
    sql.type_text("fn");

    // the engine is asked in the language of the code, and only in that one
    assert_eq!(
        rust.transcript().languages.last().map(String::as_str),
        Some("rust"),
        "{:?}",
        rust.transcript().languages
    );
    assert_eq!(
        sql.transcript().languages.last().map(String::as_str),
        Some("plsql"),
        "{:?}",
        sql.transcript().languages
    );
    assert!(
        rust.transcript()
            .languages
            .iter()
            .all(|language| language == "rust"),
        "{:?}",
        rust.transcript().languages
    );

    // the strings of the interface are the same in both cases: the interface has a language of its own
    assert_eq!(
        rust.transcript().rows,
        sql.transcript().rows,
        "the rows of the window depend on the language of the code"
    );
    assert_eq!(rust.transcript().rows[0], "1|c0:fn|rule \u{b7} valid");
}
