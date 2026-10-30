//! Input session epochs, realtime AI scheduling and Direct fallback (FR-027/028, NFR-102/202, DD section 8/16),
//! port of `core/scheduler.js`. Local candidates are never routed through here: this only governs the optional,
//! asynchronous AI path.
//!
//! The reference mixes decisions with timers, `AbortController`s and a promise chain. Here the decisions are a pure
//! state machine ([`Scheduler`]): the bridge keeps the timer, the abort signal and the one-request-at-a-time chain,
//! reports what happened (`schedule`, `fire`, `finish`, `cancel`) and carries out the effects it gets back. Time is
//! never read: the debounce delay is returned, and tests drive a virtual clock.

use crate::pack::{LANGUAGES, Registry};
use crate::resolve::{Decision, ResolveContext, resolve_language};

pub const DEBOUNCE_MIN_MS: i64 = 150;
pub const DEBOUNCE_MAX_MS: i64 = 1000;
pub const DEBOUNCE_DEFAULT_MS: i64 = 350;

/// `clampDebounce`: a non-finite value is the default.
pub fn clamp_debounce(ms: Option<f64>) -> i64 {
    match ms {
        Some(v) if v.is_finite() => v.clamp(DEBOUNCE_MIN_MS as f64, DEBOUNCE_MAX_MS as f64) as i64,
        _ => DEBOUNCE_DEFAULT_MS,
    }
}

/// `{seq, profileEpoch, documentEpoch}`: any change of one invalidates results tied to the older value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub seq: u64,
    pub profile_epoch: u64,
    pub document_epoch: u64,
}

// ---------------------------------------------------------------- input session

/// Where a lock applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockScope {
    File,
    Global,
}

#[derive(Clone, Debug)]
pub struct InputSession {
    pub language: String,
    pub profile_epoch: u64,
    pub document_epoch: u64,
    pub input_seq: u64,
    /// Insertion-ordered like a JavaScript `Map` (setting an existing key keeps its position).
    file_locks: Vec<(String, String)>,
    global_lock: Option<String>,
}

impl Default for InputSession {
    fn default() -> Self {
        InputSession::new("unknown")
    }
}

impl InputSession {
    pub fn new(language: &str) -> InputSession {
        InputSession {
            language: language.to_string(),
            profile_epoch: 0,
            document_epoch: 0,
            input_seq: 0,
            file_locks: Vec::new(),
            global_lock: None,
        }
    }

    pub fn note_input(&mut self) -> u64 {
        self.input_seq += 1;
        self.input_seq
    }

    pub fn note_document_change(&mut self) -> u64 {
        self.document_epoch += 1;
        self.document_epoch
    }

    /// `true` when the language actually changed (which starts a new profile epoch).
    pub fn set_language(&mut self, language: &str) -> bool {
        if language == self.language {
            return false;
        }
        self.language = language.to_string();
        self.profile_epoch += 1;
        true
    }

    /// `false` for a profile that is not a supported language.
    pub fn lock(&mut self, profile: &str, scope: LockScope, file_key: &str) -> bool {
        if !LANGUAGES.contains(&profile) {
            return false;
        }
        match scope {
            LockScope::Global => self.global_lock = Some(profile.to_string()),
            LockScope::File => match self.file_locks.iter_mut().find(|(k, _)| k == file_key) {
                Some(slot) => slot.1 = profile.to_string(),
                None => self
                    .file_locks
                    .push((file_key.to_string(), profile.to_string())),
            },
        }
        true
    }

    pub fn unlock(&mut self, scope: LockScope, file_key: &str) {
        match scope {
            LockScope::Global => self.global_lock = None,
            LockScope::File => self.file_locks.retain(|(k, _)| k != file_key),
        }
    }

    /// A file lock beats the global lock.
    pub fn lock_for(&self, file_key: &str) -> Option<&str> {
        self.file_locks
            .iter()
            .find(|(k, _)| k == file_key)
            .map(|(_, v)| v.as_str())
            .or(self.global_lock.as_deref())
    }

    pub fn file_locks(&self) -> &[(String, String)] {
        &self.file_locks
    }

    pub fn global_lock(&self) -> Option<&str> {
        self.global_lock.as_deref()
    }

    /// A manual lock always beats automatic detection; weak evidence never switches the profile (UT-012..015).
    /// The decision's language is the session's language after the (possible) switch.
    pub fn resolve(
        &mut self,
        registry: &Registry,
        ctx: &ResolveContext,
        file_key: &str,
    ) -> Decision {
        let lock = self.lock_for(file_key).map(str::to_string);
        let mut decision = resolve_language(registry, ctx, &self.language, lock.as_deref());
        if !decision.language.is_empty() && decision.language != "unknown" {
            let language = decision.language.clone();
            self.set_language(&language);
        }
        decision.language = self.language.clone();
        decision
    }

    pub fn key(&self) -> Key {
        Key {
            seq: self.input_seq,
            profile_epoch: self.profile_epoch,
            document_epoch: self.document_epoch,
        }
    }

    pub fn is_current(&self, k: &Key) -> bool {
        *k == self.key()
    }
}

// ---------------------------------------------------------------- realtime scheduler

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub scheduled: u64,
    pub started: u64,
    pub delivered: u64,
    pub discarded: u64,
    pub in_flight: u64,
    pub max_in_flight: u64,
}

/// What the bridge does after `schedule` / `cancel`: stop the pending timer, abort the running request and (for
/// `schedule`) start a new timer of `arm_ms`. Aborting is best effort: data already sent cannot be recalled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Effects {
    pub clear_timer: bool,
    pub abort: bool,
    pub arm_ms: Option<i64>,
}

/// The verdict on a finished request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Deliver,
    /// `STALE_RESULT`, the error code of the failed request, or `ERROR`.
    Discard(String),
}

#[derive(Clone, Debug)]
pub struct Scheduler {
    debounce_ms: i64,
    latest: Option<Key>,
    /// The request that is running has been aborted (new input or cancel arrived after it started).
    run_aborted: bool,
    pub stats: Stats,
}

impl Default for Scheduler {
    fn default() -> Self {
        Scheduler::new(None)
    }
}

impl Scheduler {
    pub fn new(debounce_ms: Option<f64>) -> Scheduler {
        Scheduler {
            debounce_ms: clamp_debounce(debounce_ms.or(Some(DEBOUNCE_DEFAULT_MS as f64))),
            latest: None,
            run_aborted: false,
            stats: Stats::default(),
        }
    }

    pub fn debounce_ms(&self) -> i64 {
        self.debounce_ms
    }

    /// The newest input the scheduler has been told about (`None` after a cancel).
    pub fn latest(&self) -> Option<Key> {
        self.latest
    }

    pub fn set_debounce(&mut self, ms: Option<f64>) -> i64 {
        self.debounce_ms = clamp_debounce(ms);
        self.debounce_ms
    }

    /// New input: local state is invalidated at once; `allowed` is the bridge's own gate (AI on, not paused, a grant).
    pub fn schedule(&mut self, key: Key, allowed: bool) -> Effects {
        self.stats.scheduled += 1;
        self.latest = Some(key);
        self.run_aborted = true;
        Effects {
            clear_timer: true,
            abort: true,
            arm_ms: allowed.then_some(self.debounce_ms),
        }
    }

    /// The debounce timer fired and the previous request (if any) is done: start the request?
    pub fn fire(&mut self, key: Key, allowed: bool) -> bool {
        if self.latest != Some(key) || !allowed {
            return false; // superseded while waiting for the previous request
        }
        self.run_aborted = false;
        self.stats.started += 1;
        self.stats.in_flight += 1;
        self.stats.max_in_flight = self.stats.max_in_flight.max(self.stats.in_flight);
        true
    }

    /// The request finished: `Ok(())` with a result, or `Err(code)` (`None` = an error without a code).
    pub fn finish(&mut self, key: Key, outcome: Result<(), Option<&str>>) -> Verdict {
        self.stats.in_flight = self.stats.in_flight.saturating_sub(1);
        match outcome {
            Ok(()) if self.latest == Some(key) && !self.run_aborted => {
                self.stats.delivered += 1;
                Verdict::Deliver
            }
            Ok(()) => {
                self.stats.discarded += 1;
                Verdict::Discard("STALE_RESULT".to_string())
            }
            Err(code) => {
                self.stats.discarded += 1;
                Verdict::Discard(
                    code.filter(|c| !c.is_empty())
                        .unwrap_or("ERROR")
                        .to_string(),
                )
            }
        }
    }

    pub fn cancel(&mut self) -> Effects {
        self.latest = None;
        self.run_aborted = true;
        Effects {
            clear_timer: true,
            abort: true,
            arm_ms: None,
        }
    }
}

// ---------------------------------------------------------------- Direct supervision

/// Core/IPC loss: the input path goes to Direct immediately and stays there until the core answers again.
#[derive(Clone, Debug, Default)]
pub struct CoreSupervisor {
    direct_since: Option<i64>,
    pub failures: u64,
}

impl CoreSupervisor {
    pub fn report_failure(&mut self, now: i64) {
        self.failures += 1;
        if self.direct_since.is_none() {
            self.direct_since = Some(now);
        }
    }

    pub fn report_recovered(&mut self) {
        self.direct_since = None;
    }

    pub fn is_direct(&self) -> bool {
        self.direct_since.is_some()
    }

    /// Time between the failure being reported and Direct being in force (design target: <= 2 s, NFR-102);
    /// `None` while the core is fine.
    pub fn time_to_direct(&self, failure_at: i64) -> Option<i64> {
        self.direct_since.map(|d| d - failure_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seq: u64) -> Key {
        Key {
            seq,
            profile_epoch: 0,
            document_epoch: 0,
        }
    }

    #[test]
    fn debounce_is_clamped_and_defaults_when_not_a_number() {
        let mut s = Scheduler::new(Some(350.0));
        assert_eq!(s.set_debounce(Some(5.0)), 150);
        assert_eq!(s.set_debounce(Some(99_999.0)), 1000);
        assert_eq!(s.set_debounce(None), 350);
        assert_eq!(s.set_debounce(Some(f64::NAN)), 350);
        assert_eq!(s.set_debounce(Some(500.0)), 500);
    }

    #[test]
    fn ut_039_only_the_latest_input_runs_and_stale_results_are_discarded() {
        let mut s = Scheduler::default();
        assert_eq!(s.schedule(key(10), true).arm_ms, Some(350));
        assert!(s.fire(key(10), true));
        // newer input: request 10 is logically stale at once
        s.schedule(key(11), true);
        assert_eq!(
            s.finish(key(10), Ok(())),
            Verdict::Discard("STALE_RESULT".into())
        );
        assert!(s.fire(key(11), true));
        assert_eq!(s.finish(key(11), Ok(())), Verdict::Deliver);
        // a timer of an older input fires late: it must not start
        s.schedule(key(20), true);
        s.schedule(key(21), true);
        assert!(!s.fire(key(20), true));
        assert!(!s.fire(key(21), false), "not allowed any more");
        assert_eq!(
            (s.stats.delivered, s.stats.discarded, s.stats.max_in_flight),
            (1, 1, 1)
        );
    }

    #[test]
    fn an_epoch_change_invalidates_and_a_failed_request_reports_its_code() {
        let mut s = Scheduler::default();
        let a = Key {
            seq: 300,
            profile_epoch: 0,
            document_epoch: 0,
        };
        let b = Key {
            seq: 300,
            profile_epoch: 0,
            document_epoch: 1,
        };
        s.schedule(a, true);
        assert!(s.fire(a, true));
        s.schedule(b, true);
        assert_eq!(s.finish(a, Ok(())), Verdict::Discard("STALE_RESULT".into()));
        assert!(s.fire(b, true));
        assert_eq!(
            s.finish(b, Err(Some("AI_TIMEOUT"))),
            Verdict::Discard("AI_TIMEOUT".into())
        );
        assert!(s.fire(b, true));
        assert_eq!(s.finish(b, Err(None)), Verdict::Discard("ERROR".into()));
        assert!(
            s.cancel().arm_ms.is_none() && !s.fire(b, true),
            "cancel forgets the latest input"
        );
    }

    #[test]
    fn ut_040_pausing_blocks_the_timer() {
        let mut s = Scheduler::default();
        assert_eq!(s.schedule(key(2), false).arm_ms, None);
    }

    #[test]
    fn epochs_locks_and_resolution() {
        let reg = crate::predict::engine().registry();
        let mut s = InputSession::new("rust");
        assert_eq!(
            (s.note_input(), s.note_input(), s.note_document_change()),
            (1, 2, 1)
        );
        assert!(!s.set_language("rust") && s.profile_epoch == 0);
        assert!(s.set_language("python") && s.profile_epoch == 1);
        assert!(!s.lock("cobol", LockScope::File, "k"));
        assert!(s.lock("typescript", LockScope::Global, ""));
        assert!(s.lock("c", LockScope::File, "k"));
        assert_eq!(
            (s.lock_for("k"), s.lock_for("other")),
            (Some("c"), Some("typescript"))
        );
        s.unlock(LockScope::File, "k");
        assert_eq!(s.lock_for("k"), Some("typescript"));
        let k = s.key();
        assert!(s.is_current(&k));
        s.note_input();
        assert!(!s.is_current(&k));
        // a manual lock beats a strong extension
        let ctx = ResolveContext {
            language_id: Some("rust"),
            file_name: Some("x.rs"),
            text: None,
        };
        let d = s.resolve(reg, &ctx, "k");
        assert_eq!(d.language, "typescript");
        assert_eq!(s.language, "typescript");
        s.unlock(LockScope::Global, "");
        let d = s.resolve(reg, &ctx, "k");
        assert_eq!(d.language, "rust");
        assert_eq!(s.profile_epoch, 3);
    }

    #[test]
    fn the_supervisor_goes_direct_at_the_first_failure() {
        let mut sup = CoreSupervisor::default();
        assert!(!sup.is_direct() && sup.time_to_direct(0).is_none());
        sup.report_failure(1500);
        sup.report_failure(9000);
        assert!(sup.is_direct());
        assert_eq!((sup.time_to_direct(1000), sup.failures), (Some(500), 2));
        sup.report_recovered();
        assert!(!sup.is_direct());
    }
}
