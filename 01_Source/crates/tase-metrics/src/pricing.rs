//! The cost of a model call (detailed design §9.2).
//!
//! The design book is careful about the cost: "价格需按调用时实际版本、plan/区域/优惠归因，不是简单按总 Token × 单价"
//! — the price is the one of the version that was called, and the cost is not "the total of the tokens times one
//! price". The cost is therefore the sum of three lines: the input, the output and the cached input, each with its
//! own price.
//!
//! Two rules are kept here:
//!
//! * **a price that is not known is `None`, never `0`** — a call whose price is unknown has an unknown cost, and an
//!   unknown cost of `0` would say that the call was free;
//! * the money is a `f64`, but two costs are never compared without a tolerance: [`PRICE_EPSILON`].

use crate::meter::Usage;

/// The tolerance of a comparison of two prices.
pub const PRICE_EPSILON: f64 = 1e-9;

/// The prices of one model of one provider.
///
/// The prices are in dollars for a million tokens.
#[derive(Clone, Debug, PartialEq)]
pub struct PriceEntry {
    /// The name of the provider.
    pub provider: String,
    /// The name of the model.
    pub model: String,
    /// The price of a million input tokens.
    pub input_per_million: f64,
    /// The price of a million output tokens.
    pub output_per_million: f64,
    /// The price of a million cached input tokens.
    pub cached_input_per_million: f64,
}

impl PriceEntry {
    /// The prices of one model of one provider.
    pub fn new(
        provider: &str,
        model: &str,
        input_per_million: f64,
        output_per_million: f64,
        cached_input_per_million: f64,
    ) -> Self {
        PriceEntry {
            provider: provider.to_string(),
            model: model.to_string(),
            input_per_million,
            output_per_million,
            cached_input_per_million,
        }
    }
}

/// The table of the prices of the providers.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PriceTable {
    /// The prices of the table.
    pub entries: Vec<PriceEntry>,
}

impl PriceTable {
    /// A table of prices.
    pub fn new(entries: Vec<PriceEntry>) -> Self {
        PriceTable { entries }
    }

    /// The prices of a model of a provider; `None` when the table does not know them.
    pub fn entry(&self, provider: &str, model: &str) -> Option<&PriceEntry> {
        self.entries
            .iter()
            .find(|entry| entry.provider == provider && entry.model == model)
    }

    /// The cost of a usage, in dollars.
    ///
    /// The cost is `None` when the table does not know the price of the model: **an unknown price is never a cost of
    /// zero**.
    ///
    /// The convention of this function is that the input tokens do **not** hold the cached ones, so the three prices
    /// apply to the three counters. A supplier that counts the cached tokens inside the input ones has to be told, with
    /// [`PriceTable::cost_of_with_convention`], so that the cached tokens are not charged twice.
    pub fn cost_of(&self, usage: &Usage, provider: &str, model: &str) -> Option<f64> {
        self.cost_of_with_convention(usage, provider, model, false)
    }

    /// The cost of a usage, in dollars, with the convention of the supplier.
    ///
    /// `input_includes_cached` says whether the input tokens of the usage hold the cached ones. When they do, the
    /// cached tokens are already priced as input tokens and are not charged a second time.
    pub fn cost_of_with_convention(
        &self,
        usage: &Usage,
        provider: &str,
        model: &str,
        input_includes_cached: bool,
    ) -> Option<f64> {
        let entry = self.entry(provider, model)?;
        let mut cost = 0.0;
        cost += tokens_in_millions(usage.input_tokens) * entry.input_per_million;
        cost += tokens_in_millions(usage.output_tokens) * entry.output_per_million;
        if !input_includes_cached {
            cost += tokens_in_millions(usage.cached_input_tokens) * entry.cached_input_per_million;
        }
        Some(cost)
    }
}

/// How many millions of tokens a counter holds.
fn tokens_in_millions(tokens: u64) -> f64 {
    tokens as f64 / 1_000_000.0
}

/// Whether two prices are the same, with the tolerance of the module.
pub fn prices_are_close(left: f64, right: f64) -> bool {
    (left - right).abs() <= PRICE_EPSILON
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table of the tests: one provider, two models.
    fn table() -> PriceTable {
        PriceTable::new(vec![
            PriceEntry::new("provider-a", "model-x", 3.0, 15.0, 0.3),
            PriceEntry::new("provider-a", "model-y", 1.0, 2.0, 0.1),
        ])
    }

    #[test]
    fn the_cost_of_a_usage_is_the_sum_of_the_three_lines() {
        let usage = Usage::new(1_000_000, 1_000_000, 1_000_000, None);
        let cost = table()
            .cost_of(&usage, "provider-a", "model-x")
            .expect("the price is known");
        assert!(prices_are_close(cost, 3.0 + 15.0 + 0.3), "{cost}");
    }

    #[test]
    fn the_cost_of_a_small_usage_is_small() {
        let usage = Usage::new(5000, 120, 3000, None);
        let cost = table()
            .cost_of(&usage, "provider-a", "model-x")
            .expect("the price is known");
        let expected = 5000.0 / 1e6 * 3.0 + 120.0 / 1e6 * 15.0 + 3000.0 / 1e6 * 0.3;
        assert!(prices_are_close(cost, expected), "{cost}");
    }

    #[test]
    fn an_unknown_price_is_none_and_not_zero() {
        let usage = Usage::new(1000, 1000, 0, None);
        let cost = table().cost_of(&usage, "provider-b", "model-x");
        assert_eq!(cost, None, "an unknown price is never a cost of zero");
        let cost = table().cost_of(&usage, "provider-a", "model-z");
        assert_eq!(cost, None, "an unknown model is never a cost of zero");
    }

    #[test]
    fn an_empty_table_gives_no_cost_at_all() {
        let usage = Usage::new(1000, 1000, 0, None);
        assert_eq!(
            PriceTable::default().cost_of(&usage, "provider-a", "model-x"),
            None
        );
    }

    #[test]
    fn the_reasoning_is_not_priced_a_second_time() {
        let with_reasoning = Usage::new(1000, 1000, 0, Some(500));
        let without_reasoning = Usage::new(1000, 1000, 0, None);
        let a = table()
            .cost_of(&with_reasoning, "provider-a", "model-x")
            .expect("the price is known");
        let b = table()
            .cost_of(&without_reasoning, "provider-a", "model-x")
            .expect("the price is known");
        assert!(prices_are_close(a, b), "{a} and {b}");
    }

    #[test]
    fn the_convention_of_the_supplier_decides_whether_the_cached_tokens_are_charged_twice() {
        let usage = Usage::new(1_000_000, 0, 1_000_000, None);
        let apart = table()
            .cost_of_with_convention(&usage, "provider-a", "model-x", false)
            .expect("the price is known");
        assert!(prices_are_close(apart, 3.0 + 0.3), "{apart}");
        let inside = table()
            .cost_of_with_convention(&usage, "provider-a", "model-x", true)
            .expect("the price is known");
        assert!(prices_are_close(inside, 3.0), "{inside}");
    }

    #[test]
    fn two_models_of_a_provider_have_two_prices() {
        let usage = Usage::new(1_000_000, 1_000_000, 0, None);
        let first = table()
            .cost_of(&usage, "provider-a", "model-x")
            .expect("the price is known");
        let second = table()
            .cost_of(&usage, "provider-a", "model-y")
            .expect("the price is known");
        assert!(prices_are_close(first, 18.0), "{first}");
        assert!(prices_are_close(second, 3.0), "{second}");
    }

    #[test]
    fn the_entry_of_a_model_is_found() {
        let table = table();
        let entry = table.entry("provider-a", "model-y").expect("the entry");
        assert_eq!(entry.input_per_million, 1.0);
        assert!(table.entry("provider-a", "model-z").is_none());
    }

    #[test]
    fn a_usage_without_a_token_costs_nothing_but_is_still_a_cost() {
        let usage = Usage::new(0, 0, 0, None);
        let cost = table()
            .cost_of(&usage, "provider-a", "model-x")
            .expect("the price is known");
        assert!(prices_are_close(cost, 0.0), "{cost}");
    }

    #[test]
    fn two_prices_are_compared_with_a_tolerance() {
        assert!(prices_are_close(1.0, 1.0 + PRICE_EPSILON / 2.0));
        assert!(!prices_are_close(1.0, 1.0 + 0.001));
    }
}
