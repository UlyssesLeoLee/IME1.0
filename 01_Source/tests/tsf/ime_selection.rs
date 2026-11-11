//! The candidate selection gesture: walking the list with the arrows and taking a candidate with Tab, Space or Enter.
//!
//! Reported symptom (2026-11-04): with the shipped defaults the list could not be walked and nothing could be
//! taken with Tab, Space or Enter. The cause was the shipped default (the direct mode, in which the input method
//! never eats a code character) and not a broken gesture.
//!
//! The user ruled the other way on 2026-11-11: the **code input** is the factory mode (the input method never
//! eats a code character), and the **natural language input** — the one whose list takes the digits, the Tab key,
//! Space and Enter — is one **Ctrl+F12** away. These tests pin both halves: the factory mode leaves the code
//! characters to the host, and one Ctrl+F12 gives the list the keys back.

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

/// The same, in the **natural language input**: the user pressed Ctrl+F12 (the factory mode is the code input)
/// and then typed `fn`.
fn typed_candidates_natural_language() -> Harness {
    let mut h = Harness::new(TipConfig::default());
    assert!(h.mode_chord(), "Ctrl+F12 is ours");
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
fn the_natural_language_input_walks_the_list_with_the_arrows() {
    // The scripted core offers twelve candidates and the page shows five of them: Down moves to the next row,
    // Up wraps around to the last candidate, so it also turns the page.
    for (name, page, row) in [("Up", 2usize, 1usize), ("Down", 0usize, 1usize)] {
        let mut h = typed_candidates_natural_language();
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
fn the_natural_language_input_takes_a_candidate_with_space_and_enter() {
    for name in ["Space", "Enter"] {
        let mut h = typed_candidates_natural_language();
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
fn the_natural_language_input_selects_with_tab_and_takes_with_enter() {
    let mut h = typed_candidates_natural_language();
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
fn the_natural_language_input_takes_the_row_a_digit_names() {
    let mut h = typed_candidates_natural_language();
    assert!(h.digit('2'), "the digit belongs to the candidate list");
    let t = h.transcript();
    assert_eq!(t.commits.len(), 1);
    assert_eq!(t.commits[0], "c1:fn", "the second row");
}

/// The **code input** is the factory mode: every code character reaches the host, and the list is a preview
/// that takes no key.
#[test]
fn the_code_input_leaves_the_code_characters_to_the_host() {
    let mut h = typed_candidates(TipConfig::default());
    assert!(!h.space(), "Space is a code character in the code input");
    assert!(!h.enter(), "Enter is a code character in the code input");
    assert!(!h.up(), "the arrow is the editor's in the code input");
    assert_eq!(h.transcript().commits.len(), 0, "nothing was written");
    assert_eq!(
        h.transcript().doc, "fn \n",
        "the text is untouched: only the space and the newline the host received were added"
    );
}

/// Ctrl+F12 is the way in and out of the natural language input: it is ours, and it changes the mode.
#[test]
fn ctrl_f12_switches_the_mode_and_the_keys_follow() {
    let mut h = Harness::new(TipConfig::default());
    assert!(!h.transcript().input_mode, "the factory mode is the code input");
    // the list is a preview in the code input: the keys belong to the host
    h.type_text("fn");
    assert!(h.transcript().visible, "the list is on screen");
    assert!(!h.enter(), "Enter is the host's in the code input");
    assert_eq!(h.transcript().commits.len(), 0, "nothing was written");
    // one Ctrl+F12 gives the list the keys back
    assert!(h.mode_chord(), "Ctrl+F12 is ours");
    assert!(h.transcript().input_mode, "the natural language input is on");
    let mut h = typed_candidates_natural_language();
    assert!(h.enter(), "Enter belongs to the candidate list now");
    assert_eq!(h.transcript().commits.len(), 1, "one candidate was written");
    // and the other way round
    assert!(h.mode_chord(), "Ctrl+F12 is ours");
    assert!(!h.transcript().input_mode, "the code input is back");
}
