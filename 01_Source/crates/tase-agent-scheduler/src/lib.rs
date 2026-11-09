//! TASE agent scheduler: the fair queues of the agents, the resource quotas, the leases with their fencing
//! tokens, the budgets and the dispatch of the events (detailed design §17.5).
//!
//! The scheduler is the part of the resident Local Agent Hub that decides **which job runs next and which
//! job has to wait**. The Hub itself owns the sessions, the workspaces, the transport and the persistence
//! (`tase-local-hub`, `tase-local-ipc`, `tase-local-store`); the scheduler owns the order of the work and the
//! limits that keep the machine from being drowned by it.
//!
//! # The five parts
//!
//! * [`queues`] — the five layered work classes of the design book (`interactive_candidate`, `preview_read`,
//!   `prepare`, `verify`, `commit`) and, inside every class, the deficit round robin over the agents;
//! * [`quota`] — the concurrency and the rate limits of the global scope, of the workspace, of the tenant and
//!   of the agent, with the behaviour of an overflow (refuse or queue) and a reason code;
//! * [`leases`] — the lease of a job, its TTL, the reclaim of an expired lease and the fencing token that
//!   refuses the late write of a holder that has been replaced;
//! * [`budget`] — the budget of the tokens and of the cost, per agent and per session, and the verdict of a
//!   request that goes over it;
//! * [`events`] — the dispatch of the events to the subscribers: ordered per agent, delivered at least once
//!   and filtered by a subscription.
//!
//! # The two rules of §17.5 that this crate implements
//!
//! The design book asks for two things at once, and they are two different kinds of fairness:
//!
//! * **between the classes**: a flood of cheap reads must not drown the verification work. The class of a
//!   task decides how soon it runs, and the aging ([`queues::Aging`]) promotes a class that has been waiting,
//!   so a stream of fresh `preview_read` work cannot postpone a `commit` for ever;
//! * **between the agents**: inside a class, the agents share the work by their weight. The deficit round
//!   robin grants every agent `weight × quantum` credits per visit and the agent spends what it can, so the
//!   share of the agents follows their weights and no agent is starved.
//!
//! Both are decidable: while every queue stays full, the share of an agent is within one round of its ideal
//! share, and the wait of an agent is bounded by the sum of the weights.
//!
//! # What this crate does NOT hold
//!
//! * **The persistence.** The jobs, the leases and the outbox of §18 live in `tase-local-store`; the
//!   scheduler holds them in memory and hands them back. Nothing here survives a restart of the Hub.
//! * **The distributed fairness.** The round robin of this crate is the fairness **inside one Hub process** on
//!   one Windows machine, which is exactly what §17.5 asks for ("使用单进程 Hub 的分层队列（同机）"). A fair
//!   share between two machines, or between two Hub processes of two logon sessions, is a different problem:
//!   it needs a shared store and a distributed agreement on the order, and it belongs to a later layer, not
//!   to a pure in-memory crate.
//! * **The pipeline of the Hub.** The design book draws the pipeline of `on_submit` and `on_schedule`: the
//!   session check, the persistence of the job and of the outbox in one transaction, the class choice, the
//!   round robin, the quota, the budget, the lease and the run of the preparation. This crate owns the four
//!   middle steps — the class, the round robin, the quota, the budget and the lease — and leaves the check
//!   of the session, the transaction and the run to the Hub, which owns the transport and the disk.
//! * **The clock.** The crate never reads the wall clock. Every deadline, every window and every age comes
//!   from the caller as a [`Timestamp`], so a test moves the clock by exactly the duration it needs and no
//!   test ever waits.
//!
//! # The crate has no third-party dependency
//!
//! The identifiers and the business error codes come from `tase-contract`; everything else is the standard
//! library.

#![forbid(unsafe_code)]

pub mod budget;
pub mod events;
pub mod leases;
pub mod quota;
pub mod queues;

use std::fmt;
use std::time::Duration;

pub use budget::{BudgetCheck, BudgetDecision, BudgetKind, BudgetLedger, BudgetOutcome, BudgetScope, BudgetVerdict};
pub use events::{DeliveryReport, Dispatcher, Event, EventId, EventKind, SubscriberId, SubscriptionFilter};
pub use leases::{FenceCheck, FencingToken, Lease, LeaseRegistry};
pub use quota::{OverflowBehavior, QuotaDecision, QuotaReason, QuotaRule, QuotaScope, QuotaSubject, QuotaTracker, RateLimit};
pub use queues::{Aging, ClassQueue, Scheduler, Task, WorkClass};

/// The version of the scheduler.
pub const SCHEDULER_VERSION: &str = "1.1.0";

/// A logical instant, in milliseconds, counted from an epoch the caller chooses.
///
/// The crate never reads the wall clock: every time comes from the caller, as a [`Timestamp`]. A test moves
/// the clock with a [`LogicalClock`] and every deadline, every window and every age is then exact.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(u64);

impl Timestamp {
    /// The first instant of the epoch of the caller.
    pub const ZERO: Timestamp = Timestamp(0);

    /// An instant of `millis` milliseconds after the epoch of the caller.
    pub const fn from_millis(millis: u64) -> Self {
        Timestamp(millis)
    }

    /// The instant of the zero of the epoch.
    pub const fn zero() -> Self {
        Self::ZERO
    }

    /// The number of the milliseconds between the epoch and this instant.
    pub const fn as_millis(self) -> u64 {
        self.0
    }

    /// The duration between `earlier` and this instant; zero when `earlier` is later.
    pub fn saturating_duration_since(self, earlier: Timestamp) -> Duration {
        Duration::from_millis(self.0.saturating_sub(earlier.0))
    }

    /// This instant plus `delay`, or `None` when the addition would overflow.
    pub fn checked_add(self, delay: Duration) -> Option<Timestamp> {
        let millis = u64::try_from(delay.as_millis()).ok()?;
        self.0.checked_add(millis).map(Timestamp)
    }

    /// This instant plus `delay`, saturating at the end of the range.
    pub fn saturating_add(self, delay: Duration) -> Timestamp {
        let millis = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX);
        Timestamp(self.0.saturating_add(millis))
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "+{}ms", self.0)
    }
}

/// A clock the caller drives: the only source of the time of this crate.
///
/// The clock counts the milliseconds of the epoch of the caller and never reads the wall clock, so a test
/// moves it forward by exactly the duration it needs and every deadline of the crate is then exact. No test
/// of this crate ever waits.
#[derive(Clone, Copy, Debug, Default)]
pub struct LogicalClock {
    now: Timestamp,
}

impl LogicalClock {
    /// A clock at `start`.
    pub const fn new(start: Timestamp) -> Self {
        LogicalClock { now: start }
    }

    /// The current instant of the clock.
    pub const fn now(&self) -> Timestamp {
        self.now
    }

    /// Moves the clock forward by `delay` and returns the new instant.
    pub fn advance(&mut self, delay: Duration) -> Timestamp {
        self.now = self.now.saturating_add(delay);
        self.now
    }

    /// Moves the clock forward by `millis` milliseconds and returns the new instant.
    pub fn advance_millis(&mut self, millis: u64) -> Timestamp {
        self.now = Timestamp(self.now.0.saturating_add(millis));
        self.now
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_timestamp_is_a_count_of_the_milliseconds() {
        let start = Timestamp::from_millis(1_000);
        assert_eq!(start.as_millis(), 1_000);
        assert_eq!(Timestamp::zero(), Timestamp::from_millis(0));
        assert_eq!(Timestamp::default(), Timestamp::zero());
        assert_eq!(start.to_string(), "+1000ms");
    }

    #[test]
    fn the_duration_between_two_instants_never_goes_below_zero() {
        let early = Timestamp::from_millis(100);
        let late = Timestamp::from_millis(400);
        assert_eq!(late.saturating_duration_since(early), Duration::from_millis(300));
        assert_eq!(early.saturating_duration_since(late), Duration::ZERO);
    }

    #[test]
    fn an_instant_is_added_with_a_duration() {
        let start = Timestamp::from_millis(10);
        assert_eq!(start.saturating_add(Duration::from_millis(5)).as_millis(), 15);
        assert_eq!(start.checked_add(Duration::from_millis(5)), Some(Timestamp::from_millis(15)));
        assert_eq!(start.checked_add(Duration::MAX), None);
        assert_eq!(start.saturating_add(Duration::MAX), Timestamp::from_millis(u64::MAX));
    }

    #[test]
    fn the_clock_only_moves_when_the_caller_moves_it() {
        let mut clock = LogicalClock::new(Timestamp::from_millis(100));
        assert_eq!(clock.now().as_millis(), 100);
        assert_eq!(clock.advance(Duration::from_millis(50)).as_millis(), 150);
        assert_eq!(clock.advance_millis(25).as_millis(), 175);
        assert_eq!(clock.now().as_millis(), 175);
    }

    #[test]
    fn the_clock_of_the_default_starts_at_the_zero() {
        let clock = LogicalClock::default();
        assert_eq!(clock.now(), Timestamp::zero());
    }
}
