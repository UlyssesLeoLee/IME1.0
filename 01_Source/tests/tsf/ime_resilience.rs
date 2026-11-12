//! The TSF and the core when the core does not answer: the raw keystrokes must not be interrupted, and the core must
//! isolate the request that timed out.
//!
//! The design case is IT-042 ("Worker 卡死不阻塞 TSF", criterion: "the raw keystrokes are not interrupted; the core
//! can isolate the task that timed out"). Two things are measured here:
//!
//! * the text service: when the core does not answer, every key still belongs to the host and the input method goes
//!   to the Direct fallback instead of eating keys;
//! * the core: a request that never gets an answer is declared lost after its deadline and the input path is in
//!   Direct well inside the design budget of two seconds (NFR-102).
//!
//! What this does NOT prove: that a worker that hangs for 30 s over a pipe leaves the text service alone in the
//! shipped build. The text service talks to the core **in process** today
//! (`crates/pime-tsf/src/predictor.rs` -> `pime_core::input::LocalPredictor`), and the call is synchronous
//! (`crates/pime-core/src/input.rs`: `self.port.predict(&request)`), so a core that never returns would block the key
//! event. The isolation that is measured here is the one the core implements for a request that is in flight and gets
//! no answer; the text service has no adapter that arms it yet (the controller never calls
//! `InputMachine::tick`). That gap is reported, not hidden.

mod common;

use common::{FakeHost, code_of};
use pime_core::candidate::Candidate;
use pime_core::input::{
    CORE_REQUEST_TIMEOUT_MS, CoreFault, DIRECT_DEADLINE_MS, InputMachine, KeyEvent, KeyOutcome,
    LocalPredictor, PredictPort, PredictRequest, State,
};
use pime_tsf::controller::{KeyInput, TipConfig, TipController};
use pime_tsf::keymap::Modifiers;
use pime_tsf::ui::RecordingUi;

/// No modifier is held.
const NONE: Modifiers = Modifiers {
    shift: false,
    ctrl: false,
    alt: false,
    win: false,
};

/// A core that does not answer: the request is in flight and the deadline passes (this is what an IPC client with a
/// deadline reports when the other side hangs).
#[derive(Default)]
struct SilentCore {
    calls: usize,
}

impl PredictPort for SilentCore {
    fn predict(&mut self, _request: &PredictRequest) -> Result<Vec<Candidate>, CoreFault> {
        self.calls += 1;
        Err(CoreFault::Timeout)
    }
}

/// The text service under test, driven the way a host drives it.
struct Rig {
    ctl: TipController<RecordingUi, FakeHost, SilentCore>,
    clock: u64,
}

impl Rig {
    fn new() -> Rig {
        let mut ctl = TipController::new(
            RecordingUi::default(),
            FakeHost::with_doc(""),
            SilentCore::default(),
            TipConfig::default(),
        );
        ctl.activate(false);
        Rig { ctl, clock: 0 }
    }

    /// One key press, delivered the way the TSF contract says. Returns whether the key was eaten.
    fn press(&mut self, code: u32, ch: Option<char>) -> bool {
        self.clock += 20;
        let down = KeyInput {
            code,
            lparam: ((code as isize) << 16) | 1,
            ch,
            mods: NONE,
            now_ms: self.clock,
        };
        let eaten = self.ctl.on_test_key_down(down) && self.ctl.on_key_down(down);
        if !eaten {
            // what the host does with a key the input method did not eat
            if let Some(c) = ch {
                if !c.is_control() {
                    self.ctl.host_mut().doc.push(c);
                }
            }
        }
        self.clock += 20;
        let up = KeyInput {
            code,
            lparam: ((code as isize) << 16) | 1,
            ch,
            mods: NONE,
            now_ms: self.clock,
        };
        self.ctl.on_test_key_up(up);
        self.ctl.on_key_up(up);
        eaten
    }
}

/// The text service keeps the keyboard when the core does not answer.
#[test]
fn a_core_that_does_not_answer_does_not_interrupt_the_raw_keystrokes() {
    let mut rig = Rig::new();
    for c in "fn".chars() {
        assert!(!rig.press(code_of(c), Some(c)), "the key {c:?} was eaten");
    }
    assert_eq!(rig.ctl.host().doc, "fn", "the host got the characters");
    assert_eq!(
        rig.ctl.predictor().calls,
        1,
        "the core was asked once; after the failure no further request is sent"
    );
    assert_eq!(
        rig.ctl.machine().state(),
        State::FaultFallback,
        "the input method knows that the core is gone"
    );
    assert!(
        rig.ctl.machine().candidates().is_empty(),
        "no candidates are shown"
    );
    assert!(
        rig.ctl.machine().context().is_empty(),
        "the text is not cached: {:?}",
        rig.ctl.machine().context()
    );
    assert!(!rig.ctl.ui().visible, "no window is shown");
}

/// The core isolates a request that never gets an answer, and the input path is in Direct inside the budget.
#[test]
fn the_core_isolates_a_request_that_never_gets_an_answer() {
    let mut machine = InputMachine::new(LocalPredictor, "rust");
    machine.grant_observation(true);
    // the request is sent and the other side hangs: nothing answers it
    machine.begin_core_request(0);
    let mut direct_at = None;
    for now in 0..=DIRECT_DEADLINE_MS {
        machine.tick(now);
        // the keyboard is replayed while the request is in flight: no key is swallowed or dropped
        let step = machine.on_key(KeyEvent::ch('x'), now);
        assert_eq!(step.outcome, KeyOutcome::PassThrough, "now={now}");
        if direct_at.is_none() && machine.state() == State::FaultFallback {
            direct_at = Some(now);
        }
    }
    let direct_at = direct_at.expect("Direct must be in force by 2000 ms");
    assert_eq!(
        direct_at, CORE_REQUEST_TIMEOUT_MS,
        "the request is declared lost after its own deadline"
    );
    assert!(
        direct_at <= DIRECT_DEADLINE_MS,
        "design budget: <= 2 s (the system level measurement belongs to ST)"
    );
    assert!(machine.candidates().is_empty(), "the candidates are gone");
    assert!(machine.context().is_empty(), "the text is not cached");
}

/// A request that is answered is never declared lost: the watchdog only fires for a request in flight.
#[test]
fn an_answered_request_never_triggers_the_watchdog() {
    let mut machine = InputMachine::new(LocalPredictor, "rust");
    machine.grant_observation(true);
    machine.begin_core_request(0);
    machine.complete_core_request();
    for now in 0..=DIRECT_DEADLINE_MS {
        assert!(machine.tick(now).is_empty(), "now={now}");
    }
    assert_eq!(machine.state(), State::Observing);
}
