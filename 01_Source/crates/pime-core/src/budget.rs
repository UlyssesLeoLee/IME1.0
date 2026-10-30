//! AI budget accounting (FR-023), port of the `Budget` class of `core/ai.js`: reserve before sending, settle with the
//! real usage afterwards, refuse when the budget is used up. This is only the bookkeeping; the HTTP client is not
//! part of the core.

use crate::error::{Result, fail};

/// A reservation; settle or release it exactly once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ticket {
    pub estimate: i64,
    pub settled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Usage {
    pub input: i64,
    pub output: i64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snapshot {
    pub requests: i64,
    pub tokens: i64,
    /// At least one response had no usage figures and was settled with its estimate.
    pub estimated: bool,
    /// `None` when no price is configured.
    pub cost: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct Budget {
    max_requests: Option<i64>,
    max_tokens: Option<i64>,
    price: f64,
    requests: i64,
    tokens: i64,
    any_estimated: bool,
}

impl Default for Budget {
    fn default() -> Self {
        Budget::new(None, None, 0.0)
    }
}

impl Budget {
    /// `None` = unlimited (the bridge's setting `0` means unlimited).
    pub fn new(
        max_requests: Option<i64>,
        max_tokens: Option<i64>,
        price_per_1k_tokens: f64,
    ) -> Budget {
        Budget {
            max_requests,
            max_tokens,
            price: price_per_1k_tokens,
            requests: 0,
            tokens: 0,
            any_estimated: false,
        }
    }

    /// Changes the limits and the price; the counters keep running.
    pub fn configure(
        &mut self,
        max_requests: Option<i64>,
        max_tokens: Option<i64>,
        price_per_1k_tokens: f64,
    ) {
        self.max_requests = max_requests;
        self.max_tokens = max_tokens;
        self.price = price_per_1k_tokens;
    }

    pub fn reserve(&mut self, estimate: i64) -> Result<Ticket> {
        if self.max_requests.is_some_and(|m| self.requests + 1 > m)
            || self.max_tokens.is_some_and(|m| self.tokens + estimate > m)
        {
            return fail(
                "AI_BUDGET_EXCEEDED",
                "AI budget reached; AI is paused, local input is unaffected",
            );
        }
        self.requests += 1;
        self.tokens += estimate;
        Ok(Ticket {
            estimate,
            settled: false,
        })
    }

    pub fn settle(&mut self, ticket: &mut Ticket, usage: Option<Usage>) {
        if ticket.settled {
            return;
        }
        ticket.settled = true;
        self.tokens -= ticket.estimate;
        match usage {
            Some(u) => self.tokens += u.input + u.output,
            None => {
                self.tokens += ticket.estimate;
                self.any_estimated = true;
            }
        }
    }

    pub fn release(&mut self, ticket: &mut Ticket) {
        if !ticket.settled {
            ticket.settled = true;
            self.requests -= 1;
            self.tokens -= ticket.estimate;
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            requests: self.requests,
            tokens: self.tokens,
            estimated: self.any_estimated,
            cost: (self.price != 0.0).then(|| {
                // `+(tokens / 1000 * price).toFixed(6)`
                format!("{:.6}", self.tokens as f64 / 1000.0 * self.price)
                    .parse()
                    .unwrap_or(0.0)
            }),
        }
    }

    pub fn reset(&mut self) {
        self.requests = 0;
        self.tokens = 0;
        self.any_estimated = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code<T>(r: Result<T>) -> &'static str {
        match r {
            Ok(_) => "NO_ERROR",
            Err(e) => e.code,
        }
    }

    #[test]
    fn ut_035_budget_reserve_before_sending_settle_with_real_usage_refuse_when_exceeded() {
        let mut b = Budget::new(None, Some(100), 2.0);
        let mut t1 = b.reserve(80).unwrap();
        assert_eq!(code(b.reserve(80)), "AI_BUDGET_EXCEEDED");
        b.settle(
            &mut t1,
            Some(Usage {
                input: 20,
                output: 10,
            }),
        );
        assert_eq!(
            b.snapshot(),
            Snapshot {
                requests: 1,
                tokens: 30,
                estimated: false,
                cost: Some(0.06)
            }
        );
        let mut t2 = b.reserve(60).unwrap();
        b.settle(&mut t2, None);
        assert!(
            b.snapshot().estimated,
            "a response without usage is marked estimated"
        );
        let mut t3 = b.reserve(1).unwrap();
        b.release(&mut t3);
        assert_eq!(b.snapshot().requests, 2);
        let mut r = Budget::new(Some(2), None, 0.0);
        r.reserve(1).unwrap();
        r.reserve(1).unwrap();
        assert_eq!(code(r.reserve(1)), "AI_BUDGET_EXCEEDED");
        r.reset();
        assert!(r.reserve(1).is_ok());
    }

    #[test]
    fn a_ticket_settles_or_releases_only_once() {
        let mut b = Budget::new(Some(3), Some(1000), 0.0);
        let mut t = b.reserve(100).unwrap();
        b.settle(
            &mut t,
            Some(Usage {
                input: 5,
                output: 5,
            }),
        );
        b.settle(
            &mut t,
            Some(Usage {
                input: 500,
                output: 500,
            }),
        ); // ignored
        b.release(&mut t); // ignored: already settled
        assert_eq!((b.snapshot().requests, b.snapshot().tokens), (1, 10));
        let mut u = b.reserve(50).unwrap();
        b.release(&mut u);
        b.release(&mut u);
        b.settle(&mut u, None); // ignored: already released
        assert_eq!((b.snapshot().requests, b.snapshot().tokens), (1, 10));
        assert!(!b.snapshot().estimated);
        assert_eq!(b.snapshot().cost, None, "no price, no cost");
    }

    #[test]
    fn the_token_limit_counts_the_reservation_and_zero_limits_are_not_special_here() {
        let mut b = Budget::new(None, Some(100), 0.0);
        assert!(b.reserve(100).is_ok(), "exactly the limit is allowed");
        assert_eq!(code(b.reserve(1)), "AI_BUDGET_EXCEEDED");
        let mut unlimited = Budget::default();
        for _ in 0..1000 {
            unlimited.reserve(1_000_000).unwrap();
        }
        assert_eq!(unlimited.snapshot().requests, 1000);
    }

    #[test]
    fn configure_changes_limits_but_keeps_the_counters() {
        let mut b = Budget::new(Some(1), None, 0.0);
        b.reserve(10).unwrap();
        assert_eq!(code(b.reserve(10)), "AI_BUDGET_EXCEEDED");
        b.configure(Some(5), None, 1.0);
        assert!(b.reserve(10).is_ok());
        let s = b.snapshot();
        assert_eq!((s.requests, s.tokens), (2, 20));
        assert_eq!(s.cost, Some(0.02));
        b.configure(None, None, 0.0);
        assert_eq!(b.snapshot().cost, None);
    }
}
