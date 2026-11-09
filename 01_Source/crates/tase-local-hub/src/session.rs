//! The sessions of the agents: who is connected, and what it may do.
//!
//! A session is the relation between a connection and an agent (detailed design §17.4). The rules of
//! the module:
//!
//! * **a session belongs to one agent and one connection.** The agent is named by its provider (the
//!   model it talks to) and by its instance (which process of that provider it is): two instances of
//!   the same provider are two sessions, because they are two agents that write into the same files.
//! * **a session lives as long as its connection.** When the connection goes, the session is marked as
//!   gone and everything it held (its jobs, its write locks) is released (detailed design §17.6).
//! * **a session has a lease.** An agent that stops sending heartbeats loses its session after the
//!   lease runs out; the Hub does not wait for ever.

use tase_contract::{AgentId, SessionId, TaseError, WorkspaceId};

/// The state of a session of the Hub.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    /// The agent is connected and its heartbeats arrive.
    Connected,
    /// The agent left on its own.
    Disconnected,
    /// The agent stopped sending heartbeats and the Hub gave up on it.
    Expired,
}

impl SessionState {
    /// The name of the state, as the design book writes it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            SessionState::Connected => "CONNECTED",
            SessionState::Disconnected => "DISCONNECTED",
            SessionState::Expired => "EXPIRED",
        }
    }
}

/// A session of an agent in the Hub.
///
/// The five fields are the identity of the session: the id of the session, the agent, the provider of
/// the model of the agent, the instance of that provider and the capabilities the agent says it has.
/// The state of the session (its workspace, its scopes, its lease) lives in the registry, in
/// [`SessionEntry`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSession {
    /// The id of the session.
    pub session_id: SessionId,
    /// The id of the agent.
    pub agent: AgentId,
    /// The provider of the model of the agent (`claude`, `codex`, `local`, ...).
    pub provider: String,
    /// The instance of the agent; two instances of a provider are two sessions.
    pub instance: String,
    /// The capabilities the agent says it has.
    pub capabilities: Vec<String>,
}

impl AgentSession {
    /// A session with nothing but its ids; the registration fills the rest.
    pub fn new(session_id: SessionId, agent: AgentId) -> AgentSession {
        AgentSession {
            session_id,
            agent,
            provider: String::new(),
            instance: String::new(),
            capabilities: Vec::new(),
        }
    }

    /// Whether the session names the provider and the instance of an agent.
    #[must_use]
    pub fn is_registered(&self) -> bool {
        !self.provider.is_empty()
    }
}

/// What the Hub knows about a session besides its identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionEntry {
    /// The identity of the session.
    pub session: AgentSession,
    /// The workspace the agent works in.
    pub workspace: WorkspaceId,
    /// The scopes the Hub granted to the session.
    pub granted_scopes: Vec<String>,
    /// The state of the session.
    pub state: SessionState,
    /// The time of the last heartbeat.
    pub last_heartbeat_ms: u64,
    /// The time the lease of the session runs out.
    pub lease_expires_at_ms: u64,
    /// The number of the jobs of the session that are not finished.
    pub inflight_jobs: u32,
}

/// The sessions of the Hub.
///
/// The registry is the list of the sessions that are known; it is the only place that decides whether a
/// session may do something.
#[derive(Clone, Debug, Default)]
pub struct SessionRegistry {
    entries: Vec<SessionEntry>,
}

impl SessionRegistry {
    /// A registry without a session.
    #[must_use]
    pub fn new() -> SessionRegistry {
        SessionRegistry {
            entries: Vec::new(),
        }
    }

    /// Registers a session.
    ///
    /// The session id must be free: the Hub does not take a session over without saying so. A session
    /// of the same provider and the same instance that is still connected is marked as gone: the
    /// instance came back, the old session did not.
    pub fn register(
        &mut self,
        session: AgentSession,
        workspace: WorkspaceId,
        granted_scopes: Vec<String>,
        now_ms: u64,
        lease_ttl_ms: u64,
    ) -> Result<&SessionEntry, TaseError> {
        if self.find(&session.session_id).is_some() {
            return Err(TaseError::invalid_argument(format!(
                "the session {} is already registered",
                session.session_id.as_str()
            )));
        }
        for entry in &mut self.entries {
            if entry.state == SessionState::Connected
                && entry.session.provider == session.provider
                && entry.session.instance == session.instance
                && !session.provider.is_empty()
            {
                entry.state = SessionState::Disconnected;
            }
        }
        let entry = SessionEntry {
            session,
            workspace,
            granted_scopes,
            state: SessionState::Connected,
            last_heartbeat_ms: now_ms,
            lease_expires_at_ms: now_ms.saturating_add(lease_ttl_ms),
            inflight_jobs: 0,
        };
        self.entries.push(entry);
        Ok(self.entries.last().expect("the entry was just pushed"))
    }

    /// Says that the session is still there.
    pub fn heartbeat(
        &mut self,
        session_id: &SessionId,
        now_ms: u64,
        lease_ttl_ms: u64,
    ) -> Result<u64, TaseError> {
        let entry = self
            .find_mut(session_id)
            .ok_or_else(|| TaseError::not_found(format!("the session {session_id} is not known")))?;
        if entry.state != SessionState::Connected {
            return Err(TaseError::not_found(format!(
                "the session {session_id} is not connected"
            )));
        }
        entry.last_heartbeat_ms = now_ms;
        entry.lease_expires_at_ms = now_ms.saturating_add(lease_ttl_ms);
        Ok(entry.lease_expires_at_ms)
    }

    /// Removes the session and gives it back.
    pub fn unregister(&mut self, session_id: &SessionId) -> Result<SessionEntry, TaseError> {
        let index = self
            .entries
            .iter()
            .position(|entry| &entry.session.session_id == session_id)
            .ok_or_else(|| TaseError::not_found(format!("the session {session_id} is not known")))?;
        Ok(self.entries.remove(index))
    }

    /// The session of an id.
    #[must_use]
    pub fn get(&self, session_id: &SessionId) -> Option<&SessionEntry> {
        self.find(session_id)
    }

    /// Every session, in the order they were registered.
    #[must_use]
    pub fn entries(&self) -> &[SessionEntry] {
        &self.entries
    }

    /// The number of the sessions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there is no session.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The sessions that are connected.
    #[must_use]
    pub fn connected(&self) -> Vec<&SessionEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.state == SessionState::Connected)
            .collect()
    }

    /// The number of the jobs that are not finished, over every session.
    #[must_use]
    pub fn inflight_jobs(&self) -> u32 {
        self.entries.iter().map(|entry| entry.inflight_jobs).sum()
    }

    /// Gives up on the sessions whose lease ran out, and gives them back.
    pub fn expire(&mut self, now_ms: u64) -> Vec<SessionId> {
        let mut expired = Vec::new();
        for entry in &mut self.entries {
            if entry.state == SessionState::Connected && entry.lease_expires_at_ms <= now_ms {
                entry.state = SessionState::Expired;
                expired.push(entry.session.session_id.clone());
            }
        }
        expired
    }

    /// Marks the session as gone, without removing it.
    pub fn disconnect(&mut self, session_id: &SessionId) -> bool {
        match self.find_mut(session_id) {
            Some(entry) => {
                entry.state = SessionState::Disconnected;
                true
            }
            None => false,
        }
    }

    fn find(&self, session_id: &SessionId) -> Option<&SessionEntry> {
        self.entries
            .iter()
            .find(|entry| &entry.session.session_id == session_id)
    }

    fn find_mut(&mut self, session_id: &SessionId) -> Option<&mut SessionEntry> {
        self.entries
            .iter_mut()
            .find(|entry| &entry.session.session_id == session_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOGON: &str = "S-1-5-5-1000-1000";

    fn new_session(name: &str) -> AgentSession {
        AgentSession::new(
            SessionId::new(format!("sess_{name}")).expect("a legal session id"),
            AgentId::new(format!("agent_{name}")).expect("a legal agent id"),
        )
    }

    fn workspace(name: &str) -> WorkspaceId {
        WorkspaceId::new(format!("ws_{name}")).expect("a legal workspace id")
    }

    #[test]
    fn a_session_is_registered_and_found_again() {
        let mut registry = SessionRegistry::new();
        let mut session = new_session("one");
        session.provider = "claude".to_string();
        session.instance = "inst-1".to_string();
        registry
            .register(session.clone(), workspace("a"), vec!["catalog:read".to_string()], 100, 1000)
            .expect("the session is registered");
        let entry = registry.get(&session.session_id).expect("the session is there");
        assert_eq!(entry.session, session);
        assert_eq!(entry.state, SessionState::Connected);
        assert_eq!(entry.last_heartbeat_ms, 100);
        assert_eq!(entry.lease_expires_at_ms, 1100);
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
    }

    #[test]
    fn two_instances_of_a_provider_are_two_sessions() {
        let mut registry = SessionRegistry::new();
        let mut first = new_session("one");
        first.provider = "claude".to_string();
        first.instance = "inst-1".to_string();
        let mut second = new_session("two");
        second.provider = "claude".to_string();
        second.instance = "inst-2".to_string();
        registry
            .register(first.clone(), workspace("a"), Vec::new(), 0, 1000)
            .expect("the first is registered");
        registry
            .register(second.clone(), workspace("a"), Vec::new(), 0, 1000)
            .expect("the second is registered");
        assert_eq!(registry.len(), 2);
        assert_ne!(first.session_id, second.session_id);
        assert_ne!(first.agent, second.agent);
        assert_eq!(registry.connected().len(), 2);
    }

    #[test]
    fn the_same_instance_coming_back_ends_the_old_session() {
        let mut registry = SessionRegistry::new();
        let mut first = new_session("one");
        first.provider = "claude".to_string();
        first.instance = "inst-1".to_string();
        let mut second = new_session("two");
        second.provider = "claude".to_string();
        second.instance = "inst-1".to_string();
        registry
            .register(first.clone(), workspace("a"), Vec::new(), 0, 1000)
            .expect("the first is registered");
        registry
            .register(second.clone(), workspace("a"), Vec::new(), 0, 1000)
            .expect("the second is registered");
        assert_eq!(registry.len(), 2);
        assert_eq!(
            registry.get(&first.session_id).expect("the first is there").state,
            SessionState::Disconnected
        );
        assert_eq!(
            registry.get(&second.session_id).expect("the second is there").state,
            SessionState::Connected
        );
    }

    #[test]
    fn the_same_session_id_is_refused() {
        let mut registry = SessionRegistry::new();
        let mut session = new_session("one");
        session.provider = "claude".to_string();
        registry
            .register(session.clone(), workspace("a"), Vec::new(), 0, 1000)
            .expect("the session is registered");
        let error = registry
            .register(session, workspace("a"), Vec::new(), 0, 1000)
            .expect_err("the session is refused");
        assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);
    }

    #[test]
    fn a_heartbeat_moves_the_lease() {
        let mut registry = SessionRegistry::new();
        let mut session = new_session("one");
        session.provider = "claude".to_string();
        registry
            .register(session.clone(), workspace("a"), Vec::new(), 100, 1000)
            .expect("the session is registered");
        let expires = registry
            .heartbeat(&session.session_id, 500, 1000)
            .expect("the heartbeat is accepted");
        assert_eq!(expires, 1500);
        let entry = registry.get(&session.session_id).expect("the session is there");
        assert_eq!(entry.last_heartbeat_ms, 500);
        assert_eq!(entry.lease_expires_at_ms, 1500);
    }

    #[test]
    fn a_heartbeat_of_a_session_that_is_not_there_is_refused() {
        let mut registry = SessionRegistry::new();
        let error = registry
            .heartbeat(&new_session("ghost").session_id, 10, 1000)
            .expect_err("the heartbeat is refused");
        assert_eq!(error.code, tase_contract::ErrorCode::NotFound);
    }

    #[test]
    fn a_session_is_given_up_when_its_lease_runs_out() {
        let mut registry = SessionRegistry::new();
        let mut session = new_session("one");
        session.provider = "claude".to_string();
        registry
            .register(session.clone(), workspace("a"), Vec::new(), 100, 1000)
            .expect("the session is registered");
        assert!(registry.expire(500).is_empty());
        let expired = registry.expire(1200);
        assert_eq!(expired, vec![session.session_id.clone()]);
        assert_eq!(
            registry.get(&session.session_id).expect("the session is there").state,
            SessionState::Expired
        );
    }

    #[test]
    fn a_session_is_removed_when_it_unregisters() {
        let mut registry = SessionRegistry::new();
        let mut session = new_session("one");
        session.provider = "claude".to_string();
        registry
            .register(session.clone(), workspace("a"), Vec::new(), 0, 1000)
            .expect("the session is registered");
        let entry = registry
            .unregister(&session.session_id)
            .expect("the session is removed");
        assert_eq!(entry.session, session);
        assert!(registry.is_empty());
        let error = registry
            .unregister(&session.session_id)
            .expect_err("the session is not there any more");
        assert_eq!(error.code, tase_contract::ErrorCode::NotFound);
    }

    #[test]
    fn a_session_that_is_disconnected_is_marked_as_gone() {
        let mut registry = SessionRegistry::new();
        let mut session = new_session("one");
        session.provider = "claude".to_string();
        registry
            .register(session.clone(), workspace("a"), Vec::new(), 0, 1000)
            .expect("the session is registered");
        assert!(registry.disconnect(&session.session_id));
        assert_eq!(
            registry.get(&session.session_id).expect("the session is there").state,
            SessionState::Disconnected
        );
        assert!(!registry.disconnect(&new_session("ghost").session_id));
        assert_eq!(registry.connected().len(), 0);
    }

    #[test]
    fn a_session_knows_whether_it_is_registered() {
        let mut session = new_session("one");
        assert!(!session.is_registered());
        session.provider = "claude".to_string();
        assert!(session.is_registered());
    }

    #[test]
    fn the_state_has_a_name() {
        assert_eq!(SessionState::Connected.as_str(), "CONNECTED");
        assert_eq!(SessionState::Disconnected.as_str(), "DISCONNECTED");
        assert_eq!(SessionState::Expired.as_str(), "EXPIRED");
        assert!(!LOGON.is_empty());
    }
}
