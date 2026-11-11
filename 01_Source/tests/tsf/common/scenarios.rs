//! The scenario table of the IME test scripts and the ledger of the IME functions.
//!
//! A scenario drives the text service the way a host does and then says what the user must see. A function entry
//! says which IME function is covered and by which test - a scenario of this table or a unit test of the crate
//! (the ledger checks that the named test really exists in the sources, so it cannot rot silently).

use super::{FakeHost, Harness, ScriptedPredictor, Transcript, write_evidence};
use pime_core::input::KeyMap;
use pime_tsf::controller::TipConfig;
use pime_tsf::ui::ListAction;

/// One scenario: a set of steps and what must be true afterwards.
pub struct Scenario {
    /// The stable id (IME-S01 ...).
    pub id: &'static str,
    /// The IME function this scenario covers.
    pub function: &'static str,
    /// What the scenario is about.
    pub title: &'static str,
    /// The steps.
    pub body: fn(&mut Harness),
    /// What must be true at the end.
    pub expect: fn(&Transcript),
}

/// The functions of the input method. Every one of them must name the test that covers it.
pub struct ImeFunction {
    pub id: &'static str,
    pub name: &'static str,
    /// The module of the crate that implements it.
    pub module: &'static str,
    /// The scenarios of this table that cover it (empty when a unit test of the crate covers it).
    pub scenarios: &'static [&'static str],
    /// The unit test of the crate that covers it ("" when a scenario of this table covers it).
    pub unit_test: &'static str,
    /// The source file the unit test lives in.
    pub unit_file: &'static str,
}

pub const IME_FUNCTIONS: &[ImeFunction] = &[
    ImeFunction {
        id: "F-ACTIVATE",
        name: "Activate, deactivate and focus change leave nothing behind",
        module: "controller",
        scenarios: &["IME-S13", "IME-S15"],
        unit_test: "deactivate_leaves_no_window_and_no_open_composition",
        unit_file: "controller.rs",
    },
    ImeFunction {
        id: "F-INPUT-MODE",
        name: "Direct mode and input mode (Alt+F1, the floating window button)",
        module: "controller",
        scenarios: &["IME-S10"],
        unit_test: "alt_f1_switches_between_direct_mode_and_input_mode",
        unit_file: "controller.rs",
    },
    ImeFunction {
        id: "F-OBSERVE",
        name: "Keys are observed but never eaten in direct mode",
        module: "controller",
        scenarios: &["IME-S01"],
        unit_test: "typing_code_in_the_direct_mode_eats_and_commits_nothing",
        unit_file: "controller.rs",
    },
    ImeFunction {
        id: "F-CANDIDATE-WINDOW",
        name: "The candidate window shows labelled rows, a selection and a page footer",
        module: "controller",
        scenarios: &["IME-S01"],
        unit_test: "typing_shows_a_window_of_labelled_rows_without_eating_anything",
        unit_file: "controller.rs",
    },
    ImeFunction {
        id: "F-LAYOUT",
        name: "The candidate window is placed below the caret, flipped and clamped into the work area",
        module: "layout",
        scenarios: &[],
        unit_test: "ut_055_flips_above_the_caret_when_the_bottom_edge_is_too_close",
        unit_file: "layout.rs",
    },
    ImeFunction {
        id: "F-PAGING",
        name: "PageUp / PageDown walk through the pages",
        module: "controller",
        scenarios: &["IME-S02"],
        unit_test: "page_down_moves_the_page_and_updates_the_window",
        unit_file: "controller.rs",
    },
    ImeFunction {
        id: "F-SELECT",
        name: "A candidate is taken with a digit, Enter or Space; Tab selects in the list",
        module: "controller",
        scenarios: &["IME-S03", "IME-S04", "IME-S06", "IME-S06B", "IME-S07"],
        unit_test: "ut_009_a_digit_commits_the_page_relative_candidate_exactly_once",
        unit_file: "controller.rs",
    },
    ImeFunction {
        id: "F-CANCEL",
        name: "Esc cancels the list and writes nothing",
        module: "controller",
        scenarios: &["IME-S05"],
        unit_test: "ut_004_esc_cancels_and_writes_nothing",
        unit_file: "controller.rs",
    },
    ImeFunction {
        id: "F-COMPOSE",
        name: "The explicit composition (Ctrl+Alt+Space) and the automatic one",
        module: "controller",
        scenarios: &["IME-S08", "IME-S09", "IME-S16"],
        unit_test: "ctrl_alt_space_with_candidates_starts_an_explicit_composition",
        unit_file: "controller.rs",
    },
    ImeFunction {
        id: "F-SHIFT-TAP",
        name: "A Shift tap pauses and resumes the input method",
        module: "keymap",
        scenarios: &["IME-S11"],
        unit_test: "ut_009_shift_tap_pauses_and_resumes_and_hides_the_window",
        unit_file: "controller.rs",
    },
    ImeFunction {
        id: "F-SECURE",
        name: "A password field and the secure desktop collect nothing",
        module: "controller",
        scenarios: &["IME-S12"],
        unit_test: "ut_005_a_secure_field_shows_nothing_and_eats_nothing",
        unit_file: "controller.rs",
    },
    ImeFunction {
        id: "F-DEGRADE",
        name: "A host that refuses a write hands the keyboard back",
        module: "controller",
        scenarios: &["IME-S14"],
        unit_test: "ut_064_a_failed_commit_hands_the_key_to_the_host_and_everything_passes_after",
        unit_file: "controller.rs",
    },
    ImeFunction {
        id: "F-HOST-ACTION",
        name: "The host (or the user) acts on the candidate list",
        module: "controller",
        scenarios: &["IME-S17"],
        unit_test: "the_behaviour_interface_reports_the_users_choice_to_the_text_service",
        unit_file: "uielement_com.rs",
    },
    ImeFunction {
        id: "F-UI-ELEMENT",
        name: "The candidate list is published to the host as a UI element",
        module: "uielement",
        scenarios: &["IME-S18"],
        unit_test: "begin_when_a_list_appears_update_on_every_visible_change_end_when_it_hides",
        unit_file: "uielement.rs",
    },
    ImeFunction {
        id: "F-POPUP",
        name: "The native candidate window is drawn at the right DPI and never steals focus",
        module: "popup",
        scenarios: &[],
        unit_test: "the_candidate_list_is_drawn_at_the_dpi_of_the_coordinate_space_it_lives_in",
        unit_file: "popup.rs",
    },
    ImeFunction {
        id: "F-FLOATING",
        name: "The floating interface of the product is the laser interface (the blue glow, the beam and the panel)",
        module: "laser_ui",
        scenarios: &[],
        unit_test: "the_mouth_is_a_thin_pale_line_inside_the_window_of_the_glow",
        unit_file: "laser_ui.rs",
    },
    ImeFunction {
        id: "F-LANGUAGE",
        name: "Alt+F2 walks through the three languages of the product",
        module: "keymap",
        scenarios: &[],
        unit_test: "alt_f2_is_the_language_shortcut_and_nothing_else_is",
        unit_file: "keymap.rs",
    },
    ImeFunction {
        id: "F-KEYMAP",
        name: "The schemes of the keys (the programmer habit, Microsoft, Sogou, ATOK) and the three ways of the Tab key",
        module: "imesettings",
        scenarios: &[],
        unit_test: "the_schemes_of_the_keys_are_the_ones_the_user_asked_for",
        unit_file: "imesettings.rs",
    },
    ImeFunction {
        id: "F-TAB",
        name: "The Tab key is the selection key of the candidate list (the programmer habit)",
        module: "settings_window",
        scenarios: &[],
        unit_test: "the_three_ways_of_the_tab_key_are_the_ones_the_user_asked_for",
        unit_file: "settings_window.rs",
    },
    ImeFunction {
        id: "F-SKIN",
        name: "The colours of the product: the panel of the laser interface and its menus",
        module: "skin",
        scenarios: &[],
        unit_test: "the_skins_have_readable_colours",
        unit_file: "skin.rs",
    },
    ImeFunction {
        id: "F-MENU",
        name: "The menus filter, lay out and hit-test",
        module: "menus",
        scenarios: &[],
        unit_test: "a_click_finds_the_row_under_it",
        unit_file: "menus.rs",
    },
    ImeFunction {
        id: "F-SETTINGS-WINDOW",
        name: "The settings window has the pages of the settings (the way Sogou splits them) and lays out its controls without overlaps",
        module: "settings_window",
        scenarios: &[],
        unit_test: "the_controls_of_a_page_do_not_overlap_each_other",
        unit_file: "settings_window.rs",
    },
    ImeFunction {
        id: "F-SETTINGS-STORE",
        name: "The settings are stored per language and survive a broken file",
        module: "imesettings",
        scenarios: &[],
        unit_test: "a_broken_file_does_not_take_the_input_method_down",
        unit_file: "imesettings.rs",
    },
    ImeFunction {
        id: "F-REGISTER",
        name: "Registration writes only its own keys and rolls back on a failure",
        module: "register",
        scenarios: &[],
        unit_test: "a_failure_at_every_step_rolls_everything_back_to_the_start",
        unit_file: "register.rs",
    },
    ImeFunction {
        id: "F-RESOLVE",
        name: "The language of a file is resolved (extension, editor id, manual lock)",
        module: "resolve",
        scenarios: &["IME-S19"],
        unit_test: "",
        unit_file: "",
    },
    ImeFunction {
        id: "F-ENGINE",
        name: "The candidates are generated, validated and ranked",
        module: "predict",
        scenarios: &["IME-S20"],
        unit_test: "",
        unit_file: "",
    },
    ImeFunction {
        id: "F-PLUGIN-LANGUAGE",
        name: "A plugin language produces the candidates of its own pack",
        module: "languages",
        scenarios: &["IME-S21"],
        unit_test: "",
        unit_file: "",
    },
    ImeFunction {
        id: "F-LASER",
        name: "A character that is written is lit up by a laser of the glow on the left",
        module: "laser",
        scenarios: &["IME-S22"],
        unit_test: "a_beam_is_over_after_half_a_second",
        unit_file: "laser.rs",
    },
    ImeFunction {
        id: "F-GLOW-PANEL",
        name: "The mouth of the glow opens the panel with the settings that are used most often",
        module: "laser",
        scenarios: &[],
        unit_test: "the_mouth_is_hit_only_by_the_lens",
        unit_file: "laser.rs",
    },
    ImeFunction {
        id: "F-LASER-PANEL",
        name: "The rows of the laser panel switch the settings and open the lists",
        module: "laser",
        scenarios: &[],
        unit_test: "a_click_on_a_switch_turns_it_around",
        unit_file: "laser.rs",
    },
    ImeFunction {
        id: "F-LAYERED-WINDOW",
        name: "The windows of the light are layered windows that are moved by their picture (no residue at the edges)",
        module: "glass",
        scenarios: &[],
        unit_test: "a_window_with_a_picture_is_moved_by_its_picture",
        unit_file: "glass.rs",
    },
    ImeFunction {
        id: "F-DICTIONARY",
        name: "The input method carries its own Chinese, Japanese and English dictionaries (it calls no other IME)",
        module: "cjk",
        scenarios: &[],
        unit_test: "the_chinese_profile_types_chinese_with_the_dictionary_of_the_product",
        unit_file: "controller.rs",
    },
    ImeFunction {
        id: "F-LASER-CANDIDATES",
        name: "The candidate list lives in the laser panel (no Sogou-style floating window while typing)",
        module: "laser_candidates",
        scenarios: &[],
        unit_test: "the_rows_of_the_list_become_the_rows_of_the_panel",
        unit_file: "laser_candidates.rs",
    },
    ImeFunction {
        id: "F-SETTINGS-PLUGINS",
        name: "The settings window has a plugin page with a check box for every plugin language",
        module: "settings_window",
        scenarios: &[],
        unit_test: "the_plugins_page_has_one_check_box_for_every_plugin",
        unit_file: "settings_window.rs",
    },
    ImeFunction {
        id: "F-DICTIONARY-SERVICES",
        name: "The reserved interfaces of the dictionary management (AI curation, remote update, sign in)",
        module: "services",
        scenarios: &[],
        unit_test: "the_services_can_be_used_as_objects",
        unit_file: "services.rs",
    },
    ImeFunction {
        id: "F-AGENT-BRIDGE",
        name: "The bridge window: an AI agent asks the input method to write into the document that has the focus",
        module: "agent_bridge",
        scenarios: &[],
        unit_test: "the_bridge_answers_an_agent_of_another_process",
        unit_file: "agent_bridge.rs",
    },
];

/// The sources of the crate, so the ledger can check that the named unit tests really exist.
pub const CRATE_SOURCES: &[(&str, &str)] = &[
    (
        "controller.rs",
        include_str!("../../../crates/pime-tsf/src/controller.rs"),
    ),
    (
        "keymap.rs",
        include_str!("../../../crates/pime-tsf/src/keymap.rs"),
    ),
    (
        "layout.rs",
        include_str!("../../../crates/pime-tsf/src/layout.rs"),
    ),
    (
        "popup.rs",
        include_str!("../../../crates/pime-tsf/src/popup.rs"),
    ),
    (
        "menus.rs",
        include_str!("../../../crates/pime-tsf/src/menus.rs"),
    ),
    (
        "skin.rs",
        include_str!("../../../crates/pime-tsf/src/skin.rs"),
    ),
    (
        "settings_window.rs",
        include_str!("../../../crates/pime-tsf/src/settings_window.rs"),
    ),
    (
        "imesettings.rs",
        include_str!("../../../crates/pime-tsf/src/imesettings.rs"),
    ),
    (
        "register.rs",
        include_str!("../../../crates/pime-tsf/src/register.rs"),
    ),
    (
        "laser.rs",
        include_str!("../../../crates/pime-tsf/src/laser.rs"),
    ),
    (
        "laser_ui.rs",
        include_str!("../../../crates/pime-tsf/src/laser_ui.rs"),
    ),
    (
        "laser_candidates.rs",
        include_str!("../../../crates/pime-tsf/src/laser_candidates.rs"),
    ),
    (
        "agent_bridge.rs",
        include_str!("../../../crates/pime-tsf/src/agent_bridge.rs"),
    ),
    (
        "services.rs",
        include_str!("../../../crates/pime-core/src/services.rs"),
    ),
    (
        "glass.rs",
        include_str!("../../../crates/pime-tsf/src/glass.rs"),
    ),
    (
        "tip.rs",
        include_str!("../../../crates/pime-tsf/src/tip.rs"),
    ),
    (
        "uielement.rs",
        include_str!("../../../crates/pime-tsf/src/uielement.rs"),
    ),
    (
        "uielement_com.rs",
        include_str!("../../../crates/pime-tsf/src/uielement_com.rs"),
    ),
];

fn rows(t: &Transcript) -> Vec<&str> {
    t.rows.iter().map(|s| s.as_str()).collect()
}

// ------------------------------------------------------------------ the configurations

/// The shipped defaults: the input mode (the mainstream behaviour).
fn defaults() -> TipConfig {
    TipConfig::default()
}

/// The direct mode: the input method never eats a code character (the mode for code that must not be touched).
fn direct() -> TipConfig {
    TipConfig {
        candidate_capture: false,
        auto_compose: false,
        input_mode: false,
        keymap: KeyMap {
            space_confirms: false,
            ..KeyMap::default()
        },
        ..TipConfig::default()
    }
}

/// The always-listening configuration (the mainstream behaviour).
fn sogou() -> TipConfig {
    TipConfig {
        candidate_capture: true,
        auto_compose: true,
        input_mode: false,
        ..TipConfig::default()
    }
}

/// The configuration of a scenario (the way the product is meant to be used).
pub fn scenario_config(id: &str) -> TipConfig {
    match id {
        // The Tab key belongs to the host while the list is on screen.
        "IME-S07" => TipConfig {
            keymap: KeyMap {
                tab_to_host: true,
                ..KeyMap::default()
            },
            ..TipConfig::default()
        },
        // The explicit composition and the switch between the two modes: both are about the direct mode, in which
        // the list is a preview and the chord is the way into a composition.
        "IME-S08" | "IME-S10" => direct(),
        // Paging, taking a candidate with a digit and Enter: the always-listening mode.
        // Esc and the composition need it as well: in direct mode the list is only a preview, so Esc belongs to
        // the editor (the documented behaviour: "Space, Enter, Esc, digits and the arrows all reach the host").
        "IME-S02" | "IME-S03" | "IME-S04" | "IME-S05" | "IME-S14" | "IME-S16" | "IME-S20" => {
            sogou()
        }
        // The file is PL/SQL: the engine must be asked in that language.
        "IME-S19" => TipConfig {
            language: "plsql".to_string(),
            ..TipConfig::default()
        },
        // The file is Cypher: a plugin language reaches the engine like any other.
        "IME-S21" => TipConfig {
            language: "cypher".to_string(),
            ..TipConfig::default()
        },
        _ => defaults(),
    }
}

/// The host a scenario needs (a password field, a host that refuses writes, ...).
pub fn scenario_host(id: &str) -> FakeHost {
    match id {
        "IME-S12" => FakeHost {
            secure: true,
            ..FakeHost::with_doc("")
        },
        "IME-S14" => FakeHost {
            refuse_commit: true,
            ..FakeHost::with_doc("")
        },
        _ => FakeHost::with_doc(""),
    }
}

/// Runs one scenario and checks its expectation. Returns the transcript for the evidence file.
pub fn run(id: &str) -> Transcript {
    let Some(scenario) = SCENARIOS.iter().find(|s| s.id == id) else {
        panic!("no scenario {id}");
    };
    let mut harness = Harness::with(scenario_config(id), scenario_host(id));
    (scenario.body)(&mut harness);
    let transcript = harness.transcript();
    write_evidence(id, &transcript);
    (scenario.expect)(&transcript);
    transcript
}

/// A predictor that returns nothing: used by the scenarios that must not reach the engine.
pub fn silent_predictor() -> ScriptedPredictor {
    ScriptedPredictor::silent()
}

// ------------------------------------------------------------------ the scenarios

pub static SCENARIOS: &[Scenario] = &[
    Scenario {
        id: "IME-S01",
        function: "F-OBSERVE",
        title: "Typing shows a candidate window and reaches the host untouched",
        body: |h| {
            h.type_text("fn");
        },
        expect: |t| {
            assert_eq!(t.doc, "fn", "the host typed the characters itself");
            assert!(t.visible, "the candidate window is on screen");
            assert_eq!(rows(t).len(), 5, "five rows on a page: {t:?}");
            assert_eq!(t.rows[0], "1|c0:fn|rule \u{b7} valid");
            assert_eq!((t.page, t.page_count, t.selected), (0, 3, 0));
            assert!(t.eaten.is_empty(), "nothing was eaten: {:?}", t.eaten);
            assert_eq!(t.passed, ["f", "n"]);
        },
    },
    Scenario {
        id: "IME-S02",
        function: "F-PAGING",
        title: "PageDown and PageUp walk through the pages",
        body: |h| {
            h.type_text("fn");
            h.page_down();
            h.page_down();
            h.page_up();
        },
        expect: |t| {
            assert!(t.visible);
            assert_eq!((t.page, t.page_count), (1, 3), "{t:?}");
            assert_eq!(
                t.rows[0], "1|c5:fn|rule \u{b7} valid",
                "the second page starts at the sixth candidate"
            );
        },
    },
    Scenario {
        id: "IME-S03",
        function: "F-SELECT",
        title: "A digit takes the candidate of its row on the page",
        body: |h| {
            h.type_text("fn");
            h.digit('2');
        },
        expect: |t| {
            assert_eq!(t.commits, ["c1:fn"], "{t:?}");
            assert_eq!(t.doc, "c1:fn", "the typed text was replaced");
            assert!(!t.visible, "the window is gone after the commit");
        },
    },
    Scenario {
        id: "IME-S04",
        function: "F-SELECT",
        title: "Enter takes the highlighted candidate",
        body: |h| {
            h.type_text("fn");
            h.enter();
        },
        expect: |t| {
            assert_eq!(t.commits, ["c0:fn"], "{t:?}");
            assert_eq!(t.doc, "c0:fn");
        },
    },
    Scenario {
        id: "IME-S05",
        function: "F-CANCEL",
        title: "Esc cancels the list and writes nothing",
        body: |h| {
            h.type_text("fn");
            h.esc();
        },
        expect: |t| {
            assert!(t.commits.is_empty(), "nothing was written: {:?}", t.commits);
            assert_eq!(t.doc, "fn");
            assert!(!t.visible, "the window is closed");
        },
    },
    Scenario {
        id: "IME-S06",
        function: "F-SELECT",
        title: "Tab selects in the candidate list (the default: Tab moves the highlight, Enter takes the candidate)",
        body: |h| {
            h.type_text("fn");
            h.tab();
        },
        expect: |t| {
            assert!(
                t.commits.is_empty(),
                "Tab writes nothing, it selects: {:?}",
                t.commits
            );
            assert_eq!(t.selected, 1, "Tab moved the highlight");
            assert_eq!(
                t.eaten,
                ["Tab"],
                "the Tab key belonged to the candidate window"
            );
        },
    },
    Scenario {
        id: "IME-S06B",
        function: "F-SELECT",
        title: "Enter takes the candidate that Tab highlighted",
        body: |h| {
            // The factory mode is the code input: the natural language input (the one whose list takes the
            // keys) is one Ctrl+F12 away.
            assert!(h.mode_chord(), "Ctrl+F12 is ours");
            h.type_text("fn");
            h.tab();
            h.enter();
        },
        expect: |t| {
            assert_eq!(t.commits, ["c1:fn"], "{t:?}");
            assert_eq!(
                t.eaten,
                ["Ctrl+F12", "Tab", "Enter"],
                "the chord and both keys belonged to the input method"
            );
        },
    },
    Scenario {
        id: "IME-S07",
        function: "F-SELECT",
        title: "With the setting 'Tab belongs to the host' the key goes back to the host",
        body: |h| {
            h.type_text("fn");
            h.tab();
        },
        expect: |t| {
            assert!(t.commits.is_empty(), "nothing was written: {:?}", t.commits);
            assert_eq!(t.passed, ["f", "n", "Tab"], "the Tab key reached the host");
        },
    },
    Scenario {
        id: "IME-S08",
        function: "F-COMPOSE",
        title: "Ctrl+Alt+Space opens the explicit composition over the typed prefix",
        body: |h| {
            h.type_text("fn");
            h.compose_chord();
        },
        expect: |t| {
            assert_eq!(t.begins, 1, "one composition was opened: {t:?}");
            assert_eq!(t.eaten, ["Ctrl+Alt+Space"], "the chord belonged to the IME");
        },
    },
    Scenario {
        id: "IME-S09",
        function: "F-COMPOSE",
        title: "Without candidates the chord belongs to the host",
        body: |h| {
            h.compose_chord();
        },
        expect: |t| {
            assert_eq!(t.passed, ["Ctrl+Alt+Space"], "{t:?}");
            assert_eq!(t.begins, 0, "no composition was opened");
        },
    },
    Scenario {
        id: "IME-S10",
        function: "F-INPUT-MODE",
        title: "Alt+F1 switches between direct mode and input mode",
        body: |h| {
            h.input_mode_chord();
            h.input_mode_chord();
        },
        expect: |t| {
            assert!(!t.input_mode, "two switches are back in direct mode: {t:?}");
            assert_eq!(t.eaten, ["Alt+F1", "Alt+F1"]);
        },
    },
    Scenario {
        id: "IME-S11",
        function: "F-SHIFT-TAP",
        title: "A Shift tap pauses the input method",
        body: |h| {
            h.press(0x10, None, super::NONE, "Shift");
        },
        expect: |t| {
            assert!(t.paused, "the input method is paused: {t:?}");
            assert!(!t.visible, "a paused input method shows no candidates");
            assert_eq!(t.eaten.len(), 0, "Shift is never eaten");
        },
    },
    Scenario {
        id: "IME-S12",
        function: "F-SECURE",
        title: "A password field collects nothing",
        body: |h| {
            h.type_text("secret");
        },
        expect: |t| {
            assert!(!t.visible, "no window over a password field: {t:?}");
            assert!(
                t.queries.is_empty(),
                "the engine was never asked: {:?}",
                t.queries
            );
            assert_eq!(t.doc, "secret", "the host got the characters");
        },
    },
    Scenario {
        id: "IME-S13",
        function: "F-ACTIVATE",
        title: "A focus change closes the window and forgets the prefix",
        body: |h| {
            h.type_text("fn");
            h.ctl.focus_changed();
        },
        expect: |t| {
            assert!(!t.visible, "{t:?}");
            assert!(!t.degraded);
        },
    },
    Scenario {
        id: "IME-S14",
        function: "F-DEGRADE",
        title: "A host that refuses the write hands the keyboard back",
        body: |h| {
            h.type_text("fn");
            h.digit('1');
        },
        expect: |t| {
            assert!(t.degraded, "the input method is degraded: {t:?}");
            assert!(t.commits.is_empty(), "nothing was written");
            assert_eq!(
                t.passed.last().map(|s| s.as_str()),
                Some("1"),
                "the digit went back to the host"
            );
        },
    },
    Scenario {
        id: "IME-S15",
        function: "F-ACTIVATE",
        title: "Deactivating leaves no window and no open composition",
        body: |h| {
            h.type_text("fn");
            h.ctl.deactivate();
        },
        expect: |t| {
            assert!(!t.visible, "{t:?}");
        },
    },
    Scenario {
        id: "IME-S16",
        function: "F-COMPOSE",
        title: "A composition the host ended behind our back is closed, not left open",
        body: |h| {
            h.type_text("fn");
            h.ctl.composition_terminated();
            h.type_text("x");
        },
        expect: |t| {
            assert_eq!(
                t.ends, 0,
                "the host ended the composition itself: the IME must not end it again: {t:?}"
            );
            assert_eq!(t.later_ends, 0, "{t:?}");
            assert_eq!(
                t.doc, "fnx",
                "typing goes on after the composition ended: {t:?}"
            );
            assert!(
                t.rows.first().is_some_and(|r| r.contains("c0:x")),
                "the stale candidates are gone: {t:?}"
            );
        },
    },
    Scenario {
        id: "IME-S17",
        function: "F-HOST-ACTION",
        title: "The host takes a row of the list (the UI element behaviour)",
        body: |h| {
            h.type_text("fn");
            h.list_action(ListAction::Take(1));
        },
        expect: |t| {
            assert_eq!(t.commits, ["c1:fn"], "{t:?}");
            assert_eq!(t.doc, "c1:fn");
        },
    },
    Scenario {
        id: "IME-S18",
        function: "F-UI-ELEMENT",
        title: "The host aborts the list: nothing is written and the window is closed",
        body: |h| {
            h.type_text("fn");
            h.list_action(ListAction::Abort);
        },
        expect: |t| {
            assert!(t.commits.is_empty(), "{t:?}");
            assert!(!t.visible);
        },
    },
    Scenario {
        id: "IME-S19",
        function: "F-RESOLVE",
        title: "The language of the file decides which candidates are asked for",
        body: |h| {
            h.type_text("id");
        },
        expect: |t| {
            // the engine is asked after every character (the prefix grows), always in the language of the file
            assert_eq!(
                t.queries.last().map(String::as_str),
                Some("id"),
                "the engine was asked: {t:?}"
            );
            assert!(
                t.languages.iter().all(|l| l == "plsql"),
                "the engine was asked in the language of the file: {t:?}"
            );
            assert_eq!(t.languages.len(), t.queries.len(), "{t:?}");
        },
    },
    Scenario {
        id: "IME-S20",
        function: "F-ENGINE",
        title: "A candidate that cannot be inserted is not taken by a digit",
        body: |h| {
            h.type_text("fn");
            h.digit('4');
        },
        expect: |t| {
            assert!(
                t.commits.is_empty(),
                "the invalid row was not written: {:?}",
                t.commits
            );
            assert_eq!(
                t.passed.last().map(|s| s.as_str()),
                Some("4"),
                "the digit went back to the host"
            );
        },
    },
    Scenario {
        id: "IME-S21",
        function: "F-PLUGIN-LANGUAGE",
        title: "A plugin language produces the candidates of its own pack",
        body: |h| {
            h.type_text("mat");
        },
        expect: |t| {
            assert_eq!(
                t.queries.last().map(String::as_str),
                Some("mat"),
                "the plugin language reached the engine: {t:?}"
            );
            assert!(
                t.languages.iter().all(|l| l == "cypher"),
                "the plugin language reached the engine: {t:?}"
            );
            assert!(t.visible, "the window is on screen");
        },
    },
    Scenario {
        id: "IME-S22",
        function: "F-LASER",
        title: "A character that is written is lit up where it appeared",
        body: |h| {
            // The Sogou configuration: the candidates are captured and a composition is opened automatically.
            h.ctl.set_input_mode(true);
            h.type_text("fn");
            // The first candidate is taken with the digit: the text is written into the document.
            h.digit('1');
        },
        expect: |t| {
            assert!(
                t.lit.len() == 1,
                "the character that was written was lit up once: {t:?}"
            );
            let anchor = t.lit[0].expect("the place of the character is known");
            assert_eq!(
                (anchor.x, anchor.y),
                (t.anchor_x, t.anchor_y),
                "the light shines on the place the character appeared at: {t:?}"
            );
        },
    },
];
