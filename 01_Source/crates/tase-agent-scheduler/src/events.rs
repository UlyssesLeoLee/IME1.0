//! The dispatch of the events: ordered per agent, delivered at least once and filtered by a subscription
//! (detailed design §17.5 and §17.6).
//!
//! The Hub tells the agents what happens to their jobs. The dispatcher of this module is the in-memory half
//! of that: it holds the events, it hands them to the subscribers and it tells, exactly, which event was
//! delivered and which one was not.
//!
//! The three rules of the module:
//!
//! * **the order per agent**: the events of one agent are delivered in the order they were published, even
//!   when the events of the other agents are interleaved between them;
//! * **at least once**: an event stays in the list of the subscriber until the subscriber acknowledges it,
//!   so an event that was not acknowledged is delivered again;
//! * **the filter**: a subscription selects the agents, the kinds and the sessions it wants; the events it
//!   does not select are never delivered to it.
//!
//! The report of a subscriber is the truth for the reconciliation: it says what was delivered, what is still
//! waiting for an acknowledgement and what has not been delivered yet.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use tase_contract::{AgentId, SessionId};

/// The identifier of an event.
///
/// The identifier of an event is its place in the log of the dispatcher: the log only grows, so the
/// identifier is also the order of the events.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventId(u64);

impl EventId {
    /// The value of the identifier.
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "evt#{}", self.0)
    }
}

/// The kind of an event.
///
/// The kinds are the ones the Hub tells its agents about; the crate does not fix the payload of an event,
/// it only carries it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EventKind {
    /// A job was queued.
    JobQueued,
    /// A job was given to a worker.
    JobDispatched,
    /// A job made progress.
    JobProgress,
    /// A job is over and it succeeded.
    JobCompleted,
    /// A job is over and it failed.
    JobFailed,
    /// The lease of a job is over.
    LeaseExpired,
    /// The lease of a job was taken by another agent.
    LeaseRevoked,
    /// A request was refused by a quota.
    QuotaRejected,
    /// A request was refused by a budget.
    BudgetRejected,
    /// The session of an agent is closed.
    SessionClosed,
}

impl EventKind {
    /// The name of the kind, for the logs.
    pub const fn as_str(self) -> &'static str {
        match self {
            EventKind::JobQueued => "job_queued",
            EventKind::JobDispatched => "job_dispatched",
            EventKind::JobProgress => "job_progress",
            EventKind::JobCompleted => "job_completed",
            EventKind::JobFailed => "job_failed",
            EventKind::LeaseExpired => "lease_expired",
            EventKind::LeaseRevoked => "lease_revoked",
            EventKind::QuotaRejected => "quota_rejected",
            EventKind::BudgetRejected => "budget_rejected",
            EventKind::SessionClosed => "session_closed",
        }
    }
}

impl fmt::Display for EventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An event of the Hub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    /// The identifier of the event.
    pub id: EventId,
    /// The place of the event in the log: the order of the events.
    pub seq: u64,
    /// The agent the event belongs to.
    pub agent_id: AgentId,
    /// The session of the agent, when the event has one.
    pub session_id: Option<SessionId>,
    /// The kind of the event.
    pub kind: EventKind,
    /// The payload of the event; the crate carries it and does not read it.
    pub payload: String,
}

/// The filter of a subscription: `None` means "every".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubscriptionFilter {
    /// The agents the subscriber wants.
    pub agents: Option<Vec<AgentId>>,
    /// The kinds the subscriber wants.
    pub kinds: Option<Vec<EventKind>>,
    /// The sessions the subscriber wants.
    pub sessions: Option<Vec<SessionId>>,
}

impl SubscriptionFilter {
    /// A filter that selects everything.
    pub fn all() -> Self {
        SubscriptionFilter::default()
    }

    /// A filter that selects the events of one agent.
    pub fn for_agent(agent: &AgentId) -> Self {
        SubscriptionFilter {
            agents: Some(vec![agent.clone()]),
            ..SubscriptionFilter::default()
        }
    }

    /// A filter that selects the events of one kind.
    pub fn for_kind(kind: EventKind) -> Self {
        SubscriptionFilter {
            kinds: Some(vec![kind]),
            ..SubscriptionFilter::default()
        }
    }

    /// A filter that selects the events of one session.
    pub fn for_session(session: &SessionId) -> Self {
        SubscriptionFilter {
            sessions: Some(vec![session.clone()]),
            ..SubscriptionFilter::default()
        }
    }

    /// Whether the filter selects an event.
    pub fn matches(&self, event: &Event) -> bool {
        if let Some(agents) = &self.agents
            && !agents.contains(&event.agent_id)
        {
            return false;
        }
        if let Some(kinds) = &self.kinds
            && !kinds.contains(&event.kind)
        {
            return false;
        }
        if let Some(sessions) = &self.sessions {
            match &event.session_id {
                Some(session) if sessions.contains(session) => {}
                _ => return false,
            }
        }
        true
    }
}

/// The identifier of a subscriber.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubscriberId(u64);

impl fmt::Display for SubscriberId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sub#{}", self.0)
    }
}

/// The state of one subscriber.
#[derive(Clone, Debug)]
struct SubscriberState {
    filter: SubscriptionFilter,
    /// The sequence of the next event of the log to look at.
    cursor: u64,
    /// The events that were delivered and that wait for an acknowledgement.
    pending: VecDeque<u64>,
    /// The events the subscriber acknowledged.
    acked: Vec<u64>,
}

/// The truth of the delivery of one subscriber.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeliveryReport {
    /// The subscriber of the report.
    pub subscriber: SubscriberId,
    /// The events that were delivered at least once, in the order of the log.
    pub delivered: Vec<EventId>,
    /// The events the subscriber acknowledged.
    pub acked: Vec<EventId>,
    /// The events that were delivered and are still waiting for an acknowledgement.
    pub pending: Vec<EventId>,
    /// The events that match the filter and were never delivered.
    pub undelivered: Vec<EventId>,
    /// The number of the events that were published and that the filter does not select.
    pub skipped: usize,
}

/// The dispatcher of the events of the Hub.
///
/// The dispatcher holds the log of the events and the state of the subscribers. It is pure memory: the
/// persistence of the events belongs to the outbox of the store of the Hub (§18), not here.
#[derive(Clone, Debug, Default)]
pub struct Dispatcher {
    log: Vec<Event>,
    next_seq: u64,
    subscribers: BTreeMap<SubscriberId, SubscriberState>,
    next_subscriber: u64,
}

impl Dispatcher {
    /// A dispatcher without an event and without a subscriber.
    pub fn new() -> Self {
        Dispatcher::default()
    }

    /// The number of the events of the log.
    pub fn len(&self) -> usize {
        self.log.len()
    }

    /// Whether the log is empty.
    pub fn is_empty(&self) -> bool {
        self.log.is_empty()
    }

    /// The events of the log, in the order of the log.
    pub fn events(&self) -> &[Event] {
        &self.log
    }

    /// Publishes an event and gives its identifier back.
    pub fn publish(
        &mut self,
        agent_id: AgentId,
        session_id: Option<SessionId>,
        kind: EventKind,
        payload: impl Into<String>,
    ) -> EventId {
        let id = EventId(self.next_seq);
        self.next_seq = self.next_seq.saturating_add(1);
        self.log.push(Event {
            id,
            seq: id.as_u64(),
            agent_id,
            session_id,
            kind,
            payload: payload.into(),
        });
        id
    }

    /// The event of an identifier, or `None` when there is no such event.
    pub fn event(&self, id: EventId) -> Option<&Event> {
        let index = usize::try_from(id.as_u64()).ok()?;
        self.log.get(index)
    }

    /// Adds a subscriber with a filter.
    pub fn subscribe(&mut self, filter: SubscriptionFilter) -> SubscriberId {
        let id = SubscriberId(self.next_subscriber);
        self.next_subscriber = self.next_subscriber.saturating_add(1);
        self.subscribers.insert(
            id,
            SubscriberState {
                filter,
                cursor: 0,
                pending: VecDeque::new(),
                acked: Vec::new(),
            },
        );
        id
    }

    /// Removes a subscriber; the events it did not acknowledge stay in the log.
    pub fn unsubscribe(&mut self, subscriber: SubscriberId) -> bool {
        self.subscribers.remove(&subscriber).is_some()
    }

    /// The events to deliver to a subscriber, at most `max` of them.
    ///
    /// The events that were delivered and not acknowledged come first, so an event is delivered at least
    /// once: the subscriber sees it again until it acknowledges it. The new events follow, in the order of
    /// the log.
    pub fn poll(&mut self, subscriber: SubscriberId, max: usize) -> Vec<Event> {
        let Dispatcher {
            log, subscribers, ..
        } = self;
        let Some(state) = subscribers.get_mut(&subscriber) else {
            return Vec::new();
        };
        let mut ids: Vec<u64> = state.pending.iter().copied().take(max).collect();
        if ids.len() < max {
            let filter = &state.filter;
            let cursor = state.cursor;
            for event in log.iter() {
                if ids.len() >= max {
                    break;
                }
                if event.seq < cursor || !filter.matches(event) {
                    continue;
                }
                ids.push(event.id.as_u64());
                state.cursor = event.seq.saturating_add(1);
            }
        }
        for id in &ids {
            if !state.pending.contains(id) {
                state.pending.push_back(*id);
            }
        }
        let mut out = Vec::new();
        for id in ids {
            if let Some(event) = event_at(log, id).cloned() {
                out.push(event);
            }
        }
        out
    }

    /// Acknowledges an event of a subscriber.
    ///
    /// The event stays in the log, but it is not delivered again.
    pub fn ack(&mut self, subscriber: SubscriberId, event: EventId) -> bool {
        let Some(state) = self.subscribers.get_mut(&subscriber) else {
            return false;
        };
        let Some(position) = state.pending.iter().position(|id| *id == event.as_u64()) else {
            return false;
        };
        state.pending.remove(position);
        state.acked.push(event.as_u64());
        true
    }

    /// The truth of the delivery of a subscriber.
    pub fn report(&self, subscriber: SubscriberId) -> Option<DeliveryReport> {
        let state = self.subscribers.get(&subscriber)?;
        let mut delivered: Vec<EventId> = state
            .acked
            .iter()
            .chain(state.pending.iter())
            .copied()
            .map(EventId)
            .collect();
        delivered.sort_unstable();
        delivered.dedup();
        let undelivered: Vec<EventId> = self
            .log
            .iter()
            .filter(|event| event.seq >= state.cursor && state.filter.matches(event))
            .map(|event| event.id)
            .collect();
        let skipped = self
            .log
            .iter()
            .filter(|event| event.seq >= state.cursor && !state.filter.matches(event))
            .count();
        Some(DeliveryReport {
            subscriber,
            delivered,
            acked: state.acked.iter().copied().map(EventId).collect(),
            pending: state.pending.iter().copied().map(EventId).collect(),
            undelivered,
            skipped,
        })
    }
}

/// The event of the log at an identifier.
fn event_at(log: &[Event], id: u64) -> Option<&Event> {
    let index = usize::try_from(id).ok()?;
    log.get(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An agent of the tests.
    fn agent(name: &str) -> AgentId {
        AgentId::parse(name).expect("a legal agent")
    }

    /// A session of the tests.
    fn session(name: &str) -> SessionId {
        SessionId::parse(name).expect("a legal session")
    }

    #[test]
    fn the_events_of_an_agent_keep_their_order() {
        let mut dispatcher = Dispatcher::new();
        let first = agent("agent_01");
        let second = agent("agent_02");
        let sub_first = dispatcher.subscribe(SubscriptionFilter::for_agent(&first));
        let sub_second = dispatcher.subscribe(SubscriptionFilter::for_agent(&second));

        // The events of the two agents are interleaved.
        dispatcher.publish(first.clone(), None, EventKind::JobQueued, "a0");
        dispatcher.publish(second.clone(), None, EventKind::JobQueued, "b0");
        dispatcher.publish(first.clone(), None, EventKind::JobDispatched, "a1");
        dispatcher.publish(second.clone(), None, EventKind::JobProgress, "b1");
        dispatcher.publish(first.clone(), None, EventKind::JobCompleted, "a2");

        let delivered_first: Vec<String> = dispatcher
            .poll(sub_first, 10)
            .into_iter()
            .map(|event| event.payload)
            .collect();
        let delivered_second: Vec<String> = dispatcher
            .poll(sub_second, 10)
            .into_iter()
            .map(|event| event.payload)
            .collect();
        assert_eq!(delivered_first, ["a0", "a1", "a2"]);
        assert_eq!(delivered_second, ["b0", "b1"]);
    }

    #[test]
    fn an_event_is_delivered_at_least_once() {
        let mut dispatcher = Dispatcher::new();
        let who = agent("agent_01");
        let subscriber = dispatcher.subscribe(SubscriptionFilter::for_agent(&who));
        let id = dispatcher.publish(who.clone(), None, EventKind::JobQueued, "payload");

        let first = dispatcher.poll(subscriber, 10);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].id, id);

        // The event was not acknowledged: it is delivered again.
        let second = dispatcher.poll(subscriber, 10);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].id, id);

        // Once it is acknowledged, it is not delivered again.
        assert!(dispatcher.ack(subscriber, id));
        let third = dispatcher.poll(subscriber, 10);
        assert!(third.is_empty());
    }

    #[test]
    fn the_report_says_what_was_delivered_and_what_is_not() {
        let mut dispatcher = Dispatcher::new();
        let who = agent("agent_01");
        let subscriber = dispatcher.subscribe(SubscriptionFilter::for_agent(&who));
        let first = dispatcher.publish(who.clone(), None, EventKind::JobQueued, "a");
        let second = dispatcher.publish(who.clone(), None, EventKind::JobProgress, "b");
        dispatcher.publish(agent("agent_02"), None, EventKind::JobQueued, "c");

        let report = dispatcher.report(subscriber).expect("a report");
        assert!(report.delivered.is_empty());
        assert!(report.pending.is_empty());
        assert_eq!(report.undelivered, [first, second]);
        assert_eq!(report.skipped, 1, "the event of the other agent is not selected");

        dispatcher.poll(subscriber, 1);
        let report = dispatcher.report(subscriber).expect("a report");
        assert_eq!(report.delivered, [first]);
        assert_eq!(report.pending, [first]);
        assert_eq!(report.undelivered, [second]);

        dispatcher.poll(subscriber, 10);
        let report = dispatcher.report(subscriber).expect("a report");
        assert_eq!(report.delivered, [first, second]);
        assert_eq!(report.undelivered, []);
    }

    #[test]
    fn the_acknowledged_events_leave_the_pending_list() {
        let mut dispatcher = Dispatcher::new();
        let who = agent("agent_01");
        let subscriber = dispatcher.subscribe(SubscriptionFilter::for_agent(&who));
        let id = dispatcher.publish(who.clone(), None, EventKind::JobQueued, "a");

        dispatcher.poll(subscriber, 10);
        assert!(dispatcher.ack(subscriber, id));
        assert!(!dispatcher.ack(subscriber, id), "the event is acknowledged once");

        let report = dispatcher.report(subscriber).expect("a report");
        assert_eq!(report.acked, [id]);
        assert!(report.pending.is_empty());
        assert_eq!(report.delivered, [id]);
    }

    #[test]
    fn the_filter_selects_the_events_of_a_kind() {
        let mut dispatcher = Dispatcher::new();
        let who = agent("agent_01");
        let subscriber = dispatcher.subscribe(SubscriptionFilter::for_kind(EventKind::JobCompleted));
        dispatcher.publish(who.clone(), None, EventKind::JobQueued, "a");
        dispatcher.publish(who.clone(), None, EventKind::JobCompleted, "b");
        dispatcher.publish(who.clone(), None, EventKind::JobFailed, "c");

        let delivered = dispatcher.poll(subscriber, 10);
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].kind, EventKind::JobCompleted);
        assert_eq!(delivered[0].payload, "b");
    }

    #[test]
    fn the_filter_selects_the_events_of_a_session() {
        let mut dispatcher = Dispatcher::new();
        let who = agent("agent_01");
        let talk = session("sess_01");
        let subscriber = dispatcher.subscribe(SubscriptionFilter::for_session(&talk));
        dispatcher.publish(who.clone(), None, EventKind::JobQueued, "a");
        dispatcher.publish(who.clone(), Some(talk.clone()), EventKind::JobQueued, "b");

        let delivered = dispatcher.poll(subscriber, 10);
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].payload, "b");
    }

    #[test]
    fn the_subscribers_do_not_interfere() {
        let mut dispatcher = Dispatcher::new();
        let first = agent("agent_01");
        let second = agent("agent_02");
        let sub_first = dispatcher.subscribe(SubscriptionFilter::for_agent(&first));
        let sub_second = dispatcher.subscribe(SubscriptionFilter::all());

        dispatcher.publish(first.clone(), None, EventKind::JobQueued, "a");
        dispatcher.publish(second.clone(), None, EventKind::JobQueued, "b");

        assert_eq!(dispatcher.poll(sub_first, 10).len(), 1);
        // The subscriber of everything has not been polled yet: it still has its two events.
        let report = dispatcher.report(sub_second).expect("a report");
        assert_eq!(report.undelivered.len(), 2);
        assert_eq!(dispatcher.poll(sub_second, 10).len(), 2);
    }

    #[test]
    fn a_subscriber_that_does_not_exist_gets_nothing() {
        let mut dispatcher = Dispatcher::new();
        let missing = SubscriberId(42);
        assert!(dispatcher.poll(missing, 10).is_empty());
        assert!(!dispatcher.ack(missing, EventId(0)));
        assert!(dispatcher.report(missing).is_none());
        assert!(!dispatcher.unsubscribe(missing));
    }

    #[test]
    fn the_log_only_grows_and_the_identifier_is_the_order() {
        let mut dispatcher = Dispatcher::new();
        let who = agent("agent_01");
        assert!(dispatcher.is_empty());
        let first = dispatcher.publish(who.clone(), None, EventKind::JobQueued, "a");
        let second = dispatcher.publish(who.clone(), None, EventKind::JobProgress, "b");
        assert_eq!(dispatcher.len(), 2);
        assert!(first < second);
        assert_eq!(dispatcher.event(first).map(|event| event.payload.as_str()), Some("a"));
        assert_eq!(dispatcher.event(second).map(|event| event.kind), Some(EventKind::JobProgress));
        assert_eq!(dispatcher.events().len(), 2);
    }

    #[test]
    fn a_subscriber_can_be_removed() {
        let mut dispatcher = Dispatcher::new();
        let who = agent("agent_01");
        let subscriber = dispatcher.subscribe(SubscriptionFilter::for_agent(&who));
        assert!(dispatcher.unsubscribe(subscriber));
        assert!(dispatcher.poll(subscriber, 10).is_empty());
    }

    #[test]
    fn the_poll_of_a_subscriber_stops_at_the_limit() {
        let mut dispatcher = Dispatcher::new();
        let who = agent("agent_01");
        let subscriber = dispatcher.subscribe(SubscriptionFilter::for_agent(&who));
        for index in 0..5u32 {
            dispatcher.publish(who.clone(), None, EventKind::JobQueued, format!("e{index}"));
        }
        assert_eq!(dispatcher.poll(subscriber, 2).len(), 2);
        // The events that were not taken are still waiting: they are delivered again by the next poll.
        assert_eq!(dispatcher.poll(subscriber, 10).len(), 5);
    }
}
