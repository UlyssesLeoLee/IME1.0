//! The workspaces of the Hub: who may write into which file, and what is the state of the file.
//!
//! The Hub is the one that arbitrates the writes of the agents (detailed design §17.4): two agents
//! that work on the same file must not write it at the same time, and a writer that comes back later
//! must learn that the file moved under it.
//!
//! Three rules the module keeps:
//!
//! * **one writer per file.** The second agent that asks for the write lock of a file that is already
//!   locked is *refused*, not queued: the Hub holds no wait queue, so the caller is told at once
//!   ([`crate::telemetry::HubEvent::LockDenied`] records the refusal) and may try again itself.
//! * **a lock is a lease.** It runs out; a process that crashes leaves no deadlock behind, because the
//!   next `acquire` takes the locks that ran out (detailed design §17.6).
//! * **a generation only grows.** Every file of a workspace has a generation; a writer that expects the
//!   generation it read and finds another one is in conflict ([`WorkspaceRegistry::conflicts`]).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tase_contract::{AgentId, SessionId, TaseError};

/// The source of the time of the Hub.
///
/// The Hub never reads the clock itself: the caller injects one. A test injects a [`ManualClock`] and
/// drives the time by hand, so that the leases of the tests are exact.
pub trait Clock: Send + Sync {
    /// The time of the caller, in milliseconds since the epoch.
    fn now_ms(&self) -> u64;
}

/// A clock that the caller drives.
///
/// The clones of the clock share the time: the Hub and its registry must say the same time.
#[derive(Clone, Debug)]
pub struct ManualClock {
    now: Arc<AtomicU64>,
}

impl ManualClock {
    /// A clock that says `now_ms`.
    #[must_use]
    pub fn new(now_ms: u64) -> ManualClock {
        ManualClock {
            now: Arc::new(AtomicU64::new(now_ms)),
        }
    }

    /// Sets the time of the clock.
    pub fn set(&self, now_ms: u64) {
        self.now.store(now_ms, Ordering::SeqCst);
    }

    /// Moves the time of the clock forward.
    pub fn advance(&self, delta_ms: u64) {
        self.now.fetch_add(delta_ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
}

/// The clock of the machine.
///
/// It is the default of the binary and of a Hub that nobody gave a clock to; the logic of the Hub never
/// calls it by itself.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0)
    }
}

/// A write lock on a file of a workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileLock {
    /// The workspace the file belongs to.
    pub workspace: String,
    /// The path of the file, normalized.
    pub path: String,
    /// The agent that holds the lock.
    pub agent: AgentId,
    /// The session of the agent, when the lock was taken with one.
    pub session: Option<SessionId>,
    /// The generation of the file when the lock was taken.
    pub generation: u64,
    /// The time the lock was taken.
    pub acquired_at_ms: u64,
    /// The time the lock runs out.
    pub expires_at_ms: u64,
}

impl FileLock {
    /// Whether the lock is held and has not run out of time yet.
    #[must_use]
    pub fn is_valid_at(&self, now_ms: u64) -> bool {
        self.expires_at_ms > now_ms
    }
}

/// The workspaces of the Hub: the write locks and the generation of every file.
pub struct WorkspaceRegistry {
    locks: HashMap<(String, String), FileLock>,
    generations: HashMap<(String, String), u64>,
    lease_ttl_ms: u64,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for WorkspaceRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceRegistry")
            .field("locks", &self.locks.len())
            .field("generations", &self.generations.len())
            .field("lease_ttl_ms", &self.lease_ttl_ms)
            .finish()
    }
}

impl WorkspaceRegistry {
    /// A registry with a clock and the time a lock lives.
    ///
    /// The clock is shared: the Hub and the registry must say the same time.
    #[must_use]
    pub fn new(clock: Arc<dyn Clock>, lease_ttl_ms: u64) -> WorkspaceRegistry {
        WorkspaceRegistry {
            locks: HashMap::new(),
            generations: HashMap::new(),
            lease_ttl_ms,
            clock,
        }
    }

    /// How long a lock lives.
    #[must_use]
    pub fn lease_ttl_ms(&self) -> u64 {
        self.lease_ttl_ms
    }

    /// Takes the write lock of a file for an agent.
    ///
    /// The agent that already holds the lock gets it again (the lease is renewed): a lock is not a
    /// fight with oneself. Another agent is refused with [`ErrorCode::SymbolConflict`]: the Hub does not
    /// hold a wait queue, and a silent queue would hide the conflict from the caller.
    pub fn acquire(
        &mut self,
        workspace: &str,
        path: &str,
        agent: &AgentId,
    ) -> Result<FileLock, TaseError> {
        self.acquire_at(workspace, path, agent, None, self.clock.now_ms())
    }

    /// Takes the write lock of a file for an agent, at a given time and for a given session.
    pub fn acquire_at(
        &mut self,
        workspace: &str,
        path: &str,
        agent: &AgentId,
        session: Option<&SessionId>,
        now_ms: u64,
    ) -> Result<FileLock, TaseError> {
        let workspace = normalize_workspace(workspace)?;
        let path = normalize_path(path)?;
        // the locks that ran out are taken back first: a crashed agent leaves no deadlock behind
        self.expire_leases(now_ms);
        let key = (workspace.clone(), path.clone());
        if let Some(lock) = self.locks.get(&key) {
            if &lock.agent == agent {
                // the same agent comes back: the lock is renewed, not refused
                let mut renewed = lock.clone();
                renewed.expires_at_ms = now_ms.saturating_add(self.lease_ttl_ms);
                self.locks.insert(key, renewed.clone());
                return Ok(renewed);
            }
            return Err(TaseError::symbol_conflict(format!(
                "the file {path} of the workspace {workspace} is locked by {}",
                lock.agent.as_str()
            )));
        }
        let lock = FileLock {
            workspace,
            path,
            agent: agent.clone(),
            session: session.cloned(),
            generation: self.generation_of(&key.0, &key.1),
            acquired_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(self.lease_ttl_ms),
        };
        self.locks.insert(key, lock.clone());
        Ok(lock)
    }

    /// Gives the write lock of a file back.
    pub fn release(
        &mut self,
        workspace: &str,
        path: &str,
        agent: &AgentId,
    ) -> Result<(), TaseError> {
        let workspace = normalize_workspace(workspace)?;
        let path = normalize_path(path)?;
        let key = (workspace, path);
        match self.locks.get(&key) {
            Some(lock) if &lock.agent == agent => {
                self.locks.remove(&key);
                Ok(())
            }
            Some(lock) => Err(TaseError::symbol_conflict(format!(
                "the file {} is locked by {}, not by {}",
                key.1,
                lock.agent.as_str(),
                agent.as_str()
            ))),
            None => Err(TaseError::not_found(format!(
                "the file {} of the workspace {} is not locked",
                key.1, key.0
            ))),
        }
    }

    /// Gives every lock of a session back; the session is gone.
    pub fn release_session(&mut self, session: &SessionId) -> Vec<FileLock> {
        let mut released = Vec::new();
        let keys: Vec<(String, String)> = self.locks.keys().cloned().collect();
        for key in keys {
            let matches = self
                .locks
                .get(&key)
                .and_then(|lock| lock.session.as_ref())
                .map(|owner| owner == session)
                .unwrap_or(false);
            if matches
                && let Some(lock) = self.locks.remove(&key)
            {
                released.push(lock);
            }
        }
        released
    }

    /// Gives every lock of an agent back.
    pub fn release_agent(&mut self, agent: &AgentId) -> Vec<FileLock> {
        let mut released = Vec::new();
        let keys: Vec<(String, String)> = self.locks.keys().cloned().collect();
        for key in keys {
            let matches = self
                .locks
                .get(&key)
                .map(|lock| &lock.agent == agent)
                .unwrap_or(false);
            if matches {
                if let Some(lock) = self.locks.remove(&key) {
                    released.push(lock);
                }
            }
        }
        released
    }

    /// Whether the file moved under the writer: the generation it expects is not the one it has.
    ///
    /// A file that was never written has the generation 0; a writer that expects 0 and finds 0 is in
    /// conflict with nobody.
    #[must_use]
    pub fn conflicts(&self, workspace: &str, path: &str, generation: u64) -> bool {
        self.generation_of(workspace, path) != generation
    }

    /// The generation of a file.
    #[must_use]
    pub fn generation(&self, workspace: &str, path: &str) -> u64 {
        self.generation_of(workspace, path)
    }

    fn generation_of(&self, workspace: &str, path: &str) -> u64 {
        let workspace = workspace.trim();
        let path = normalize_path(path).unwrap_or_else(|_| path.trim().to_string());
        self.generations
            .get(&(workspace.to_string(), path))
            .copied()
            .unwrap_or(0)
    }

    /// Moves the generation of a file forward and gives the new one back.
    pub fn bump_generation(&mut self, workspace: &str, path: &str) -> u64 {
        let workspace = workspace.trim().to_string();
        let path = normalize_path(path).unwrap_or_else(|_| path.trim().to_string());
        let key = (workspace, path);
        let generation = self.generations.entry(key).or_insert(0);
        *generation += 1;
        *generation
    }

    /// Sets the generation of a file; it never goes backwards.
    pub fn set_generation(&mut self, workspace: &str, path: &str, generation: u64) -> u64 {
        let workspace = workspace.trim().to_string();
        let path = normalize_path(path).unwrap_or_else(|_| path.trim().to_string());
        let key = (workspace, path);
        let slot = self.generations.entry(key).or_insert(0);
        if generation > *slot {
            *slot = generation;
        }
        *slot
    }

    /// Takes the locks that ran out of time back, and gives them back.
    pub fn expire_leases(&mut self, now_ms: u64) -> Vec<FileLock> {
        let keys: Vec<(String, String)> = self.locks.keys().cloned().collect();
        let mut expired = Vec::new();
        for key in keys {
            let stale = self
                .locks
                .get(&key)
                .map(|lock| !lock.is_valid_at(now_ms))
                .unwrap_or(false);
            if stale {
                if let Some(lock) = self.locks.remove(&key) {
                    expired.push(lock);
                }
            }
        }
        expired
    }

    /// The lock of a file, when there is one.
    #[must_use]
    pub fn lock_of(&self, workspace: &str, path: &str) -> Option<&FileLock> {
        let workspace = workspace.trim();
        let path = normalize_path(path).unwrap_or_else(|_| path.trim().to_string());
        self.locks.get(&(workspace.to_string(), path))
    }

    /// Every lock that is held.
    #[must_use]
    pub fn locks(&self) -> Vec<FileLock> {
        let mut locks: Vec<FileLock> = self.locks.values().cloned().collect();
        locks.sort_by(|left, right| {
            (&left.workspace, &left.path).cmp(&(&right.workspace, &right.path))
        });
        locks
    }

    /// The number of the locks that are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.locks.len()
    }

    /// Whether no lock is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.locks.is_empty()
    }
}

/// The name of a workspace; it may not be empty.
fn normalize_workspace(workspace: &str) -> Result<String, TaseError> {
    let trimmed = workspace.trim();
    if trimmed.is_empty() {
        return Err(TaseError::invalid_argument("the workspace has no name"));
    }
    if trimmed.len() > 256 {
        return Err(TaseError::invalid_argument("the name of the workspace is too long"));
    }
    Ok(trimmed.to_string())
}

/// The path of a file in a workspace, normalized.
///
/// The path of a file is relative to the root of the workspace: it may not be absolute and it may not
/// climb out of it (detailed design §17.2 refuses a path that traverses out of the workspace).
pub fn normalize_path(path: &str) -> Result<String, TaseError> {
    let trimmed = path.trim().replace('\\', "/");
    if trimmed.is_empty() {
        return Err(TaseError::invalid_argument("the file has no path"));
    }
    if trimmed.starts_with('/') {
        return Err(TaseError::invalid_argument(
            "the path of a file is relative to the workspace",
        ));
    }
    if let Some(drive) = trimmed.get(1..2) {
        if drive == ":" {
            return Err(TaseError::invalid_argument(
                "the path of a file is relative to the workspace",
            ));
        }
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in trimmed.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                return Err(TaseError::invalid_argument(
                    "the path of a file may not climb out of the workspace",
                ));
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        return Err(TaseError::invalid_argument("the file has no path"));
    }
    Ok(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tase_contract::{ErrorCode, SessionId};

    fn agent(name: &str) -> AgentId {
        AgentId::new(format!("agent_{name}")).expect("a legal agent id")
    }

    fn session(name: &str) -> SessionId {
        SessionId::new(format!("sess_{name}")).expect("a legal session id")
    }

    fn registry() -> (WorkspaceRegistry, ManualClock) {
        let clock = ManualClock::new(1000);
        let registry = WorkspaceRegistry::new(Arc::new(clock.clone()), 500);
        (registry, clock)
    }

    #[test]
    fn an_agent_takes_the_write_lock_of_a_file() {
        let (mut registry, _clock) = registry();
        let lock = registry
            .acquire("ws", "src/main.rs", &agent("one"))
            .expect("the lock is taken");
        assert_eq!(lock.workspace, "ws");
        assert_eq!(lock.path, "src/main.rs");
        assert_eq!(lock.generation, 0);
        assert_eq!(lock.acquired_at_ms, 1000);
        assert_eq!(lock.expires_at_ms, 1500);
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn the_second_agent_of_a_file_is_refused() {
        let (mut registry, _clock) = registry();
        registry
            .acquire("ws", "src/main.rs", &agent("one"))
            .expect("the lock is taken");
        let error = registry
            .acquire("ws", "src/main.rs", &agent("two"))
            .expect_err("the lock is refused");
        assert_eq!(error.code, ErrorCode::SymbolConflict);
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn the_same_agent_gets_its_lock_again() {
        let (mut registry, clock) = registry();
        registry
            .acquire("ws", "a.rs", &agent("one"))
            .expect("the lock is taken");
        clock.advance(100);
        let lock = registry
            .acquire("ws", "a.rs", &agent("one"))
            .expect("the lock is given again");
        // the lock keeps the time it was taken and gets a new lease
        assert_eq!(lock.acquired_at_ms, 1000);
        assert_eq!(lock.expires_at_ms, 1600);
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn a_lock_that_ran_out_of_time_is_taken_back() {
        let (mut registry, clock) = registry();
        registry
            .acquire("ws", "a.rs", &agent("one"))
            .expect("the lock is taken");
        clock.advance(600);
        let lock = registry
            .acquire("ws", "a.rs", &agent("two"))
            .expect("the lock is taken again");
        assert_eq!(lock.agent, agent("two"));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn the_expired_locks_are_given_back() {
        let (mut registry, clock) = registry();
        registry
            .acquire("ws", "a.rs", &agent("one"))
            .expect("the lock is taken");
        clock.advance(600);
        let expired = registry.expire_leases(clock.now_ms());
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].agent, agent("one"));
        assert!(registry.is_empty());
    }

    #[test]
    fn a_lock_is_given_back_on_request() {
        let (mut registry, _clock) = registry();
        registry
            .acquire("ws", "a.rs", &agent("one"))
            .expect("the lock is taken");
        registry
            .release("ws", "a.rs", &agent("one"))
            .expect("the lock is released");
        assert!(registry.is_empty());
        registry
            .acquire("ws", "a.rs", &agent("two"))
            .expect("the lock is taken again");
    }

    #[test]
    fn another_agent_may_not_release_a_lock() {
        let (mut registry, _clock) = registry();
        registry
            .acquire("ws", "a.rs", &agent("one"))
            .expect("the lock is taken");
        let error = registry
            .release("ws", "a.rs", &agent("two"))
            .expect_err("the release is refused");
        assert_eq!(error.code, ErrorCode::SymbolConflict);
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn releasing_a_lock_that_is_not_held_is_refused() {
        let (mut registry, _clock) = registry();
        let error = registry
            .release("ws", "a.rs", &agent("one"))
            .expect_err("the release is refused");
        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn the_locks_of_a_session_go_with_the_session() {
        let (mut registry, _clock) = registry();
        registry
            .acquire_at("ws", "a.rs", &agent("one"), Some(&session("one")), 1000)
            .expect("the lock is taken");
        registry
            .acquire_at("ws", "b.rs", &agent("two"), Some(&session("two")), 1000)
            .expect("the second lock is taken");
        let released = registry.release_session(&session("one"));
        assert_eq!(released.len(), 1);
        assert_eq!(released[0].path, "a.rs");
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn the_locks_of_an_agent_go_with_the_agent() {
        let (mut registry, _clock) = registry();
        registry
            .acquire("ws", "a.rs", &agent("one"))
            .expect("the lock is taken");
        registry
            .acquire("ws", "b.rs", &agent("one"))
            .expect("the second lock is taken");
        assert_eq!(registry.release_agent(&agent("one")).len(), 2);
        assert!(registry.is_empty());
    }

    #[test]
    fn a_file_that_moved_under_the_writer_is_a_conflict() {
        let (mut registry, _clock) = registry();
        assert!(!registry.conflicts("ws", "a.rs", 0));
        let generation = registry.bump_generation("ws", "a.rs");
        assert_eq!(generation, 1);
        assert!(registry.conflicts("ws", "a.rs", 0));
        assert!(!registry.conflicts("ws", "a.rs", 1));
    }

    #[test]
    fn the_generation_only_grows() {
        let (mut registry, _clock) = registry();
        assert_eq!(registry.bump_generation("ws", "a.rs"), 1);
        assert_eq!(registry.bump_generation("ws", "a.rs"), 2);
        assert_eq!(registry.set_generation("ws", "a.rs", 5), 5);
        assert_eq!(registry.set_generation("ws", "a.rs", 1), 5);
        assert_eq!(registry.generation("ws", "a.rs"), 5);
    }

    #[test]
    fn the_path_of_a_file_is_normalized() {
        assert_eq!(normalize_path("src\\main.rs").expect("a legal path"), "src/main.rs");
        assert_eq!(normalize_path("./src//main.rs").expect("a legal path"), "src/main.rs");
        assert_eq!(normalize_path("src/main.rs").expect("a legal path"), "src/main.rs");
    }

    #[test]
    fn a_path_that_climbs_out_of_the_workspace_is_refused() {
        for bad in ["../etc/passwd", "src/../../etc", "/etc/passwd", "C:/windows", ""] {
            let error = normalize_path(bad).expect_err("the path is refused");
            assert_eq!(error.code, ErrorCode::InvalidArgument, "{bad}");
        }
    }

    #[test]
    fn a_workspace_without_a_name_is_refused() {
        let (mut registry, _clock) = registry();
        let error = registry
            .acquire("  ", "a.rs", &agent("one"))
            .expect_err("the workspace is refused");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn the_lock_of_a_file_can_be_looked_at() {
        let (mut registry, _clock) = registry();
        registry
            .acquire("ws", "a.rs", &agent("one"))
            .expect("the lock is taken");
        let lock = registry.lock_of("ws", "a.rs").expect("the lock is there");
        assert_eq!(lock.agent, agent("one"));
        assert!(lock.is_valid_at(1000));
        assert!(!lock.is_valid_at(5000));
        assert!(registry.lock_of("ws", "b.rs").is_none());
    }

    #[test]
    fn the_registry_knows_how_long_a_lock_lives() {
        let (registry, _clock) = registry();
        assert_eq!(registry.lease_ttl_ms(), 500);
    }
}
