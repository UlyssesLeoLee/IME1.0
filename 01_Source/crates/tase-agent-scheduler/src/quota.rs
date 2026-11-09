//! The quotas of the scheduler: the concurrency and the rate limits of every scope, and the behaviour of an
//! overflow (detailed design §17.5).
//!
//! The design book asks for four limits before a job is leased: "if quota_global AND quota_workspace AND
//! quota_agent AND budget_available". The quotas of this module are the three first ones; the fourth one,
//! the budget, lives in [`crate::budget`].
//!
//! Every quota has a **scope** — the whole Hub, a workspace, a tenant or an agent — and two kinds of limit:
//!
//! * a **concurrency** limit: how many jobs of the scope may run at the same time;
//! * a **rate** limit: how many jobs of the scope may start inside a window of time.
//!
//! When a limit is reached, the behaviour is explicit and is chosen per rule: the request is **refused**
//! with a reason code, or it **stays queued** and the caller is told when to come back. The design book
//! takes the second road ("keep_queued_and_report_retry_hint"), so `Queue` is the default of the rules of
//! the Hub.
//!
//! The crate never reads the clock: the caller passes the instant of every check, so a rate window is
//! exact in a test.

use crate::Timestamp;
use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;
use tase_contract::{AgentId, TaseError, TenantId, WorkspaceId};

/// The scope a quota applies to.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QuotaScope {
    /// The whole Hub.
    Global,
    /// One workspace.
    Workspace(WorkspaceId),
    /// One tenant.
    Tenant(TenantId),
    /// One agent.
    Agent(AgentId),
}

impl QuotaScope {
    /// The name of the scope, for the logs and the reason codes.
    pub fn as_str(&self) -> &'static str {
        match self {
            QuotaScope::Global => "global",
            QuotaScope::Workspace(_) => "workspace",
            QuotaScope::Tenant(_) => "tenant",
            QuotaScope::Agent(_) => "agent",
        }
    }
}

/// What the scheduler does when a quota is reached.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverflowBehavior {
    /// The request is refused with a reason code.
    Reject,
    /// The request stays queued and the caller is told when to come back.
    #[default]
    Queue,
}

/// The reason a quota is reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuotaReason {
    /// The number of the running jobs of the scope is at the limit.
    ConcurrencyLimit,
    /// The number of the jobs of the scope inside the window is at the limit.
    RateLimit,
}

impl QuotaReason {
    /// The name of the reason, for the logs and the reason codes.
    pub const fn as_str(self) -> &'static str {
        match self {
            QuotaReason::ConcurrencyLimit => "CONCURRENCY_LIMIT",
            QuotaReason::RateLimit => "RATE_LIMIT",
        }
    }
}

/// The rate limit of a scope: at most `limit` jobs inside `window`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateLimit {
    /// The number of the jobs allowed inside the window.
    pub limit: u32,
    /// The width of the window.
    pub window: Duration,
}

impl RateLimit {
    /// A rate limit of `limit` jobs inside a window of `window`.
    pub fn new(limit: u32, window: Duration) -> Self {
        RateLimit { limit, window }
    }
}

/// One quota of one scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuotaRule {
    /// The scope of the rule.
    pub scope: QuotaScope,
    /// The number of the jobs of the scope that may run at the same time.
    pub concurrency: Option<u32>,
    /// The number of the jobs of the scope that may start inside a window.
    pub rate: Option<RateLimit>,
    /// What the scheduler does when the rule is reached.
    pub on_exceed: OverflowBehavior,
}

impl QuotaRule {
    /// A rule of a scope that only limits the concurrency.
    pub fn concurrency(scope: QuotaScope, limit: u32) -> Self {
        QuotaRule {
            scope,
            concurrency: Some(limit),
            rate: None,
            on_exceed: OverflowBehavior::default(),
        }
    }

    /// A rule of a scope that only limits the rate.
    pub fn rate(scope: QuotaScope, limit: u32, window: Duration) -> Self {
        QuotaRule {
            scope,
            concurrency: None,
            rate: Some(RateLimit::new(limit, window)),
            on_exceed: OverflowBehavior::default(),
        }
    }

    /// The same rule with another behaviour of the overflow.
    pub fn with_behavior(mut self, on_exceed: OverflowBehavior) -> Self {
        self.on_exceed = on_exceed;
        self
    }
}

/// The subject of a request: who asks, and in which workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuotaSubject {
    /// The tenant of the request.
    pub tenant: Option<TenantId>,
    /// The workspace of the request.
    pub workspace: Option<WorkspaceId>,
    /// The agent of the request.
    pub agent: AgentId,
}

impl QuotaSubject {
    /// A subject of an agent, without a tenant and without a workspace.
    pub fn agent(agent: AgentId) -> Self {
        QuotaSubject {
            tenant: None,
            workspace: None,
            agent,
        }
    }

    /// The same subject in a workspace.
    pub fn in_workspace(mut self, workspace: WorkspaceId) -> Self {
        self.workspace = Some(workspace);
        self
    }

    /// The same subject in a tenant.
    pub fn in_tenant(mut self, tenant: TenantId) -> Self {
        self.tenant = Some(tenant);
        self
    }
}

/// The verdict of the quotas for one request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuotaDecision {
    /// Every quota of the subject allows the request.
    Allowed,
    /// The request is refused: the scope that refuses it and the reason.
    Rejected {
        /// The scope that refuses the request.
        scope: QuotaScope,
        /// The reason of the refusal.
        reason: QuotaReason,
    },
    /// The request stays queued: the scope that holds it, the reason and when to come back.
    Queued {
        /// The scope that holds the request.
        scope: QuotaScope,
        /// The reason of the hold.
        reason: QuotaReason,
        /// How long the caller waits before it comes back; `None` when the wait is not known.
        retry_after: Option<Duration>,
    },
}

impl QuotaDecision {
    /// Whether every quota allows the request.
    pub fn is_allowed(&self) -> bool {
        matches!(self, QuotaDecision::Allowed)
    }

    /// The reason of the decision, or `None` when the request is allowed.
    pub fn reason(&self) -> Option<QuotaReason> {
        match self {
            QuotaDecision::Allowed => None,
            QuotaDecision::Rejected { reason, .. } | QuotaDecision::Queued { reason, .. } => Some(*reason),
        }
    }

    /// The scope that holds the request, or `None` when the request is allowed.
    pub fn scope(&self) -> Option<&QuotaScope> {
        match self {
            QuotaDecision::Allowed => None,
            QuotaDecision::Rejected { scope, .. } | QuotaDecision::Queued { scope, .. } => Some(scope),
        }
    }

    /// How long the caller waits before it comes back, or `None`.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            QuotaDecision::Queued { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    /// The failure of the contract layer for a decision that is not an allowance.
    pub fn to_error(&self) -> Option<TaseError> {
        match self {
            QuotaDecision::Allowed => None,
            QuotaDecision::Rejected { scope, reason } | QuotaDecision::Queued { scope, reason, .. } => {
                Some(
                    TaseError::rate_limited(format!(
                        "the quota of the scope {} is reached: {}",
                        scope.as_str(),
                        reason.as_str()
                    ))
                    .with_details(scope.as_str()),
                )
            }
        }
    }
}

/// The tracker of the quotas of the Hub.
///
/// The tracker counts the jobs that run and the jobs that started inside their window. It holds nothing but
/// the counters, so a restart of the Hub starts the counters from zero: the truth about the jobs that run
/// lives in the store of the Hub, not here.
#[derive(Clone, Debug, Default)]
pub struct QuotaTracker {
    rules: Vec<QuotaRule>,
    running: BTreeMap<QuotaScope, u32>,
    windows: BTreeMap<QuotaScope, VecDeque<Timestamp>>,
}

impl QuotaTracker {
    /// A tracker without a rule.
    pub fn new() -> Self {
        QuotaTracker::default()
    }

    /// Adds a rule; a rule needs at least one limit and a limit of zero is refused.
    pub fn add_rule(&mut self, rule: QuotaRule) -> Result<(), TaseError> {
        if rule.concurrency.is_none() && rule.rate.is_none() {
            return Err(TaseError::invalid_argument(
                "a quota rule needs a concurrency limit or a rate limit",
            ));
        }
        if rule.concurrency == Some(0) {
            return Err(TaseError::invalid_argument(
                "the concurrency limit of a quota must be at least one",
            ));
        }
        if let Some(rate) = rule.rate {
            if rate.limit == 0 {
                return Err(TaseError::invalid_argument(
                    "the rate limit of a quota must be at least one",
                ));
            }
            if rate.window.is_zero() {
                return Err(TaseError::invalid_argument(
                    "the window of a rate limit must not be zero",
                ));
            }
        }
        self.rules.push(rule);
        Ok(())
    }

    /// The rules of the tracker.
    pub fn rules(&self) -> &[QuotaRule] {
        &self.rules
    }

    /// The number of the jobs that run in a scope.
    pub fn running(&self, scope: &QuotaScope) -> u32 {
        self.running.get(scope).copied().unwrap_or(0)
    }

    /// The verdict of the quotas for a subject, without consuming anything.
    pub fn check(&self, subject: &QuotaSubject, now: Timestamp) -> QuotaDecision {
        for rule in &self.rules {
            if !rule.applies_to(subject) {
                continue;
            }
            if let Some(limit) = rule.concurrency
                && self.running.get(&rule.scope).copied().unwrap_or(0) >= limit
            {
                return decide(rule, QuotaReason::ConcurrencyLimit, None);
            }
            if let Some(rate) = rule.rate {
                let window = self.windows.get(&rule.scope);
                let live = live_count(window, now, rate.window);
                let live = u32::try_from(live).unwrap_or(u32::MAX);
                if live >= rate.limit {
                    let retry_after = window.and_then(|events| {
                        events
                            .iter()
                            .find(|at| now.saturating_duration_since(**at) < rate.window)
                    }).map(|oldest| {
                        rate.window
                            .saturating_sub(now.saturating_duration_since(*oldest))
                    });
                    return decide(rule, QuotaReason::RateLimit, retry_after);
                }
            }
        }
        QuotaDecision::Allowed
    }

    /// The verdict of the quotas for a subject; an allowed request consumes the quota.
    pub fn try_acquire(&mut self, subject: &QuotaSubject, now: Timestamp) -> QuotaDecision {
        let decision = self.check(subject, now);
        if !decision.is_allowed() {
            return decision;
        }
        for rule in &self.rules {
            if !rule.applies_to(subject) {
                continue;
            }
            if rule.concurrency.is_some() {
                *self.running.entry(rule.scope.clone()).or_insert(0) += 1;
            }
            if let Some(rate) = rule.rate {
                let events = self.windows.entry(rule.scope.clone()).or_default();
                while let Some(oldest) = events.front() {
                    if now.saturating_duration_since(*oldest) >= rate.window {
                        events.pop_front();
                    } else {
                        break;
                    }
                }
                events.push_back(now);
            }
        }
        QuotaDecision::Allowed
    }

    /// Gives the concurrency of a subject back when its job is over.
    pub fn release(&mut self, subject: &QuotaSubject) {
        for rule in &self.rules {
            if !rule.applies_to(subject) {
                continue;
            }
            if rule.concurrency.is_some()
                && let Some(running) = self.running.get_mut(&rule.scope)
            {
                *running = running.saturating_sub(1);
            }
        }
    }
}

impl QuotaRule {
    /// Whether the rule applies to a subject.
    ///
    /// A rule of a workspace applies only to a subject that names a workspace, and a rule of a tenant only
    /// to a subject that names a tenant: a scope that the subject does not declare cannot be checked.
    fn applies_to(&self, subject: &QuotaSubject) -> bool {
        match &self.scope {
            QuotaScope::Global => true,
            QuotaScope::Workspace(workspace) => subject.workspace.as_ref() == Some(workspace),
            QuotaScope::Tenant(tenant) => subject.tenant.as_ref() == Some(tenant),
            QuotaScope::Agent(agent) => &subject.agent == agent,
        }
    }
}

/// The decision of a rule that is reached.
fn decide(rule: &QuotaRule, reason: QuotaReason, retry_after: Option<Duration>) -> QuotaDecision {
    match rule.on_exceed {
        OverflowBehavior::Reject => QuotaDecision::Rejected {
            scope: rule.scope.clone(),
            reason,
        },
        OverflowBehavior::Queue => QuotaDecision::Queued {
            scope: rule.scope.clone(),
            reason,
            retry_after,
        },
    }
}

/// The number of the events of a window that are still inside the window at `now`.
fn live_count(events: Option<&VecDeque<Timestamp>>, now: Timestamp, window: Duration) -> usize {
    events.map_or(0, |events| {
        events
            .iter()
            .filter(|at| now.saturating_duration_since(**at) < window)
            .count()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tase_contract::ErrorCode;

    /// An agent of the tests.
    fn agent(name: &str) -> AgentId {
        AgentId::parse(name).expect("a legal agent")
    }

    /// A workspace of the tests.
    fn workspace(name: &str) -> WorkspaceId {
        WorkspaceId::parse(name).expect("a legal workspace")
    }

    /// A tenant of the tests.
    fn tenant(name: &str) -> TenantId {
        TenantId::parse(name).expect("a legal tenant")
    }

    #[test]
    fn a_rule_without_a_limit_is_refused() {
        let mut tracker = QuotaTracker::new();
        let rule = QuotaRule {
            scope: QuotaScope::Global,
            concurrency: None,
            rate: None,
            on_exceed: OverflowBehavior::Queue,
        };
        let error = tracker.add_rule(rule).expect_err("a rule needs a limit");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn a_limit_of_zero_is_refused() {
        let mut tracker = QuotaTracker::new();
        let error = tracker
            .add_rule(QuotaRule::concurrency(QuotaScope::Global, 0))
            .expect_err("a limit of zero is refused");
        assert_eq!(error.code, ErrorCode::InvalidArgument);

        let error = tracker
            .add_rule(QuotaRule::rate(
                QuotaScope::Global,
                1,
                Duration::ZERO,
            ))
            .expect_err("a window of zero is refused");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn the_concurrency_limit_refuses_the_request_that_is_too_many() {
        let mut tracker = QuotaTracker::new();
        tracker
            .add_rule(
                QuotaRule::concurrency(QuotaScope::Global, 1)
                    .with_behavior(OverflowBehavior::Reject),
            )
            .unwrap();
        let subject = QuotaSubject::agent(agent("agent_01"));

        let first = tracker.try_acquire(&subject, Timestamp::zero());
        assert!(first.is_allowed());
        assert_eq!(tracker.running(&QuotaScope::Global), 1);

        let second = tracker.try_acquire(&subject, Timestamp::zero());
        assert_eq!(
            second,
            QuotaDecision::Rejected {
                scope: QuotaScope::Global,
                reason: QuotaReason::ConcurrencyLimit,
            }
        );
        assert_eq!(second.reason(), Some(QuotaReason::ConcurrencyLimit));
        assert_eq!(second.scope(), Some(&QuotaScope::Global));

        // The job is over: the slot is free again.
        tracker.release(&subject);
        assert_eq!(tracker.running(&QuotaScope::Global), 0);
        assert!(tracker.try_acquire(&subject, Timestamp::zero()).is_allowed());
    }

    #[test]
    fn the_overflow_can_queue_the_request_instead_of_refusing_it() {
        let mut tracker = QuotaTracker::new();
        tracker
            .add_rule(QuotaRule::concurrency(QuotaScope::Agent(agent("agent_01")), 1))
            .unwrap();
        let subject = QuotaSubject::agent(agent("agent_01"));

        assert!(tracker.try_acquire(&subject, Timestamp::zero()).is_allowed());
        let held = tracker.try_acquire(&subject, Timestamp::zero());
        assert!(matches!(held, QuotaDecision::Queued { .. }));
        assert_eq!(held.reason(), Some(QuotaReason::ConcurrencyLimit));
        // The concurrency of a scope does not know when it frees a slot.
        assert_eq!(held.retry_after(), None);
    }

    #[test]
    fn the_rate_limit_counts_the_jobs_of_the_window() {
        let mut tracker = QuotaTracker::new();
        tracker
            .add_rule(QuotaRule::rate(
                QuotaScope::Global,
                2,
                Duration::from_secs(10),
            ))
            .unwrap();
        let subject = QuotaSubject::agent(agent("agent_01"));
        let start = Timestamp::zero();

        assert!(tracker.try_acquire(&subject, start).is_allowed());
        assert!(tracker
            .try_acquire(&subject, Timestamp::from_millis(1))
            .is_allowed());
        let third = tracker.try_acquire(&subject, Timestamp::from_millis(2));
        assert_eq!(third.reason(), Some(QuotaReason::RateLimit));
        assert_eq!(
            third.retry_after(),
            Some(Duration::from_millis(9_998)),
            "the window opens when the first job leaves it"
        );

        // The window slides: the first job leaves it after ten seconds.
        let later = Timestamp::from_millis(10_000);
        assert!(tracker.try_acquire(&subject, later).is_allowed());
        assert_eq!(
            tracker.try_acquire(&subject, later).reason(),
            Some(QuotaReason::RateLimit),
            "the second job is still inside the window"
        );
    }

    #[test]
    fn the_rate_limit_of_a_scope_does_not_count_the_jobs_of_another_scope() {
        let mut tracker = QuotaTracker::new();
        tracker
            .add_rule(QuotaRule::rate(
                QuotaScope::Agent(agent("agent_01")),
                1,
                Duration::from_secs(10),
            ))
            .unwrap();
        let first = QuotaSubject::agent(agent("agent_01"));
        let second = QuotaSubject::agent(agent("agent_02"));

        assert!(tracker.try_acquire(&first, Timestamp::zero()).is_allowed());
        assert!(tracker.try_acquire(&second, Timestamp::zero()).is_allowed());
        assert_eq!(
            tracker.try_acquire(&first, Timestamp::zero()).reason(),
            Some(QuotaReason::RateLimit)
        );
    }

    #[test]
    fn the_quota_of_the_agent_is_checked_next_to_the_global_one() {
        let mut tracker = QuotaTracker::new();
        tracker
            .add_rule(QuotaRule::concurrency(QuotaScope::Global, 10))
            .unwrap();
        tracker
            .add_rule(QuotaRule::concurrency(
                QuotaScope::Agent(agent("agent_01")),
                1,
            ))
            .unwrap();
        let subject = QuotaSubject::agent(agent("agent_01"));

        assert!(tracker.try_acquire(&subject, Timestamp::zero()).is_allowed());
        let second = tracker.try_acquire(&subject, Timestamp::zero());
        assert_eq!(
            second.scope(),
            Some(&QuotaScope::Agent(agent("agent_01"))),
            "the agent is the scope that holds the request"
        );
    }

    #[test]
    fn a_scope_that_the_subject_does_not_declare_is_not_checked() {
        let mut tracker = QuotaTracker::new();
        tracker
            .add_rule(QuotaRule::concurrency(QuotaScope::Workspace(workspace("ws_01")), 1))
            .unwrap();
        tracker
            .add_rule(QuotaRule::concurrency(QuotaScope::Tenant(tenant("tn_01")), 1))
            .unwrap();

        let without = QuotaSubject::agent(agent("agent_01"));
        assert!(
            tracker.try_acquire(&without, Timestamp::zero()).is_allowed(),
            "a subject without a workspace does not run into the quota of a workspace"
        );

        let with = QuotaSubject::agent(agent("agent_01"))
            .in_workspace(workspace("ws_01"))
            .in_tenant(tenant("tn_01"));
        assert!(tracker.try_acquire(&with, Timestamp::zero()).is_allowed());
        assert_eq!(
            tracker.try_acquire(&with, Timestamp::zero()).scope(),
            Some(&QuotaScope::Workspace(workspace("ws_01"))),
            "the workspace is the first scope that is checked"
        );
    }

    #[test]
    fn a_refusal_is_a_rate_limited_failure_of_the_contract() {
        let decision = QuotaDecision::Rejected {
            scope: QuotaScope::Global,
            reason: QuotaReason::ConcurrencyLimit,
        };
        let error = decision.to_error().expect("a refusal is a failure");
        assert_eq!(error.code, ErrorCode::RateLimited);
        assert!(error.message.contains("CONCURRENCY_LIMIT"));
        assert!(error.code.retryable());

        let allowed = QuotaDecision::Allowed;
        assert!(allowed.to_error().is_none());
        assert!(allowed.is_allowed());
        assert_eq!(allowed.reason(), None);
        assert_eq!(allowed.scope(), None);
        assert_eq!(allowed.retry_after(), None);
    }

    #[test]
    fn the_rules_of_the_tracker_are_kept() {
        let mut tracker = QuotaTracker::new();
        tracker
            .add_rule(QuotaRule::concurrency(QuotaScope::Global, 4))
            .unwrap();
        assert_eq!(tracker.rules().len(), 1);
        assert_eq!(tracker.rules()[0].concurrency, Some(4));
        assert_eq!(tracker.rules()[0].on_exceed, OverflowBehavior::Queue);
    }
}
