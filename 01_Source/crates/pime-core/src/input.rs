//! DD-02 section 4.1 input state machine (plus the DD-01 section 3.3 input policy): pure logic, no Windows API.
//!
//! States: DIRECT, OBSERVING, CANDIDATES, COMPOSING, SUSPENDED, FAULT_FALLBACK.
//! Hard rules enforced here (and proven by the UT-003/004/005/009/064/067 tests below):
//! * DIRECT never consumes a key and never observes text;
//! * keys are consumed only in COMPOSING (an explicit, owned composition) or in the explicitly enabled
//!   candidate-capture mode, and only for the reserved keymap (Tab, Enter, Esc, digits, Up, Down);
//! * secure input / disabled policy clears the context, refuses prediction and AI, and passes every key through;
//! * a core/IPC failure (error, timeout, panic) ends in pass-through within the 2 s design budget;
//! * the machine never writes text itself: it only emits [`Effect`]s for the host adapter to perform.

use crate::candidate::{Candidate, Validation};
use crate::error::{Result, fail};
use crate::predict::{PredictOptions, engine};
use std::panic::{AssertUnwindSafe, catch_unwind};

/// A request to the core is considered lost after this long (virtual or real milliseconds).
pub const CORE_REQUEST_TIMEOUT_MS: u64 = 250;
/// NFR-102: Direct must be in force within 2 seconds of a core failure.
pub const DIRECT_DEADLINE_MS: u64 = 2000;
const MAX_CONTEXT_CHARS: usize = 2000;
const MAX_PREFIX_CHARS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Direct,
    Observing,
    Candidates,
    Composing,
    Suspended,
    FaultFallback,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Tab,
    Enter,
    Esc,
    Up,
    Down,
    Left,
    Right,
    Backspace,
    Other(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub ctrl: bool,
    pub alt: bool,
}

impl KeyEvent {
    pub fn plain(key: Key) -> KeyEvent {
        KeyEvent {
            key,
            ctrl: false,
            alt: false,
        }
    }

    pub fn ch(c: char) -> KeyEvent {
        KeyEvent::plain(Key::Char(c))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyOutcome {
    PassThrough,
    Consumed,
}

/// What the host adapter must do. `Commit` is the ONLY effect that writes text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    ShowCandidates,
    HideCandidates,
    SelectionMoved(usize),
    BeginComposition,
    /// End (cancel) the machine-owned composition without writing anything.
    EndComposition,
    Commit {
        candidate_id: String,
        text: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub outcome: KeyOutcome,
    pub effects: Vec<Effect>,
}

impl Step {
    fn pass() -> Step {
        Step {
            outcome: KeyOutcome::PassThrough,
            effects: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreFault {
    Timeout,
    Disconnected,
}

#[derive(Clone, Debug)]
pub struct PredictRequest {
    pub language: String,
    pub before: String,
    pub prefix: String,
}

/// The port to the candidate core (local engine, IPC client or a test fake).
pub trait PredictPort {
    fn predict(
        &mut self,
        request: &PredictRequest,
    ) -> std::result::Result<Vec<Candidate>, CoreFault>;
}

/// Production port: the in-process deterministic engine.
pub struct LocalPredictor;

impl PredictPort for LocalPredictor {
    fn predict(&mut self, r: &PredictRequest) -> std::result::Result<Vec<Candidate>, CoreFault> {
        let opts = PredictOptions::new(&r.language, &r.prefix).before(&r.before);
        engine().predict(&opts).map_err(|_| CoreFault::Disconnected)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub predict_calls: u64,
    pub commits: u64,
    pub faults: u64,
}

pub struct InputMachine<P: PredictPort> {
    port: P,
    state: State,
    language: String,
    context: String,
    candidates: Vec<Candidate>,
    selected: usize,
    observation_granted: bool,
    capture_enabled: bool,
    secure: bool,
    policy_disabled: bool,
    composition_owned: bool,
    ai_queue: usize,
    profile_epoch: u64,
    pending_since: Option<u64>,
    fault_at: Option<u64>,
    pub stats: Stats,
}

fn prefix_split(text: &str) -> (&str, &str) {
    let mut start = text.len();
    for (n, (i, c)) in text.char_indices().rev().enumerate() {
        if !(c.is_ascii_alphanumeric() || matches!(c, '_' | '$' | '#')) || n >= MAX_PREFIX_CHARS {
            break;
        }
        start = i;
    }
    (&text[..start], &text[start..])
}

impl<P: PredictPort> InputMachine<P> {
    pub fn new(port: P, language: &str) -> InputMachine<P> {
        InputMachine {
            port,
            state: State::Direct,
            language: language.to_string(),
            context: String::new(),
            candidates: Vec::new(),
            selected: 0,
            observation_granted: false,
            capture_enabled: false,
            secure: false,
            policy_disabled: false,
            composition_owned: false,
            ai_queue: 0,
            profile_epoch: 0,
            pending_since: None,
            fault_at: None,
            stats: Stats::default(),
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn context(&self) -> &str {
        &self.context
    }

    pub fn candidates(&self) -> &[Candidate] {
        &self.candidates
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn profile_epoch(&self) -> u64 {
        self.profile_epoch
    }

    pub fn ai_queue_len(&self) -> usize {
        self.ai_queue
    }

    pub fn port(&self) -> &P {
        &self.port
    }

    pub fn set_language(&mut self, language: &str) {
        if self.language != language {
            self.language = language.to_string();
            self.profile_epoch += 1;
            self.candidates.clear();
            self.selected = 0;
        }
    }

    /// DIRECT <-> OBSERVING: the host/bridge may legally read a local range (never forces a composition).
    pub fn grant_observation(&mut self, on: bool) {
        self.observation_granted = on;
        match (self.state, on) {
            (State::Direct, true) => self.state = State::Observing,
            (State::Observing | State::Candidates, false) => {
                self.clear_snapshot();
                self.state = State::Direct;
            }
            _ => {}
        }
    }

    /// Explicit opt-in: Tab/Enter/digits/arrows/Esc are consumed while candidates are showing.
    pub fn set_candidate_capture(&mut self, on: bool) {
        self.capture_enabled = on;
    }

    pub fn set_secure_context(&mut self, secure: bool) -> Vec<Effect> {
        self.secure = secure;
        self.refresh_suspension()
    }

    pub fn set_policy_disabled(&mut self, disabled: bool) -> Vec<Effect> {
        self.policy_disabled = disabled;
        self.refresh_suspension()
    }

    fn refresh_suspension(&mut self) -> Vec<Effect> {
        if self.secure || self.policy_disabled {
            return self.suspend();
        }
        if self.state == State::Suspended {
            self.state = if self.observation_granted {
                State::Observing
            } else {
                State::Direct
            };
        }
        Vec::new()
    }

    /// SUSPENDED: clear everything that may hold context, refuse new AI requests.
    fn suspend(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        if self.composition_owned {
            self.composition_owned = false;
            effects.push(Effect::EndComposition);
        }
        if !self.candidates.is_empty() {
            effects.push(Effect::HideCandidates);
        }
        self.clear_snapshot();
        self.ai_queue = 0;
        self.pending_since = None;
        if self.state != State::FaultFallback {
            self.state = State::Suspended;
        }
        effects
    }

    fn clear_snapshot(&mut self) {
        self.context.clear();
        self.context.shrink_to_fit();
        self.candidates.clear();
        self.selected = 0;
    }

    /// Queue an AI request (the queue keeps only the latest). Refused in SUSPENDED, FAULT_FALLBACK and DIRECT.
    pub fn request_ai(&mut self) -> Result<()> {
        match self.state {
            State::Suspended if self.secure => fail(
                "SECURE_INPUT",
                "secure input: no context is collected and no request is made",
            ),
            State::Suspended => fail("AI_DISABLED", "disabled by application policy"),
            State::FaultFallback => fail("IPC_TIMEOUT", "core unavailable"),
            State::Direct => fail("CONTEXT_UNAVAILABLE", "no legal context in direct mode"),
            _ => {
                self.ai_queue = 1;
                Ok(())
            }
        }
    }

    /// CANDIDATES -> COMPOSING: only with candidates showing and a host range the adapter may legally edit.
    pub fn begin_composition(&mut self, host_range_permitted: bool) -> Option<Effect> {
        if self.state == State::Candidates && host_range_permitted && !self.candidates.is_empty() {
            self.state = State::Composing;
            self.composition_owned = true;
            Some(Effect::BeginComposition)
        } else {
            None
        }
    }

    /// Core/IPC failure (error, timeout, killed worker): hand the keyboard back to the host (Direct behaviour).
    pub fn report_core_failure(&mut self, now_ms: u64) -> Vec<Effect> {
        self.stats.faults += 1;
        let mut effects = Vec::new();
        if self.composition_owned {
            self.composition_owned = false;
            effects.push(Effect::EndComposition);
        }
        if !self.candidates.is_empty() {
            effects.push(Effect::HideCandidates);
        }
        self.clear_snapshot();
        self.ai_queue = 0;
        self.pending_since = None;
        if self.state != State::FaultFallback {
            self.fault_at = Some(now_ms);
        }
        self.state = State::FaultFallback;
        effects
    }

    /// The core answers again: leave FAULT_FALLBACK with a new `profile_epoch` and no stale candidates.
    pub fn core_recovered(&mut self) {
        if self.state == State::FaultFallback {
            self.profile_epoch += 1;
            self.fault_at = None;
            self.state = if self.secure || self.policy_disabled {
                State::Suspended
            } else if self.observation_granted {
                State::Observing
            } else {
                State::Direct
            };
        }
    }

    /// Milliseconds between the first failure and `now` while in FAULT_FALLBACK.
    pub fn fault_elapsed(&self, now_ms: u64) -> Option<u64> {
        self.fault_at.map(|t| now_ms.saturating_sub(t))
    }

    /// An asynchronous request to the core was sent (used with a real IPC client; the virtual clock in tests).
    pub fn begin_core_request(&mut self, now_ms: u64) {
        if self.pending_since.is_none() {
            self.pending_since = Some(now_ms);
        }
    }

    pub fn complete_core_request(&mut self) {
        self.pending_since = None;
    }

    /// Watchdog: a request without answer for [`CORE_REQUEST_TIMEOUT_MS`] means the core is gone.
    pub fn tick(&mut self, now_ms: u64) -> Vec<Effect> {
        match self.pending_since {
            Some(t) if now_ms.saturating_sub(t) >= CORE_REQUEST_TIMEOUT_MS => {
                self.report_core_failure(now_ms)
            }
            _ => Vec::new(),
        }
    }

    fn capture_active(&self) -> bool {
        self.state == State::Composing || (self.state == State::Candidates && self.capture_enabled)
    }

    fn selectable(&self, idx: usize) -> Option<&Candidate> {
        self.candidates.get(idx).filter(|c| {
            c.text.is_some()
                && !matches!(
                    c.validation,
                    Validation::Invalid | Validation::ContextMissing
                )
        })
    }

    fn accept(&mut self, idx: usize) -> Option<Vec<Effect>> {
        let c = self.selectable(idx)?;
        let (candidate_id, text) = (c.id.clone(), c.text.clone().unwrap_or_default());
        let mut effects = vec![Effect::Commit { candidate_id, text }];
        self.stats.commits += 1;
        if self.composition_owned {
            self.composition_owned = false;
            effects.push(Effect::EndComposition);
        }
        effects.push(Effect::HideCandidates);
        self.candidates.clear();
        self.selected = 0;
        self.state = State::Observing;
        Some(effects)
    }

    fn reserved_key(&mut self, ev: KeyEvent) -> Option<Step> {
        let consumed = |effects: Vec<Effect>| {
            Some(Step {
                outcome: KeyOutcome::Consumed,
                effects,
            })
        };
        match ev.key {
            Key::Tab | Key::Enter => self.accept(self.selected).and_then(consumed),
            Key::Char(d @ '1'..='9') => {
                let idx = d as usize - '1' as usize;
                self.selectable(idx)?;
                self.accept(idx).and_then(consumed)
            }
            Key::Up | Key::Down if !self.candidates.is_empty() => {
                let n = self.candidates.len();
                self.selected = if ev.key == Key::Up {
                    (self.selected + n - 1) % n
                } else {
                    (self.selected + 1) % n
                };
                consumed(vec![Effect::SelectionMoved(self.selected)])
            }
            Key::Esc => {
                let mut effects = Vec::new();
                if self.composition_owned {
                    self.composition_owned = false;
                    effects.push(Effect::EndComposition); // cancel: nothing is written
                }
                effects.push(Effect::HideCandidates);
                self.candidates.clear();
                self.selected = 0;
                self.state = State::Observing;
                consumed(effects)
            }
            _ => None,
        }
    }

    fn observe(&mut self, ev: KeyEvent, now_ms: u64) -> Vec<Effect> {
        match ev.key {
            Key::Char(c) => self.context.push(c),
            Key::Enter => self.context.push('\n'),
            Key::Backspace => {
                self.context.pop();
            }
            _ => return Vec::new(),
        }
        let excess = self
            .context
            .chars()
            .count()
            .saturating_sub(MAX_CONTEXT_CHARS);
        if excess > 0 {
            self.context = self.context.chars().skip(excess).collect();
        }
        let (before, prefix) = prefix_split(&self.context);
        let (before, prefix) = (before.to_string(), prefix.to_string());
        let had = !self.candidates.is_empty();
        self.candidates.clear();
        self.selected = 0;
        if !prefix.is_empty() && self.language != "unknown" {
            self.stats.predict_calls += 1;
            let request = PredictRequest {
                language: self.language.clone(),
                before,
                prefix,
            };
            // a request already pending (async adapter) keeps its own timer; this synchronous call only owns its own
            let owns_timer = self.pending_since.is_none();
            if owns_timer {
                self.begin_core_request(now_ms);
            }
            let result = catch_unwind(AssertUnwindSafe(|| self.port.predict(&request)));
            if owns_timer {
                self.complete_core_request();
            }
            match result {
                Ok(Ok(list)) => self.candidates = list,
                // an error, a timeout or a panicking worker never reaches the host thread
                Ok(Err(_)) | Err(_) => return self.report_core_failure(now_ms),
            }
        }
        let mut effects = Vec::new();
        if !self.candidates.is_empty() {
            if self.state != State::Composing {
                self.state = State::Candidates;
            }
            effects.push(Effect::ShowCandidates);
        } else {
            if self.state == State::Candidates {
                self.state = State::Observing;
            }
            if had {
                effects.push(Effect::HideCandidates);
            }
        }
        effects
    }

    /// One key event (DD section 3.3 `on_input`). Never fails: any internal problem degrades to pass-through.
    pub fn on_key(&mut self, ev: KeyEvent, now_ms: u64) -> Step {
        if self.secure || self.policy_disabled {
            let effects = if self.state == State::Suspended {
                Vec::new()
            } else {
                self.suspend()
            };
            return Step {
                outcome: KeyOutcome::PassThrough,
                effects,
            };
        }
        if matches!(
            self.state,
            State::FaultFallback | State::Suspended | State::Direct
        ) {
            return Step::pass();
        }
        if ev.ctrl || ev.alt {
            return Step::pass(); // shortcuts belong to the host
        }
        if self.capture_active()
            && let Some(step) = self.reserved_key(ev)
        {
            return step;
        }
        let effects = self.observe(ev, now_ms);
        Step {
            outcome: KeyOutcome::PassThrough,
            effects,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate::CandidateSource;

    /// Spy port: counts calls and serves canned candidates (or a fault / panic).
    #[derive(Default)]
    struct Spy {
        calls: usize,
        mode: u8, // 0 ok, 1 timeout, 2 disconnected, 3 panic
    }

    impl PredictPort for Spy {
        fn predict(
            &mut self,
            r: &PredictRequest,
        ) -> std::result::Result<Vec<Candidate>, CoreFault> {
            self.calls += 1;
            match self.mode {
                1 => Err(CoreFault::Timeout),
                2 => Err(CoreFault::Disconnected),
                3 => panic!("worker panicked (test)"),
                _ => {
                    let mut c = Candidate::new(&format!("t.{}", r.prefix))
                        .with_source(CandidateSource::Rule);
                    c.validation = Validation::Valid;
                    c.text = Some(format!("{}!", r.prefix));
                    let mut d = Candidate::new("t.second");
                    d.validation = Validation::Valid;
                    d.text = Some("second".into());
                    Some(vec![c, d]).ok_or(CoreFault::Disconnected)
                }
            }
        }
    }

    fn machine() -> InputMachine<Spy> {
        InputMachine::new(Spy::default(), "rust")
    }

    fn type_text(m: &mut InputMachine<Spy>, s: &str, t0: u64) {
        for (i, c) in s.chars().enumerate() {
            let step = m.on_key(KeyEvent::ch(c), t0 + i as u64);
            assert_eq!(
                step.outcome,
                KeyOutcome::PassThrough,
                "ordinary characters are never consumed"
            );
        }
    }

    #[test]
    fn ut_003_direct_never_consumes_ascii() {
        let mut m = machine();
        assert_eq!(m.state(), State::Direct);
        for code in 0u8..=127 {
            let step = m.on_key(KeyEvent::ch(code as char), 0);
            assert_eq!(step.outcome, KeyOutcome::PassThrough, "ASCII {code}");
            assert!(step.effects.is_empty());
        }
        let mut host_inputs = 0;
        let mut passed = 0;
        for i in 0..100 {
            host_inputs += 1;
            if m.on_key(KeyEvent::ch((b'a' + (i % 26) as u8) as char), i as u64)
                .outcome
                == KeyOutcome::PassThrough
            {
                passed += 1;
            }
        }
        assert_eq!(
            passed, host_inputs,
            "every host input is passed through exactly once"
        );
        assert_eq!(m.stats.commits, 0, "the machine commits nothing of its own");
        assert_eq!(m.port().calls, 0, "Direct does not even observe");
        assert!(m.context().is_empty());
    }

    #[test]
    fn ut_003_observing_does_not_consume_ordinary_characters_either() {
        let mut m = machine();
        m.grant_observation(true);
        type_text(&mut m, "fn main() { let x = 1; }", 0);
        assert_eq!(m.stats.commits, 0);
        assert!(m.port().calls > 0, "observing predicts");
    }

    #[test]
    fn ut_004_composition_cancel_writes_nothing() {
        let mut m = machine();
        m.grant_observation(true);
        m.set_candidate_capture(true);
        type_text(&mut m, "fnm", 0);
        assert_eq!(m.state(), State::Candidates);
        assert_eq!(m.begin_composition(true), Some(Effect::BeginComposition));
        assert_eq!(m.state(), State::Composing);
        let moved = m.on_key(KeyEvent::plain(Key::Down), 10);
        assert_eq!(moved.outcome, KeyOutcome::Consumed);
        assert!(
            !moved
                .effects
                .iter()
                .any(|e| matches!(e, Effect::Commit { .. }))
        );
        let esc = m.on_key(KeyEvent::plain(Key::Esc), 20);
        assert_eq!(esc.outcome, KeyOutcome::Consumed);
        assert!(esc.effects.contains(&Effect::EndComposition));
        assert!(
            !esc.effects
                .iter()
                .any(|e| matches!(e, Effect::Commit { .. })),
            "cancel must not write a candidate"
        );
        assert_eq!(m.stats.commits, 0);
        assert_eq!(m.state(), State::Observing);
        assert!(m.candidates().is_empty());
        // the characters the user typed were all passed to the host once and are not replayed
        assert_eq!(
            m.on_key(KeyEvent::ch('x'), 30).outcome,
            KeyOutcome::PassThrough
        );
        assert_eq!(m.stats.commits, 0);
    }

    #[test]
    fn ut_004_composition_needs_candidates_and_a_permitted_host_range() {
        let mut m = machine();
        assert_eq!(
            m.begin_composition(true),
            None,
            "no composition from Direct"
        );
        m.grant_observation(true);
        type_text(&mut m, "fn", 0);
        assert_eq!(
            m.begin_composition(false),
            None,
            "host does not permit a range"
        );
        assert_eq!(m.state(), State::Candidates);
    }

    #[test]
    fn ut_005_secure_input_clears_context_and_never_predicts() {
        let mut m = machine();
        m.grant_observation(true);
        type_text(&mut m, "hunter", 0);
        assert!(!m.context().is_empty() && !m.candidates().is_empty());
        let calls_before = m.port().calls;
        let effects = m.set_secure_context(true);
        assert!(effects.contains(&Effect::HideCandidates));
        assert_eq!(m.state(), State::Suspended);
        assert!(
            m.context().is_empty() && m.candidates().is_empty(),
            "context is empty after the secure flag"
        );
        for c in "password123".chars() {
            let step = m.on_key(KeyEvent::ch(c), 100);
            assert_eq!(
                step.outcome,
                KeyOutcome::PassThrough,
                "input passes through normally"
            );
        }
        assert_eq!(m.port().calls, calls_before, "no prediction while secure");
        assert!(m.context().is_empty(), "no text is retained");
        assert_eq!(m.request_ai().unwrap_err().code, "SECURE_INPUT");
        assert_eq!(m.ai_queue_len(), 0);
        m.set_secure_context(false);
        assert_eq!(
            m.state(),
            State::Observing,
            "leaving the secure field restores observation"
        );
    }

    #[test]
    fn ut_005_policy_disabled_behaves_like_secure_input() {
        let mut m = machine();
        m.grant_observation(true);
        m.request_ai().unwrap();
        assert_eq!(m.ai_queue_len(), 1);
        m.set_policy_disabled(true);
        assert_eq!((m.state(), m.ai_queue_len()), (State::Suspended, 0));
        assert_eq!(m.request_ai().unwrap_err().code, "AI_DISABLED");
        assert_eq!(
            m.on_key(KeyEvent::ch('a'), 0).outcome,
            KeyOutcome::PassThrough
        );
    }

    #[test]
    fn ut_009_reserved_keys_are_consumed_only_in_enabled_candidate_mode() {
        let keys = [Key::Tab, Key::Enter, Key::Char('1'), Key::Up, Key::Down];
        // Direct: never consumed
        let mut direct = machine();
        for k in keys {
            assert_eq!(
                direct.on_key(KeyEvent::plain(k), 0).outcome,
                KeyOutcome::PassThrough,
                "Direct {k:?}"
            );
        }
        // candidates showing but capture NOT enabled: still never consumed
        let mut shown = machine();
        shown.grant_observation(true);
        type_text(&mut shown, "fn", 0);
        assert_eq!(shown.state(), State::Candidates);
        for k in [
            Key::Tab,
            Key::Enter,
            Key::Char('1'),
            Key::Up,
            Key::Down,
            Key::Esc,
        ] {
            assert_eq!(
                shown.on_key(KeyEvent::plain(k), 0).outcome,
                KeyOutcome::PassThrough,
                "capture off {k:?}"
            );
        }
        assert_eq!(shown.stats.commits, 0);
        // capture enabled: consumed
        let mut cap = machine();
        cap.grant_observation(true);
        cap.set_candidate_capture(true);
        type_text(&mut cap, "fn", 0);
        assert_eq!(
            cap.on_key(KeyEvent::plain(Key::Down), 0).outcome,
            KeyOutcome::Consumed
        );
        assert_eq!(cap.selected(), 1);
        assert_eq!(
            cap.on_key(KeyEvent::plain(Key::Up), 0).outcome,
            KeyOutcome::Consumed
        );
        assert_eq!(cap.selected(), 0);
        let tab = cap.on_key(KeyEvent::plain(Key::Tab), 0);
        assert_eq!(tab.outcome, KeyOutcome::Consumed);
        assert!(
            tab.effects.iter().any(
                |e| matches!(e, Effect::Commit { candidate_id, .. } if candidate_id == "t.fn")
            )
        );
        type_text(&mut cap, "fn", 5);
        let digit = cap.on_key(KeyEvent::ch('2'), 0);
        assert_eq!(digit.outcome, KeyOutcome::Consumed);
        assert!(digit.effects.iter().any(
            |e| matches!(e, Effect::Commit { candidate_id, .. } if candidate_id == "t.second")
        ));
        // a digit without a matching candidate, and ctrl chords, are still the host's
        type_text(&mut cap, "fn", 9);
        assert_eq!(
            cap.on_key(KeyEvent::ch('7'), 0).outcome,
            KeyOutcome::PassThrough
        );
        let chord = KeyEvent {
            key: Key::Tab,
            ctrl: true,
            alt: false,
        };
        assert_eq!(cap.on_key(chord, 0).outcome, KeyOutcome::PassThrough);
    }

    #[test]
    fn ut_009_capture_without_candidates_consumes_nothing() {
        let mut m = machine();
        m.grant_observation(true);
        m.set_candidate_capture(true);
        // (digits last: typing "1" is itself a valid prefix and would create candidates for the next key)
        for k in [Key::Tab, Key::Enter, Key::Up, Key::Down, Key::Char('1')] {
            assert_eq!(
                m.on_key(KeyEvent::plain(k), 0).outcome,
                KeyOutcome::PassThrough,
                "{k:?}"
            );
        }
    }

    #[test]
    fn ut_064_core_failures_degrade_to_pass_through_without_propagating() {
        for mode in [1u8, 2, 3] {
            let mut m = InputMachine::new(Spy { calls: 0, mode }, "rust");
            m.grant_observation(true);
            let step = m.on_key(KeyEvent::ch('f'), 500);
            assert_eq!(
                step.outcome,
                KeyOutcome::PassThrough,
                "mode {mode}: the key still reaches the host"
            );
            assert_eq!(m.state(), State::FaultFallback, "mode {mode}");
            let calls = m.port().calls;
            for c in "unction".chars() {
                assert_eq!(
                    m.on_key(KeyEvent::ch(c), 600).outcome,
                    KeyOutcome::PassThrough
                );
            }
            assert_eq!(
                m.port().calls,
                calls,
                "a failed core is not called again (a panicking worker is not re-entered)"
            );
            assert!(m.candidates().is_empty() && m.context().is_empty());
        }
    }

    #[test]
    fn ut_064_failure_while_composing_ends_the_owned_composition_without_writing() {
        let mut m = machine();
        m.grant_observation(true);
        type_text(&mut m, "fn", 0);
        m.begin_composition(true).unwrap();
        let effects = m.report_core_failure(50);
        assert!(effects.contains(&Effect::EndComposition));
        assert!(!effects.iter().any(|e| matches!(e, Effect::Commit { .. })));
        assert_eq!(m.state(), State::FaultFallback);
        assert_eq!(
            m.on_key(KeyEvent::plain(Key::Tab), 60).outcome,
            KeyOutcome::PassThrough
        );
    }

    #[test]
    fn ut_064_recovery_bumps_the_profile_epoch_and_refreshes() {
        let mut m = machine();
        m.grant_observation(true);
        m.report_core_failure(0);
        let epoch = m.profile_epoch();
        m.core_recovered();
        assert_eq!(m.state(), State::Observing);
        assert_eq!(m.profile_epoch(), epoch + 1);
        assert!(m.candidates().is_empty());
        type_text(&mut m, "fn", 10);
        assert_eq!(m.state(), State::Candidates);
    }

    #[test]
    fn ut_067_direct_is_in_force_within_two_seconds_on_a_virtual_clock() {
        let mut m = machine();
        m.grant_observation(true);
        m.set_candidate_capture(true);
        type_text(&mut m, "fn", 0);
        // t = 0: a request is sent and the core is killed (never answers)
        m.begin_core_request(0);
        let mut direct_at = None;
        for t in 0..=DIRECT_DEADLINE_MS {
            m.tick(t);
            // while the failure is undetected, no key is swallowed or dropped
            let step = m.on_key(KeyEvent::ch('x'), t);
            assert_eq!(step.outcome, KeyOutcome::PassThrough, "t={t}");
            if direct_at.is_none() && m.state() == State::FaultFallback {
                direct_at = Some(t);
            }
        }
        let direct_at = direct_at.expect("Direct must be in force by 2000 ms");
        assert_eq!(direct_at, CORE_REQUEST_TIMEOUT_MS);
        assert!(
            direct_at <= DIRECT_DEADLINE_MS,
            "design budget: <= 2 s (system-level measurement belongs to ST)"
        );
        assert_eq!(
            m.fault_elapsed(DIRECT_DEADLINE_MS),
            Some(DIRECT_DEADLINE_MS - direct_at)
        );
        assert_eq!(
            m.on_key(KeyEvent::plain(Key::Tab), DIRECT_DEADLINE_MS)
                .outcome,
            KeyOutcome::PassThrough
        );
    }

    #[test]
    fn ut_067_an_answered_request_never_triggers_the_watchdog() {
        let mut m = machine();
        m.grant_observation(true);
        m.begin_core_request(0);
        m.complete_core_request();
        for t in 0..=DIRECT_DEADLINE_MS {
            assert!(m.tick(t).is_empty());
        }
        assert_eq!(m.state(), State::Observing);
    }

    #[test]
    fn local_predictor_drives_the_real_engine() {
        let mut m = InputMachine::new(LocalPredictor, "rust");
        m.grant_observation(true);
        for c in "fnm".chars() {
            m.on_key(KeyEvent::ch(c), 0);
        }
        assert_eq!(m.state(), State::Candidates);
        assert_eq!(m.candidates()[0].id, "rust.main#0");
    }
}
