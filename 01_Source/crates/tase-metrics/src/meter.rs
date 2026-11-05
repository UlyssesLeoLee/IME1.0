//! The metering of the usage of the model (detailed design §9.2).
//!
//! The design book has two rules that this module never breaks:
//!
//! * **a token that the supplier did not report is `null`, never `0`** (detailed design §2.6: "费用或供应商未报告的
//!   token 记 `null`，不得默认 0"). The difference between "no token was used" and "the supplier did not tell" is the
//!   difference between a free operation and an unknown bill;
//! * **the reasoning tokens are not counted twice** (detailed design §9.2: "总 Token 汇总需分别保留供应商的计费
//!   粒度，防止 reasoning 已包含于 output 时重复计数"). A supplier that reports the reasoning apart may already count
//!   it in the output, so the total never adds the reasoning to the output.
//!
//! The meter is a book of records: every record is a usage and the time the caller gave. It reads no clock and goes
//! nowhere.

use tase_contract::api::Metrics;

/// The usage of one model call.
///
/// The three counters are the counters of the supplier. The reasoning is `None` when the supplier did not report it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Usage {
    /// The tokens that were read.
    pub input_tokens: u64,
    /// The tokens that were written.
    pub output_tokens: u64,
    /// The tokens that were read from the cache of the supplier.
    pub cached_input_tokens: u64,
    /// The tokens of the reasoning, when the supplier reports them apart.
    ///
    /// `None` means "the supplier did not tell"; it never means "there were none".
    pub reasoning_tokens: Option<u64>,
}

impl Usage {
    /// A usage with its counters.
    pub fn new(
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: u64,
        reasoning_tokens: Option<u64>,
    ) -> Self {
        Usage {
            input_tokens,
            output_tokens,
            cached_input_tokens,
            reasoning_tokens,
        }
    }

    /// The tokens of the call, without the reasoning.
    ///
    /// The reasoning is left out on purpose: it may already be inside the output.
    pub fn tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cached_input_tokens)
    }

    /// The usage as the counters of a job (detailed design §2.6).
    pub fn into_metrics(
        self,
        llm_cost_usd: Option<f64>,
        worker_cost_usd: Option<f64>,
        elapsed_ms: Option<u64>,
    ) -> Metrics {
        Metrics {
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cached_input_tokens: self.cached_input_tokens,
            reasoning_tokens: self.reasoning_tokens,
            llm_cost_usd,
            worker_cost_usd,
            elapsed_ms,
        }
    }
}

/// One record of the meter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Record {
    /// When the call was made, in milliseconds; the caller gives it.
    at_ms: u64,
    /// What the call used.
    usage: Usage,
}

/// The book of the usages of the model calls of a job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meter {
    records: Vec<Record>,
    input_includes_cached: bool,
}

impl Default for Meter {
    fn default() -> Self {
        Meter::new()
    }
}

impl Meter {
    /// A meter with the usual convention: the input tokens hold the cached ones.
    ///
    /// The design book says that the convention of the supplier has to be kept as metadata ("`input_tokens` 是否包含
    /// `cached_input_tokens` 按提供者定义留元数据"). The usual convention of the suppliers is that the cached tokens are
    /// a part of the input tokens.
    pub fn new() -> Self {
        Meter {
            records: Vec::new(),
            input_includes_cached: true,
        }
    }

    /// A meter with the convention of a supplier that counts the cached tokens apart from the input ones.
    pub fn with_input_convention(input_includes_cached: bool) -> Self {
        Meter {
            records: Vec::new(),
            input_includes_cached,
        }
    }

    /// Whether the input tokens of this meter hold the cached ones.
    pub fn input_includes_cached(&self) -> bool {
        self.input_includes_cached
    }

    /// Records a usage with the time the caller gives.
    pub fn record(&mut self, usage: Usage, now_ms: u64) {
        self.records.push(Record {
            at_ms: now_ms,
            usage,
        });
    }

    /// How many records the meter holds.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the meter holds no record.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// The usage of every record, in the order of the time.
    ///
    /// The records that carry the same time keep the order they were recorded in.
    pub fn snapshots(&self) -> Vec<Usage> {
        self.timeline()
            .into_iter()
            .map(|(_, usage)| usage)
            .collect()
    }

    /// The records with their time, in the order of the time.
    pub fn timeline(&self) -> Vec<(u64, Usage)> {
        let mut records: Vec<Record> = self.records.clone();
        records.sort_by_key(|record| record.at_ms);
        records
            .into_iter()
            .map(|record| (record.at_ms, record.usage))
            .collect()
    }

    /// The total of the records.
    ///
    /// The reasoning is the sum of the reasoning of the records **only when every record reported it**: a record that
    /// did not report it makes the total unknown, and an unknown total is `None` and never `0`.
    pub fn total(&self) -> Usage {
        let mut input_tokens = 0u64;
        let mut output_tokens = 0u64;
        let mut cached_input_tokens = 0u64;
        let mut reasoning_sum = 0u64;
        let mut reasoning_known = true;

        for record in &self.records {
            input_tokens = input_tokens.saturating_add(record.usage.input_tokens);
            output_tokens = output_tokens.saturating_add(record.usage.output_tokens);
            cached_input_tokens =
                cached_input_tokens.saturating_add(record.usage.cached_input_tokens);
            match record.usage.reasoning_tokens {
                Some(value) => reasoning_sum = reasoning_sum.saturating_add(value),
                None => reasoning_known = false,
            }
        }

        Usage {
            input_tokens,
            output_tokens,
            cached_input_tokens,
            reasoning_tokens: if reasoning_known {
                Some(reasoning_sum)
            } else {
                None
            },
        }
    }

    /// The tokens that are counted once for the budget of the job.
    ///
    /// * the reasoning tokens are **never** added: the supplier may already count them in the output, and the meter
    ///   has no way to know which convention the supplier uses. The risk of counting them twice is worse than the
    ///   risk of counting them once too few;
    /// * the cached input tokens are added only when the input tokens do not hold them, which is the convention the
    ///   meter was built with.
    ///
    /// This number is a volume, not a price: the cost of a call is the sum of the three prices of the price table,
    /// never this number times one price.
    pub fn billable_tokens(&self) -> u64 {
        let total = self.total();
        let cached = if self.input_includes_cached {
            0
        } else {
            total.cached_input_tokens
        };
        total
            .input_tokens
            .saturating_add(total.output_tokens)
            .saturating_add(cached)
    }
}

/// The verdict of a budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BudgetVerdict {
    /// The usage stays inside the budget.
    Allowed,
    /// The usage is over the budget.
    Exceeded,
}

impl BudgetVerdict {
    /// Whether the usage stays inside the budget.
    pub fn is_allowed(self) -> bool {
        matches!(self, BudgetVerdict::Allowed)
    }
}

/// The budget of the tokens of a job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenBudget {
    /// How many tokens the job may use; the limit itself is allowed.
    pub limit: u64,
}

impl TokenBudget {
    /// A budget of a limit.
    pub fn new(limit: u64) -> Self {
        TokenBudget { limit }
    }

    /// What the budget says about a usage.
    ///
    /// A usage that is exactly the limit is allowed; one token more is not.
    pub fn check(&self, used: u64) -> BudgetVerdict {
        if used <= self.limit {
            BudgetVerdict::Allowed
        } else {
            BudgetVerdict::Exceeded
        }
    }

    /// How many tokens are left; zero when the budget is used up.
    pub fn remaining(&self, used: u64) -> u64 {
        self.limit.saturating_sub(used)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A usage of 5000 tokens in, 120 out and 3000 cached.
    fn usage() -> Usage {
        Usage::new(5000, 120, 3000, None)
    }

    #[test]
    fn the_usage_carries_its_counters() {
        let usage = usage();
        assert_eq!(usage.input_tokens, 5000);
        assert_eq!(usage.output_tokens, 120);
        assert_eq!(usage.cached_input_tokens, 3000);
        assert_eq!(usage.reasoning_tokens, None);
    }

    #[test]
    fn a_meter_that_holds_nothing_has_a_total_of_zero() {
        let meter = Meter::new();
        let total = meter.total();
        assert_eq!(total.input_tokens, 0);
        assert_eq!(total.output_tokens, 0);
        assert_eq!(total.cached_input_tokens, 0);
        assert_eq!(total.reasoning_tokens, Some(0));
        assert!(meter.is_empty());
        assert_eq!(meter.len(), 0);
    }

    #[test]
    fn the_total_sums_the_records() {
        let mut meter = Meter::new();
        meter.record(usage(), 10);
        meter.record(Usage::new(100, 20, 0, None), 20);
        let total = meter.total();
        assert_eq!(total.input_tokens, 5100);
        assert_eq!(total.output_tokens, 140);
        assert_eq!(total.cached_input_tokens, 3000);
        // Neither record reported the reasoning: the total is unknown, not zero.
        assert_eq!(total.reasoning_tokens, None);
    }

    #[test]
    fn the_total_sums_the_reasoning_when_every_record_reported_it() {
        let mut meter = Meter::new();
        meter.record(Usage::new(100, 20, 0, Some(500)), 10);
        meter.record(Usage::new(100, 20, 0, Some(300)), 20);
        let total = meter.total();
        assert_eq!(total.reasoning_tokens, Some(800));
    }

    #[test]
    fn the_reasoning_is_not_added_to_the_output() {
        let mut meter = Meter::new();
        meter.record(Usage::new(100, 20, 0, Some(500)), 10);
        let total = meter.total();
        assert_eq!(
            total.output_tokens, 20,
            "the output stays as it was reported"
        );
        assert_eq!(total.reasoning_tokens, Some(500));
        assert_ne!(total.output_tokens, 520);
    }

    #[test]
    fn the_reasoning_of_a_record_that_did_not_report_it_is_unknown_and_not_zero() {
        let mut meter = Meter::new();
        meter.record(Usage::new(100, 20, 0, Some(500)), 10);
        meter.record(Usage::new(100, 20, 0, None), 20);
        let total = meter.total();
        assert_eq!(total.reasoning_tokens, None, "an unknown total is never 0");
        assert_ne!(total.reasoning_tokens, Some(0));
    }

    #[test]
    fn the_billable_tokens_never_hold_the_reasoning() {
        let mut meter = Meter::new();
        meter.record(Usage::new(100, 20, 0, Some(500)), 10);
        assert_eq!(meter.billable_tokens(), 120);
    }

    #[test]
    fn the_billable_tokens_hold_the_cached_tokens_only_when_the_input_does_not() {
        let mut inside = Meter::new();
        inside.record(usage(), 10);
        assert_eq!(inside.billable_tokens(), 5120);

        let mut apart = Meter::with_input_convention(false);
        apart.record(usage(), 10);
        assert_eq!(apart.billable_tokens(), 8120);
        assert!(!apart.input_includes_cached());
    }

    #[test]
    fn the_snapshots_are_in_the_order_of_the_time() {
        let mut meter = Meter::new();
        meter.record(Usage::new(1, 0, 0, None), 300);
        meter.record(Usage::new(2, 0, 0, None), 100);
        meter.record(Usage::new(3, 0, 0, None), 200);
        let inputs: Vec<u64> = meter
            .snapshots()
            .into_iter()
            .map(|usage| usage.input_tokens)
            .collect();
        assert_eq!(inputs, [2, 3, 1]);
    }

    #[test]
    fn the_records_of_the_same_time_keep_the_order_of_the_recording() {
        let mut meter = Meter::new();
        meter.record(Usage::new(1, 0, 0, None), 100);
        meter.record(Usage::new(2, 0, 0, None), 100);
        let inputs: Vec<u64> = meter
            .snapshots()
            .into_iter()
            .map(|usage| usage.input_tokens)
            .collect();
        assert_eq!(inputs, [1, 2]);
    }

    #[test]
    fn the_timeline_holds_the_time_of_every_record() {
        let mut meter = Meter::new();
        meter.record(usage(), 42);
        let timeline = meter.timeline();
        assert_eq!(timeline.len(), 1);
        assert_eq!(timeline[0].0, 42);
        assert_eq!(timeline[0].1, usage());
    }

    #[test]
    fn the_meter_holds_the_records_it_was_given() {
        let mut meter = Meter::new();
        meter.record(usage(), 10);
        meter.record(usage(), 20);
        assert_eq!(meter.len(), 2);
        assert!(!meter.is_empty());
        assert_eq!(meter.snapshots().len(), 2);
    }

    #[test]
    fn the_budget_allows_the_limit_itself_and_refuses_one_token_more() {
        let budget = TokenBudget::new(1000);
        assert_eq!(budget.check(0), BudgetVerdict::Allowed);
        assert_eq!(budget.check(999), BudgetVerdict::Allowed);
        assert_eq!(budget.check(1000), BudgetVerdict::Allowed);
        assert_eq!(budget.check(1001), BudgetVerdict::Exceeded);
        assert!(budget.check(1000).is_allowed());
        assert!(!budget.check(1001).is_allowed());
    }

    #[test]
    fn the_budget_of_zero_allows_nothing_but_zero() {
        let budget = TokenBudget::new(0);
        assert_eq!(budget.check(0), BudgetVerdict::Allowed);
        assert_eq!(budget.check(1), BudgetVerdict::Exceeded);
    }

    #[test]
    fn the_remaining_of_a_budget_never_goes_below_zero() {
        let budget = TokenBudget::new(100);
        assert_eq!(budget.remaining(40), 60);
        assert_eq!(budget.remaining(100), 0);
        assert_eq!(budget.remaining(140), 0);
    }

    #[test]
    fn the_usage_becomes_the_counters_of_a_job() {
        let metrics = usage().into_metrics(Some(0.5), None, Some(1200));
        assert_eq!(metrics.input_tokens, 5000);
        assert_eq!(metrics.output_tokens, 120);
        assert_eq!(metrics.cached_input_tokens, 3000);
        assert_eq!(metrics.reasoning_tokens, None);
        assert_eq!(metrics.llm_cost_usd, Some(0.5));
        assert_eq!(metrics.worker_cost_usd, None);
        assert_eq!(metrics.elapsed_ms, Some(1200));
    }

    #[test]
    fn the_tokens_of_a_usage_hold_the_three_counters() {
        assert_eq!(usage().tokens(), 8120);
    }
}
