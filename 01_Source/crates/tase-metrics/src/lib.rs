//! TASE metrics: the metering of the tokens, the cost of a call, the structured assertions of a suite (ACI) and the
//! events of the telemetry (detailed design §9 and §10).
//!
//! The crate measures and it counts. It never goes on the network, it never reads the clock (every time comes from the
//! caller) and it never writes a file by itself.
//!
//! # The two rules of the design book that this crate keeps
//!
//! * **what is not known is `null`, never `0`** (detailed design §2.6 and §13): a token that the supplier did not
//!   report, a cost whose price is unknown and an elapsed time that was not measured are all `null`. A `0` would say
//!   that the call was free, and it was not: it says that nobody told.
//! * **the reasoning tokens are not counted twice** (detailed design §9.2): a supplier that reports the reasoning
//!   apart may already count it in the output, so the total never adds the reasoning to the output and the billable
//!   tokens never hold the reasoning.
//!
//! # What the crate holds
//!
//! * [`meter`] — the usage of the model calls of a job and the budget of the tokens;
//! * [`pricing`] — the table of the prices of the providers and the cost of a call;
//! * [`aci`] — the summary of a suite of assertions and the JSON of the acceptance file;
//! * [`telemetry`] — the stream of the events of a job.
//!
//! The crate has no third-party dependency: the JSON comes from `pime-core` and the states of a check from
//! `tase-contract`.

#![forbid(unsafe_code)]

pub mod aci;
pub mod meter;
pub mod pricing;
pub mod telemetry;

pub use aci::{AciReport, AciSummary, Assertion, summarise, to_json};
pub use meter::{BudgetVerdict, Meter, TokenBudget, Usage};
pub use pricing::{PRICE_EPSILON, PriceEntry, PriceTable, prices_are_close};
pub use telemetry::{Telemetry, TelemetryEvent};

/// The version of the metrics.
pub const METRICS_VERSION: &str = "1.1.0";
