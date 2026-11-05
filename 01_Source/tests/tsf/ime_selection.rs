//! The candidate selection gesture: walking the list with the arrows and taking a candidate with Tab, Space or Enter.
//!
//! Reported symptom (2026-11-04): with the shipped defaults the list could not be walked and nothing could be
//! taken with Tab, Space or Enter. The cause was the shipped default (the direct mode, in which the input method
//! never eats a code character) and not a broken gesture. These tests pin the behaviour so it cannot rot again:
//! the shipped default is the input mode, and the direct mode stays the opt-in mode for code that must not be
//! touched.

mod common;

use common::Harness;
use pime_tsf::controller::TipConfig;

/// A harness with the candidates on screen: the user typed `fn` and the list is showing.
fn typed_candidates(config: TipConfig) -> Harness {
    let mut h = Harness::new(config);
    h.type_text("fn");
    assert!(
        h.transcript().visible,
        "the candidate list must be on screen after typing"
    );
    h
}

/// The text of the first candidate the scripted core offers.
const FIRST: &str = "c0:fn";

#[test]
fn the_shipped_default_walks_the_list_with_the_arrows() {
    // The scripted core offers twelve candidates and the page shows five of them: Down moves to the next row,
    // Up wraps around to the last candidate, so it also turns the page.
    for (name, page, row) in [("Up", 2usize, 1usize), ("Down", 0usize, 1usize)] {
        let mut h = typed_candidates(TipConfig::default());
        assert_eq!(
            h.transcript().selected, 0,
            "{name}: the first row starts highlighted"
        );
        let eaten = if name == "Up" { h.up() } else { h.down() };
        assert!(eaten, "{name}: the arrow belongs to the candidate list");
        assert_eq!(h.transcript().page, page, "{name}: the page follows");
        assert_eq!(h.transcript().selected, row, "{name}: the highlighted row");
        assert!(
            h.transcript().visible,
            "{name}: the list is still on screen after the move"
        );
    }
}

#[test]
fn the_shipped_default_takes_a_candidate_with_space_and_enter() {
    for name in ["Space", "Enter"] {
        let mut h = typed_candidates(TipConfig::default());
        let eaten = match name {
            "Space" => h.space(),
            _ => h.enter(),
        };
        assert!(eaten, "{name}: the key belongs to the candidate list");
        let t = h.transcript();
        assert_eq!(t.commits.len(), 1, "{name}: one candidate was written");
        assert_eq!(t.commits[0], FIRST, "{name}: the highlighted candidate");
        assert_eq!(t.doc, FIRST, "{name}: the candidate is in the document");
        assert!(!t.visible, "{name}: the list is gone after the take");
    }
}

/// The Tab key is the selection key of the list (the programmer habit): it moves the highlight and writes nothing.
#[test]
fn the_shipped_default_selects_with_tab_and_takes_with_enter() {
    let mut h = typed_candidates(TipConfig::default());
    assert!(h.tab(), "the Tab key belongs to the candidate list");
    let t = h.transcript();
    assert!(t.commits.is_empty(), "Tab writes nothing: {:?}", t.commits);
    assert_eq!(t.selected, 1, "Tab moved the highlight");
    assert!(t.visible, "the list is still on screen");
    // Enter takes the candidate that Tab highlighted
    assert!(h.enter(), "Enter belongs to the candidate list");
    let t = h.transcript();
    assert_eq!(t.commits.len(), 1, "one candidate was written");
    assert_eq!(t.commits[0], "c1:fn", "the highlighted candidate");
}

#[test]
fn the_shipped_default_takes_the_row_a_digit_names() {
    let mut h = typed_candidates(TipConfig::default());
    assert!(h.digit('2'), "the digit belongs to the candidate list");
    let t = h.transcript();
    assert_eq!(t.commits.len(), 1);
    assert_eq!(t.commits[0], "c1:fn", "the second row");
}

/// The direct mode is the opt-in for code that must not be touched: every code character reaches the host.
#[test]
fn the_direct_mode_leaves_the_code_characters_to_the_host() {
    let config = TipConfig {
        candidate_capture: false,
        auto_compose: false,
        input_mode: false,
        ..TipConfig::default()
    };
    let mut h = typed_candidates(config);
    assert!(!h.space(), "Space is a code character in the direct mode");
    assert!(!h.enter(), "Enter is a code character in the direct mode");
    assert!(!h.up(), "the arrow is the editor's in the direct mode");
    assert_eq!(h.transcript().commits.len(), 0, "nothing was written");
    assert_eq!(
        h.transcript().doc, "fn \n",
        "the text is untouched: only the space and the newline the host received were added"
    );
}
