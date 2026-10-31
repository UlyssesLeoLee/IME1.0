//! The decision logic of the text input processor, with no COM in it: what a key means, what is shown, what is written.
//! `tip` (the COM glue) only forwards TSF callbacks here and implements [`HostText`] with edit sessions, so everything
//! that can be wrong about the behaviour is testable without Windows.
//!
//! Contract with TSF (docs: `ITfKeystrokeMgr::TestKeyDown`): the host calls `OnKeyDown` only for keys that
//! `OnTestKeyDown` reported as eaten. A key that is NOT eaten (ordinary typing) therefore never reaches `OnKeyDown`, yet
//! the machine has to see it to follow what the user types. So:
//! * `on_test_key_down` answers with the machine's pure `would_consume`; for a key that is not eaten it also feeds the
//!   machine (the observation) and remembers the key, so that a host which calls `OnKeyDown` as well, or the test again,
//!   does not make the machine see it twice (the test phase is idempotent: see `SAME_EVENT_MS`);
//! * `on_key_down` performs eaten keys, and observes keys it has not seen yet (a host that skips the test).
//!
//! Default behaviour (DD 5.3, UT-009): the input method never eats a code character. Typing is observed and the window
//! shows suggestions, but Space, Enter, Tab, Esc, digits and the arrows all reach the host untouched. The user enters a
//! composition explicitly with Ctrl+Alt+Space while candidates show (the chord is eaten only then); inside it the
//! mainstream keys work (digits pick within the page, PageUp/PageDown, Up/Down, Shift+Tab, Tab/Enter/Space confirm, Esc
//! cancels) and the composition ends after a commit or a cancel. `TipConfig` can switch on the always-on variants.
//!
//! Safety rules enforced here: Direct and Observing never eat (the machine guarantees it); nothing is eaten while the
//! candidate window is not visible; a failure of the host (edit session refused, text not where expected) hands the
//! keyboard back to the host until the focus moves; a panic anywhere below is contained and also ends in pass-through.

use std::panic::{AssertUnwindSafe, catch_unwind};

use pime_core::candidate::{Candidate, Validation};
use pime_core::input::{
    Effect, InputMachine, Key, KeyEvent, KeyMap, KeyOutcome, LocalPredictor, PredictPort, State,
};
use pime_core::pack::render_plain;

use crate::keymap::{Modifiers, ShiftTapDetector, breaks_context, is_compose_chord, translate};
use crate::ui::{Anchor, CandidateRow, CandidateUi, CandidateView};

/// Why a host operation did not happen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostError {
    /// The host or the document refused the edit session (read-only field, lock not granted, no context).
    Refused,
    /// The text before the caret is not what the machine observed: writing would damage the document.
    Mismatch,
}

/// What the controller needs from the document the user is typing into. The COM glue implements it with edit sessions;
/// tests implement it with a string. Every method may fail; the controller never assumes success.
pub trait HostText {
    /// True for a password / secure field, and ALSO when the host cannot tell (fail closed, DD section 3.3: collecting
    /// a password is worse than losing the candidates). The controller asks once per key event and never on a key-up;
    /// an implementation must not cache the answer across events (a field can change without any focus event).
    fn is_secure_field(&mut self) -> bool;
    /// The caret in screen coordinates; None when the host cannot say.
    fn caret_anchor(&mut self) -> Option<Anchor>;
    /// Open a composition over `typed`, the text the user typed just before the caret. Must fail with
    /// [`HostError::Mismatch`] unless the text before the caret is exactly `typed` (no blind ranges).
    fn begin_composition(&mut self, typed: &str) -> Result<(), HostError>;
    /// Close the composition and leave the text as it is (cancel). Succeeds when none is open.
    fn end_composition(&mut self) -> Result<(), HostError>;
    /// Replace `typed` (verified, just before the caret; empty means "insert at the caret") with `text` and put the
    /// caret after it. A composition stays open until [`HostText::end_composition`]. Never simulates key presses.
    fn commit(&mut self, typed: &str, text: &str) -> Result<(), HostError>;
}

/// Product choices that change which keys are eaten while the user types code. One place, no hidden defaults.
///
/// The defaults are non-destructive: with `candidate_capture` and `auto_compose` both off, no key is ever eaten except
/// Ctrl+Alt+Space while candidates show (which starts an explicit composition). The two switches turn on the
/// always-listening mainstream behaviour, at the price that code characters get eaten while suggestions are visible
/// (digits in `i32`, Space after `fn`).
#[derive(Clone, Debug)]
pub struct TipConfig {
    /// Page size, Space and punctuation paging (see `pime_core::input::KeyMap`).
    pub keymap: KeyMap,
    /// OFF by default. On: Tab/Enter/digits/arrows/Esc/PageUp/PageDown act on the visible candidates without an
    /// explicit composition (plan section 11: an explicit opt-in).
    pub candidate_capture: bool,
    /// OFF by default. On: open a TSF composition over the typed prefix as soon as candidates appear, so that Space
    /// confirms too (the Sogou behaviour). Needs a host that grants the composition; without it `candidate_capture`
    /// still works.
    pub auto_compose: bool,
    /// Language profile of the prediction engine.
    pub language: String,
}

impl Default for TipConfig {
    fn default() -> TipConfig {
        TipConfig {
            keymap: KeyMap::default(),
            candidate_capture: false,
            auto_compose: false,
            language: "rust".to_string(),
        }
    }
}

/// One keyboard event as TSF reports it. `ch` is the character the key produces with the current layout (computed by
/// the glue; None for non-character and dead keys).
#[derive(Clone, Copy, Debug)]
pub struct KeyInput {
    /// Virtual-key code (`wParam`).
    pub code: u32,
    /// `lParam` of the key message: identifies one physical event, used only to recognise a duplicate call.
    pub lparam: isize,
    pub ch: Option<char>,
    pub mods: Modifiers,
    /// Monotonic milliseconds (bookkeeping of the machine's watchdog).
    pub now_ms: u64,
}

/// Reports of one `(code, lparam)` stamp closer together than this are ONE key event: a host may ask `OnTestKeyDown`
/// twice for a key, or test and then perform it, within a few milliseconds. A held key repeats with an identical `lparam`
/// (repeat count 1, "was down" bit set) but never faster than the Windows maximum typematic rate of about 30 per second,
/// so its repeats are at least ~30 ms apart and stay separate events. Half of that is the margin.
const SAME_EVENT_MS: u64 = 15;

/// A key event the machine has already observed (it was not eaten, so `OnKeyDown` may or may not follow).
#[derive(Clone, Copy, Debug)]
struct Observed {
    stamp: (u32, isize),
    at_ms: u64,
}

const PREVIEW_CHARS: usize = 48;

/// A candidate the machine will not insert (mirrors the machine's rule; the grid test in `pime-core` pins the rule).
fn insertable(c: &Candidate) -> bool {
    c.text.is_some()
        && !matches!(
            c.validation,
            Validation::Invalid | Validation::ContextMissing
        )
}

/// One line of what would be inserted: placeholders rendered plain, first non-blank line, shortened.
fn preview(c: &Candidate) -> String {
    let shown = c
        .text
        .as_deref()
        .map(|t| render_plain(t, ""))
        .filter(|t| !t.trim().is_empty())
        .or_else(|| Some(c.label.clone()).filter(|l| !l.trim().is_empty()))
        .unwrap_or_else(|| c.id.clone());
    let mut lines = shown.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines.next().unwrap_or(shown.trim());
    let mut out: String = first.chars().take(PREVIEW_CHARS).collect();
    if first.chars().count() > PREVIEW_CHARS || lines.next().is_some() {
        out.push('…');
    }
    if out.is_empty() {
        out = c.id.clone();
    }
    out
}

/// Source and validation in words, never colour alone: "rule · valid", plus a note for rows the digit cannot insert.
fn detail(c: &Candidate) -> String {
    let sources: Vec<&str> = if c.sources.is_empty() {
        vec![c.source.as_str()]
    } else {
        c.sources.iter().map(|s| s.as_str()).collect()
    };
    let mut text = format!(
        "{} · {}",
        sources.join("+").replace('_', " "),
        c.validation.as_str().replace('_', " ")
    );
    if !insertable(c) {
        text.push_str(" · cannot insert");
    }
    text
}

/// What the candidate window shows for the machine's current page; None when there is nothing to show.
pub fn candidate_view<P: PredictPort>(machine: &InputMachine<P>) -> Option<CandidateView> {
    let items = machine.page_items();
    if items.is_empty() {
        return None;
    }
    let page_size = machine.keymap().page_size.clamp(1, 9);
    let rows = items
        .iter()
        .enumerate()
        .map(|(i, c)| CandidateRow {
            label: (i + 1).to_string(),
            text: preview(c),
            detail: detail(c),
        })
        .collect();
    Some(CandidateView {
        rows,
        selected_in_page: machine.selected() % page_size,
        page: machine.page(),
        page_count: machine.page_count().max(1),
    })
}

pub struct TipController<U: CandidateUi, H: HostText, P: PredictPort = LocalPredictor> {
    machine: InputMachine<P>,
    ui: U,
    host: H,
    config: TipConfig,
    shift: ShiftTapDetector,
    /// Whether the machine has been told this is a secure context (also while `secure_mode`).
    secure: bool,
    /// Activated on the secure desktop (`TF_TMAE_SECUREMODE`): nothing is ever collected.
    secure_mode: bool,
    /// A host operation failed: every key passes through until the focus moves.
    degraded: bool,
    visible: bool,
    /// The host refused a composition for the candidates now showing: stay in plain candidate mode.
    composition_refused: bool,
    /// A composition is open in the host (the machine's own flag cannot see a panic that skipped the closing call).
    host_composing: bool,
    last_anchor: Option<Anchor>,
    /// The key `on_test_key_down` just observed (it is not eaten, so `OnKeyDown` may or may not follow).
    seen: Option<Observed>,
    /// The key event the host was last asked about the secure field for (see `refresh_secure`).
    probed: Option<Observed>,
    now_ms: u64,
}

impl<U: CandidateUi, H: HostText, P: PredictPort> TipController<U, H, P> {
    pub fn new(ui: U, host: H, port: P, config: TipConfig) -> TipController<U, H, P> {
        TipController {
            machine: InputMachine::new(port, &config.language),
            ui,
            host,
            config,
            shift: ShiftTapDetector::default(),
            secure: false,
            secure_mode: false,
            degraded: false,
            visible: false,
            composition_refused: false,
            host_composing: false,
            last_anchor: None,
            seen: None,
            probed: None,
            now_ms: 0,
        }
    }

    pub fn host(&self) -> &H {
        &self.host
    }

    pub fn host_mut(&mut self) -> &mut H {
        &mut self.host
    }

    pub fn ui(&self) -> &U {
        &self.ui
    }

    pub fn predictor(&self) -> &P {
        self.machine.port()
    }

    pub fn machine(&self) -> &InputMachine<P> {
        &self.machine
    }

    pub fn is_paused(&self) -> bool {
        self.machine.is_paused()
    }

    pub fn is_degraded(&self) -> bool {
        self.degraded
    }

    /// The thread activated the text service. `secure_mode`: `ActivateEx` reported `TF_TMAE_SECUREMODE`.
    pub fn activate(&mut self, secure_mode: bool) {
        self.guarded((), |c| {
            c.secure_mode = secure_mode;
            c.machine.set_keymap(c.config.keymap);
            c.machine.set_candidate_capture(c.config.candidate_capture);
            c.machine.grant_observation(true);
            if secure_mode {
                c.secure = true;
                let effects = c.machine.set_secure_context(true);
                c.apply(effects);
            }
        });
    }

    /// The text service is being deactivated: leave nothing behind (no window, no open composition).
    pub fn deactivate(&mut self) {
        self.guarded((), |c| {
            c.reset();
            c.machine.grant_observation(false);
            c.hide();
            c.last_anchor = None;
        });
    }

    /// The focus moved to another document or window: forget everything about the old one and give a degraded
    /// machine a fresh start.
    pub fn focus_changed(&mut self) {
        self.guarded((), |c| {
            c.reset();
            c.hide();
            c.shift.reset();
            c.seen = None;
            c.probed = None;
            c.last_anchor = None;
            c.composition_refused = false;
            c.degraded = false;
            c.machine.core_recovered(); // also leaves a fault the machine ran into itself (a failing predictor)
        });
    }

    /// The host or another input method ended our composition behind our back: cancel without touching the host.
    pub fn composition_terminated(&mut self) {
        self.guarded((), |c| {
            c.reset_machine_only();
            c.hide();
        });
    }

    /// `OnTestKeyDown`: is this key ours? Eaten keys are decided without changing anything; other keys are observed.
    pub fn on_test_key_down(&mut self, key: KeyInput) -> bool {
        self.guarded(false, |c| c.key_down(key, true))
    }

    /// `OnKeyDown`: perform the key. Returns whether it was eaten (a failed commit hands the key back to the host).
    pub fn on_key_down(&mut self, key: KeyInput) -> bool {
        self.guarded(false, |c| c.key_down(key, false))
    }

    /// `OnTestKeyUp`: key-ups are never eaten; a completed Shift tap toggles the pause.
    pub fn on_test_key_up(&mut self, key: KeyInput) -> bool {
        self.guarded(false, |c| c.key_up(key))
    }

    /// `OnKeyUp`: the same as [`TipController::on_test_key_up`] (a repeated report of one release is harmless).
    pub fn on_key_up(&mut self, key: KeyInput) -> bool {
        self.guarded(false, |c| c.key_up(key))
    }

    /// Run `f`; a panic (host, window or machine) is contained and degrades to pass-through.
    fn guarded<R>(&mut self, fallback: R, f: impl FnOnce(&mut Self) -> R) -> R {
        match catch_unwind(AssertUnwindSafe(|| f(self))) {
            Ok(value) => value,
            Err(_) => {
                self.fail_safe();
                fallback
            }
        }
    }

    /// After a panic nothing can be trusted: no window, no host calls, every key to the host until the focus moves.
    fn fail_safe(&mut self) {
        self.degraded = true;
        self.visible = false;
        let now = self.now_ms;
        let _ = catch_unwind(AssertUnwindSafe(|| self.ui.hide()));
        let _ = catch_unwind(AssertUnwindSafe(|| self.machine.report_core_failure(now)));
        let _ = catch_unwind(AssertUnwindSafe(|| self.end_host_composition()));
    }

    fn key_down(&mut self, key: KeyInput, test_only: bool) -> bool {
        self.now_ms = key.now_ms;
        self.shift.feed(key.code, true, key.mods);
        if self.degraded {
            return false;
        }
        let stamp = (key.code, key.lparam);
        let reported_again = if test_only {
            // the test phase is idempotent: a second `OnTestKeyDown` for an event that was already observed must not feed
            // the machine again (it would see "aa" for one 'a'). Nothing is cleared, so the `OnKeyDown` that may follow is
            // still recognised below.
            self.seen.is_some_and(|s| {
                s.stamp == stamp && key.now_ms.saturating_sub(s.at_ms) <= SAME_EVENT_MS
            })
        } else {
            // already observed by the OnTestKeyDown of the same event
            self.seen.take().is_some_and(|s| s.stamp == stamp)
        };
        if reported_again {
            return false;
        }
        self.refresh_secure(stamp, key.now_ms);
        if is_compose_chord(key.code, key.mods) && self.can_compose() {
            // the one chord that is ours: it starts an explicit composition, and only while candidates show
            self.seen = None;
            return test_only || self.begin_composition();
        }
        let Some(ev) = translate(key.code, key.ch, key.mods) else {
            self.seen = None;
            if breaks_context(key.code, key.mods) {
                self.reset();
            }
            return false;
        };
        self.seen = None;
        self.prepare();
        if test_only {
            if self.machine.would_consume(ev) {
                return true;
            }
            self.run_machine(key, ev);
            self.seen = Some(Observed {
                stamp,
                at_ms: key.now_ms,
            });
            return false;
        }
        self.run_machine(key, ev)
    }

    fn key_up(&mut self, key: KeyInput) -> bool {
        self.now_ms = key.now_ms;
        self.seen = None;
        self.probed = None; // a key-up asks nothing; it only makes sure the next press of this key asks again
        if self.shift.feed(key.code, false, key.mods) {
            let step = self
                .machine
                .on_key(KeyEvent::plain(Key::ShiftTap), key.now_ms);
            self.apply(step.effects);
            self.settle();
        }
        false
    }

    /// Feed one translated key to the machine and perform what it asks. Returns whether the key is eaten.
    fn run_machine(&mut self, key: KeyInput, ev: KeyEvent) -> bool {
        let step = self.machine.on_key(ev, key.now_ms);
        let eaten = step.outcome == KeyOutcome::Consumed;
        let committed = step
            .effects
            .iter()
            .any(|e| matches!(e, Effect::Commit { .. }));
        if !self.apply(step.effects) {
            self.degrade();
            return false;
        }
        if committed || (!eaten && breaks_context(key.code, key.mods)) {
            // the document no longer matches what the machine observed (committed text, caret moved, paste...)
            self.reset();
        }
        self.settle();
        eaten
    }

    /// Open the composition over the typed prefix on the first key after candidates appeared. Doing it now and not when
    /// they appear is deliberate: the host inserts a typed character only after the key callback returned, so the text
    /// before the caret matches the prefix only at the next key event.
    fn prepare(&mut self) {
        if self.config.auto_compose && !self.composition_refused && self.can_compose() {
            self.begin_composition();
        }
    }

    /// Candidates are showing and nothing stops a composition over them.
    fn can_compose(&self) -> bool {
        !self.degraded
            && !self.secure
            && self.visible
            && self.machine.state() == State::Candidates
            && !self.machine.candidates().is_empty()
    }

    /// Open the host composition over the typed prefix and move the machine to COMPOSING. A refusal is not a failure of
    /// the host: the candidates stay as they are and the refusal is remembered until they go.
    fn begin_composition(&mut self) -> bool {
        let typed = self.machine.typed_prefix().to_string();
        if self.host.begin_composition(&typed).is_err() {
            self.composition_refused = true;
            return false;
        }
        self.host_composing = true;
        if self.machine.begin_composition(true).is_none() {
            self.end_host_composition();
            return false;
        }
        true
    }

    /// Ask the host whether the field is a password field: once per key event, never on a key-up. No answer outlives the
    /// event it was given for, because a host can turn a text field into a password field (or back) without any focus
    /// event. The perform call of an event (and a repeated report of it, see `SAME_EVENT_MS`) reuses the answer of its
    /// test call, so one key never probes twice; the next key asks again.
    fn refresh_secure(&mut self, stamp: (u32, isize), now_ms: u64) {
        let probed = self
            .probed
            .is_some_and(|p| p.stamp == stamp && now_ms.saturating_sub(p.at_ms) <= SAME_EVENT_MS);
        if probed {
            return;
        }
        self.probed = Some(Observed {
            stamp,
            at_ms: now_ms,
        });
        let secure = self.secure_mode || self.host.is_secure_field();
        if secure != self.secure {
            self.secure = secure;
            let effects = self.machine.set_secure_context(secure);
            self.apply(effects);
            self.settle();
        }
    }

    /// Perform the machine's effects on the window and the host. False when a commit could not be written.
    fn apply(&mut self, effects: Vec<Effect>) -> bool {
        let mut written = true;
        for effect in effects {
            match effect {
                Effect::ShowCandidates | Effect::SelectionMoved(_) => self.show_view(),
                Effect::HideCandidates => self.hide(),
                // the host side was opened by `begin_composition` before the machine moved to COMPOSING
                Effect::BeginComposition | Effect::ModeChanged { .. } => {}
                Effect::EndComposition => self.end_host_composition(),
                Effect::Commit { text, .. } => {
                    let typed = self.machine.typed_prefix().to_string();
                    written &= self.host.commit(&typed, &render_plain(&text, "")).is_ok();
                }
            }
        }
        written
    }

    fn end_host_composition(&mut self) {
        if self.host_composing {
            self.host_composing = false;
            let _ = self.host.end_composition();
        }
    }

    fn show_view(&mut self) {
        let Some(view) = candidate_view(&self.machine) else {
            self.hide();
            return;
        };
        let anchor = self.host.caret_anchor().or(self.last_anchor);
        let Some(anchor) = anchor else {
            return; // cannot place the window: `settle` then cancels, because nothing may be eaten behind a hidden window
        };
        self.last_anchor = Some(anchor);
        // `visible` is what the USER sees, not what was asked for: a window that could not be drawn (creation failed, no
        // monitor, nothing to lay out) is a hidden window, and `settle` cancels exactly as for a missing anchor
        self.visible = self.ui.show(&view, anchor);
    }

    fn hide(&mut self) {
        self.ui.hide();
        self.visible = false;
        self.composition_refused = false;
    }

    /// Invariants that must hold when control returns to the host.
    fn settle(&mut self) {
        if self.visible && self.machine.candidates().is_empty() {
            // the window never outlives the list it shows (belt and braces on top of the machine's HIDE effects)
            self.hide();
        }
        if self.machine.state() == State::Composing && self.machine.candidates().is_empty() {
            // nothing left to confirm: close the composition like Esc would
            let step = self.machine.on_key(KeyEvent::plain(Key::Esc), self.now_ms);
            self.apply(step.effects);
        }
        if !self.machine.candidates().is_empty() && !self.visible {
            self.reset();
        }
    }

    /// Drop the machine's context, candidates, composition and the window; keep the user's pause, the secure flag and
    /// a fault. The machine has no "clear" of its own: suspending for a secure context clears all of it, and putting
    /// the flag back restores the state the flags call for.
    fn reset(&mut self) {
        let effects = self.machine.set_secure_context(true);
        self.apply(effects);
        let effects = self.machine.set_secure_context(self.secure);
        self.apply(effects);
        self.end_host_composition();
        self.hide();
    }

    /// `reset` for a composition the host already ended: no host call.
    fn reset_machine_only(&mut self) {
        self.host_composing = false;
        let effects = self.machine.set_secure_context(true);
        let effects: Vec<Effect> = effects
            .into_iter()
            .filter(|e| *e != Effect::EndComposition)
            .collect();
        self.apply(effects);
        let effects = self.machine.set_secure_context(self.secure);
        self.apply(effects);
    }

    /// A host operation failed: tell the machine and hand the keyboard back until the focus moves.
    fn degrade(&mut self) {
        self.degraded = true;
        let effects = self.machine.report_core_failure(self.now_ms);
        self.apply(effects);
        self.end_host_composition();
        self.hide();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::vk;
    use crate::ui::RecordingUi;
    use pime_core::candidate::CandidateSource;
    use pime_core::input::{CoreFault, PredictRequest};

    const ANCHOR: Anchor = Anchor {
        x: 100,
        y: 200,
        line_height: 18,
    };
    const NONE: Modifiers = Modifiers {
        shift: false,
        ctrl: false,
        alt: false,
        win: false,
    };

    /// Twelve candidates (three pages of five) for every prefix; "zz" has none; "boom" panics.
    #[derive(Default)]
    struct FakePredictor {
        calls: Vec<String>,
    }

    impl PredictPort for FakePredictor {
        fn predict(&mut self, r: &PredictRequest) -> Result<Vec<Candidate>, CoreFault> {
            self.calls.push(r.prefix.clone());
            if r.prefix.contains("boom") {
                panic!("predictor panicked (test)");
            }
            if r.prefix.contains("zz") {
                return Ok(Vec::new());
            }
            Ok((0..12)
                .map(|i| {
                    let mut c = Candidate::new(&format!("c{i}")).with_source(CandidateSource::Rule);
                    c.validation = if i == 3 {
                        Validation::Invalid
                    } else {
                        Validation::Valid
                    };
                    c.text = Some(format!("T{i}:{}", r.prefix));
                    c
                })
                .collect())
        }
    }

    #[derive(Default)]
    struct FakeHost {
        doc: String,
        composing: bool,
        secure: bool,
        anchor: Option<Anchor>,
        fail_begin: bool,
        fail_commit: bool,
        panic_commit: bool,
        begins: usize,
        ends: usize,
        secure_checks: usize,
        commits: Vec<(String, String)>,
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
            if self.fail_begin {
                return Err(HostError::Refused);
            }
            if !self.doc.ends_with(typed) {
                return Err(HostError::Mismatch);
            }
            self.composing = true;
            Ok(())
        }

        fn end_composition(&mut self) -> Result<(), HostError> {
            self.ends += 1;
            self.composing = false;
            Ok(())
        }

        fn commit(&mut self, typed: &str, text: &str) -> Result<(), HostError> {
            if self.panic_commit {
                panic!("host panicked (test)");
            }
            if self.fail_commit {
                return Err(HostError::Refused);
            }
            if !self.doc.ends_with(typed) {
                return Err(HostError::Mismatch);
            }
            self.doc.truncate(self.doc.len() - typed.len());
            self.doc.push_str(text);
            self.commits.push((typed.to_string(), text.to_string()));
            Ok(())
        }
    }

    /// How a host drives the key sink. The documented contract is `Contract`; the others exist because the machine must
    /// behave the same under any of them.
    #[derive(Clone, Copy, Debug)]
    enum Host {
        /// OnTestKeyDown; OnKeyDown only when it reported "eaten".
        Contract,
        /// Both calls for every key, whatever the test said.
        Always,
        /// OnKeyDown without a test.
        DownOnly,
    }

    struct Rig {
        ctl: TipController<RecordingUi, FakeHost, FakePredictor>,
        t: u64,
    }

    fn input(code: u32, ch: Option<char>, mods: Modifiers, t: u64) -> KeyInput {
        KeyInput {
            code,
            lparam: ((code as isize) << 16) | 1,
            ch,
            mods,
            now_ms: t,
        }
    }

    fn code_of(c: char) -> u32 {
        match c {
            'a'..='z' => 0x41 + (c as u32 - 'a' as u32),
            '0'..='9' => 0x30 + (c as u32 - '0' as u32),
            ' ' => vk::SPACE,
            _ => 0xBC,
        }
    }

    fn sogou() -> TipConfig {
        TipConfig {
            candidate_capture: true,
            auto_compose: true,
            ..TipConfig::default()
        }
    }

    impl Rig {
        fn with(config: TipConfig) -> Rig {
            let host = FakeHost {
                anchor: Some(ANCHOR),
                ..FakeHost::default()
            };
            let mut ctl = TipController::new(
                RecordingUi::default(),
                host,
                FakePredictor::default(),
                config,
            );
            ctl.activate(false);
            Rig { ctl, t: 0 }
        }

        /// The always-listening Sogou-style configuration (both switches on).
        fn new() -> Rig {
            Rig::with(sogou())
        }

        /// The shipped defaults.
        fn defaults() -> Rig {
            Rig::with(TipConfig::default())
        }

        /// The Ctrl+Alt+Space chord; returns whether it was eaten.
        fn chord(&mut self, style: Host) -> bool {
            let mods = Modifiers {
                ctrl: true,
                alt: true,
                ..NONE
            };
            self.press(vk::SPACE, None, mods, style)
        }

        /// The host's own behaviour for a key the sink did not eat.
        fn host_inserts(&mut self, code: u32, ch: Option<char>) {
            match (code, ch) {
                (vk::BACK, _) => {
                    self.ctl.host.doc.pop();
                }
                (vk::RETURN, _) => self.ctl.host.doc.push('\n'),
                (_, Some(c)) if !c.is_control() => self.ctl.host.doc.push(c),
                _ => {}
            }
        }

        /// One physical key press (down and up) delivered the way `style` does. Returns whether it was eaten.
        fn press(&mut self, code: u32, ch: Option<char>, mods: Modifiers, style: Host) -> bool {
            self.t += 1;
            let down = input(code, ch, mods, self.t);
            let eaten = match style {
                Host::Contract => self.ctl.on_test_key_down(down) && self.ctl.on_key_down(down),
                Host::Always => {
                    let test = self.ctl.on_test_key_down(down);
                    let real = self.ctl.on_key_down(down);
                    test && real
                }
                Host::DownOnly => self.ctl.on_key_down(down),
            };
            if !eaten {
                self.host_inserts(code, ch);
            }
            self.t += 1;
            let up = input(code, ch, mods, self.t);
            self.ctl.on_test_key_up(up);
            self.ctl.on_key_up(up);
            eaten
        }

        fn type_text(&mut self, text: &str, style: Host) {
            for c in text.chars() {
                let eaten = self.press(code_of(c), Some(c), NONE, style);
                assert!(!eaten, "typing {c:?} must reach the host");
            }
        }

        fn plain(&mut self, code: u32) -> bool {
            self.press(code, None, NONE, Host::Contract)
        }

        fn digit(&mut self, d: char) -> bool {
            self.press(code_of(d), Some(d), NONE, Host::Contract)
        }

        fn view(&self) -> CandidateView {
            self.ctl
                .ui()
                .last
                .as_ref()
                .expect("a window was shown")
                .0
                .clone()
        }
    }

    #[test]
    fn typing_shows_a_window_of_labelled_rows_without_eating_anything() {
        let mut rig = Rig::new();
        rig.type_text("fn", Host::Contract);
        assert!(rig.ctl.ui().visible);
        let (view, anchor) = rig.ctl.ui().last.clone().unwrap();
        assert_eq!(anchor, ANCHOR);
        assert_eq!(
            rig.ctl.host().doc,
            "fn",
            "the host got every character itself"
        );
        let labels: Vec<_> = view.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["1", "2", "3", "4", "5"]);
        assert_eq!(
            (view.page, view.page_count, view.selected_in_page),
            (0, 3, 0)
        );
        for row in &view.rows {
            assert!(!row.text.is_empty() && !row.detail.is_empty(), "{row:?}");
        }
        assert_eq!(view.rows[0].text, "T0:fn");
        assert_eq!(view.rows[0].detail, "rule · valid");
        assert_eq!(
            view.rows[3].detail, "rule · invalid · cannot insert",
            "state is said in words"
        );
    }

    #[test]
    fn a_long_multi_line_template_is_one_short_line_in_the_window() {
        let mut c = Candidate::new("x");
        c.text = Some(format!(
            "fn ${{name}}() {{\n    ${{cursor}}\n}}{}",
            "x".repeat(80)
        ));
        assert_eq!(preview(&c), "fn name() {…");
        c.text = Some("y".repeat(100));
        assert_eq!(preview(&c).chars().count(), PREVIEW_CHARS + 1);
        c.text = None;
        c.label = "the label".into();
        assert_eq!(preview(&c), "the label");
        c.label = " ".into();
        assert_eq!(preview(&c), "x", "never empty: the id is the last resort");
        c.sources = vec![CandidateSource::UserRule, CandidateSource::Ai];
        assert_eq!(detail(&c), "user rule+ai · unchecked · cannot insert");
    }

    #[test]
    fn ut_004_esc_cancels_and_writes_nothing() {
        let mut rig = Rig::new();
        rig.type_text("fn", Host::Contract);
        assert!(rig.ctl.ui().visible);
        let before = rig.ctl.host().doc.clone();
        assert!(rig.plain(vk::ESCAPE), "Esc is eaten while candidates show");
        assert!(!rig.ctl.ui().visible, "the window is gone");
        assert_eq!(rig.ctl.host().doc, before, "nothing was written");
        assert!(rig.ctl.host().commits.is_empty());
        assert!(!rig.ctl.host().composing, "an owned composition was ended");
        assert!(
            !rig.plain(vk::ESCAPE),
            "with no candidates Esc belongs to the editor"
        );
    }

    #[test]
    fn ut_009_a_digit_commits_the_page_relative_candidate_exactly_once() {
        let mut rig = Rig::new();
        rig.type_text("fn", Host::Contract);
        assert!(rig.plain(vk::NEXT), "PageDown pages");
        assert_eq!(rig.view().page, 1);
        assert!(rig.digit('2'), "the digit is eaten");
        assert_eq!(
            rig.ctl.host().commits,
            vec![("fn".to_string(), "T6:fn".to_string())],
            "digit 2 on page 2 is candidate 7 (index 6), replacing the typed prefix"
        );
        assert_eq!(rig.ctl.host().doc, "T6:fn");
        assert!(!rig.ctl.ui().visible);
        assert!(!rig.ctl.host().composing);
        // pressing the same digit again types it: no second commit
        assert!(!rig.digit('2'));
        assert_eq!(rig.ctl.host().commits.len(), 1);
        assert_eq!(rig.ctl.host().doc, "T6:fn2");
    }

    #[test]
    fn a_digit_beyond_the_page_or_on_an_uninsertable_row_is_typed_not_eaten() {
        let mut rig = Rig::new();
        rig.type_text("fn", Host::Contract);
        assert!(!rig.digit('4'), "row 4 is invalid: the digit is the host's");
        assert!(rig.ctl.host().commits.is_empty());
        assert_eq!(rig.ctl.host().doc, "fn4");
    }

    #[test]
    fn page_down_moves_the_page_and_updates_the_window() {
        let mut rig = Rig::new();
        rig.type_text("fn", Host::Contract);
        let shows = rig.ctl.ui().shows;
        assert!(rig.plain(vk::NEXT));
        let view = rig.view();
        assert_eq!((view.page, view.selected_in_page), (1, 0));
        assert_eq!(view.rows[0].text, "T5:fn");
        assert!(rig.ctl.ui().shows > shows, "the window was redrawn");
        assert!(rig.plain(vk::PRIOR));
        assert_eq!(rig.view().page, 0);
        let down = rig.plain(vk::DOWN);
        assert!(down);
        assert_eq!(rig.view().selected_in_page, 1);
        assert!(rig.press(
            vk::TAB,
            None,
            Modifiers {
                shift: true,
                ..NONE
            },
            Host::Contract
        ));
        assert_eq!(rig.view().selected_in_page, 0, "Shift+Tab goes back");
        assert!(rig.ctl.host().commits.is_empty(), "paging writes nothing");
    }

    #[test]
    fn space_confirms_in_the_composition_and_enter_and_tab_confirm_too() {
        for (code, name) in [
            (vk::SPACE, "Space"),
            (vk::RETURN, "Enter"),
            (vk::TAB, "Tab"),
        ] {
            let mut rig = Rig::new();
            rig.type_text("fn", Host::Contract);
            let eaten = rig.press(code, Some(' '), NONE, Host::Contract);
            assert!(eaten, "{name}");
            assert_eq!(rig.ctl.host().doc, "T0:fn", "{name}");
            assert_eq!(rig.ctl.host().commits.len(), 1, "{name}");
            assert!(!rig.ctl.host().composing, "{name}");
        }
    }

    #[test]
    fn the_composition_opens_over_the_typed_prefix_at_the_next_key_event() {
        let mut rig = Rig::new();
        rig.type_text("fn", Host::Contract);
        assert_eq!(
            rig.ctl.host().begins,
            1,
            "opened by the key after 'f' made candidates"
        );
        assert_eq!(rig.ctl.machine().state(), State::Composing);
        assert!(rig.ctl.host().composing);
    }

    #[test]
    fn space_is_the_hosts_when_the_host_refuses_the_composition_but_digits_still_work() {
        let mut rig = Rig::new();
        rig.ctl.host_mut().fail_begin = true;
        rig.type_text("fn", Host::Contract);
        assert_eq!(rig.ctl.machine().state(), State::Candidates);
        assert_eq!(
            rig.ctl.host().begins,
            1,
            "refused once, not retried on every key"
        );
        assert!(
            rig.digit('1'),
            "candidate capture does not need a composition"
        );
        assert_eq!(rig.ctl.host().doc, "T0:fn");
        // Space types a space in plain candidate mode
        rig.type_text("fn", Host::Contract);
        assert!(!rig.press(vk::SPACE, Some(' '), NONE, Host::Contract));
        assert!(rig.ctl.host().doc.ends_with(' '));
    }

    #[test]
    fn with_capture_off_and_no_composition_nothing_is_ever_eaten() {
        let mut rig = Rig::defaults();
        rig.type_text("fn", Host::Contract);
        assert!(rig.ctl.ui().visible, "the window still shows");
        for code in [vk::TAB, vk::RETURN, vk::ESCAPE, vk::NEXT, vk::DOWN] {
            assert!(!rig.plain(code), "{code:#x}");
            rig.type_text("fn", Host::Contract);
        }
        assert!(!rig.digit('1'));
        assert!(rig.ctl.host().commits.is_empty());
    }

    #[test]
    fn ut_009_shift_tap_pauses_and_resumes_and_hides_the_window() {
        let mut rig = Rig::new();
        rig.type_text("fn", Host::Contract);
        assert!(rig.ctl.ui().visible);
        // a lone Shift tap
        assert!(
            !rig.press(vk::SHIFT, None, NONE, Host::Contract),
            "Shift is never eaten"
        );
        assert!(rig.ctl.is_paused());
        assert!(!rig.ctl.ui().visible, "pausing hides the window");
        assert!(!rig.ctl.host().composing, "and ends the owned composition");
        let calls = rig.ctl.predictor().calls.len();
        rig.type_text("fn", Host::Contract);
        assert_eq!(
            rig.ctl.predictor().calls.len(),
            calls,
            "nothing is predicted while paused"
        );
        assert!(!rig.ctl.ui().visible);
        assert!(
            !rig.plain(vk::ESCAPE) && !rig.digit('1'),
            "a paused IME eats nothing"
        );
        // tap again: resumes
        rig.press(vk::SHIFT, None, NONE, Host::Contract);
        assert!(!rig.ctl.is_paused());
        rig.type_text("fn", Host::Contract);
        assert!(rig.ctl.ui().visible, "predicting again");
    }

    #[test]
    fn shift_plus_a_letter_is_a_capital_not_a_pause() {
        let mut rig = Rig::new();
        let shift = Modifiers {
            shift: true,
            ..NONE
        };
        rig.t += 1;
        rig.ctl
            .on_test_key_down(input(vk::SHIFT, None, shift, rig.t));
        rig.press(0x41, Some('A'), shift, Host::Contract);
        rig.t += 1;
        rig.ctl.on_test_key_up(input(vk::SHIFT, None, NONE, rig.t));
        assert!(!rig.ctl.is_paused());
    }

    #[test]
    fn ut_009_test_and_down_never_disagree_about_eating() {
        // every key in a spread of situations, under all three host call patterns
        let keys: Vec<(u32, Option<char>, Modifiers)> = {
            let shift = Modifiers {
                shift: true,
                ..NONE
            };
            let ctrl = Modifiers { ctrl: true, ..NONE };
            let mut keys = vec![
                (vk::TAB, None, NONE),
                (vk::TAB, None, shift),
                (vk::RETURN, None, NONE),
                (vk::ESCAPE, None, NONE),
                (vk::SPACE, Some(' '), NONE),
                (vk::UP, None, NONE),
                (vk::DOWN, None, NONE),
                (vk::LEFT, None, NONE),
                (vk::PRIOR, None, NONE),
                (vk::NEXT, None, NONE),
                (vk::BACK, None, NONE),
                (0x41, Some('a'), NONE),
                (0x43, Some('\u{3}'), ctrl),
                (vk::TAB, None, ctrl),
                (0x70, None, NONE),
                (
                    vk::SPACE,
                    None,
                    Modifiers {
                        ctrl: true,
                        alt: true,
                        ..NONE
                    },
                ),
            ];
            keys.extend("0123456789".chars().map(|d| (code_of(d), Some(d), NONE)));
            keys
        };
        type Setup = fn(&mut Rig);
        let setups: [(&str, Setup); 9] = [
            ("idle", |_| {}),
            ("candidates", |r| r.type_text("fn", Host::Contract)),
            ("explicit composition", |r| {
                r.type_text("fn", Host::Contract);
                r.chord(Host::Contract);
            }),
            ("second page", |r| {
                r.type_text("fn", Host::Contract);
                r.plain(vk::NEXT);
            }),
            ("paused", |r| {
                r.type_text("fn", Host::Contract);
                r.press(vk::SHIFT, None, NONE, Host::Contract);
            }),
            ("secure", |r| {
                r.ctl.host_mut().secure = true;
                r.type_text("fn", Host::Contract);
            }),
            ("no candidates", |r| r.type_text("zz", Host::Contract)),
            ("refused composition", |r| {
                r.ctl.host_mut().fail_begin = true;
                r.type_text("fn", Host::Contract);
            }),
            ("degraded", |r| {
                r.type_text("fn", Host::Contract);
                r.ctl.degrade();
            }),
        ];
        // the shipped defaults and the always-listening configuration
        for config in [TipConfig::default(), sogou()] {
            for (name, setup) in setups {
                for &(code, ch, mods) in &keys {
                    if name == "refused composition" && is_compose_chord(code, mods) {
                        // the one intended disagreement besides a failed commit: the test cannot know whether the host
                        // will grant the composition, so the chord is eaten there and handed back by OnKeyDown
                        // (own test: the_chord_is_the_hosts_whenever_a_composition_is_impossible)
                        continue;
                    }
                    let mut a = Rig::with(config.clone());
                    setup(&mut a);
                    let mut b = Rig::with(config.clone());
                    setup(&mut b);
                    let key = input(code, ch, mods, a.t + 1);
                    let test = a.ctl.on_test_key_down(key);
                    let down_after_test = test && a.ctl.on_key_down(key);
                    let down_alone = b.ctl.on_key_down(key);
                    let what = format!(
                        "{name} (capture {}): {code:#x} {mods:?}",
                        config.candidate_capture
                    );
                    assert_eq!(test, down_alone, "{what}: test vs down alone");
                    assert_eq!(test, down_after_test, "{what}: test vs down");
                    // and the observable result is the same whichever way the host asked
                    assert_eq!(a.ctl.host().doc, b.ctl.host().doc, "{what}");
                    assert_eq!(a.ctl.host().commits, b.ctl.host().commits, "{what}");
                    assert_eq!(a.ctl.host().composing, b.ctl.host().composing, "{what}");
                    assert_eq!(
                        a.ctl.machine().candidates().len(),
                        b.ctl.machine().candidates().len(),
                        "{what}"
                    );
                }
            }
        }
    }

    #[test]
    fn ctrl_alt_space_with_candidates_starts_an_explicit_composition() {
        for style in [Host::Contract, Host::Always, Host::DownOnly] {
            let mut rig = Rig::defaults();
            rig.type_text("fn", style);
            assert!(rig.ctl.ui().visible);
            assert_eq!(rig.ctl.machine().state(), State::Candidates);
            assert!(
                rig.chord(style),
                "{style:?}: the chord is eaten while candidates show"
            );
            assert_eq!(rig.ctl.machine().state(), State::Composing);
            assert!(rig.ctl.host().composing);
            assert_eq!(rig.ctl.host().begins, 1, "{style:?}: opened exactly once");
            assert!(rig.ctl.ui().visible, "the window stays");
            assert_eq!(rig.ctl.host().doc, "fn", "the chord wrote nothing");
            assert!(rig.ctl.host().commits.is_empty());
        }
    }

    #[test]
    fn inside_the_explicit_composition_the_mainstream_keys_work() {
        let enter = || {
            let mut rig = Rig::defaults();
            rig.type_text("fn", Host::Contract);
            assert!(rig.chord(Host::Contract));
            rig
        };
        // digits pick within the current page, and the composition ends with the commit
        let mut rig = enter();
        assert!(rig.plain(vk::NEXT), "PageDown");
        assert_eq!(rig.view().page, 1);
        assert!(rig.digit('2'));
        assert_eq!(
            rig.ctl.host().commits,
            vec![("fn".to_string(), "T6:fn".to_string())],
            "digit 2 on page 2 is the seventh candidate"
        );
        assert_eq!(rig.ctl.host().doc, "T6:fn");
        assert!(!rig.ctl.host().composing && !rig.ctl.ui().visible);
        assert_eq!(rig.ctl.machine().state(), State::Observing);
        assert!(!rig.digit('2'), "and the next digit is typed again");
        // paging and moving the highlight write nothing and keep the composition open
        let mut rig = enter();
        assert!(rig.plain(vk::NEXT));
        assert_eq!(rig.view().page, 1);
        assert!(rig.plain(vk::PRIOR));
        assert_eq!(rig.view().page, 0);
        assert!(rig.plain(vk::DOWN));
        assert_eq!(rig.view().selected_in_page, 1);
        assert!(rig.press(
            vk::TAB,
            None,
            Modifiers {
                shift: true,
                ..NONE
            },
            Host::Contract
        ));
        assert_eq!(rig.view().selected_in_page, 0, "Shift+Tab goes back");
        assert!(rig.plain(vk::UP));
        assert!(rig.ctl.host().commits.is_empty() && rig.ctl.host().composing);
        // Tab, Enter and Space confirm the highlighted candidate
        for (code, name) in [
            (vk::TAB, "Tab"),
            (vk::RETURN, "Enter"),
            (vk::SPACE, "Space"),
        ] {
            let mut rig = enter();
            assert!(rig.press(code, Some(' '), NONE, Host::Contract), "{name}");
            assert_eq!(rig.ctl.host().doc, "T0:fn", "{name}");
            assert_eq!(rig.ctl.host().commits.len(), 1, "{name}");
            assert!(!rig.ctl.host().composing, "{name}: the composition ended");
        }
        // Esc cancels and writes nothing
        let mut rig = enter();
        assert!(rig.plain(vk::ESCAPE));
        assert_eq!(rig.ctl.host().doc, "fn");
        assert!(rig.ctl.host().commits.is_empty());
        assert!(!rig.ctl.host().composing && !rig.ctl.ui().visible);
        // candidates vanishing (the user typed `(`) end the composition too
        let mut rig = enter();
        rig.type_text("(", Host::Contract);
        assert!(!rig.ctl.host().composing);
        assert_ne!(rig.ctl.machine().state(), State::Composing);
    }

    #[test]
    fn ctrl_alt_space_without_candidates_passes_to_the_host() {
        let mut rig = Rig::defaults();
        assert!(!rig.chord(Host::Contract), "nothing is showing yet");
        rig.type_text("zz", Host::Contract); // no candidates for this prefix
        assert!(!rig.ctl.ui().visible);
        assert!(!rig.chord(Host::Contract));
        rig.type_text("fn(", Host::Contract); // candidates came and went
        assert!(!rig.ctl.ui().visible);
        assert!(!rig.chord(Host::Contract));
        assert_eq!(rig.ctl.host().begins, 0);
        assert!(!rig.ctl.host().composing);
    }

    #[test]
    fn the_chord_is_the_hosts_whenever_a_composition_is_impossible() {
        // already composing
        let mut rig = Rig::defaults();
        rig.type_text("fn", Host::Contract);
        assert!(rig.chord(Host::Contract));
        assert!(!rig.chord(Host::Contract), "a second chord is not ours");
        assert_eq!(rig.ctl.host().begins, 1);
        // the host refuses the composition: the key goes through and nothing else changes
        let mut rig = Rig::defaults();
        rig.ctl.host_mut().fail_begin = true;
        rig.type_text("fn", Host::Contract);
        assert!(!rig.chord(Host::Contract));
        assert_eq!(rig.ctl.host().begins, 1);
        assert!(
            !rig.ctl.is_degraded(),
            "a refused composition is not a host failure"
        );
        assert_eq!(rig.ctl.machine().state(), State::Candidates);
        assert!(rig.ctl.ui().visible, "the suggestions stay");
        assert!(!rig.digit('1'), "and digits are still typed");
        // a secure field and a paused input method
        let mut rig = Rig::defaults();
        rig.ctl.host_mut().secure = true;
        rig.type_text("fn", Host::Contract);
        assert!(!rig.chord(Host::Contract));
        assert_eq!(rig.ctl.host().begins, 0);
        let mut rig = Rig::defaults();
        rig.type_text("fn", Host::Contract);
        rig.press(vk::SHIFT, None, NONE, Host::Contract);
        assert!(rig.ctl.is_paused());
        assert!(!rig.chord(Host::Contract));
        assert_eq!(rig.ctl.host().begins, 0);
    }

    #[test]
    fn after_esc_the_next_typed_characters_pass_through_again() {
        let mut rig = Rig::defaults();
        rig.type_text("fn", Host::Contract);
        assert!(rig.chord(Host::Contract));
        assert!(rig.plain(vk::ESCAPE), "Esc cancels the composition");
        assert!(!rig.ctl.host().composing);
        assert_ne!(rig.ctl.machine().state(), State::Composing);
        rig.type_text("x1 y2", Host::Contract); // digits and Space included
        assert!(!rig.plain(vk::ESCAPE), "Esc is the editor's again");
        assert_eq!(rig.ctl.host().doc, "fnx1 y2");
        assert!(rig.ctl.host().commits.is_empty());
    }

    #[test]
    fn a_commit_that_fails_inside_the_explicit_composition_degrades_like_any_other() {
        let mut rig = Rig::defaults();
        rig.type_text("fn", Host::Contract);
        assert!(rig.chord(Host::Contract));
        rig.ctl.host_mut().fail_commit = true;
        assert!(!rig.digit('1'), "the key goes to the host");
        assert!(rig.ctl.is_degraded());
        assert!(!rig.ctl.host().composing && !rig.ctl.ui().visible);
    }

    #[test]
    fn a_pure_test_does_not_change_anything_for_an_eaten_key() {
        let mut rig = Rig::new();
        rig.type_text("fn", Host::Contract);
        let before = (
            rig.ctl.machine().selected(),
            rig.ctl.machine().candidates().len(),
            rig.ctl.ui().shows,
            rig.ctl.host().doc.clone(),
        );
        for _ in 0..3 {
            assert!(rig.ctl.on_test_key_down(input(vk::NEXT, None, NONE, 99)));
        }
        let after = (
            rig.ctl.machine().selected(),
            rig.ctl.machine().candidates().len(),
            rig.ctl.ui().shows,
            rig.ctl.host().doc.clone(),
        );
        assert_eq!(before, after);
    }

    #[test]
    fn a_typed_key_is_observed_exactly_once_under_every_host_call_pattern() {
        for style in [Host::Contract, Host::Always, Host::DownOnly] {
            let mut rig = Rig::new();
            rig.type_text("fn", style);
            assert_eq!(
                rig.ctl.predictor().calls,
                ["f", "fn"],
                "{style:?}: one prediction per typed character"
            );
            assert_eq!(rig.ctl.machine().context(), "fn", "{style:?}");
            assert_eq!(rig.ctl.host().doc, "fn", "{style:?}");
        }
    }

    #[test]
    fn a_host_that_tests_one_key_twice_does_not_make_the_machine_see_it_twice() {
        // review finding 8: only OnKeyDown consulted `seen`, so a second OnTestKeyDown for one event ran the machine again
        // (prefix "aa" for one 'a', wrong candidates, a commit whose read-back no longer matches)
        for performs in [false, true] {
            let mut rig = Rig::new();
            for c in "aab".chars() {
                rig.t += 40;
                let down = input(code_of(c), Some(c), NONE, rig.t);
                assert!(!rig.ctl.on_test_key_down(down), "{c}");
                assert!(
                    !rig.ctl.on_test_key_down(down),
                    "{c}: the same event, reported again"
                );
                if performs {
                    assert!(!rig.ctl.on_key_down(down), "{c}: and then performed");
                }
                rig.ctl.host_mut().doc.push(c);
                rig.t += 1;
                let up = input(code_of(c), Some(c), NONE, rig.t);
                rig.ctl.on_test_key_up(up);
                rig.ctl.on_key_up(up);
            }
            assert_eq!(rig.ctl.machine().context(), "aab", "performs: {performs}");
            assert_eq!(
                rig.ctl.predictor().calls,
                ["a", "aa", "aab"],
                "performs: {performs}: one prediction per typed character"
            );
        }
    }

    #[test]
    fn a_held_key_is_observed_once_per_repeat_even_when_the_host_sends_no_key_up() {
        // the guard against the naive fix for finding 8 ("same stamp => ignore"): every autorepeat of a held key carries
        // the same lparam, and for a key that is not eaten the host calls neither OnKeyDown nor sends a key-up between
        // the repeats, so only the time between reports tells a repeat from a duplicate
        let mut rig = Rig::new();
        let mut t = 100;
        for repeat in 0..5 {
            let mut down = input(0x41, Some('a'), NONE, t);
            if repeat > 0 {
                down.lparam |= 1 << 30; // "the key was already down"
            }
            assert!(!rig.ctl.on_test_key_down(down), "repeat {repeat}");
            rig.ctl.host_mut().doc.push('a');
            t += 33; // the fastest typematic rate
        }
        assert_eq!(rig.ctl.machine().context(), "aaaaa");
        assert_eq!(rig.ctl.predictor().calls.len(), 5);
    }

    #[test]
    fn the_same_character_typed_twice_is_two_observations() {
        for style in [Host::Contract, Host::Always, Host::DownOnly] {
            let mut rig = Rig::new();
            rig.type_text("aa", style);
            assert_eq!(rig.ctl.machine().context(), "aa", "{style:?}");
        }
    }

    #[test]
    fn eaten_keys_work_under_every_host_call_pattern() {
        for style in [Host::Contract, Host::Always, Host::DownOnly] {
            let mut rig = Rig::new();
            rig.type_text("fn", style);
            assert!(rig.press(code_of('2'), Some('2'), NONE, style), "{style:?}");
            assert_eq!(rig.ctl.host().commits.len(), 1, "{style:?}: committed once");
            assert_eq!(rig.ctl.host().doc, "T1:fn", "{style:?}");
        }
    }

    #[test]
    fn ut_005_a_secure_field_shows_nothing_and_eats_nothing() {
        let mut rig = Rig::new();
        rig.ctl.host_mut().secure = true;
        rig.type_text("hunter2", Host::Contract);
        assert!(!rig.ctl.ui().visible);
        assert_eq!(rig.ctl.ui().shows, 0, "no window, ever");
        assert!(
            rig.ctl.predictor().calls.is_empty(),
            "nothing is predicted from a password"
        );
        assert!(
            rig.ctl.machine().context().is_empty(),
            "nothing is retained"
        );
        for code in [vk::TAB, vk::RETURN, vk::ESCAPE, vk::NEXT, vk::SPACE] {
            assert!(!rig.plain(code), "{code:#x}");
        }
        assert!(!rig.digit('1'));
        assert_eq!(rig.ctl.host().begins, 0, "no composition either");
        // leaving the secure field restores normal behaviour
        rig.ctl.host_mut().secure = false;
        rig.ctl.focus_changed();
        rig.type_text("fn", Host::Contract);
        assert!(rig.ctl.ui().visible);
    }

    #[test]
    fn the_host_is_asked_about_the_secure_field_once_per_key_event_and_never_on_a_key_up() {
        // review finding 7: no answer is kept for the context any more, so the cost is bounded here instead
        for style in [Host::Contract, Host::Always, Host::DownOnly] {
            let mut rig = Rig::new();
            rig.type_text("fn", style);
            assert_eq!(
                rig.ctl.host().secure_checks,
                2,
                "{style:?}: one probe per typed key, test and perform of one key share it (key-ups are in there too)"
            );
            assert!(rig.press(code_of('1'), Some('1'), NONE, style), "{style:?}");
            assert_eq!(
                rig.ctl.host().secure_checks,
                3,
                "{style:?}: an eaten key is tested and performed on one probe"
            );
            rig.t += 40;
            let down = input(code_of('x'), Some('x'), NONE, rig.t);
            rig.ctl.on_test_key_down(down);
            rig.ctl.on_test_key_down(down);
            assert_eq!(
                rig.ctl.host().secure_checks,
                4,
                "{style:?}: a key reported twice is probed once"
            );
            rig.t += 1;
            let up = input(code_of('x'), Some('x'), NONE, rig.t);
            rig.ctl.on_test_key_up(up);
            rig.ctl.on_key_up(up);
            assert_eq!(
                rig.ctl.host().secure_checks,
                4,
                "{style:?}: a key-up never probes"
            );
        }
    }

    #[test]
    fn a_field_that_turns_into_a_password_field_inside_one_context_is_noticed_at_the_next_key() {
        // review finding 7: the glue kept the first answer per context, so a host that swaps a text field for a password
        // field without a focus event kept being treated as a text field. The fake host here keeps one context throughout.
        for style in [Host::Contract, Host::Always, Host::DownOnly] {
            let mut rig = Rig::new();
            rig.type_text("fn", style);
            assert!(
                rig.ctl.ui().visible && rig.ctl.host().composing,
                "{style:?}"
            );
            let (shows, calls) = (rig.ctl.ui().shows, rig.ctl.predictor().calls.len());
            rig.ctl.host_mut().secure = true; // the same context becomes a password field
            rig.type_text("hunter2", style);
            assert!(!rig.ctl.ui().visible, "{style:?}");
            assert!(
                !rig.ctl.host().composing,
                "{style:?}: the composition ended"
            );
            assert_eq!(
                rig.ctl.ui().shows,
                shows,
                "{style:?}: no window for a password"
            );
            assert_eq!(
                rig.ctl.predictor().calls.len(),
                calls,
                "{style:?}: nothing was predicted from it"
            );
            assert!(rig.ctl.machine().context().is_empty(), "{style:?}");
            assert!(!rig.digit('1'), "{style:?}");
            // and back to a text field
            rig.ctl.host_mut().secure = false;
            rig.type_text("fn", style);
            assert!(rig.ctl.ui().visible, "{style:?}: suggestions again");
        }
    }

    #[test]
    fn becoming_secure_while_candidates_show_hides_them_and_clears_the_context() {
        let mut rig = Rig::new();
        rig.type_text("fn", Host::Contract);
        assert!(rig.ctl.ui().visible && rig.ctl.host().composing);
        rig.ctl.host_mut().secure = true;
        assert!(!rig.press(code_of('x'), Some('x'), NONE, Host::Contract));
        assert!(!rig.ctl.ui().visible);
        assert!(!rig.ctl.host().composing, "the composition ended");
        assert!(rig.ctl.machine().context().is_empty());
    }

    #[test]
    fn secure_desktop_activation_is_secure_from_the_start() {
        let mut rig = Rig::new();
        rig.ctl.activate(true);
        rig.type_text("fn", Host::Contract);
        assert_eq!(rig.ctl.ui().shows, 0);
        assert!(!rig.digit('1'));
        // the host's answer cannot lift it
        rig.ctl.host_mut().secure = false;
        rig.type_text("fn", Host::Contract);
        assert_eq!(rig.ctl.ui().shows, 0);
    }

    #[test]
    fn ut_064_a_failed_commit_hands_the_key_to_the_host_and_everything_passes_after() {
        let mut rig = Rig::new();
        rig.type_text("fn", Host::Contract);
        rig.ctl.host_mut().fail_commit = true;
        let key = input(code_of('1'), Some('1'), NONE, 50);
        assert!(rig.ctl.on_test_key_down(key), "the test could not know");
        assert!(
            !rig.ctl.on_key_down(key),
            "but the key goes to the host when the write failed"
        );
        assert!(rig.ctl.is_degraded());
        assert!(!rig.ctl.ui().visible);
        assert!(!rig.ctl.host().composing);
        assert_eq!(rig.ctl.host().doc, "fn", "the document is untouched");
        // degraded: nothing is eaten and no window appears, whatever is typed
        for code in [vk::TAB, vk::RETURN, vk::ESCAPE, vk::NEXT, vk::SPACE] {
            assert!(!rig.plain(code), "{code:#x}");
        }
        rig.type_text("fn", Host::Contract);
        assert!(!rig.ctl.ui().visible);
        assert!(!rig.digit('1'));
        assert_eq!(rig.ctl.machine().state(), State::FaultFallback);
        // the next focus change gives it a fresh start
        rig.ctl.host_mut().fail_commit = false;
        rig.ctl.focus_changed();
        assert!(!rig.ctl.is_degraded());
        rig.type_text("fn", Host::Contract);
        assert!(rig.ctl.ui().visible);
        assert!(rig.digit('1'));
    }

    #[test]
    fn a_commit_into_changed_text_is_refused_not_forced() {
        let mut rig = Rig::new();
        rig.type_text("fn", Host::Contract);
        rig.ctl.host_mut().doc = "completely different".to_string(); // the user clicked elsewhere
        assert!(
            !rig.digit('1'),
            "the host rejected the replace: the key goes through"
        );
        assert!(rig.ctl.host().commits.is_empty());
        assert_eq!(rig.ctl.host().doc, "completely different1");
        assert!(rig.ctl.is_degraded());
    }

    #[test]
    fn ut_064_a_panic_inside_the_predictor_never_escapes() {
        let mut rig = Rig::new();
        rig.type_text("fn", Host::Contract);
        // 'boom' makes the predictor panic on its last character
        for c in "boom".chars() {
            let eaten = rig.press(code_of(c), Some(c), NONE, Host::Contract);
            assert!(!eaten);
        }
        assert!(!rig.ctl.ui().visible, "no window after the failure");
        assert!(!rig.digit('1'), "and the keyboard is the host's");
        assert!(!rig.ctl.host().composing);
        assert_eq!(rig.ctl.machine().state(), State::FaultFallback);
        // still usable after a focus change
        rig.ctl.focus_changed();
        rig.type_text("fn", Host::Contract);
        assert!(rig.ctl.ui().visible);
    }

    #[test]
    fn a_panic_in_the_host_or_the_window_is_contained_too() {
        let mut rig = Rig::new();
        rig.type_text("fn", Host::Contract);
        rig.ctl.host_mut().panic_commit = true;
        let key = input(code_of('1'), Some('1'), NONE, 70);
        assert!(!rig.ctl.on_key_down(key), "the panic became pass-through");
        assert!(rig.ctl.is_degraded());
        assert!(!rig.ctl.ui().visible);
        assert!(
            !rig.ctl.host().composing,
            "the open composition is closed after the panic"
        );

        struct PanickyUi;
        impl CandidateUi for PanickyUi {
            fn show(&mut self, _: &CandidateView, _: Anchor) -> bool {
                panic!("window panicked (test)");
            }
            fn hide(&mut self) {}
        }
        let host = FakeHost {
            anchor: Some(ANCHOR),
            ..FakeHost::default()
        };
        let mut ctl = TipController::new(
            PanickyUi,
            host,
            FakePredictor::default(),
            TipConfig::default(),
        );
        ctl.activate(false);
        let f = input(0x46, Some('f'), NONE, 1);
        assert!(!ctl.on_test_key_down(f), "typing survives a broken window");
        assert!(ctl.is_degraded());
        assert!(!ctl.on_test_key_down(input(vk::TAB, None, NONE, 2)));
    }

    #[test]
    fn losing_the_focus_hides_the_window_and_forgets_the_prefix() {
        let mut rig = Rig::new();
        rig.type_text("fn", Host::Contract);
        assert!(rig.ctl.ui().visible);
        rig.ctl.focus_changed();
        assert!(!rig.ctl.ui().visible);
        assert!(!rig.ctl.host().composing, "the composition was ended");
        assert!(
            rig.ctl.machine().candidates().is_empty() && rig.ctl.machine().context().is_empty()
        );
        assert!(!rig.plain(vk::ESCAPE), "nothing left to eat");
        rig.ctl.host_mut().doc.clear();
        rig.type_text("x", Host::Contract);
        assert_eq!(
            rig.ctl.machine().context(),
            "x",
            "a fresh start in the new document"
        );
    }

    #[test]
    fn the_pause_survives_a_focus_change() {
        let mut rig = Rig::new();
        rig.press(vk::SHIFT, None, NONE, Host::Contract);
        rig.ctl.focus_changed();
        assert!(rig.ctl.is_paused());
        rig.type_text("fn", Host::Contract);
        assert!(!rig.ctl.ui().visible);
    }

    #[test]
    fn a_fault_survives_a_plain_reset_but_not_a_focus_change() {
        let mut rig = Rig::new();
        rig.ctl.machine.report_core_failure(1);
        rig.ctl.reset();
        assert_eq!(rig.ctl.machine().state(), State::FaultFallback);
    }

    #[test]
    fn deactivate_leaves_no_window_and_no_open_composition() {
        let mut rig = Rig::new();
        rig.type_text("fn", Host::Contract);
        rig.ctl.deactivate();
        assert!(!rig.ctl.ui().visible);
        assert!(!rig.ctl.host().composing);
        assert!(!rig.plain(vk::ESCAPE));
    }

    #[test]
    fn no_caret_position_means_no_eaten_keys_behind_a_hidden_window() {
        let mut rig = Rig::new();
        rig.ctl.host_mut().anchor = None;
        rig.type_text("fn", Host::Contract);
        assert_eq!(rig.ctl.ui().shows, 0);
        assert!(
            rig.ctl.machine().candidates().is_empty(),
            "the candidates were dropped"
        );
        assert!(!rig.digit('1'), "so the digit is typed");
        assert!(!rig.plain(vk::ESCAPE));
        assert!(!rig.ctl.host().composing);
    }

    #[test]
    fn a_window_that_could_not_be_drawn_means_no_eaten_keys_behind_it() {
        // review finding 5: the popup failed (no window, no monitor, nothing to lay out), the controller used to believe
        // the list was on screen and ate Space/Enter/Tab/digits and the chord behind a window nobody sees
        for config in [TipConfig::default(), sogou()] {
            for style in [Host::Contract, Host::Always, Host::DownOnly] {
                let what = format!("{style:?} (capture {})", config.candidate_capture);
                let mut rig = Rig::with(config.clone());
                rig.ctl.ui.refuse = true;
                rig.type_text("fn", style);
                assert!(rig.ctl.ui().shows > 0, "{what}: the window was asked");
                assert!(!rig.ctl.ui().visible, "{what}");
                assert!(
                    rig.ctl.machine().candidates().is_empty(),
                    "{what}: the candidates were dropped, as for a missing caret position"
                );
                for code in [vk::TAB, vk::RETURN, vk::ESCAPE, vk::NEXT, vk::DOWN] {
                    assert!(!rig.press(code, None, NONE, style), "{what}: {code:#x}");
                    rig.type_text("fn", style);
                }
                assert!(!rig.press(vk::SPACE, Some(' '), NONE, style), "{what}");
                assert!(!rig.press(code_of('1'), Some('1'), NONE, style), "{what}");
                assert!(!rig.chord(style), "{what}: not even the chord");
                assert_eq!(rig.ctl.host().begins, 0, "{what}: no composition");
                assert!(!rig.ctl.host().composing, "{what}");
                assert!(rig.ctl.host().commits.is_empty(), "{what}");
                assert!(
                    !rig.ctl.is_degraded(),
                    "{what}: a missing window is not a host failure"
                );
                // a window that works again brings the suggestions back
                rig.ctl.ui.refuse = false;
                rig.ctl.host_mut().doc.clear();
                rig.type_text("fn", style);
                assert!(rig.ctl.ui().visible, "{what}");
                assert!(!rig.ctl.machine().candidates().is_empty(), "{what}");
            }
        }
    }

    #[test]
    fn a_window_that_fails_in_the_middle_of_a_composition_ends_it_and_hands_the_keys_back() {
        let mut rig = Rig::new();
        rig.type_text("fn", Host::Contract);
        assert!(rig.ctl.ui().visible && rig.ctl.host().composing);
        rig.ctl.ui.refuse = true;
        // the key was decided while the list was still visible; what matters is the state it leaves behind
        rig.plain(vk::NEXT);
        assert!(!rig.ctl.ui().visible);
        assert!(!rig.ctl.host().composing, "the composition was ended");
        assert_ne!(rig.ctl.machine().state(), State::Composing);
        assert!(rig.ctl.machine().candidates().is_empty());
        assert!(!rig.digit('1') && !rig.plain(vk::ESCAPE) && !rig.plain(vk::TAB));
        assert!(rig.ctl.host().commits.is_empty());
        assert_eq!(
            rig.ctl.host().doc,
            "fn1",
            "nothing was written but the typed digit"
        );
    }

    #[test]
    fn a_lost_caret_position_keeps_the_last_known_one() {
        let mut rig = Rig::new();
        rig.type_text("f", Host::Contract);
        rig.ctl.host_mut().anchor = None;
        rig.type_text("n", Host::Contract);
        assert!(rig.ctl.ui().visible);
        assert_eq!(rig.ctl.ui().last.as_ref().unwrap().1, ANCHOR);
    }

    #[test]
    fn candidates_vanishing_while_composing_close_the_composition() {
        let mut rig = Rig::new();
        rig.type_text("fn", Host::Contract);
        assert!(rig.ctl.host().composing);
        rig.type_text("(", Host::Contract); // not a prefix character: the list empties
        assert!(
            !rig.ctl.host().composing,
            "no composition without candidates"
        );
        assert!(!rig.ctl.ui().visible);
        assert_ne!(rig.ctl.machine().state(), State::Composing);
        assert_eq!(rig.ctl.host().doc, "fn(");
    }

    #[test]
    fn keys_that_move_the_caret_or_paste_drop_the_stale_candidates() {
        for (code, ch, mods) in [
            (vk::LEFT, None, NONE),
            (vk::HOME, None, NONE),
            (vk::DELETE, None, NONE),
            (0x56, Some('\u{16}'), Modifiers { ctrl: true, ..NONE }),
        ] {
            let mut rig = Rig::new();
            rig.type_text("fn", Host::Contract);
            let eaten = rig.press(code, ch, mods, Host::Contract);
            assert!(!eaten, "{code:#x}");
            assert!(!rig.ctl.ui().visible, "{code:#x}: window gone");
            assert!(!rig.ctl.host().composing, "{code:#x}");
            assert!(rig.ctl.machine().context().is_empty(), "{code:#x}");
            assert!(!rig.digit('1'), "{code:#x}: the digit is typed");
        }
    }

    #[test]
    fn after_a_commit_the_next_prediction_starts_from_the_new_text() {
        let mut rig = Rig::new();
        rig.type_text("fn", Host::Contract);
        assert!(rig.digit('1'));
        assert!(
            rig.ctl.machine().context().is_empty(),
            "the typed prefix is no longer in the document"
        );
        rig.type_text("x", Host::Contract);
        assert_eq!(
            rig.ctl.predictor().calls.last().map(String::as_str),
            Some("x")
        );
        assert_eq!(rig.ctl.host().doc, "T0:fnx");
    }

    #[test]
    fn the_typed_prefix_is_what_the_machine_asked_the_engine_about() {
        // the machine's rule: the trailing run of ASCII letters, digits, '_', '$' and '#' (at most 64)
        for (typed, prefix) in [("q(foo_bar", "foo_bar"), ("x.y$b#", "y$b#"), ("a-b", "b")] {
            let mut rig = Rig::new();
            rig.type_text(typed, Host::Contract);
            assert_eq!(rig.ctl.machine().typed_prefix(), prefix, "{typed}");
            assert_eq!(
                rig.ctl.predictor().calls.last().map(String::as_str),
                Some(prefix)
            );
        }
    }

    #[test]
    fn a_key_the_machine_has_no_use_for_changes_nothing() {
        let mut rig = Rig::new();
        rig.type_text("fn", Host::Contract);
        let shows = rig.ctl.ui().shows;
        assert!(!rig.press(0x70, None, NONE, Host::Contract), "F1");
        assert!(!rig.press(vk::CAPITAL, None, NONE, Host::Contract));
        assert!(rig.ctl.ui().visible && rig.ctl.ui().shows == shows);
    }

    #[test]
    fn the_config_defaults_never_eat_code_characters() {
        let c = TipConfig::default();
        assert!(
            !c.candidate_capture && !c.auto_compose,
            "non-destructive by default (DD 5.3, UT-009)"
        );
        assert_eq!(c.keymap, KeyMap::default());
        assert_eq!(c.language, "rust");
    }

    /// Every character of ordinary code, typed with the shipped defaults, reaches the host and nothing is committed,
    /// although the window shows suggestions the whole time.
    #[test]
    fn typing_code_with_the_defaults_eats_and_commits_nothing() {
        for style in [Host::Contract, Host::Always, Host::DownOnly] {
            for script in [
                "fn main",
                "i32",
                "x - 1",
                "a.b",
                "vec2",
                "let y = x1 + 2;\n",
            ] {
                let mut rig = Rig::defaults();
                let mut shown = false;
                for c in script.chars() {
                    let (code, ch) = if c == '\n' {
                        (vk::RETURN, None)
                    } else {
                        (code_of(c), Some(c))
                    };
                    assert!(
                        !rig.press(code, ch, NONE, style),
                        "{style:?} {script:?}: {c:?} was eaten"
                    );
                    shown |= rig.ctl.ui().visible;
                }
                assert_eq!(rig.ctl.host().doc, script, "the host got every character");
                assert!(rig.ctl.host().commits.is_empty(), "{script:?}");
                assert_eq!(rig.ctl.host().begins, 0, "{script:?}: no composition");
                assert_eq!(rig.ctl.machine().stats.commits, 0, "{script:?}");
                assert!(shown, "{script:?}: observation is on, suggestions show");
            }
        }
    }

    #[test]
    fn the_real_engine_suggests_without_taking_a_key_under_the_defaults() {
        let host = FakeHost {
            anchor: Some(ANCHOR),
            ..FakeHost::default()
        };
        let mut ctl = TipController::new(
            RecordingUi::default(),
            host,
            LocalPredictor,
            TipConfig::default(),
        );
        ctl.activate(false);
        let mut t = 0;
        let mut type_key =
            |ctl: &mut TipController<RecordingUi, FakeHost>, code, ch: Option<char>| {
                t += 1;
                let key = input(code, ch, NONE, t);
                assert!(!ctl.on_test_key_down(key), "{ch:?}");
                ctl.host_mut().doc.extend(ch);
            };
        for c in "fn".chars() {
            type_key(&mut ctl, code_of(c), Some(c));
        }
        assert!(ctl.ui().visible, "suggestions for `fn`");
        for (code, ch) in [(vk::SPACE, Some(' ')), (code_of('m'), Some('m'))] {
            type_key(&mut ctl, code, ch);
        }
        assert_eq!(ctl.host().doc, "fn m");
        assert_eq!(ctl.machine().stats.commits, 0);
    }

    #[test]
    fn the_production_predictor_drives_the_controller_end_to_end() {
        let host = FakeHost {
            anchor: Some(ANCHOR),
            ..FakeHost::default()
        };
        let mut ctl = TipController::new(RecordingUi::default(), host, LocalPredictor, sogou());
        ctl.activate(false);
        for (i, c) in "fnm".chars().enumerate() {
            let key = input(code_of(c), Some(c), NONE, i as u64);
            assert!(!ctl.on_test_key_down(key));
            ctl.host_mut().doc.push(c);
        }
        let view = ctl.ui().last.as_ref().expect("a window").0.clone();
        assert!(
            view.rows[0].text.starts_with("fn main"),
            "{:?}",
            view.rows[0]
        );
        assert!(ctl.on_key_down(input(vk::TAB, None, NONE, 9)));
        assert!(
            ctl.host().doc.starts_with("fn main"),
            "{:?}",
            ctl.host().doc
        );
        assert!(
            !ctl.host().doc.contains("${"),
            "placeholders are rendered plain"
        );
    }
}
