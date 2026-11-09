//! The events of the Hub: what it saw, in the order it saw it.
//!
//! The Hub is a resident process: nobody watches its screen. What it saw is therefore written down, and
//! the events are the only trace of a session that ended badly (detailed design §17.6 asks for a trail
//! with the events `RECOVERED` and `ORPHANED`).
//!
//! Three rules the module keeps:
//!
//! * **an event never holds a secret.** The free texts of the events (the name of the peer, the reason
//!   of a close, the detail of a degradation) go through [`redact`]: a text that names a token, a
//!   secret or a key is replaced by [`REDACTED`], and a text that is too long is cut.
//! * **the order is the order.** The events are kept in a vector and are numbered from 1; a client asks
//!   for the events after a number and gets them in the order they happened.
//! * **the shape is the same everywhere.** Every event is written as an object with a `kind` and the
//!   fields of that kind, so that a reader of the log does not have to know the kinds beforehand.

use pime_core::json::{Json, Limits, parse};
use tase_contract::{ErrorCode, TaseError};

/// The text that stands for a secret that was taken out of an event.
pub const REDACTED: &str = "<redacted>";

/// The most characters a free text of an event may hold.
pub const MAX_DETAIL_CHARS: usize = 120;

/// The words that say that a text may hold a secret.
const SENSITIVE_MARKERS: [&str; 10] = [
    "token",
    "secret",
    "password",
    "passwd",
    "api_key",
    "apikey",
    "authorization",
    "bearer",
    "private_key",
    "credential",
];

/// Takes the secret out of a text that goes into an event.
///
/// The rule is blunt on purpose: a text that names a token, a secret, a password or a key is replaced
/// as a whole, because a half-redacted secret is a leaked secret. A text that is not suspicious is cut
/// at [`MAX_DETAIL_CHARS`], because an event is a sign, not a log file.
pub fn redact(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    if SENSITIVE_MARKERS.iter().any(|marker| lower.contains(marker)) {
        return REDACTED.to_string();
    }
    let mut out = String::new();
    for character in text.chars().take(MAX_DETAIL_CHARS) {
        if character.is_control() {
            out.push('?');
        } else {
            out.push(character);
        }
    }
    if text.chars().count() > MAX_DETAIL_CHARS {
        out.push_str("...");
    }
    out
}

/// The kind of an event of the Hub.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HubEventKind {
    /// A connection was opened.
    ConnectionOpened,
    /// A connection was closed.
    ConnectionClosed,
    /// An agent registered itself.
    AgentRegistered,
    /// A job was put in the queue.
    JobEnqueued,
    /// A job was handed to the scheduler.
    JobDispatched,
    /// A job is done.
    JobFinished,
    /// A write lock was refused.
    LockDenied,
    /// The Hub runs on less than it should.
    Degraded,
}

impl HubEventKind {
    /// Every kind of an event.
    pub const ALL: [HubEventKind; 8] = [
        HubEventKind::ConnectionOpened,
        HubEventKind::ConnectionClosed,
        HubEventKind::AgentRegistered,
        HubEventKind::JobEnqueued,
        HubEventKind::JobDispatched,
        HubEventKind::JobFinished,
        HubEventKind::LockDenied,
        HubEventKind::Degraded,
    ];

    /// The name of the kind; the same in the log and on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            HubEventKind::ConnectionOpened => "connection_opened",
            HubEventKind::ConnectionClosed => "connection_closed",
            HubEventKind::AgentRegistered => "agent_registered",
            HubEventKind::JobEnqueued => "job_enqueued",
            HubEventKind::JobDispatched => "job_dispatched",
            HubEventKind::JobFinished => "job_finished",
            HubEventKind::LockDenied => "lock_denied",
            HubEventKind::Degraded => "degraded",
        }
    }

    /// The kind of a name; `None` when the name is not one of ours.
    #[must_use]
    pub fn from_name(name: &str) -> Option<HubEventKind> {
        HubEventKind::ALL
            .into_iter()
            .find(|kind| kind.as_str() == name)
    }
}

/// An event of the Hub.
///
/// The eight kinds are the eight things the Hub has to say about a session: a connection came, a
/// connection went, an agent registered, a job was queued, a job was dispatched, a job is done, a write
/// was refused, and the Hub runs on less than it should.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HubEvent {
    /// A connection was opened.
    ConnectionOpened {
        /// The number of the connection.
        connection_id: u64,
        /// The name of the peer, redacted.
        peer: String,
        /// The time of the call.
        at_ms: u64,
    },
    /// A connection was closed.
    ConnectionClosed {
        /// The number of the connection.
        connection_id: u64,
        /// Why the connection ended, redacted.
        reason: String,
        /// The time of the call.
        at_ms: u64,
    },
    /// An agent registered itself.
    AgentRegistered {
        /// The session of the agent.
        session_id: String,
        /// The agent.
        agent_id: String,
        /// The provider of the model of the agent.
        provider: String,
        /// The instance of the provider.
        instance: String,
        /// The time of the call.
        at_ms: u64,
    },
    /// A job was put in the queue.
    JobEnqueued {
        /// The job.
        job_id: String,
        /// The session that asked for it.
        session_id: String,
        /// The agent that asked for it.
        agent_id: String,
        /// The workspace it works in.
        workspace: String,
        /// The time of the call.
        at_ms: u64,
    },
    /// A job was handed to the scheduler.
    JobDispatched {
        /// The job.
        job_id: String,
        /// The agent that owns it.
        agent_id: String,
        /// The time of the call.
        at_ms: u64,
    },
    /// A job is done.
    JobFinished {
        /// The job.
        job_id: String,
        /// The state it ended in.
        status: String,
        /// The time of the call.
        at_ms: u64,
    },
    /// A write lock was refused.
    LockDenied {
        /// The workspace of the file.
        workspace: String,
        /// The path of the file.
        path: String,
        /// The agent that asked for the lock.
        agent_id: String,
        /// The time of the call.
        at_ms: u64,
    },
    /// The Hub runs on less than it should.
    Degraded {
        /// Why, redacted.
        reason: String,
        /// The time of the call.
        at_ms: u64,
    },
}

impl HubEvent {
    /// The kind of the event.
    #[must_use]
    pub const fn kind(&self) -> HubEventKind {
        match self {
            HubEvent::ConnectionOpened { .. } => HubEventKind::ConnectionOpened,
            HubEvent::ConnectionClosed { .. } => HubEventKind::ConnectionClosed,
            HubEvent::AgentRegistered { .. } => HubEventKind::AgentRegistered,
            HubEvent::JobEnqueued { .. } => HubEventKind::JobEnqueued,
            HubEvent::JobDispatched { .. } => HubEventKind::JobDispatched,
            HubEvent::JobFinished { .. } => HubEventKind::JobFinished,
            HubEvent::LockDenied { .. } => HubEventKind::LockDenied,
            HubEvent::Degraded { .. } => HubEventKind::Degraded,
        }
    }

    /// The time of the event.
    #[must_use]
    pub const fn at_ms(&self) -> u64 {
        match self {
            HubEvent::ConnectionOpened { at_ms, .. }
            | HubEvent::ConnectionClosed { at_ms, .. }
            | HubEvent::AgentRegistered { at_ms, .. }
            | HubEvent::JobEnqueued { at_ms, .. }
            | HubEvent::JobDispatched { at_ms, .. }
            | HubEvent::JobFinished { at_ms, .. }
            | HubEvent::LockDenied { at_ms, .. }
            | HubEvent::Degraded { at_ms, .. } => *at_ms,
        }
    }

    /// The event as the JSON of the log.
    #[must_use]
    pub fn to_json(&self) -> Json {
        let mut fields: Vec<(&str, Json)> = vec![("kind", Json::str(self.kind().as_str()))];
        match self {
            HubEvent::ConnectionOpened {
                connection_id,
                peer,
                at_ms,
            } => {
                fields.push(("connection_id", Json::int(*connection_id as i64)));
                fields.push(("peer", Json::str(peer)));
                fields.push(("at_ms", Json::int(*at_ms as i64)));
            }
            HubEvent::ConnectionClosed {
                connection_id,
                reason,
                at_ms,
            } => {
                fields.push(("connection_id", Json::int(*connection_id as i64)));
                fields.push(("reason", Json::str(reason)));
                fields.push(("at_ms", Json::int(*at_ms as i64)));
            }
            HubEvent::AgentRegistered {
                session_id,
                agent_id,
                provider,
                instance,
                at_ms,
            } => {
                fields.push(("session_id", Json::str(session_id)));
                fields.push(("agent_id", Json::str(agent_id)));
                fields.push(("provider", Json::str(provider)));
                fields.push(("instance", Json::str(instance)));
                fields.push(("at_ms", Json::int(*at_ms as i64)));
            }
            HubEvent::JobEnqueued {
                job_id,
                session_id,
                agent_id,
                workspace,
                at_ms,
            } => {
                fields.push(("job_id", Json::str(job_id)));
                fields.push(("session_id", Json::str(session_id)));
                fields.push(("agent_id", Json::str(agent_id)));
                fields.push(("workspace", Json::str(workspace)));
                fields.push(("at_ms", Json::int(*at_ms as i64)));
            }
            HubEvent::JobDispatched {
                job_id,
                agent_id,
                at_ms,
            } => {
                fields.push(("job_id", Json::str(job_id)));
                fields.push(("agent_id", Json::str(agent_id)));
                fields.push(("at_ms", Json::int(*at_ms as i64)));
            }
            HubEvent::JobFinished {
                job_id,
                status,
                at_ms,
            } => {
                fields.push(("job_id", Json::str(job_id)));
                fields.push(("status", Json::str(status)));
                fields.push(("at_ms", Json::int(*at_ms as i64)));
            }
            HubEvent::LockDenied {
                workspace,
                path,
                agent_id,
                at_ms,
            } => {
                fields.push(("workspace", Json::str(workspace)));
                fields.push(("path", Json::str(path)));
                fields.push(("agent_id", Json::str(agent_id)));
                fields.push(("at_ms", Json::int(*at_ms as i64)));
            }
            HubEvent::Degraded { reason, at_ms } => {
                fields.push(("reason", Json::str(reason)));
                fields.push(("at_ms", Json::int(*at_ms as i64)));
            }
        }
        Json::obj(fields)
    }

    /// The event out of the JSON of the log.
    ///
    /// The answer is an error when the JSON is not an event of ours: a log that cannot be read back is
    /// not a log.
    pub fn from_json(value: &Json) -> Result<HubEvent, TaseError> {
        let kind = value
            .get("kind")
            .and_then(Json::as_str)
            .and_then(HubEventKind::from_name)
            .ok_or_else(|| TaseError::invalid_argument("the event has no known kind"))?;
        let at_ms = value
            .get("at_ms")
            .and_then(Json::as_i64)
            .and_then(|number| u64::try_from(number).ok())
            .unwrap_or(0);
        let text = |key: &str| -> String {
            value
                .get(key)
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let number = |key: &str| -> u64 {
            value
                .get(key)
                .and_then(Json::as_i64)
                .and_then(|number| u64::try_from(number).ok())
                .unwrap_or(0)
        };
        let event = match kind {
            HubEventKind::ConnectionOpened => HubEvent::ConnectionOpened {
                connection_id: number("connection_id"),
                peer: text("peer"),
                at_ms,
            },
            HubEventKind::ConnectionClosed => HubEvent::ConnectionClosed {
                connection_id: number("connection_id"),
                reason: text("reason"),
                at_ms,
            },
            HubEventKind::AgentRegistered => HubEvent::AgentRegistered {
                session_id: text("session_id"),
                agent_id: text("agent_id"),
                provider: text("provider"),
                instance: text("instance"),
                at_ms,
            },
            HubEventKind::JobEnqueued => HubEvent::JobEnqueued {
                job_id: text("job_id"),
                session_id: text("session_id"),
                agent_id: text("agent_id"),
                workspace: text("workspace"),
                at_ms,
            },
            HubEventKind::JobDispatched => HubEvent::JobDispatched {
                job_id: text("job_id"),
                agent_id: text("agent_id"),
                at_ms,
            },
            HubEventKind::JobFinished => HubEvent::JobFinished {
                job_id: text("job_id"),
                status: text("status"),
                at_ms,
            },
            HubEventKind::LockDenied => HubEvent::LockDenied {
                workspace: text("workspace"),
                path: text("path"),
                agent_id: text("agent_id"),
                at_ms,
            },
            HubEventKind::Degraded => HubEvent::Degraded {
                reason: text("reason"),
                at_ms,
            },
        };
        Ok(event)
    }
}

/// The events of the Hub, in the order they happened.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Telemetry {
    events: Vec<HubEvent>,
}

impl Telemetry {
    /// A telemetry without an event.
    #[must_use]
    pub fn new() -> Telemetry {
        Telemetry { events: Vec::new() }
    }

    /// Adds an event at the end.
    pub fn push(&mut self, event: HubEvent) {
        self.events.push(event);
    }

    /// The events, in the order they happened.
    #[must_use]
    pub fn events(&self) -> &[HubEvent] {
        &self.events
    }

    /// The number of the events.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether there is no event.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Forgets every event.
    pub fn clear(&mut self) {
        self.events.clear();
    }

    /// The events after the number `since`; the numbers of the events start at 1.
    ///
    /// A number that is behind everything gives every event; a number that is ahead of everything gives
    /// none. The Hub never guesses: a client that asks for the events after a number gets exactly the
    /// events after that number.
    #[must_use]
    pub fn since(&self, since: u64) -> Vec<&HubEvent> {
        self.events
            .iter()
            .skip(since.saturating_sub(1) as usize)
            .collect()
    }

    /// The events as the JSON of the log: an object with the events and the number of the next one.
    #[must_use]
    pub fn to_json(&self) -> Json {
        let events: Vec<Json> = self.events.iter().map(HubEvent::to_json).collect();
        Json::obj(vec![
            ("events", Json::Arr(events)),
            ("next_seq", Json::int(self.events.len() as i64 + 1)),
        ])
    }

    /// The events as the text of the log.
    #[must_use]
    pub fn to_json_string(&self) -> String {
        self.to_json().to_json_string()
    }

    /// The events out of the JSON of the log.
    pub fn from_json(value: &Json) -> Result<Telemetry, TaseError> {
        let list = value
            .get("events")
            .and_then(Json::as_array)
            .ok_or_else(|| TaseError::invalid_argument("the telemetry has no events"))?;
        let mut telemetry = Telemetry::new();
        for item in list {
            telemetry.push(HubEvent::from_json(item)?);
        }
        Ok(telemetry)
    }

    /// The events out of the text of the log.
    pub fn from_json_string(text: &str) -> Result<Telemetry, TaseError> {
        let value = parse(text, Limits::DEFAULT)
            .map_err(|error| TaseError::invalid_argument(format!("the log is not JSON: {error}")))?;
        Telemetry::from_json(&value)
    }
}

/// The error of a telemetry that cannot be read; the code is the one of the contract.
pub const TELEMETRY_ERROR_CODE: ErrorCode = ErrorCode::InvalidArgument;

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<HubEvent> {
        vec![
            HubEvent::ConnectionOpened {
                connection_id: 1,
                peer: "client".to_string(),
                at_ms: 10,
            },
            HubEvent::AgentRegistered {
                session_id: "sess_1".to_string(),
                agent_id: "agent_1".to_string(),
                provider: "claude".to_string(),
                instance: "inst-1".to_string(),
                at_ms: 20,
            },
            HubEvent::JobEnqueued {
                job_id: "job_1".to_string(),
                session_id: "sess_1".to_string(),
                agent_id: "agent_1".to_string(),
                workspace: "ws_1".to_string(),
                at_ms: 30,
            },
            HubEvent::JobDispatched {
                job_id: "job_1".to_string(),
                agent_id: "agent_1".to_string(),
                at_ms: 40,
            },
            HubEvent::JobFinished {
                job_id: "job_1".to_string(),
                status: "COMMITTED".to_string(),
                at_ms: 50,
            },
            HubEvent::LockDenied {
                workspace: "ws_1".to_string(),
                path: "src/main.rs".to_string(),
                agent_id: "agent_2".to_string(),
                at_ms: 60,
            },
            HubEvent::ConnectionClosed {
                connection_id: 1,
                reason: "the peer left".to_string(),
                at_ms: 70,
            },
            HubEvent::Degraded {
                reason: "the queue is full".to_string(),
                at_ms: 80,
            },
        ]
    }

    #[test]
    fn the_events_keep_the_order_they_happened_in() {
        let mut telemetry = Telemetry::new();
        for event in sample() {
            telemetry.push(event);
        }
        let kinds: Vec<&str> = telemetry
            .events()
            .iter()
            .map(HubEvent::kind)
            .map(HubEventKind::as_str)
            .collect();
        assert_eq!(
            kinds,
            vec![
                "connection_opened",
                "agent_registered",
                "job_enqueued",
                "job_dispatched",
                "job_finished",
                "lock_denied",
                "connection_closed",
                "degraded",
            ]
        );
        assert_eq!(telemetry.len(), 8);
        assert!(!telemetry.is_empty());
    }

    #[test]
    fn every_event_survives_the_round_trip() {
        let mut telemetry = Telemetry::new();
        for event in sample() {
            telemetry.push(event);
        }
        let text = telemetry.to_json_string();
        let back = Telemetry::from_json_string(&text).expect("the log is read back");
        assert_eq!(back, telemetry);
    }

    #[test]
    fn every_event_survives_the_round_trip_on_its_own() {
        for event in sample() {
            let back = HubEvent::from_json(&event.to_json()).expect("the event is read back");
            assert_eq!(back, event);
        }
    }

    #[test]
    fn the_events_after_a_number_are_the_events_that_came_after_it() {
        let mut telemetry = Telemetry::new();
        for event in sample() {
            telemetry.push(event);
        }
        assert_eq!(telemetry.since(0).len(), 8);
        assert_eq!(telemetry.since(1).len(), 8);
        assert_eq!(telemetry.since(5).len(), 4);
        assert_eq!(telemetry.since(9).len(), 0);
        assert_eq!(telemetry.since(100).len(), 0);
    }

    #[test]
    fn a_text_that_names_a_token_is_taken_out_of_the_event() {
        let redacted = redact("the header holds a token: abcdef123456");
        assert_eq!(redacted, REDACTED);
        assert!(!redacted.contains("abcdef123456"));
        for marker in [
            "TOKEN",
            "Secret",
            "password",
            "api_key",
            "Authorization",
            "bearer",
            "private_key",
            "credential",
        ] {
            assert_eq!(redact(marker), REDACTED, "{marker} must be taken out");
        }
    }

    #[test]
    fn an_event_never_writes_a_secret_into_the_log() {
        let mut telemetry = Telemetry::new();
        telemetry.push(HubEvent::ConnectionOpened {
            connection_id: 7,
            peer: redact("client token=sk-live-0123456789"),
            at_ms: 1,
        });
        let text = telemetry.to_json_string();
        assert!(!text.contains("sk-live-0123456789"), "{text}");
        assert!(text.contains(REDACTED), "{text}");
    }

    #[test]
    fn a_long_text_is_cut() {
        let long = "a".repeat(MAX_DETAIL_CHARS + 50);
        let cut = redact(&long);
        assert!(cut.len() <= MAX_DETAIL_CHARS + 3);
        assert!(cut.ends_with("..."));
    }

    #[test]
    fn a_control_character_does_not_go_into_the_log() {
        let cleaned = redact("a\u{0}b\nc");
        assert!(!cleaned.contains('\u{0}'));
        assert!(!cleaned.contains('\n'));
    }

    #[test]
    fn an_event_that_is_not_one_of_ours_is_refused() {
        let value = Json::obj(vec![("kind", Json::str("nonsense"))]);
        let error = HubEvent::from_json(&value).expect_err("the event is refused");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn a_log_that_is_not_json_is_refused() {
        let error = Telemetry::from_json_string("{not json").expect_err("the log is refused");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn every_kind_has_a_name_and_comes_back_from_it() {
        for kind in HubEventKind::ALL {
            assert_eq!(HubEventKind::from_name(kind.as_str()), Some(kind));
        }
        assert_eq!(HubEventKind::from_name("nonsense"), None);
        assert_eq!(HubEventKind::ALL.len(), 8);
    }

    #[test]
    fn the_telemetry_can_be_emptied() {
        let mut telemetry = Telemetry::new();
        telemetry.push(HubEvent::Degraded {
            reason: "why".to_string(),
            at_ms: 1,
        });
        assert_eq!(telemetry.len(), 1);
        telemetry.clear();
        assert!(telemetry.is_empty());
        assert_eq!(telemetry.to_json_string(), "{\"events\":[],\"next_seq\":1}");
    }
}
