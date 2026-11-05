//! The events of the telemetry (detailed design §9.2 and §2).
//!
//! The telemetry is the stream of what happened in a job: a plan was created, a preview was produced, an apply was
//! asked for, a job finished, a cost was recorded. The stream is kept in the memory of the caller and is written by
//! the caller; this crate never sends it anywhere.
//!
//! # The two rules of this module
//!
//! * **it never goes on the network and never reads the clock**: every time comes from the caller, so a replay gives
//!   the same stream;
//! * **a cost that is not known is `null`, never `0`** (detailed design §13: "成本供应商不返回 → null + 来源说明").

use pime_core::json::{Json, Limits, parse};
use tase_contract::error::TaseError;

/// One event of the telemetry.
#[derive(Clone, Debug, PartialEq)]
pub enum TelemetryEvent {
    /// A plan was created.
    PlanCreated {
        /// The id of the plan.
        plan_id: String,
        /// How many operations the plan holds.
        operations: usize,
    },
    /// A preview was produced.
    PreviewProduced {
        /// The id of the preview.
        preview_id: String,
        /// How many changes the preview holds.
        changes: usize,
    },
    /// An apply was asked for.
    ApplyRequested {
        /// The id of the job.
        job_id: String,
        /// Whether the request was a repetition of an idempotency key.
        idempotent: bool,
    },
    /// A job finished.
    JobFinished {
        /// The id of the job.
        job_id: String,
        /// The state the job finished in (`COMMITTED`, `FAILED`, ...).
        state: String,
        /// How long the job took, when the caller measured it.
        elapsed_ms: Option<u64>,
    },
    /// A cost was recorded.
    CostRecorded {
        /// The name of the provider.
        provider: String,
        /// The name of the model.
        model: String,
        /// The cost in dollars; `None` when the price of the model is not known.
        cost: Option<f64>,
    },
}

impl TelemetryEvent {
    /// The name of the event on the wire.
    pub fn name(&self) -> &'static str {
        match self {
            TelemetryEvent::PlanCreated { .. } => "plan_created",
            TelemetryEvent::PreviewProduced { .. } => "preview_produced",
            TelemetryEvent::ApplyRequested { .. } => "apply_requested",
            TelemetryEvent::JobFinished { .. } => "job_finished",
            TelemetryEvent::CostRecorded { .. } => "cost_recorded",
        }
    }

    /// The event as the JSON that is written.
    pub fn to_json_value(&self) -> Json {
        let mut pairs: Vec<(&str, Json)> = vec![("event", Json::str(self.name()))];
        match self {
            TelemetryEvent::PlanCreated {
                plan_id,
                operations,
            } => {
                pairs.push(("plan_id", Json::str(plan_id)));
                pairs.push(("operations", Json::int(*operations as i64)));
            }
            TelemetryEvent::PreviewProduced {
                preview_id,
                changes,
            } => {
                pairs.push(("preview_id", Json::str(preview_id)));
                pairs.push(("changes", Json::int(*changes as i64)));
            }
            TelemetryEvent::ApplyRequested { job_id, idempotent } => {
                pairs.push(("job_id", Json::str(job_id)));
                pairs.push(("idempotent", Json::Bool(*idempotent)));
            }
            TelemetryEvent::JobFinished {
                job_id,
                state,
                elapsed_ms,
            } => {
                pairs.push(("job_id", Json::str(job_id)));
                pairs.push(("state", Json::str(state)));
                pairs.push((
                    "elapsed_ms",
                    match elapsed_ms {
                        Some(value) => Json::int(*value as i64),
                        None => Json::Null,
                    },
                ));
            }
            TelemetryEvent::CostRecorded {
                provider,
                model,
                cost,
            } => {
                pairs.push(("provider", Json::str(provider)));
                pairs.push(("model", Json::str(model)));
                pairs.push((
                    "cost",
                    match cost {
                        Some(value) => Json::Num(*value),
                        None => Json::Null,
                    },
                ));
            }
        }
        Json::obj(pairs)
    }

    /// The event of a JSON value.
    pub fn from_json(value: &Json) -> Result<Self, TaseError> {
        let name = req_str(value, "event")?;
        match name.as_str() {
            "plan_created" => Ok(TelemetryEvent::PlanCreated {
                plan_id: req_str(value, "plan_id")?,
                operations: req_usize(value, "operations")?,
            }),
            "preview_produced" => Ok(TelemetryEvent::PreviewProduced {
                preview_id: req_str(value, "preview_id")?,
                changes: req_usize(value, "changes")?,
            }),
            "apply_requested" => Ok(TelemetryEvent::ApplyRequested {
                job_id: req_str(value, "job_id")?,
                idempotent: req_bool(value, "idempotent")?,
            }),
            "job_finished" => Ok(TelemetryEvent::JobFinished {
                job_id: req_str(value, "job_id")?,
                state: req_str(value, "state")?,
                elapsed_ms: opt_u64(value, "elapsed_ms")?,
            }),
            "cost_recorded" => Ok(TelemetryEvent::CostRecorded {
                provider: req_str(value, "provider")?,
                model: req_str(value, "model")?,
                cost: opt_f64(value, "cost")?,
            }),
            other => Err(TaseError::invalid_argument(format!(
                "unknown telemetry event {other:?}"
            ))),
        }
    }
}

/// The stream of the events of a job.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Telemetry {
    events: Vec<TelemetryEvent>,
}

impl Telemetry {
    /// An empty stream.
    pub fn new() -> Self {
        Telemetry { events: Vec::new() }
    }

    /// Adds an event to the end of the stream.
    pub fn push(&mut self, event: TelemetryEvent) {
        self.events.push(event);
    }

    /// The events of the stream, in the order they were pushed.
    pub fn events(&self) -> &[TelemetryEvent] {
        &self.events
    }

    /// How many events the stream holds.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether the stream holds no event.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// The stream as the JSON that is written.
    pub fn to_json(&self) -> String {
        Json::obj(vec![(
            "events",
            Json::Arr(
                self.events
                    .iter()
                    .map(TelemetryEvent::to_json_value)
                    .collect(),
            ),
        )])
        .to_json_string()
    }

    /// The stream of a JSON text.
    pub fn from_json(text: &str) -> Result<Self, TaseError> {
        let value = parse(text, Limits::DEFAULT).map_err(|error| {
            TaseError::invalid_argument(format!("the telemetry is not a valid JSON: {error}"))
        })?;
        let items = value
            .get("events")
            .and_then(|events| events.as_array())
            .ok_or_else(|| TaseError::invalid_argument("the field \"events\" is missing"))?;
        let mut events = Vec::with_capacity(items.len());
        for item in items {
            events.push(TelemetryEvent::from_json(item)?);
        }
        Ok(Telemetry { events })
    }
}

/// Reads a string field.
fn req_str(value: &Json, key: &str) -> Result<String, TaseError> {
    value
        .get(key)
        .and_then(|item| item.as_str())
        .map(str::to_string)
        .ok_or_else(|| TaseError::invalid_argument(format!("the field {key:?} must be a string")))
}

/// Reads a boolean field.
fn req_bool(value: &Json, key: &str) -> Result<bool, TaseError> {
    value
        .get(key)
        .and_then(|item| item.as_bool())
        .ok_or_else(|| TaseError::invalid_argument(format!("the field {key:?} must be a boolean")))
}

/// Reads a counter field.
fn req_usize(value: &Json, key: &str) -> Result<usize, TaseError> {
    let number = value
        .get(key)
        .and_then(|item| item.as_i64())
        .ok_or_else(|| {
            TaseError::invalid_argument(format!("the field {key:?} must be a number"))
        })?;
    usize::try_from(number)
        .map_err(|_| TaseError::invalid_argument(format!("the field {key:?} must not be negative")))
}

/// Reads an optional counter field; a missing field and a `null` are both "absent".
fn opt_u64(value: &Json, key: &str) -> Result<Option<u64>, TaseError> {
    match value.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(item) => {
            let number = item.as_i64().ok_or_else(|| {
                TaseError::invalid_argument(format!("the field {key:?} must be a whole number"))
            })?;
            u64::try_from(number).map(Some).map_err(|_| {
                TaseError::invalid_argument(format!("the field {key:?} must not be negative"))
            })
        }
    }
}

/// Reads an optional cost field; a missing field and a `null` are both "absent".
fn opt_f64(value: &Json, key: &str) -> Result<Option<f64>, TaseError> {
    match value.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(item) => item.as_f64().map(Some).ok_or_else(|| {
            TaseError::invalid_argument(format!("the field {key:?} must be a number"))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stream with one event of every kind.
    fn full_stream() -> Telemetry {
        let mut telemetry = Telemetry::new();
        telemetry.push(TelemetryEvent::PlanCreated {
            plan_id: "plan_001".to_string(),
            operations: 3,
        });
        telemetry.push(TelemetryEvent::PreviewProduced {
            preview_id: "prev_001".to_string(),
            changes: 2,
        });
        telemetry.push(TelemetryEvent::ApplyRequested {
            job_id: "job_001".to_string(),
            idempotent: false,
        });
        telemetry.push(TelemetryEvent::JobFinished {
            job_id: "job_001".to_string(),
            state: "COMMITTED".to_string(),
            elapsed_ms: Some(2840),
        });
        telemetry.push(TelemetryEvent::CostRecorded {
            provider: "provider-a".to_string(),
            model: "model-x".to_string(),
            cost: Some(0.25),
        });
        telemetry
    }

    #[test]
    fn the_stream_holds_the_events_it_was_given() {
        let telemetry = full_stream();
        assert_eq!(telemetry.len(), 5);
        assert!(!telemetry.is_empty());
        assert_eq!(telemetry.events()[0].name(), "plan_created");
        assert_eq!(telemetry.events()[4].name(), "cost_recorded");
    }

    #[test]
    fn an_empty_stream_is_empty() {
        let telemetry = Telemetry::new();
        assert!(telemetry.is_empty());
        assert_eq!(telemetry.len(), 0);
        assert_eq!(telemetry.events(), &[]);
    }

    #[test]
    fn the_json_of_the_stream_holds_every_event() {
        let json = full_stream().to_json();
        let value = parse(&json, Limits::DEFAULT).expect("the stream must be valid JSON");
        let events = value
            .get("events")
            .and_then(|e| e.as_array())
            .expect("the events");
        assert_eq!(events.len(), 5);
        assert_eq!(
            events[0].get("event").and_then(|e| e.as_str()),
            Some("plan_created")
        );
        assert_eq!(
            events[0].get("operations").and_then(|o| o.as_i64()),
            Some(3)
        );
    }

    #[test]
    fn the_stream_comes_back_from_its_json() {
        let telemetry = full_stream();
        let json = telemetry.to_json();
        let back = Telemetry::from_json(&json).expect("the stream must come back");
        assert_eq!(back.events(), telemetry.events());
    }

    #[test]
    fn an_event_comes_back_from_its_json() {
        for event in full_stream().events() {
            let value = event.to_json_value();
            let back = TelemetryEvent::from_json(&value).expect("the event must come back");
            assert_eq!(&back, event);
        }
    }

    #[test]
    fn a_cost_that_is_not_known_is_null_and_not_zero() {
        let event = TelemetryEvent::CostRecorded {
            provider: "provider-a".to_string(),
            model: "model-z".to_string(),
            cost: None,
        };
        let value = event.to_json_value();
        assert!(value.get("cost").expect("the cost").is_null());
        let back = TelemetryEvent::from_json(&value).expect("the event must come back");
        assert_eq!(back, event);
    }

    #[test]
    fn an_elapsed_time_that_was_not_measured_is_null() {
        let event = TelemetryEvent::JobFinished {
            job_id: "job_001".to_string(),
            state: "FAILED".to_string(),
            elapsed_ms: None,
        };
        let value = event.to_json_value();
        assert!(value.get("elapsed_ms").expect("the elapsed time").is_null());
        let back = TelemetryEvent::from_json(&value).expect("the event must come back");
        assert_eq!(back, event);
    }

    #[test]
    fn an_unknown_event_is_refused() {
        let value = Json::obj(vec![("event", Json::str("nonsense"))]);
        assert!(TelemetryEvent::from_json(&value).is_err());
    }

    #[test]
    fn a_text_that_is_not_json_is_refused() {
        assert!(Telemetry::from_json("not a json").is_err());
        assert!(Telemetry::from_json("{}").is_err());
    }

    #[test]
    fn the_json_of_the_stream_is_stable() {
        let telemetry = full_stream();
        assert_eq!(telemetry.to_json(), telemetry.to_json());
    }

    #[test]
    fn the_events_keep_the_order_they_were_pushed_in() {
        let mut telemetry = Telemetry::new();
        telemetry.push(TelemetryEvent::ApplyRequested {
            job_id: "job_002".to_string(),
            idempotent: true,
        });
        telemetry.push(TelemetryEvent::ApplyRequested {
            job_id: "job_001".to_string(),
            idempotent: false,
        });
        let ids: Vec<&str> = telemetry
            .events()
            .iter()
            .map(|event| match event {
                TelemetryEvent::ApplyRequested { job_id, .. } => job_id.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(ids, ["job_002", "job_001"]);
    }
}
