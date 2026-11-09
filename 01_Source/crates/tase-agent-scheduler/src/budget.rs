//! The budgets of the scheduler: the tokens and the cost, per agent and per session, and the verdict of a
//! request that goes over them.
//!
//! The design book asks for the budget before a job is leased: "if quota_global AND quota_workspace AND
//! quota_agent AND budget_available". The budget of this module is the fourth condition: it says how much an
//! agent or a session may spend.
//!
//! The budget is counted in two kinds of units:
//!
//! * the **tokens** of the model calls;
//! * the **cost** of the model calls, in micro-units of the price table, so that no float ever reaches a
//!   decision.
//!
//! The verdict is explicit: a request is allowed when the usage it asks for stays **inside** the limit, the
//! limit itself included. A request that goes over is refused, and the decision says which budget refused
//! it, how much was used and how much was left.
//!
//! The crate has no dependency on the metrics of TASE: the verdict of this module is its own type, so that
//! the scheduler stays on the contract layer alone.

use std::collections::BTreeMap;
use tase_contract::{AgentId, SessionId, TaseError};

/// What a budget counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BudgetKind {
    /// The tokens of the model calls.
    Tokens,
    /// The cost of the model calls, in micro-units.
    Cost,
}

impl BudgetKind {
    /// The name of the kind, for the logs and the details of the failures.
    pub const fn as_str(self) -> &'static str {
        match self {
            BudgetKind::Tokens => "tokens",
            BudgetKind::Cost => "cost",
        }
    }
}

/// Who a budget belongs to.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BudgetScope {
    /// The whole Hub.
    Global,
    /// One agent.
    Agent(AgentId),
    /// One session.
    Session(SessionId),
}

impl BudgetScope {
    /// The name of the scope, for the logs and the details of the failures.
    pub fn as_str(&self) -> &'static str {
        match self {
            BudgetScope::Global => "global",
            BudgetScope::Agent(_) => "agent",
            BudgetScope::Session(_) => "session",
        }
    }
}

/// The verdict of a budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BudgetVerdict {
    /// The usage stays inside the budget.
    Allowed,
    /// The usage goes over the budget.
    Exceeded,
}

impl BudgetVerdict {
    /// Whether the usage stays inside the budget.
    pub fn is_allowed(self) -> bool {
        matches!(self, BudgetVerdict::Allowed)
    }
}

/// The decision of one budget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BudgetDecision {
    /// The budget of the decision.
    pub scope: BudgetScope,
    /// The kind of the budget.
    pub kind: BudgetKind,
    /// The verdict.
    pub verdict: BudgetVerdict,
    /// The limit of the budget; `None` when there is no limit.
    pub limit: Option<u64>,
    /// What is already used.
    pub used: u64,
    /// What the request asks for.
    pub requested: u64,
    /// What is left; `None` when there is no limit.
    pub remaining: Option<u64>,
}

impl BudgetDecision {
    /// Whether the request is inside the budget.
    pub fn is_allowed(&self) -> bool {
        self.verdict.is_allowed()
    }

    /// The failure of the contract layer for a request that goes over the budget.
    pub fn to_error(&self) -> Option<TaseError> {
        if self.is_allowed() {
            return None;
        }
        Some(
            TaseError::rate_limited(format!(
                "the budget of the {} in {} is over: {} used, {} asked",
                self.kind.as_str(),
                self.scope.as_str(),
                self.used,
                self.requested
            ))
            .with_details(self.scope.as_str()),
        )
    }
}

/// One budget to check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BudgetCheck {
    /// The budget to check.
    pub scope: BudgetScope,
    /// The kind of the budget.
    pub kind: BudgetKind,
    /// What the request asks for.
    pub requested: u64,
}

impl BudgetCheck {
    /// The budget of an agent.
    pub fn agent(agent: &AgentId, kind: BudgetKind, requested: u64) -> Self {
        BudgetCheck {
            scope: BudgetScope::Agent(agent.clone()),
            kind,
            requested,
        }
    }

    /// The budget of a session.
    pub fn session(session: &SessionId, kind: BudgetKind, requested: u64) -> Self {
        BudgetCheck {
            scope: BudgetScope::Session(session.clone()),
            kind,
            requested,
        }
    }

    /// The budget of the whole Hub.
    pub fn global(kind: BudgetKind, requested: u64) -> Self {
        BudgetCheck {
            scope: BudgetScope::Global,
            kind,
            requested,
        }
    }
}

/// The verdict of a set of budgets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BudgetOutcome {
    /// The verdict of the whole set: `Exceeded` as soon as one budget refuses.
    pub verdict: BudgetVerdict,
    /// The decision of every budget of the set.
    pub decisions: Vec<BudgetDecision>,
}

impl BudgetOutcome {
    /// Whether every budget of the set allows the request.
    pub fn is_allowed(&self) -> bool {
        self.verdict.is_allowed()
    }

    /// The budgets that refused the request.
    pub fn exceeded(&self) -> impl Iterator<Item = &BudgetDecision> {
        self.decisions
            .iter()
            .filter(|decision| !decision.is_allowed())
    }

    /// The failure of the contract layer for an outcome that refuses the request.
    pub fn to_error(&self) -> Option<TaseError> {
        self.exceeded()
            .next()
            .and_then(|decision| decision.to_error())
    }
}

/// The ledger of the budgets of the Hub.
///
/// The ledger holds the limit and the usage of every budget. The limit of a budget that was never set is
/// nothing: a request without a limit is always allowed, and the decision says so with a `None`.
#[derive(Clone, Debug, Default)]
pub struct BudgetLedger {
    limits: BTreeMap<(BudgetScope, BudgetKind), u64>,
    usage: BTreeMap<(BudgetScope, BudgetKind), u64>,
}

impl BudgetLedger {
    /// A ledger without a budget.
    pub fn new() -> Self {
        BudgetLedger::default()
    }

    /// Sets the limit of a budget; a limit of zero is legal and allows nothing.
    pub fn set_limit(&mut self, scope: BudgetScope, kind: BudgetKind, limit: u64) {
        self.limits.insert((scope, kind), limit);
    }

    /// The limit of a budget, or `None` when the budget has no limit.
    pub fn limit(&self, scope: &BudgetScope, kind: BudgetKind) -> Option<u64> {
        self.limits.get(&(scope.clone(), kind)).copied()
    }

    /// What is already used of a budget.
    pub fn used(&self, scope: &BudgetScope, kind: BudgetKind) -> u64 {
        self.usage.get(&(scope.clone(), kind)).copied().unwrap_or(0)
    }

    /// What is left of a budget, or `None` when the budget has no limit.
    pub fn remaining(&self, scope: &BudgetScope, kind: BudgetKind) -> Option<u64> {
        self.limit(scope, kind)
            .map(|limit| limit.saturating_sub(self.used(scope, kind)))
    }

    /// The decision of one budget, without consuming anything.
    pub fn evaluate(&self, check: &BudgetCheck) -> BudgetDecision {
        let limit = self.limit(&check.scope, check.kind);
        let used = self.used(&check.scope, check.kind);
        match limit {
            None => BudgetDecision {
                scope: check.scope.clone(),
                kind: check.kind,
                verdict: BudgetVerdict::Allowed,
                limit: None,
                used,
                requested: check.requested,
                remaining: None,
            },
            Some(limit) => {
                let requested_total = used.saturating_add(check.requested);
                let verdict = if requested_total <= limit {
                    BudgetVerdict::Allowed
                } else {
                    BudgetVerdict::Exceeded
                };
                BudgetDecision {
                    scope: check.scope.clone(),
                    kind: check.kind,
                    verdict,
                    limit: Some(limit),
                    used,
                    requested: check.requested,
                    remaining: Some(limit.saturating_sub(used)),
                }
            }
        }
    }

    /// The verdict of a set of budgets, without consuming anything.
    pub fn evaluate_all(&self, checks: &[BudgetCheck]) -> BudgetOutcome {
        let decisions: Vec<BudgetDecision> = checks
            .iter()
            .map(|check| self.evaluate(check))
            .collect();
        let verdict = if decisions.iter().all(|decision| decision.is_allowed()) {
            BudgetVerdict::Allowed
        } else {
            BudgetVerdict::Exceeded
        };
        BudgetOutcome { verdict, decisions }
    }

    /// Records the usage of a budget and gives the new usage back.
    ///
    /// The usage is always recorded, even when it goes over the limit: the usage is a fact, the budget is a
    /// gate. The gate is [`BudgetLedger::commit`].
    pub fn record(&mut self, scope: &BudgetScope, kind: BudgetKind, amount: u64) -> u64 {
        let usage = self.usage.entry((scope.clone(), kind)).or_insert(0);
        *usage = usage.saturating_add(amount);
        *usage
    }

    /// The gate of the budgets: the usage is recorded only when every budget of the set allows it.
    pub fn commit(&mut self, checks: &[BudgetCheck]) -> BudgetOutcome {
        let outcome = self.evaluate_all(checks);
        if outcome.is_allowed() {
            for check in checks {
                self.record(&check.scope, check.kind, check.requested);
            }
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tase_contract::ErrorCode;

    /// An agent of the tests.
    fn agent(name: &str) -> AgentId {
        AgentId::parse(name).expect("a legal agent")
    }

    /// A session of the tests.
    fn session(name: &str) -> SessionId {
        SessionId::parse(name).expect("a legal session")
    }

    #[test]
    fn the_limit_itself_is_allowed_and_one_token_more_is_not() {
        let mut ledger = BudgetLedger::new();
        let who = agent("agent_01");
        ledger.set_limit(BudgetScope::Agent(who.clone()), BudgetKind::Tokens, 1_000);

        let decision = ledger.evaluate(&BudgetCheck::agent(&who, BudgetKind::Tokens, 1_000));
        assert_eq!(decision.verdict, BudgetVerdict::Allowed);
        assert!(decision.is_allowed());
        assert_eq!(decision.limit, Some(1_000));
        assert_eq!(decision.used, 0);
        assert_eq!(decision.remaining, Some(1_000));

        let over = ledger.evaluate(&BudgetCheck::agent(&who, BudgetKind::Tokens, 1_001));
        assert_eq!(over.verdict, BudgetVerdict::Exceeded);
        assert!(!over.is_allowed());
        let error = over.to_error().expect("a refusal is a failure");
        assert_eq!(error.code, ErrorCode::RateLimited);
    }

    #[test]
    fn a_budget_without_a_limit_allows_everything() {
        let ledger = BudgetLedger::new();
        let who = agent("agent_01");
        let decision = ledger.evaluate(&BudgetCheck::agent(&who, BudgetKind::Tokens, u64::MAX));
        assert!(decision.is_allowed());
        assert_eq!(decision.limit, None);
        assert_eq!(decision.remaining, None);
        assert_eq!(ledger.limit(&BudgetScope::Agent(who), BudgetKind::Tokens), None);
    }

    #[test]
    fn the_budget_of_the_agent_and_the_budget_of_the_session_are_both_checked() {
        let mut ledger = BudgetLedger::new();
        let who = agent("agent_01");
        let talk = session("sess_01");
        ledger.set_limit(BudgetScope::Agent(who.clone()), BudgetKind::Tokens, 1_000);
        ledger.set_limit(BudgetScope::Session(talk.clone()), BudgetKind::Tokens, 500);

        let checks = [
            BudgetCheck::agent(&who, BudgetKind::Tokens, 600),
            BudgetCheck::session(&talk, BudgetKind::Tokens, 600),
        ];
        let outcome = ledger.evaluate_all(&checks);
        assert_eq!(outcome.verdict, BudgetVerdict::Exceeded);
        assert!(!outcome.is_allowed());
        assert_eq!(outcome.exceeded().count(), 1);
        let refused = outcome.exceeded().next().expect("the session refuses");
        assert_eq!(refused.scope, BudgetScope::Session(talk));
        assert_eq!(refused.used, 0);
        assert_eq!(refused.requested, 600);

        // The agent alone would be inside its budget.
        let alone = ledger.evaluate(&BudgetCheck::agent(&who, BudgetKind::Tokens, 600));
        assert!(alone.is_allowed());
    }

    #[test]
    fn the_usage_is_recorded_only_when_the_budget_allows_it() {
        let mut ledger = BudgetLedger::new();
        let who = agent("agent_01");
        ledger.set_limit(BudgetScope::Agent(who.clone()), BudgetKind::Tokens, 100);

        let outcome = ledger.commit(&[BudgetCheck::agent(&who, BudgetKind::Tokens, 60)]);
        assert!(outcome.is_allowed());
        assert_eq!(ledger.used(&BudgetScope::Agent(who.clone()), BudgetKind::Tokens), 60);
        assert_eq!(
            ledger.remaining(&BudgetScope::Agent(who.clone()), BudgetKind::Tokens),
            Some(40)
        );

        // The second request does not fit any more: it is refused and nothing is recorded.
        let outcome = ledger.commit(&[BudgetCheck::agent(&who, BudgetKind::Tokens, 60)]);
        assert!(!outcome.is_allowed());
        assert_eq!(
            ledger.used(&BudgetScope::Agent(who.clone()), BudgetKind::Tokens),
            60,
            "a refused request is not counted"
        );

        // A request that fits is counted.
        let outcome = ledger.commit(&[BudgetCheck::agent(&who, BudgetKind::Tokens, 40)]);
        assert!(outcome.is_allowed());
        assert_eq!(ledger.used(&BudgetScope::Agent(who.clone()), BudgetKind::Tokens), 100);
    }

    #[test]
    fn the_budget_of_the_cost_is_counted_apart_from_the_budget_of_the_tokens() {
        let mut ledger = BudgetLedger::new();
        let who = agent("agent_01");
        ledger.set_limit(BudgetScope::Agent(who.clone()), BudgetKind::Cost, 500);
        ledger.set_limit(BudgetScope::Agent(who.clone()), BudgetKind::Tokens, 10);

        let outcome = ledger.commit(&[
            BudgetCheck::agent(&who, BudgetKind::Cost, 400),
            BudgetCheck::agent(&who, BudgetKind::Tokens, 4),
        ]);
        assert!(outcome.is_allowed());
        assert_eq!(ledger.used(&BudgetScope::Agent(who.clone()), BudgetKind::Cost), 400);
        assert_eq!(ledger.used(&BudgetScope::Agent(who.clone()), BudgetKind::Tokens), 4);

        // The cost is over, the tokens are not: the whole set is refused.
        let outcome = ledger.evaluate_all(&[
            BudgetCheck::agent(&who, BudgetKind::Cost, 200),
            BudgetCheck::agent(&who, BudgetKind::Tokens, 1),
        ]);
        assert!(!outcome.is_allowed());
        assert_eq!(outcome.exceeded().count(), 1);
        assert_eq!(outcome.exceeded().next().unwrap().kind, BudgetKind::Cost);
    }

    #[test]
    fn a_budget_of_zero_allows_nothing() {
        let mut ledger = BudgetLedger::new();
        let who = agent("agent_01");
        ledger.set_limit(BudgetScope::Agent(who.clone()), BudgetKind::Tokens, 0);
        assert!(ledger
            .evaluate(&BudgetCheck::agent(&who, BudgetKind::Tokens, 0))
            .is_allowed());
        assert!(!ledger
            .evaluate(&BudgetCheck::agent(&who, BudgetKind::Tokens, 1))
            .is_allowed());
    }

    #[test]
    fn the_usage_is_always_recorded_when_it_is_a_fact() {
        let mut ledger = BudgetLedger::new();
        let who = agent("agent_01");
        ledger.set_limit(BudgetScope::Agent(who.clone()), BudgetKind::Tokens, 10);
        assert_eq!(ledger.record(&BudgetScope::Agent(who.clone()), BudgetKind::Tokens, 25), 25);
        assert_eq!(
            ledger.remaining(&BudgetScope::Agent(who.clone()), BudgetKind::Tokens),
            Some(0),
            "the remaining never goes below zero"
        );
    }

    #[test]
    fn the_global_budget_covers_the_hub() {
        let mut ledger = BudgetLedger::new();
        ledger.set_limit(BudgetScope::Global, BudgetKind::Cost, 1_000);
        let outcome = ledger.commit(&[BudgetCheck::global(BudgetKind::Cost, 1_000)]);
        assert!(outcome.is_allowed());
        assert_eq!(ledger.used(&BudgetScope::Global, BudgetKind::Cost), 1_000);
        assert!(!ledger
            .evaluate_all(&[BudgetCheck::global(BudgetKind::Cost, 1)])
            .is_allowed());
    }

    #[test]
    fn an_allowed_outcome_has_no_failure() {
        let ledger = BudgetLedger::new();
        let outcome = ledger.evaluate_all(&[BudgetCheck::global(BudgetKind::Tokens, 1)]);
        assert!(outcome.to_error().is_none());
        assert!(BudgetVerdict::Allowed.is_allowed());
        assert!(!BudgetVerdict::Exceeded.is_allowed());
    }
}
