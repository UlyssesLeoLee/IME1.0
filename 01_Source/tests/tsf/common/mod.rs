//! Shared harness for the IME (TSF text service) test scripts.
//!
//! The text service is driven the way a TSF host drives it: `OnTestKeyDown`, then `OnKeyDown` only when the test
//! reported "eaten", then `OnTestKeyUp` / `OnKeyUp`. The document is a plain string, the candidate window is the
//! recording double of `pime_tsf::ui`, and the prediction core is a scripted port - so every scenario is
//! deterministic, needs no window and no Windows message loop, and runs in milliseconds.
//!
//! The scripts live in `01_Source/tests/tsf` (one home for every test script, like `tests/rust` for pime-core and
//! `tests/setup` for pime-setup) and are registered as `[[test]]` targets in `crates/pime-tsf/Cargo.toml`.

#![allow(dead_code)]

pub mod scenarios;

use std::cell::RefCell;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::rc::Rc;

use pime_core::candidate::{Candidate, CandidateSource, Validation};
use pime_core::input::{CoreFault, PredictPort, PredictRequest};
use pime_tsf::controller::{HostError, HostText, KeyInput, TipConfig, TipController};
use pime_tsf::keymap::{Modifiers, vk};
use pime_tsf::ui::{Anchor, CandidateView, ListAction, RecordingUi};

/// The caret position every scenario uses unless it says otherwise.
pub const ANCHOR: Anchor = Anchor {
    x: 100,
    y: 200,
    line_height: 18,
};

/// No modifier is held.
pub const NONE: Modifiers = Modifiers {
    shift: false,
    ctrl: false,
    alt: false,
    win: false,
};

/// The virtual-key code of a printable ASCII character (the US layout the test uses).
pub fn code_of(c: char) -> u32 {
    match c {
        'a'..='z' => 0x41 + (c as u32 - 'a' as u32),
        'A'..='Z' => 0x41 + (c as u32 - 'A' as u32),
        '0'..='9' => 0x30 + (c as u32 - '0' as u32),
        ' ' => vk::SPACE,
        _ => 0xBC,
    }
}

// ------------------------------------------------------------------ the document (the "host")

/// The document the fake host keeps: a string with the caret at the end.
#[derive(Debug, Default, Clone)]
pub struct FakeHost {
    pub doc: String,
    /// A composition is open in the document.
    pub composing: bool,
    /// The field is a password field (or the host cannot tell - the contract says: fail closed).
    pub secure: bool,
    /// Where the caret is; None when the host cannot say.
    pub anchor: Option<Anchor>,
    /// The host refuses to open a composition.
    pub refuse_begin: bool,
    /// The host refuses to write.
    pub refuse_commit: bool,
    /// The host refuses to end the composition synchronously.
    pub refuse_end: bool,
    /// The host refuses the asynchronous retry as well.
    pub refuse_later: bool,
    /// The text before the caret is not what the machine observed.
    pub mismatch: bool,
    pub begins: usize,
    pub ends: usize,
    pub later_ends: usize,
    pub secure_checks: usize,
    /// Every write the host accepted: (the typed prefix, the text written).
    pub commits: Vec<(String, String)>,
}

impl FakeHost {
    pub fn with_doc(doc: &str) -> FakeHost {
        FakeHost {
            doc: doc.to_string(),
            anchor: Some(ANCHOR),
            ..FakeHost::default()
        }
    }
}

impl HostText for FakeHost {
    fn is_secure_field(&mut self) -> bool {
        self.secure_checks += 1;
        self.secure
    }

    fn caret_anchor(&mut self) -> Option<Anchor> {
        self.anchor
    }

    fn begin_composition(&mut self, typed: &str) -> Result<(), HostError> {
        self.begins += 1;
        if self.refuse_begin {
            return Err(HostError::Refused);
        }
        if self.mismatch || !self.doc.ends_with(typed) {
            return Err(HostError::Mismatch);
        }
        self.composing = true;
        Ok(())
    }

    fn end_composition(&mut self) -> Result<(), HostError> {
        self.ends += 1;
        if self.refuse_end {
            return Err(HostError::Refused); // the composition stays open in the document
        }
        self.composing = false;
        Ok(())
    }

    fn end_composition_later(&mut self) -> Result<(), HostError> {
        self.later_ends += 1;
        if self.refuse_later {
            return Err(HostError::Refused);
        }
        self.composing = false;
        Ok(())
    }

    fn commit(&mut self, typed: &str, text: &str) -> Result<(), HostError> {
        if self.refuse_commit {
            return Err(HostError::Refused);
        }
        if self.mismatch || !self.doc.ends_with(typed) {
            return Err(HostError::Mismatch);
        }
        self.doc.truncate(self.doc.len() - typed.len());
        self.doc.push_str(text);
        self.commits.push((typed.to_string(), text.to_string()));
        Ok(())
    }
}

// ------------------------------------------------------------------ the prediction core (the "engine")

/// A scripted prediction core: the same candidates for every prefix, so a scenario can name the text it expects.
///
/// The text of a candidate is `c<i>:<prefix>`; the fourth candidate is `invalid` (the machine must not offer it).
#[derive(Debug, Clone)]
pub struct ScriptedPredictor {
    /// Every prefix the text service asked about, in order.
    pub queries: Vec<String>,
    /// The language every query was made in, in order.
    pub languages: Vec<String>,
    /// How many candidates a query returns.
    pub count: usize,
    /// Prefixes that return nothing.
    pub empty_for: Vec<String>,
    /// The text every candidate carries (overrides the default `c<i>:<prefix>`).
    pub text: Option<String>,
}

impl Default for ScriptedPredictor {
    fn default() -> ScriptedPredictor {
        ScriptedPredictor {
            queries: Vec::new(),
            languages: Vec::new(),
            count: 12,
            empty_for: Vec::new(),
            text: None,
        }
    }
}

impl ScriptedPredictor {
    /// A predictor that returns nothing for every prefix.
    pub fn silent() -> ScriptedPredictor {
        ScriptedPredictor {
            count: 0,
            ..ScriptedPredictor::default()
        }
    }
}

impl PredictPort for ScriptedPredictor {
    fn predict(&mut self, r: &PredictRequest) -> Result<Vec<Candidate>, CoreFault> {
        self.queries.push(r.prefix.clone());
        self.languages.push(r.language.clone());
        if self.empty_for.iter().any(|p| r.prefix.contains(p)) {
            return Ok(Vec::new());
        }
        Ok((0..self.count)
            .map(|i| {
                let mut c = Candidate::new(&format!("c{i}")).with_source(CandidateSource::Rule);
                c.validation = if i == 3 {
                    Validation::Invalid
                } else {
                    Validation::Valid
                };
                c.text = Some(match &self.text {
                    Some(t) => t.to_string(),
                    None => format!("c{i}:{}", r.prefix),
                });
                c
            })
            .collect())
    }
}

// ------------------------------------------------------------------ the harness

/// What the scenario scripts assert on: the visible state after the last step.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Transcript {
    /// The text of the document.
    pub doc: String,
    /// Whether the candidate window is on screen.
    pub visible: bool,
    /// The rows of the visible page: "<label>|<text>|<detail>".
    pub rows: Vec<String>,
    /// The highlighted row within the page.
    pub selected: usize,
    /// The zero-based page and the number of pages.
    pub page: usize,
    pub page_count: usize,
    /// Every text the host accepted.
    pub commits: Vec<String>,
    /// The keys the input method ate, in order.
    pub eaten: Vec<String>,
    /// The keys that reached the host, in order.
    pub passed: Vec<String>,
    /// The prefixes the input method asked the engine about.
    pub queries: Vec<String>,
    /// The languages the input method asked the engine in.
    pub languages: Vec<String>,
    /// The composition bookkeeping of the host.
    pub begins: usize,
    pub ends: usize,
    pub later_ends: usize,
    /// Whether the input method is degraded (the host refused something).
    pub degraded: bool,
    /// Whether the input method is in input mode (the mainstream behaviour) instead of direct mode.
    pub input_mode: bool,
    /// Whether the user paused the input method (a Shift tap).
    pub paused: bool,
    /// How often the window was shown and hidden.
    pub shows: usize,
    pub hides: usize,
    /// Where the characters that were written appeared (the hook of the input method).
    pub lit: Vec<Option<Anchor>>,
    /// Where the caret is according to the host.
    pub anchor_x: i32,
    pub anchor_y: i32,
}

impl Transcript {
    /// A one-line summary, used in the assertion messages and in the evidence files.
    pub fn summary(&self) -> String {
        format!(
            "doc={:?} visible={} rows={} selected={} page={}/{} commits={:?} eaten={:?} passed={:?} queries={:?}",
            self.doc,
            self.visible,
            self.rows.len(),
            self.selected,
            self.page,
            self.page_count,
            self.commits,
            self.eaten,
            self.passed,
            self.queries
        )
    }
}

/// The text service under test, plus the record of what happened.
pub struct Harness {
    pub ctl: TipController<RecordingUi, FakeHost, ScriptedPredictor>,
    /// The keys the input method ate, in order.
    pub eaten: Vec<String>,
    /// The keys that reached the host, in order.
    pub passed: Vec<String>,
    /// Where the characters that were written appeared (the hook of the input method).
    pub lit: Rc<RefCell<Vec<Option<Anchor>>>>,
    /// The label of the key that is being pressed (for the record).
    clock: u64,
}

impl Harness {
    /// A harness with the given configuration and a fresh, empty document.
    pub fn new(config: TipConfig) -> Harness {
        Harness::with(config, FakeHost::with_doc(""))
    }

    /// A harness with the given configuration and the given document.
    pub fn with(config: TipConfig, host: FakeHost) -> Harness {
        let mut ctl = TipController::new(
            RecordingUi::default(),
            host,
            ScriptedPredictor::default(),
            config,
        );
        // The light of the input method: it is told where a character appeared, so that the glow can shine on it.
        let lit: Rc<RefCell<Vec<Option<Anchor>>>> = Rc::new(RefCell::new(Vec::new()));
        let recorder = Rc::clone(&lit);
        ctl.set_commit_hook(Rc::new(move |anchor| {
            recorder.borrow_mut().push(anchor);
        }));
        ctl.activate(false);
        Harness {
            ctl,
            eaten: Vec::new(),
            passed: Vec::new(),
            lit,
            clock: 0,
        }
    }

    /// The shipped defaults (direct mode: no key is ever eaten except the explicit chord).
    pub fn defaults() -> Harness {
        Harness::new(TipConfig::default())
    }

    /// The always-listening configuration (candidate capture + automatic composition).
    pub fn sogou() -> Harness {
        Harness::new(TipConfig {
            candidate_capture: true,
            auto_compose: true,
            ..TipConfig::default()
        })
    }

    /// One physical key press, delivered the way the TSF contract says. Returns whether it was eaten.
    ///
    /// The clock advances by 20 ms per event: two presses of the same key inside 15 ms are one event for the
    /// machine (a host may ask twice about the same key), so the harness must not create them that fast.
    pub fn press(&mut self, code: u32, ch: Option<char>, mods: Modifiers, label: &str) -> bool {
        self.clock += 20;
        let down = self.event(code, ch, mods);
        let eaten = self.ctl.on_test_key_down(down) && self.ctl.on_key_down(down);
        if eaten {
            self.eaten.push(label.to_string());
        } else {
            self.passed.push(label.to_string());
            self.host_inserts(code, ch);
        }
        self.clock += 20;
        let up = self.event(code, ch, mods);
        self.ctl.on_test_key_up(up);
        self.ctl.on_key_up(up);
        eaten
    }

    fn event(&self, code: u32, ch: Option<char>, mods: Modifiers) -> KeyInput {
        KeyInput {
            code,
            lparam: ((code as isize) << 16) | 1,
            ch,
            mods,
            now_ms: self.clock,
        }
    }

    /// What the host does with a key the input method did not eat: the character goes into the document.
    fn host_inserts(&mut self, code: u32, ch: Option<char>) {
        match (code, ch) {
            (vk::BACK, _) => {
                self.ctl.host_mut().doc.pop();
            }
            (vk::RETURN, _) => self.ctl.host_mut().doc.push('\n'),
            (_, Some(c)) if !c.is_control() => self.ctl.host_mut().doc.push(c),
            _ => {}
        }
    }

    /// Types the text; every character must reach the host (the default behaviour never eats a code character).
    pub fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            let label = c.to_string();
            let eaten = self.press(code_of(c), Some(c), NONE, &label);
            assert!(!eaten, "typing {c:?} must reach the host");
        }
    }

    pub fn enter(&mut self) -> bool {
        self.press(vk::RETURN, None, NONE, "Enter")
    }
    pub fn esc(&mut self) -> bool {
        self.press(vk::ESCAPE, None, NONE, "Esc")
    }
    pub fn tab(&mut self) -> bool {
        self.press(vk::TAB, None, NONE, "Tab")
    }
    pub fn space(&mut self) -> bool {
        self.press(vk::SPACE, Some(' '), NONE, "Space")
    }
    pub fn digit(&mut self, d: char) -> bool {
        self.press(code_of(d), Some(d), NONE, &d.to_string())
    }
    pub fn up(&mut self) -> bool {
        self.press(vk::UP, None, NONE, "Up")
    }
    pub fn down(&mut self) -> bool {
        self.press(vk::DOWN, None, NONE, "Down")
    }
    pub fn page_down(&mut self) -> bool {
        self.press(vk::NEXT, None, NONE, "PageDown")
    }
    pub fn page_up(&mut self) -> bool {
        self.press(vk::PRIOR, None, NONE, "PageUp")
    }
    /// Ctrl+Alt+Space: the explicit "take these candidates" gesture.
    pub fn compose_chord(&mut self) -> bool {
        self.press(
            vk::SPACE,
            Some(' '),
            Modifiers {
                ctrl: true,
                alt: true,
                ..NONE
            },
            "Ctrl+Alt+Space",
        )
    }
    /// Alt+F1: switch between direct mode and input mode.
    pub fn input_mode_chord(&mut self) -> bool {
        self.press(vk::F1, None, Modifiers { alt: true, ..NONE }, "Alt+F1")
    }

    /// The user (or the host) acts on the candidate list.
    pub fn list_action(&mut self, action: ListAction) {
        self.ctl.apply_list_action(action);
    }

    /// The visible state right now.
    pub fn transcript(&self) -> Transcript {
        let ui = self.ctl.ui();
        let (rows, selected, page, page_count) = match &ui.last {
            Some((view, _)) => (
                view.rows
                    .iter()
                    .map(|r| format!("{}|{}|{}", r.label, r.text, r.detail))
                    .collect(),
                view.selected_in_page,
                view.page,
                view.page_count,
            ),
            None => (Vec::new(), 0, 0, 0),
        };
        let host = self.ctl.host();
        Transcript {
            doc: host.doc.clone(),
            visible: ui.visible,
            rows,
            selected,
            page,
            page_count,
            commits: host.commits.iter().map(|(_, t)| t.clone()).collect(),
            eaten: self.eaten.clone(),
            passed: self.passed.clone(),
            queries: self.ctl.predictor().queries.clone(),
            languages: self.ctl.predictor().languages.clone(),
            begins: host.begins,
            ends: host.ends,
            later_ends: host.later_ends,
            degraded: self.ctl.is_degraded(),
            input_mode: self.ctl.input_mode(),
            paused: self.ctl.is_paused(),
            shows: ui.shows,
            hides: ui.hides,
            lit: self.lit.borrow().clone(),
            anchor_x: host.anchor.map(|a| a.x).unwrap_or(0),
            anchor_y: host.anchor.map(|a| a.y).unwrap_or(0),
        }
    }

    /// The candidate window as the text service last drew it.
    pub fn view(&self) -> Option<CandidateView> {
        self.ctl.ui().last.as_ref().map(|(v, _)| v.clone())
    }

    /// Whether a composition is open in the document.
    pub fn composing(&self) -> bool {
        self.ctl.host().composing
    }
}

// ------------------------------------------------------------------ evidence

/// Writes the transcript of a scenario into `PIME_IME_EVIDENCE_DIR` when that variable is set.
///
/// The gate chain does not set it, so a normal `cargo test` writes nothing; the evidence run does, and the files land
/// in `02_Verification/ime/` where the other evidence lives.
pub fn write_evidence(id: &str, transcript: &Transcript) {
    let Ok(dir) = std::env::var("PIME_IME_EVIDENCE_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let mut out = String::new();
    let _ = writeln!(out, "{{");
    let _ = writeln!(out, "  \"id\": {},", json_str(id));
    let _ = writeln!(out, "  \"doc\": {},", json_str(&transcript.doc));
    let _ = writeln!(out, "  \"visible\": {},", transcript.visible);
    let _ = writeln!(out, "  \"selected\": {},", transcript.selected);
    let _ = writeln!(out, "  \"page\": {},", transcript.page);
    let _ = writeln!(out, "  \"pageCount\": {},", transcript.page_count);
    let _ = writeln!(out, "  \"rows\": [");
    for (i, r) in transcript.rows.iter().enumerate() {
        let comma = if i + 1 == transcript.rows.len() {
            ""
        } else {
            ","
        };
        let _ = writeln!(out, "    {}{}", json_str(r), comma);
    }
    let _ = writeln!(out, "  ],");
    let _ = writeln!(out, "  \"commits\": [");
    for (i, c) in transcript.commits.iter().enumerate() {
        let comma = if i + 1 == transcript.commits.len() {
            ""
        } else {
            ","
        };
        let _ = writeln!(out, "    {}{}", json_str(c), comma);
    }
    let _ = writeln!(out, "  ],");
    let _ = writeln!(out, "  \"eaten\": [");
    for (i, c) in transcript.eaten.iter().enumerate() {
        let comma = if i + 1 == transcript.eaten.len() {
            ""
        } else {
            ","
        };
        let _ = writeln!(out, "    {}{}", json_str(c), comma);
    }
    let _ = writeln!(out, "  ],");
    let _ = writeln!(out, "  \"passed\": [");
    for (i, c) in transcript.passed.iter().enumerate() {
        let comma = if i + 1 == transcript.passed.len() {
            ""
        } else {
            ","
        };
        let _ = writeln!(out, "    {}{}", json_str(c), comma);
    }
    let _ = writeln!(out, "  ],");
    let _ = writeln!(out, "  \"queries\": [");
    for (i, c) in transcript.queries.iter().enumerate() {
        let comma = if i + 1 == transcript.queries.len() {
            ""
        } else {
            ","
        };
        let _ = writeln!(out, "    {}{}", json_str(c), comma);
    }
    let _ = writeln!(out, "  ],");
    let _ = writeln!(
        out,
        "  \"compositions\": {{\"begins\": {}, \"ends\": {}, \"laterEnds\": {}}},",
        transcript.begins, transcript.ends, transcript.later_ends
    );
    let _ = writeln!(out, "  \"degraded\": {},", transcript.degraded);
    let _ = writeln!(out, "  \"inputMode\": {},", transcript.input_mode);
    let _ = writeln!(out, "  \"shows\": {},", transcript.shows);
    let _ = writeln!(out, "  \"hides\": {}", transcript.hides);
    let _ = writeln!(out, "}}");
    let _ = std::fs::write(dir.join(format!("{id}.json")), out);
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
