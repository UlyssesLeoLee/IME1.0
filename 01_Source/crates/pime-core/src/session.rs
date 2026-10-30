//! Protocol 0.3 sessions: the core-side state of one editor bridge (settings, snapshots, edit proposals, edit
//! plans and tokens, grants, budget). Every 0.3 op except `health` and `session.open` must name an open session;
//! an unknown, evicted or expired session answers `NO_SESSION`.
//!
//! Sessions are independent of each other. The store keeps at most [`MAX_SESSIONS`] of them (the least recently
//! used one is dropped when a new one is opened) and forgets a session that has been idle for [`SESSION_IDLE_MS`].
//! The idle limit is long on purpose: an interactive editor session lasts hours, and the only thing that must not
//! linger is source text, which has its own, much shorter lifetime (snapshots are released with `ctx.release`).

use crate::budget::{Budget, Ticket};
use crate::context::Snapshot;
use crate::edit::diff::Hunk;
use crate::edit::transaction::Coordinator;
use crate::grants::PermissionBroker;
use crate::ids::IdGen;
use crate::json::Json;
use crate::settings;
use crate::validators::Dialect;
use std::collections::HashMap;

pub const MAX_SESSIONS: usize = 16;
pub const SESSION_IDLE_MS: u64 = 24 * 3600 * 1000;
/// At most this many snapshots are kept per session (the oldest is dropped first); the bridge releases a snapshot
/// as soon as its F1 session closes, so the limit is only a safety net.
pub const MAX_SNAPSHOTS: usize = 8;
/// Upper bound of the document text kept per session.
pub const MAX_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_PROPOSALS: usize = 16;
pub const MAX_TICKETS: usize = 1024;

/// A model proposal for one snapshot: the proposed text and its line hunks, held by the core so that the bridge
/// never has to resend either. A proposal is not yet an edit plan: the plan (with its lifetime) is created when
/// the user applies.
pub struct Proposal {
    pub id: String,
    pub snapshot_id: String,
    pub proposed_text: String,
    pub hunks: Vec<Hunk>,
    pub language: Option<String>,
    pub dialect: Dialect,
}

pub struct Session {
    pub id: String,
    pub locale: String,
    /// Effective settings (see [`crate::settings::effective`]).
    pub settings: Json,
    pub ids: IdGen,
    pub last_used: u64,
    /// Oldest first. Public so that a handler can borrow a snapshot while it uses other parts of the session.
    pub snapshots: Vec<Snapshot>,
    pub proposals: Vec<Proposal>,
    pub coordinator: Coordinator,
    pub broker: PermissionBroker,
    pub budget: Budget,
    pub tickets: HashMap<String, Ticket>,
}

pub struct SessionStore {
    sessions: Vec<Session>,
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Why `session.open` was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum OpenError {
    /// Another open session was created from the same entropy (ids would collide).
    EntropyReused,
}

impl SessionStore {
    pub fn new() -> Self {
        SessionStore {
            sessions: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    fn drop_expired(&mut self, now: u64) {
        self.sessions
            .retain(|s| now.saturating_sub(s.last_used) <= SESSION_IDLE_MS);
    }

    /// Creates a session from validated entropy. The id is derived from the session's own id generator.
    pub fn open(
        &mut self,
        now: u64,
        mut ids: IdGen,
        locale: String,
        settings: Json,
    ) -> Result<&mut Session, OpenError> {
        self.drop_expired(now);
        let fingerprint = ids.fingerprint();
        if self
            .sessions
            .iter()
            .any(|s| s.ids.fingerprint() == fingerprint)
        {
            return Err(OpenError::EntropyReused);
        }
        if self.sessions.len() >= MAX_SESSIONS {
            if let Some(oldest) = self
                .sessions
                .iter()
                .enumerate()
                .min_by_key(|(_, s)| s.last_used)
                .map(|(i, _)| i)
            {
                self.sessions.remove(oldest);
            }
        }
        let id = ids.next("session", 16);
        self.sessions.push(Session {
            id,
            locale,
            settings,
            ids,
            last_used: now,
            snapshots: Vec::new(),
            proposals: Vec::new(),
            coordinator: Coordinator::default(),
            broker: PermissionBroker::new(),
            budget: Budget::default(),
            tickets: HashMap::new(),
        });
        Ok(self.sessions.last_mut().expect("just pushed"))
    }

    /// The session with this id, if it is still open; using it refreshes its idle timer.
    pub fn get(&mut self, now: u64, id: &str) -> Option<&mut Session> {
        self.drop_expired(now);
        let s = self.sessions.iter_mut().find(|s| s.id == id)?;
        s.last_used = now;
        Some(s)
    }

    pub fn close(&mut self, id: &str) -> bool {
        let before = self.sessions.len();
        self.sessions.retain(|s| s.id != id);
        self.sessions.len() != before
    }
}

impl Session {
    pub fn default_settings() -> Json {
        settings::defaults()
    }

    /// Stores a snapshot (assigning its id) and returns it. The oldest snapshots (and their proposals) are dropped
    /// when the session holds more than [`MAX_SNAPSHOTS`] of them or more than [`MAX_SNAPSHOT_BYTES`] of text.
    pub fn store_snapshot(&mut self, mut snapshot: Snapshot) -> &Snapshot {
        snapshot.snapshot_id = self.ids.next("snapshot", 16);
        self.snapshots.push(snapshot);
        while self.snapshots.len() > 1
            && (self.snapshots.len() > MAX_SNAPSHOTS
                || self
                    .snapshots
                    .iter()
                    .map(|s| s.content.len())
                    .sum::<usize>()
                    > MAX_SNAPSHOT_BYTES)
        {
            let gone = self.snapshots.remove(0);
            self.proposals.retain(|p| p.snapshot_id != gone.snapshot_id);
        }
        self.snapshots.last().expect("just pushed")
    }

    pub fn snapshot(&self, id: &str) -> Option<&Snapshot> {
        self.snapshots.iter().find(|s| s.snapshot_id == id)
    }

    pub fn snapshot_count(&self) -> usize {
        self.snapshots.len()
    }

    /// Forgets a snapshot and every proposal made from it (source text must not linger, DD section 11).
    pub fn release_snapshot(&mut self, id: &str) -> bool {
        let before = self.snapshots.len();
        self.snapshots.retain(|s| s.snapshot_id != id);
        self.proposals.retain(|p| p.snapshot_id != id);
        self.snapshots.len() != before
    }

    pub fn store_proposal(&mut self, p: Proposal) {
        self.proposals.push(p);
        while self.proposals.len() > MAX_PROPOSALS {
            self.proposals.remove(0);
        }
    }

    pub fn proposal(&self, id: &str) -> Option<&Proposal> {
        self.proposals.iter().find(|p| p.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gen(seed: u8) -> IdGen {
        let entropy: Vec<u8> = (0..32u8)
            .map(|i| i.wrapping_mul(5).wrapping_add(seed))
            .collect();
        IdGen::new(&entropy).unwrap()
    }

    #[test]
    fn sessions_expire_when_idle_and_are_refreshed_by_use() {
        let mut store = SessionStore::new();
        let id = store
            .open(1000, gen(1), "en-US".into(), Session::default_settings())
            .unwrap()
            .id
            .clone();
        assert!(
            store.get(1000 + SESSION_IDLE_MS, &id).is_some(),
            "refreshed"
        );
        assert!(store.get(1000 + 2 * SESSION_IDLE_MS, &id).is_some());
        assert!(
            store.get(1000 + 4 * SESSION_IDLE_MS, &id).is_none(),
            "idle too long"
        );
        assert!(store.is_empty());
    }

    #[test]
    fn the_least_recently_used_session_is_evicted() {
        let mut store = SessionStore::new();
        let mut first = String::new();
        for i in 0..MAX_SESSIONS {
            let id = store
                .open(10 + i as u64, gen(i as u8 + 1), "en-US".into(), Json::Null)
                .unwrap()
                .id
                .clone();
            if i == 0 {
                first = id;
            }
        }
        assert_eq!(store.len(), MAX_SESSIONS);
        store
            .open(500, gen(100), "en-US".into(), Json::Null)
            .unwrap();
        assert_eq!(store.len(), MAX_SESSIONS);
        assert!(store.get(501, &first).is_none(), "the oldest one is gone");
    }

    #[test]
    fn reusing_entropy_is_refused_and_close_removes_the_session() {
        let mut store = SessionStore::new();
        let id = store
            .open(1, gen(3), "en-US".into(), Json::Null)
            .unwrap()
            .id
            .clone();
        assert_eq!(
            store.open(2, gen(3), "en-US".into(), Json::Null).err(),
            Some(OpenError::EntropyReused)
        );
        assert!(store.close(&id));
        assert!(!store.close(&id));
        assert!(store.open(3, gen(3), "en-US".into(), Json::Null).is_ok());
    }
}
