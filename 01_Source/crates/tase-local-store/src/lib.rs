//! The persistence layer of the TASE Local Agent Hub: the sessions of the agents, the jobs, the leases, the
//! idempotency records and the generation of the files, plus the migrations, the snapshots and the reconciliation
//! after a crash (detailed design §18, §17.4, §17.6).
//!
//! The store is SQLite in WAL mode (detailed design §18.1). SQLite is public domain and `rusqlite` is MIT, so the
//! dependency is clean; the SQLite amalgamation is compiled into the library, so the crate needs no system SQLite
//! and builds the same on every target.
//!
//! What the crate keeps:
//!
//! * **the sessions** of the agents (the provider, the instance, the granted scopes, the capabilities, the state and
//!   the time of the last heartbeat);
//! * **the jobs** with their state, their number of attempts, their idempotency key and their digest;
//! * **the leases**, with one holder at a time and a fencing token that only grows;
//! * **the idempotency records**: the same key with the same digest gives the same job, the same key with another
//!   digest is a conflict (detailed design §2.5);
//! * **the generation of every file of a workspace**, so that two agents that write the same file are told that
//!   they conflict;
//! * **the journal of the changes** and the snapshots of the state, so that the state can be rebuilt.
//!
//! What the crate does not keep: the text of the files, the tokens of the models and the cost. Those belong to the
//! workspace, to the metrics and to the catalog.
//!
//! The crate never reads the clock: every timestamp comes from the caller, so a test is deterministic and a
//! recovery can be replayed. The only exception is the bookkeeping of the migrations, which says when a step ran.

#![forbid(unsafe_code)]

pub mod migrations;
pub mod reconcile;
pub mod snapshots;
pub mod sqlite;

use std::time::{SystemTime, UNIX_EPOCH};

use pime_core::json::{Json, Limits, parse};
use rusqlite::Connection;
use tase_contract::{AgentId, ErrorCode, JobId, SessionId, TaseError, WorkspaceId};

pub use reconcile::{ReconcileOptions, ReconcileReport, reconcile};
pub use snapshots::{RestoreReport, Snapshot, SnapshotInfo};
pub use sqlite::{JournalMode, Store, StoreOptions, Synchronous};

/// The current time in milliseconds since the epoch.
///
/// The state of the store never uses it: every timestamp of a row comes from the caller. Only the bookkeeping of the
/// migrations uses it, to say when a step ran.
#[must_use]
pub fn clock_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------------------------
// The states
// ---------------------------------------------------------------------------------------------

/// The state of a session of an agent (detailed design §18.1, `local_agent_sessions.state`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    /// The agent is connected and its heartbeats arrive.
    Connected,
    /// The agent left on its own.
    Disconnected,
    /// The agent stopped sending heartbeats and the recovery gave up on it.
    Expired,
}

impl SessionState {
    /// The name of the state, as the design book writes it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            SessionState::Connected => "CONNECTED",
            SessionState::Disconnected => "DISCONNECTED",
            SessionState::Expired => "EXPIRED",
        }
    }

    /// The state of a name, or `None` when the name is not one of the three.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "CONNECTED" => Some(SessionState::Connected),
            "DISCONNECTED" => Some(SessionState::Disconnected),
            "EXPIRED" => Some(SessionState::Expired),
            _ => None,
        }
    }
}

/// The state of a job (detailed design §6.3, §18.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobStatus {
    /// The job waits for a worker.
    Queued,
    /// A worker holds the lease of the job and works on it.
    Running,
    /// The worker wrote the candidate and the checks run.
    Verifying,
    /// The job is done and the commit is on the ref.
    Committed,
    /// The job failed.
    Failed,
    /// The job was cancelled.
    Cancelled,
    /// The job met a conflict: the file or the symbol changed under it.
    Conflict,
}

impl JobStatus {
    /// The name of the state, as the design book writes it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            JobStatus::Queued => "QUEUED",
            JobStatus::Running => "RUNNING",
            JobStatus::Verifying => "VERIFYING",
            JobStatus::Committed => "COMMITTED",
            JobStatus::Failed => "FAILED",
            JobStatus::Cancelled => "CANCELLED",
            JobStatus::Conflict => "CONFLICT",
        }
    }

    /// The state of a name, or `None` when the name is not a state of a job.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "QUEUED" => Some(JobStatus::Queued),
            "RUNNING" => Some(JobStatus::Running),
            "VERIFYING" => Some(JobStatus::Verifying),
            "COMMITTED" => Some(JobStatus::Committed),
            "FAILED" => Some(JobStatus::Failed),
            "CANCELLED" => Some(JobStatus::Cancelled),
            "CONFLICT" => Some(JobStatus::Conflict),
            _ => None,
        }
    }

    /// Whether the job is in a state it may not leave (detailed design §6.3).
    ///
    /// The same notification for a job that is already in a terminal state may be accepted again, but a job in a
    /// terminal state is never run again: a retry is a new job.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            JobStatus::Committed | JobStatus::Failed | JobStatus::Cancelled | JobStatus::Conflict
        )
    }

    /// The states a job may be in while it is not finished.
    #[must_use]
    pub fn is_active(self) -> bool {
        matches!(self, JobStatus::Running | JobStatus::Verifying)
    }
}

/// The state of a lease.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseState {
    /// The lease is held by its holder.
    Held,
    /// The holder gave the lease back on its own.
    Released,
    /// The lease ran out of time.
    Expired,
}

impl LeaseState {
    /// The name of the state, as the design book writes it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            LeaseState::Held => "HELD",
            LeaseState::Released => "RELEASED",
            LeaseState::Expired => "EXPIRED",
        }
    }

    /// The state of a name, or `None` when the name is not a state of a lease.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "HELD" => Some(LeaseState::Held),
            "RELEASED" => Some(LeaseState::Released),
            "EXPIRED" => Some(LeaseState::Expired),
            _ => None,
        }
    }
}

/// The state of a conflict of a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictState {
    /// The conflict was seen and not yet judged.
    Open,
    /// The reconciliation saw that the file really moved on.
    Confirmed,
    /// The file came back to the generation the writer expected.
    Resolved,
}

impl ConflictState {
    /// The name of the state, as the store writes it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ConflictState::Open => "OPEN",
            ConflictState::Confirmed => "CONFIRMED",
            ConflictState::Resolved => "RESOLVED",
        }
    }

    /// The state of a name, or `None` when the name is not a state of a conflict.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "OPEN" => Some(ConflictState::Open),
            "CONFIRMED" => Some(ConflictState::Confirmed),
            "RESOLVED" => Some(ConflictState::Resolved),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The rows
// ---------------------------------------------------------------------------------------------

/// The registration of an agent: a session of the hub (detailed design §18.1, `local_agent_sessions`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentRegistration {
    /// The id of the session.
    pub session_id: SessionId,
    /// The id of the agent.
    pub agent_id: AgentId,
    /// The provider of the model of the agent (`claude`, `codex`, `local`, ...).
    pub provider: String,
    /// The instance of the agent, when one provider runs several.
    pub instance_id: String,
    /// The workspace the agent works in.
    pub workspace_id: WorkspaceId,
    /// The hash of the logon of the owner: an audit group, never a proof of an identity.
    pub owner_logon_sid_hash: String,
    /// The scopes the session was given.
    pub granted_scopes: Vec<String>,
    /// The capabilities the agent says it has.
    pub capabilities: Vec<String>,
    /// The state of the session.
    pub state: SessionState,
    /// The epoch of the session: it grows when the session is taken again.
    pub lease_epoch: u64,
    /// The time of the last heartbeat.
    pub heartbeat_at_ms: u64,
    /// The time the session was opened.
    pub created_at_ms: u64,
}

/// A job of the hub (detailed design §18.1, `local_jobs`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    /// The id of the job.
    pub job_id: JobId,
    /// The kind of the job (`apply`, `preview`, `verify`, ...).
    pub job_type: String,
    /// The workspace the job works in.
    pub workspace_id: WorkspaceId,
    /// The agent that asked for the job.
    pub agent_id: AgentId,
    /// The session that asked for the job.
    pub session_id: SessionId,
    /// The idempotency key of the job.
    pub idempotency_key: String,
    /// The digest of the request.
    pub request_digest: String,
    /// The digest of the base the job works on.
    pub base_snapshot_digest: String,
    /// The state of the job.
    pub status: JobStatus,
    /// The number of times the job was tried.
    pub attempt: u32,
    /// The epoch of the lease of the job: it grows at every claim.
    pub lease_epoch: u64,
    /// The worker that holds the lease of the job.
    pub worker_id: Option<String>,
    /// The digest of what the job produced.
    pub result_digest: Option<String>,
    /// The digest of the intent to commit.
    pub commit_intent_digest: Option<String>,
    /// The ref the job pushed.
    pub committed_ref: Option<String>,
    /// The commit the job made.
    pub committed_oid: Option<String>,
    /// The business code of the failure of the job.
    pub error_code: Option<ErrorCode>,
    /// The priority of the job inside the queue of its agent.
    pub priority: u8,
    /// The time the job was registered.
    pub created_at_ms: u64,
    /// The time the job was touched the last time.
    pub updated_at_ms: u64,
}

/// A lease of a resource (a job, a branch, a file): one holder at a time, with a fencing token that only grows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lease {
    /// The id of the lease.
    pub lease_id: String,
    /// What is leased: the id of a job, of a branch, ...
    pub resource: String,
    /// The holder of the lease.
    pub holder: String,
    /// The session of the holder.
    pub session_id: Option<SessionId>,
    /// The fencing token of the lease: it only grows, so a holder that comes back late is told it is old.
    pub fencing_token: u64,
    /// The time the lease was taken.
    pub acquired_at_ms: u64,
    /// The time the lease runs out.
    pub expires_at_ms: u64,
    /// The time the lease was given back.
    pub released_at_ms: Option<u64>,
    /// The state of the lease.
    pub state: LeaseState,
}

impl Lease {
    /// Whether the lease is held and has not run out of time yet.
    #[must_use]
    pub fn is_valid_at(&self, now_ms: u64) -> bool {
        self.state == LeaseState::Held && self.expires_at_ms > now_ms
    }
}

/// The record of an idempotency key (detailed design §2.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdempotencyRecord {
    /// The scope of the key: the workspace and the agent.
    pub scope: String,
    /// The key itself.
    pub idempotency_key: String,
    /// The digest of the request that took the key.
    pub request_digest: String,
    /// The digest of the answer the key gave.
    pub response_digest: String,
    /// The job the key belongs to.
    pub job_id: JobId,
    /// The time the key was taken.
    pub created_at_ms: u64,
}

/// The generation of a file of a workspace (detailed design §16.4, §17.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileGeneration {
    /// The workspace the file belongs to.
    pub workspace_id: WorkspaceId,
    /// The path of the file, normalized.
    pub normalized_path: String,
    /// The generation of the file: it only grows.
    pub generation: u64,
    /// The digest of the content of the file.
    pub content_digest: String,
    /// The time the generation was written.
    pub updated_at_ms: u64,
}

/// A conflict of a file: two writers, one file, two generations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileConflict {
    /// The id of the conflict.
    pub conflict_id: u64,
    /// The workspace the file belongs to.
    pub workspace_id: WorkspaceId,
    /// The path of the file.
    pub normalized_path: String,
    /// The generation the writer expected.
    pub expected_generation: u64,
    /// The generation the file really had.
    pub actual_generation: u64,
    /// The agent that met the conflict.
    pub agent_id: AgentId,
    /// The session of that agent.
    pub session_id: SessionId,
    /// The job that met the conflict.
    pub job_id: Option<JobId>,
    /// The state of the conflict.
    pub state: ConflictState,
    /// The time the conflict was seen.
    pub detected_at_ms: u64,
}

/// An event of the outbox: a notification that must be sent again when it was not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboxEvent {
    /// The id of the event.
    pub event_id: i64,
    /// The job the event belongs to.
    pub job_id: JobId,
    /// The digest of the payload of the event.
    pub payload_digest: String,
    /// The time the event was published.
    pub published_at_ms: Option<u64>,
}

/// A line of the audit trail (detailed design §18.1, `local_audit_events`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditEvent {
    /// The sequence number of the event.
    pub seq: i64,
    /// The job the event belongs to.
    pub job_id: Option<String>,
    /// The agent the event belongs to.
    pub agent_id: String,
    /// The session the event belongs to.
    pub session_id: String,
    /// The workspace the event belongs to.
    pub workspace_id: String,
    /// The kind of the event (`RECOVERED`, `ORPHANED`, ...).
    pub event_type: String,
    /// The digest of the payload of the event.
    pub payload_digest: String,
    /// The time of the event.
    pub created_at_ms: u64,
}

/// A line of the audit trail to write (detailed design §18.1, `local_audit_events`).
#[derive(Clone, Debug)]
pub struct AuditRecord {
    /// The kind of the event (`RECOVERED`, `ORPHANED`, ...).
    pub event_type: String,
    /// The agent the event belongs to.
    pub agent_id: String,
    /// The session the event belongs to.
    pub session_id: String,
    /// The workspace the event belongs to.
    pub workspace_id: String,
    /// The job the event belongs to.
    pub job_id: Option<String>,
    /// The digest of the payload of the event.
    pub payload_digest: String,
    /// The time of the event.
    pub now_ms: u64,
}

/// The answer of the registration of a job (detailed design §2.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobSubmission {
    /// The id of the job.
    pub job_id: JobId,
    /// Whether the job was already known: the same key with the same digest gives the same job again.
    pub deduplicated: bool,
    /// The state of the job.
    pub status: JobStatus,
}

/// The session to open.
#[derive(Clone, Debug)]
pub struct NewSession {
    /// The id of the session.
    pub session_id: SessionId,
    /// The id of the agent.
    pub agent_id: AgentId,
    /// The provider of the model of the agent.
    pub provider: String,
    /// The instance of the agent.
    pub instance_id: String,
    /// The workspace the agent works in.
    pub workspace_id: WorkspaceId,
    /// The hash of the logon of the owner.
    pub owner_logon_sid_hash: String,
    /// The scopes the session is given.
    pub granted_scopes: Vec<String>,
    /// The capabilities the agent says it has.
    pub capabilities: Vec<String>,
    /// The time of the call.
    pub now_ms: u64,
}

/// The job to register.
#[derive(Clone, Debug)]
pub struct NewJob {
    /// The kind of the job.
    pub job_type: String,
    /// The workspace the job works in.
    pub workspace_id: WorkspaceId,
    /// The agent that asks.
    pub agent_id: AgentId,
    /// The session that asks.
    pub session_id: SessionId,
    /// The idempotency key of the job.
    pub idempotency_key: String,
    /// The digest of the request.
    pub request_digest: String,
    /// The digest of the base the job works on.
    pub base_snapshot_digest: String,
    /// The priority of the job.
    pub priority: u8,
    /// The time of the call.
    pub now_ms: u64,
}

/// The lease to take.
#[derive(Clone, Debug)]
pub struct NewLease {
    /// What is leased.
    pub resource: String,
    /// The holder of the lease.
    pub holder: String,
    /// The session of the holder.
    pub session_id: Option<SessionId>,
    /// How long the lease is good for, in milliseconds.
    pub ttl_ms: u64,
    /// The time of the call.
    pub now_ms: u64,
}

/// The write of a file of a workspace.
#[derive(Clone, Debug)]
pub struct NewFileTransaction {
    /// The workspace the file belongs to.
    pub workspace_id: WorkspaceId,
    /// The path of the file, normalized.
    pub normalized_path: String,
    /// The generation the writer expects, or `None` when the writer does not care.
    pub expected_generation: Option<u64>,
    /// The digest of the new content of the file.
    pub content_digest: String,
    /// The agent that writes.
    pub agent_id: AgentId,
    /// The session of that agent.
    pub session_id: SessionId,
    /// The job the write belongs to.
    pub job_id: Option<JobId>,
    /// The time of the call.
    pub now_ms: u64,
}

// ---------------------------------------------------------------------------------------------
// The operations
// ---------------------------------------------------------------------------------------------

impl Store {
    // -- the sessions ------------------------------------------------------------------------

    /// Opens a session of an agent.
    ///
    /// The session starts `CONNECTED` with the time of the call as its first heartbeat. A session id that is already
    /// known is refused: the hub must not take a session over without saying so.
    pub fn open_session(&mut self, req: NewSession) -> Result<AgentRegistration, TaseError> {
        let tx = self.transaction()?;
        if row_session(&tx, req.session_id.as_str())?.is_some() {
            return Err(TaseError::invalid_argument(format!(
                "the session {} is already registered",
                req.session_id.as_str()
            )));
        }
        let row = AgentRegistration {
            session_id: req.session_id.clone(),
            agent_id: req.agent_id.clone(),
            provider: req.provider,
            instance_id: req.instance_id,
            workspace_id: req.workspace_id.clone(),
            owner_logon_sid_hash: req.owner_logon_sid_hash,
            granted_scopes: req.granted_scopes,
            capabilities: req.capabilities,
            state: SessionState::Connected,
            lease_epoch: 0,
            heartbeat_at_ms: req.now_ms,
            created_at_ms: req.now_ms,
        };
        upsert_session(&tx, &row)?;
        journal_session(&tx, &row, req.now_ms)?;
        tx.commit()?;
        Ok(row)
    }

    /// Records the heartbeat of a session.
    ///
    /// A session that is not known is `NOT_FOUND`; a session that already left is `UNAUTHENTICATED`: its holder
    /// must open a new one.
    pub fn heartbeat(&mut self, session_id: &SessionId, now_ms: u64) -> Result<(), TaseError> {
        let tx = self.transaction()?;
        let mut row = row_session(&tx, session_id.as_str())?.ok_or_else(|| {
            TaseError::not_found(format!("the session {} is not registered", session_id.as_str()))
        })?;
        if row.state != SessionState::Connected {
            return Err(TaseError::unauthenticated(format!(
                "the session {} is {} and may not heartbeat",
                session_id.as_str(),
                row.state.as_str()
            )));
        }
        row.heartbeat_at_ms = now_ms;
        upsert_session(&tx, &row)?;
        journal_session(&tx, &row, now_ms)?;
        tx.commit()?;
        Ok(())
    }

    /// Closes a session: the agent leaves on its own.
    pub fn unregister_session(&mut self, session_id: &SessionId) -> Result<(), TaseError> {
        let tx = self.transaction()?;
        let mut row = row_session(&tx, session_id.as_str())?.ok_or_else(|| {
            TaseError::not_found(format!("the session {} is not registered", session_id.as_str()))
        })?;
        if row.state == SessionState::Disconnected {
            // closing a closed session is accepted again, it changes nothing
            tx.commit()?;
            return Ok(());
        }
        row.state = SessionState::Disconnected;
        upsert_session(&tx, &row)?;
        journal_session(&tx, &row, 0)?;
        tx.commit()?;
        Ok(())
    }

    /// The registration of a session, or `None` when there is none.
    pub fn get_session(&self, session_id: &SessionId) -> Result<Option<AgentRegistration>, TaseError> {
        row_session(self.connection(), session_id.as_str())
    }

    /// Every registration of a session.
    pub fn list_sessions(&self) -> Result<Vec<AgentRegistration>, TaseError> {
        list_sessions(self.connection())
    }

    // -- the jobs ----------------------------------------------------------------------------

    /// Registers a job.
    ///
    /// The idempotency key of the job is the pair of the workspace and the agent with the key, as the unique
    /// constraint of the design book says. The same key with the same digest gives the same job again, with
    /// `deduplicated` set; the same key with another digest is `IDEMPOTENCY_MISMATCH` (detailed design §2.5).
    pub fn submit_job(&mut self, req: NewJob) -> Result<JobSubmission, TaseError> {
        let tx = self.transaction()?;
        let scope = idempotency_scope(&req.workspace_id, &req.agent_id);
        if let Some(record) = row_idempotency(&tx, &scope, &req.idempotency_key)? {
            if record.request_digest != req.request_digest {
                return Err(TaseError::idempotency_mismatch(format!(
                    "the idempotency key {:?} of {} was already used with another digest",
                    req.idempotency_key,
                    scope
                )));
            }
            let job = row_job(&tx, record.job_id.as_str())?.ok_or_else(|| {
                TaseError::internal(format!(
                    "the job {} of the idempotency record is gone",
                    record.job_id.as_str()
                ))
            })?;
            tx.commit()?;
            return Ok(JobSubmission {
                job_id: job.job_id,
                deduplicated: true,
                status: job.status,
            });
        }
        if row_session(&tx, req.session_id.as_str())?.is_none() {
            return Err(TaseError::not_found(format!(
                "the session {} of the job is not registered",
                req.session_id.as_str()
            )));
        }
        let job_id = JobId::new(format!("job_{:012}", next_counter(&tx, "job_id")?))?;
        let row = Job {
            job_id: job_id.clone(),
            job_type: req.job_type,
            workspace_id: req.workspace_id.clone(),
            agent_id: req.agent_id.clone(),
            session_id: req.session_id.clone(),
            idempotency_key: req.idempotency_key.clone(),
            request_digest: req.request_digest.clone(),
            base_snapshot_digest: req.base_snapshot_digest,
            status: JobStatus::Queued,
            attempt: 0,
            lease_epoch: 0,
            worker_id: None,
            result_digest: None,
            commit_intent_digest: None,
            committed_ref: None,
            committed_oid: None,
            error_code: None,
            priority: req.priority,
            created_at_ms: req.now_ms,
            updated_at_ms: req.now_ms,
        };
        let record = IdempotencyRecord {
            scope: scope.clone(),
            idempotency_key: req.idempotency_key.clone(),
            request_digest: req.request_digest.clone(),
            response_digest: req.request_digest.clone(),
            job_id: job_id.clone(),
            created_at_ms: req.now_ms,
        };
        upsert_job(&tx, &row)?;
        upsert_idempotency(&tx, &record)?;
        // the outbox line is written with the job, in the same transaction, so that a notification is never lost
        // (detailed design §17.5).
        tx.execute(
            "INSERT INTO local_outbox (job_id, payload_digest) VALUES (?1, ?2)",
            rusqlite::params![job_id.as_str(), req.request_digest],
        )?;
        journal_job(&tx, &row, req.now_ms)?;
        journal_idempotency(&tx, &record, req.now_ms)?;
        tx.commit()?;
        Ok(JobSubmission {
            job_id,
            deduplicated: false,
            status: JobStatus::Queued,
        })
    }

    /// The job of an id, or `None` when there is none.
    pub fn get_job(&self, job_id: &JobId) -> Result<Option<Job>, TaseError> {
        row_job(self.connection(), job_id.as_str())
    }

    /// Every job.
    pub fn list_jobs(&self) -> Result<Vec<Job>, TaseError> {
        list_jobs(self.connection())
    }

    /// Every job in a state.
    pub fn list_jobs_by_status(&self, status: JobStatus) -> Result<Vec<Job>, TaseError> {
        list_jobs_by_status(self.connection(), status)
    }

    /// Moves a job from one state to another, and says whether it happened.
    ///
    /// The move only happens when the job is in the state the caller expects and holds the epoch the caller holds
    /// (detailed design §6.3). A worker whose lease ran out has an old epoch and may not move the job.
    pub fn transition_job(
        &mut self,
        job_id: &JobId,
        from: JobStatus,
        to: JobStatus,
        lease_epoch: u64,
        now_ms: u64,
    ) -> Result<bool, TaseError> {
        let tx = self.transaction()?;
        let changed = tx.execute(
            "UPDATE local_jobs SET status = ?2, updated_at_ms = ?3
              WHERE job_id = ?1 AND status = ?4 AND lease_epoch = ?5",
            rusqlite::params![
                job_id.as_str(),
                to.as_str(),
                now_ms as i64,
                from.as_str(),
                lease_epoch as i64
            ],
        )?;
        if changed == 0 {
            tx.commit()?;
            return Ok(false);
        }
        let mut row = row_job(&tx, job_id.as_str())?.ok_or_else(|| {
            TaseError::internal(format!("the job {} vanished in its own transition", job_id.as_str()))
        })?;
        row.status = to;
        row.updated_at_ms = now_ms;
        journal_job(&tx, &row, now_ms)?;
        tx.commit()?;
        Ok(true)
    }

    /// Finishes a job: it writes the result and the business code of the failure.
    ///
    /// A job that is already in a terminal state is not touched again: the same notification may be accepted, but
    /// a job in a terminal state is never run again (detailed design §6.3).
    pub fn finish_job(
        &mut self,
        job_id: &JobId,
        status: JobStatus,
        result_digest: Option<String>,
        error_code: Option<ErrorCode>,
        now_ms: u64,
    ) -> Result<bool, TaseError> {
        let tx = self.transaction()?;
        let mut row = row_job(&tx, job_id.as_str())?.ok_or_else(|| {
            TaseError::not_found(format!("there is no job {}", job_id.as_str()))
        })?;
        if row.status.is_terminal() {
            tx.commit()?;
            return Ok(false);
        }
        row.status = status;
        row.result_digest = result_digest;
        row.error_code = error_code;
        row.updated_at_ms = now_ms;
        upsert_job(&tx, &row)?;
        journal_job(&tx, &row, now_ms)?;
        tx.commit()?;
        Ok(true)
    }

    // -- the leases --------------------------------------------------------------------------

    /// Takes a lease of a resource.
    ///
    /// Only one holder at a time: the lease that ran out of time is taken back first, and a resource that is held
    /// by somebody else is `RATE_LIMITED` (the holder may be gone, the caller may try again later). The fencing
    /// token of the new lease is greater than the token of every lease that came before it.
    pub fn acquire_lease(&mut self, req: NewLease) -> Result<Lease, TaseError> {
        let tx = self.transaction()?;
        reclaim_expired_of(&tx, &req.resource, req.now_ms)?;
        if let Some(held) = row_held_lease(&tx, &req.resource)? {
            return Err(TaseError::rate_limited(format!(
                "the resource {} is leased by {} until {}",
                held.resource, held.holder, held.expires_at_ms
            )));
        }
        let row = Lease {
            lease_id: format!("lease_{:012}", next_counter(&tx, "lease_id")?),
            resource: req.resource,
            holder: req.holder,
            session_id: req.session_id,
            fencing_token: next_counter(&tx, "fencing_token")?,
            acquired_at_ms: req.now_ms,
            expires_at_ms: req.now_ms.saturating_add(req.ttl_ms),
            released_at_ms: None,
            state: LeaseState::Held,
        };
        upsert_lease(&tx, &row)?;
        journal_lease(&tx, &row, req.now_ms)?;
        tx.commit()?;
        Ok(row)
    }

    /// Takes the lease of a job and starts the job.
    ///
    /// The claim is atomic: the job is only given to one holder. The number of attempts of the job grows, the
    /// epoch of the job grows and the fencing token of the lease grows, so a worker that comes back late is told
    /// that it is old (detailed design §6.2).
    pub fn lease_job(
        &mut self,
        job_id: &JobId,
        holder: &str,
        ttl_ms: u64,
        now_ms: u64,
    ) -> Result<Lease, TaseError> {
        let tx = self.transaction()?;
        let mut job = row_job(&tx, job_id.as_str())?.ok_or_else(|| {
            TaseError::not_found(format!("there is no job {}", job_id.as_str()))
        })?;
        let resource = job_id.as_str().to_string();
        if job.status.is_terminal() {
            // a job in a terminal state is never run again: a retry is a new job (detailed design §6.3)
            return Err(TaseError::invalid_argument(format!(
                "the job {} is {} and may not be leased again",
                job_id.as_str(),
                job.status.as_str()
            )));
        }
        reclaim_expired_of(&tx, &resource, now_ms)?;
        let held = row_held_lease(&tx, &resource)?;
        if job.status != JobStatus::Queued {
            // a job that is not queued may only be taken again when its lease died
            let alive = held
                .as_ref()
                .is_some_and(|lease| lease.is_valid_at(now_ms));
            if alive {
                return Err(TaseError::rate_limited(format!(
                    "the job {} is {} and its lease is held",
                    job_id.as_str(),
                    job.status.as_str()
                )));
            }
        }
        if let Some(lease) = held {
            return Err(TaseError::rate_limited(format!(
                "the job {} is leased by {} until {}",
                job_id.as_str(),
                lease.holder,
                lease.expires_at_ms
            )));
        }
        let lease = Lease {
            lease_id: format!("lease_{:012}", next_counter(&tx, "lease_id")?),
            resource: resource.clone(),
            holder: holder.to_string(),
            session_id: Some(job.session_id.clone()),
            fencing_token: next_counter(&tx, "fencing_token")?,
            acquired_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(ttl_ms),
            released_at_ms: None,
            state: LeaseState::Held,
        };
        job.attempt = job.attempt.saturating_add(1);
        job.lease_epoch = job.lease_epoch.saturating_add(1);
        job.status = JobStatus::Running;
        job.worker_id = Some(holder.to_string());
        job.updated_at_ms = now_ms;
        upsert_lease(&tx, &lease)?;
        upsert_job(&tx, &job)?;
        journal_lease(&tx, &lease, now_ms)?;
        journal_job(&tx, &job, now_ms)?;
        tx.commit()?;
        Ok(lease)
    }

    /// Renews a lease: the holder asks for more time.
    ///
    /// The lease must be held by the caller and must not have run out of time: a lease that ran out is not renewed,
    /// the caller must take a new one (detailed design §6.2).
    pub fn renew_lease(
        &mut self,
        lease_id: &str,
        holder: &str,
        expires_at_ms: u64,
        now_ms: u64,
    ) -> Result<Lease, TaseError> {
        let tx = self.transaction()?;
        let mut row = row_lease(&tx, lease_id)?.ok_or_else(|| {
            TaseError::not_found(format!("there is no lease {lease_id}"))
        })?;
        if row.holder != holder {
            return Err(TaseError::permission_denied(format!(
                "the lease {lease_id} is held by {} and not by {holder}",
                row.holder
            )));
        }
        if row.state != LeaseState::Held {
            return Err(TaseError::worker_unavailable(format!(
                "the lease {lease_id} is {} and is not held any more",
                row.state.as_str()
            )));
        }
        if row.expires_at_ms <= now_ms {
            return Err(TaseError::worker_unavailable(format!(
                "the lease {lease_id} ran out at {} and may not be renewed",
                row.expires_at_ms
            )));
        }
        row.expires_at_ms = expires_at_ms;
        upsert_lease(&tx, &row)?;
        journal_lease(&tx, &row, now_ms)?;
        tx.commit()?;
        Ok(row)
    }

    /// Gives a lease back.
    ///
    /// A lease that is not held any more is accepted again and keeps its state: the call changes nothing.
    pub fn release_lease(&mut self, lease_id: &str, holder: &str) -> Result<Lease, TaseError> {
        let tx = self.transaction()?;
        let mut row = row_lease(&tx, lease_id)?.ok_or_else(|| {
            TaseError::not_found(format!("there is no lease {lease_id}"))
        })?;
        if row.holder != holder {
            return Err(TaseError::permission_denied(format!(
                "the lease {lease_id} is held by {} and not by {holder}",
                row.holder
            )));
        }
        if row.state == LeaseState::Held {
            row.state = LeaseState::Released;
            row.released_at_ms = Some(row.acquired_at_ms);
            upsert_lease(&tx, &row)?;
            journal_lease(&tx, &row, 0)?;
        }
        tx.commit()?;
        Ok(row)
    }

    /// Takes back the leases that ran out of time, and returns them.
    pub fn reclaim_expired_leases(&mut self, now_ms: u64) -> Result<Vec<Lease>, TaseError> {
        let tx = self.transaction()?;
        let expired = expired_leases(&tx, now_ms)?;
        for lease in &expired {
            upsert_lease(&tx, lease)?;
            journal_lease(&tx, lease, now_ms)?;
        }
        tx.commit()?;
        Ok(expired)
    }

    /// The lease of an id, or `None` when there is none.
    pub fn get_lease(&self, lease_id: &str) -> Result<Option<Lease>, TaseError> {
        row_lease(self.connection(), lease_id)
    }

    /// Every lease.
    pub fn list_leases(&self) -> Result<Vec<Lease>, TaseError> {
        list_leases(self.connection())
    }

    // -- the files ---------------------------------------------------------------------------

    /// The generation of a file of a workspace: `0` when the file was never written.
    pub fn file_generation(
        &self,
        workspace_id: &WorkspaceId,
        normalized_path: &str,
    ) -> Result<u64, TaseError> {
        Ok(row_file_generation(self.connection(), workspace_id.as_str(), normalized_path)?
            .map(|row| row.generation)
            .unwrap_or(0))
    }

    /// Writes a file of a workspace and gives it the next generation.
    ///
    /// When the writer expects a generation and the file has another one, the write is refused: the conflict is
    /// written down and `SYMBOL_CONFLICT` is returned (detailed design §16.4, §17.4).
    pub fn record_file_transaction(
        &mut self,
        req: NewFileTransaction,
    ) -> Result<FileGeneration, TaseError> {
        let tx = self.transaction()?;
        let current = row_file_generation(&tx, req.workspace_id.as_str(), &req.normalized_path)?;
        if let Some(expected) = req.expected_generation {
            let actual = current.as_ref().map(|row| row.generation).unwrap_or(0);
            if actual != expected {
                let conflict = FileConflict {
                    conflict_id: next_counter(&tx, "conflict_id")?,
                    workspace_id: req.workspace_id.clone(),
                    normalized_path: req.normalized_path.clone(),
                    expected_generation: expected,
                    actual_generation: actual,
                    agent_id: req.agent_id.clone(),
                    session_id: req.session_id.clone(),
                    job_id: req.job_id.clone(),
                    state: ConflictState::Open,
                    detected_at_ms: req.now_ms,
                };
                insert_file_conflict(&tx, &conflict)?;
                journal_file_conflict(&tx, &conflict, req.now_ms)?;
                tx.commit()?;
                return Err(TaseError::symbol_conflict(format!(
                    "the file {} is at the generation {} and not at the generation {}",
                    req.normalized_path, actual, expected
                )));
            }
        }
        let row = FileGeneration {
            workspace_id: req.workspace_id.clone(),
            normalized_path: req.normalized_path.clone(),
            generation: current.map(|row| row.generation).unwrap_or(0) + 1,
            content_digest: req.content_digest.clone(),
            updated_at_ms: req.now_ms,
        };
        upsert_file_generation(&tx, &row)?;
        journal_file_generation(&tx, &row, req.now_ms)?;
        tx.commit()?;
        Ok(row)
    }

    /// Looks for a conflict of a file without writing anything down.
    ///
    /// The conflict that is returned has the id `0`: it was not written down yet.
    pub fn detect_file_conflict(
        &self,
        workspace_id: &WorkspaceId,
        normalized_path: &str,
        expected_generation: u64,
    ) -> Result<Option<FileConflict>, TaseError> {
        let actual = self.file_generation(workspace_id, normalized_path)?;
        if actual == expected_generation {
            return Ok(None);
        }
        Ok(Some(FileConflict {
            conflict_id: 0,
            workspace_id: workspace_id.clone(),
            normalized_path: normalized_path.to_string(),
            expected_generation,
            actual_generation: actual,
            agent_id: AgentId::new("agent_unknown")?,
            session_id: SessionId::new("session_unknown")?,
            job_id: None,
            state: ConflictState::Open,
            detected_at_ms: 0,
        }))
    }

    /// Every conflict of a file.
    pub fn list_file_conflicts(&self) -> Result<Vec<FileConflict>, TaseError> {
        list_file_conflicts(self.connection())
    }

    // -- the idempotency records -------------------------------------------------------------

    /// The record of an idempotency key, or `None` when the key is free.
    pub fn lookup_idempotency(
        &self,
        scope: &str,
        idempotency_key: &str,
    ) -> Result<Option<IdempotencyRecord>, TaseError> {
        row_idempotency(self.connection(), scope, idempotency_key)
    }

    /// Every idempotency record.
    pub fn list_idempotency(&self) -> Result<Vec<IdempotencyRecord>, TaseError> {
        list_idempotency(self.connection())
    }

    // -- the outbox and the audit ------------------------------------------------------------

    /// The events of the outbox that were not published yet.
    pub fn list_unpublished_outbox(&self) -> Result<Vec<OutboxEvent>, TaseError> {
        list_unpublished_outbox(self.connection())
    }

    /// Says that an event of the outbox was published.
    pub fn mark_outbox_published(
        &mut self,
        event_id: i64,
        now_ms: u64,
    ) -> Result<bool, TaseError> {
        let tx = self.transaction()?;
        let changed = tx.execute(
            "UPDATE local_outbox SET published_at_ms = ?2 WHERE event_id = ?1 AND published_at_ms IS NULL",
            rusqlite::params![event_id, now_ms as i64],
        )?;
        tx.commit()?;
        Ok(changed > 0)
    }

    /// Every line of the audit trail.
    pub fn list_audit_events(&self) -> Result<Vec<AuditEvent>, TaseError> {
        list_audit_events(self.connection())
    }

    /// Writes a line in the audit trail and gives its number back.
    pub fn audit(&mut self, record: AuditRecord) -> Result<i64, TaseError> {
        let tx = self.transaction()?;
        tx.execute(
            "INSERT INTO local_audit_events
               (job_id, agent_id, session_id, workspace_id, event_type, payload_digest, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                record.job_id,
                record.agent_id,
                record.session_id,
                record.workspace_id,
                record.event_type,
                record.payload_digest,
                record.now_ms as i64
            ],
        )?;
        let seq = tx.query_row(
            "SELECT last_insert_rowid()",
            rusqlite::params![],
            |row| row.get::<_, i64>(0),
        )?;
        tx.commit()?;
        Ok(seq)
    }
}

// ---------------------------------------------------------------------------------------------
// The rows of the tables
// ---------------------------------------------------------------------------------------------

const SESSION_COLUMNS: &str = "session_id, agent_id, provider, instance_id, workspace_id, \
     owner_logon_sid_hash, granted_scopes_json, capabilities_json, state, lease_epoch, \
     heartbeat_at_ms, created_at_ms";

const JOB_COLUMNS: &str = "job_id, job_type, workspace_id, agent_id, session_id, idempotency_key, \
     request_digest, base_snapshot_digest, status, attempt, lease_epoch, worker_id, result_digest, \
     commit_intent_digest, committed_ref, committed_oid, error_code, priority, created_at_ms, \
     updated_at_ms";

const LEASE_COLUMNS: &str = "lease_id, resource, holder, session_id, fencing_token, acquired_at_ms, \
     expires_at_ms, released_at_ms, state";

const IDEMPOTENCY_COLUMNS: &str =
    "scope, idempotency_key, request_digest, response_digest, job_id, created_at_ms";

const FILE_GENERATION_COLUMNS: &str =
    "workspace_id, normalized_path, buffer_generation, file_content_digest, updated_at_ms";

const FILE_CONFLICT_COLUMNS: &str = "conflict_id, workspace_id, normalized_path, \
     expected_generation, actual_generation, agent_id, session_id, job_id, state, detected_at_ms";

/// Runs a statement and reads every row with the reader.
fn read_all<T>(
    conn: &Connection,
    sql: &str,
    params: sqlite::SqlParams<'_>,
    reader: impl Fn(&rusqlite::Row<'_>) -> Result<T, TaseError>,
) -> Result<Vec<T>, TaseError> {
    let mut statement = conn.prepare(sql).map_err(sqlite::map_sqlite)?;
    let mut rows = statement.query(params).map_err(sqlite::map_sqlite)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(sqlite::map_sqlite)? {
        out.push(reader(row)?);
    }
    Ok(out)
}

/// Runs a statement and reads the first row with the reader.
fn read_first<T>(
    conn: &Connection,
    sql: &str,
    params: sqlite::SqlParams<'_>,
    reader: impl Fn(&rusqlite::Row<'_>) -> Result<T, TaseError>,
) -> Result<Option<T>, TaseError> {
    Ok(read_all(conn, sql, params, reader)?.into_iter().next())
}

/// The text of a column of a row.
fn column_text(row: &rusqlite::Row<'_>, index: usize) -> Result<String, TaseError> {
    row.get::<_, String>(index).map_err(sqlite::map_sqlite)
}

/// The number of a column of a row.
fn column_u64(row: &rusqlite::Row<'_>, index: usize) -> Result<u64, TaseError> {
    let value: i64 = row.get(index).map_err(sqlite::map_sqlite)?;
    u64::try_from(value).map_err(|_| {
        TaseError::internal(format!("the column {index} holds a number that is not a time"))
    })
}

fn row_session(conn: &Connection, session_id: &str) -> Result<Option<AgentRegistration>, TaseError> {
    let sql = format!("SELECT {SESSION_COLUMNS} FROM local_agent_sessions WHERE session_id = ?1");
    read_first(conn, &sql, rusqlite::params![session_id], read_session)
}

fn list_sessions(conn: &Connection) -> Result<Vec<AgentRegistration>, TaseError> {
    let sql = format!(
        "SELECT {SESSION_COLUMNS} FROM local_agent_sessions ORDER BY created_at_ms, session_id"
    );
    read_all(conn, &sql, rusqlite::params![], read_session)
}

fn read_session(row: &rusqlite::Row<'_>) -> Result<AgentRegistration, TaseError> {
    let state = column_text(row, 8)?;
    Ok(AgentRegistration {
        session_id: SessionId::new(column_text(row, 0)?)?,
        agent_id: AgentId::new(column_text(row, 1)?)?,
        provider: column_text(row, 2)?,
        instance_id: column_text(row, 3)?,
        workspace_id: WorkspaceId::new(column_text(row, 4)?)?,
        owner_logon_sid_hash: column_text(row, 5)?,
        granted_scopes: decode_str_list(&column_text(row, 6)?)?,
        capabilities: decode_str_list(&column_text(row, 7)?)?,
        state: SessionState::from_name(&state)
            .ok_or_else(|| TaseError::internal(format!("unknown state of a session {state:?}")))?,
        lease_epoch: column_u64(row, 9)?,
        heartbeat_at_ms: column_u64(row, 10)?,
        created_at_ms: column_u64(row, 11)?,
    })
}

fn row_job(conn: &Connection, job_id: &str) -> Result<Option<Job>, TaseError> {
    let sql = format!("SELECT {JOB_COLUMNS} FROM local_jobs WHERE job_id = ?1");
    read_first(conn, &sql, rusqlite::params![job_id], read_job)
}

fn list_jobs(conn: &Connection) -> Result<Vec<Job>, TaseError> {
    let sql = format!("SELECT {JOB_COLUMNS} FROM local_jobs ORDER BY created_at_ms, job_id");
    read_all(conn, &sql, rusqlite::params![], read_job)
}

fn list_jobs_by_status(conn: &Connection, status: JobStatus) -> Result<Vec<Job>, TaseError> {
    let sql = format!(
        "SELECT {JOB_COLUMNS} FROM local_jobs WHERE status = ?1 ORDER BY priority, created_at_ms, job_id"
    );
    read_all(conn, &sql, rusqlite::params![status.as_str()], read_job)
}

fn read_job(row: &rusqlite::Row<'_>) -> Result<Job, TaseError> {
    let status = column_text(row, 8)?;
    let error_code: Option<String> = row.get(16).map_err(sqlite::map_sqlite)?;
    Ok(Job {
        job_id: JobId::new(column_text(row, 0)?)?,
        job_type: column_text(row, 1)?,
        workspace_id: WorkspaceId::new(column_text(row, 2)?)?,
        agent_id: AgentId::new(column_text(row, 3)?)?,
        session_id: SessionId::new(column_text(row, 4)?)?,
        idempotency_key: column_text(row, 5)?,
        request_digest: column_text(row, 6)?,
        base_snapshot_digest: column_text(row, 7)?,
        status: JobStatus::from_name(&status)
            .ok_or_else(|| TaseError::internal(format!("unknown state of a job {status:?}")))?,
        attempt: column_u64(row, 9)? as u32,
        lease_epoch: column_u64(row, 10)?,
        worker_id: row.get(11).map_err(sqlite::map_sqlite)?,
        result_digest: row.get(12).map_err(sqlite::map_sqlite)?,
        commit_intent_digest: row.get(13).map_err(sqlite::map_sqlite)?,
        committed_ref: row.get(14).map_err(sqlite::map_sqlite)?,
        committed_oid: row.get(15).map_err(sqlite::map_sqlite)?,
        error_code: error_code.as_deref().and_then(ErrorCode::from_str),
        priority: column_u64(row, 17)? as u8,
        created_at_ms: column_u64(row, 18)?,
        updated_at_ms: column_u64(row, 19)?,
    })
}

fn row_lease(conn: &Connection, lease_id: &str) -> Result<Option<Lease>, TaseError> {
    let sql = format!("SELECT {LEASE_COLUMNS} FROM local_leases WHERE lease_id = ?1");
    read_first(conn, &sql, rusqlite::params![lease_id], read_lease)
}

fn row_held_lease(conn: &Connection, resource: &str) -> Result<Option<Lease>, TaseError> {
    let sql = format!(
        "SELECT {LEASE_COLUMNS} FROM local_leases WHERE resource = ?1 AND state = 'HELD'"
    );
    read_first(conn, &sql, rusqlite::params![resource], read_lease)
}

fn list_leases(conn: &Connection) -> Result<Vec<Lease>, TaseError> {
    let sql = format!("SELECT {LEASE_COLUMNS} FROM local_leases ORDER BY fencing_token, lease_id");
    read_all(conn, &sql, rusqlite::params![], read_lease)
}

fn read_lease(row: &rusqlite::Row<'_>) -> Result<Lease, TaseError> {
    let state = column_text(row, 8)?;
    let session_id: Option<String> = row.get(3).map_err(sqlite::map_sqlite)?;
    let released_at_ms: Option<i64> = row.get(7).map_err(sqlite::map_sqlite)?;
    Ok(Lease {
        lease_id: column_text(row, 0)?,
        resource: column_text(row, 1)?,
        holder: column_text(row, 2)?,
        session_id: match session_id {
            Some(id) => Some(SessionId::new(id)?),
            None => None,
        },
        fencing_token: column_u64(row, 4)?,
        acquired_at_ms: column_u64(row, 5)?,
        expires_at_ms: column_u64(row, 6)?,
        released_at_ms: released_at_ms.map(|value| value as u64),
        state: LeaseState::from_name(&state)
            .ok_or_else(|| TaseError::internal(format!("unknown state of a lease {state:?}")))?,
    })
}

fn expired_leases(conn: &Connection, now_ms: u64) -> Result<Vec<Lease>, TaseError> {
    let sql = format!(
        "SELECT {LEASE_COLUMNS} FROM local_leases
          WHERE state = 'HELD' AND expires_at_ms <= ?1 ORDER BY fencing_token"
    );
    let mut leases = read_all(conn, &sql, rusqlite::params![now_ms as i64], read_lease)?;
    for lease in &mut leases {
        lease.state = LeaseState::Expired;
    }
    Ok(leases)
}

fn row_idempotency(
    conn: &Connection,
    scope: &str,
    idempotency_key: &str,
) -> Result<Option<IdempotencyRecord>, TaseError> {
    let sql = format!(
        "SELECT {IDEMPOTENCY_COLUMNS} FROM local_idempotency
          WHERE scope = ?1 AND idempotency_key = ?2"
    );
    read_first(conn, &sql, rusqlite::params![scope, idempotency_key], read_idempotency)
}

fn list_idempotency(conn: &Connection) -> Result<Vec<IdempotencyRecord>, TaseError> {
    let sql = format!(
        "SELECT {IDEMPOTENCY_COLUMNS} FROM local_idempotency
          ORDER BY created_at_ms, scope, idempotency_key"
    );
    read_all(conn, &sql, rusqlite::params![], read_idempotency)
}

fn read_idempotency(row: &rusqlite::Row<'_>) -> Result<IdempotencyRecord, TaseError> {
    Ok(IdempotencyRecord {
        scope: column_text(row, 0)?,
        idempotency_key: column_text(row, 1)?,
        request_digest: column_text(row, 2)?,
        response_digest: column_text(row, 3)?,
        job_id: JobId::new(column_text(row, 4)?)?,
        created_at_ms: column_u64(row, 5)?,
    })
}

fn row_file_generation(
    conn: &Connection,
    workspace_id: &str,
    normalized_path: &str,
) -> Result<Option<FileGeneration>, TaseError> {
    let sql = format!(
        "SELECT {FILE_GENERATION_COLUMNS} FROM local_workspace_versions
          WHERE workspace_id = ?1 AND normalized_path = ?2"
    );
    read_first(
        conn,
        &sql,
        rusqlite::params![workspace_id, normalized_path],
        read_file_generation,
    )
}

fn read_file_generation(row: &rusqlite::Row<'_>) -> Result<FileGeneration, TaseError> {
    Ok(FileGeneration {
        workspace_id: WorkspaceId::new(column_text(row, 0)?)?,
        normalized_path: column_text(row, 1)?,
        generation: column_u64(row, 2)?,
        content_digest: column_text(row, 3)?,
        updated_at_ms: column_u64(row, 4)?,
    })
}

fn list_file_conflicts(conn: &Connection) -> Result<Vec<FileConflict>, TaseError> {
    let sql = format!(
        "SELECT {FILE_CONFLICT_COLUMNS} FROM local_file_conflicts ORDER BY conflict_id"
    );
    read_all(conn, &sql, rusqlite::params![], read_file_conflict)
}

fn read_file_conflict(row: &rusqlite::Row<'_>) -> Result<FileConflict, TaseError> {
    let state = column_text(row, 8)?;
    let job_id: Option<String> = row.get(7).map_err(sqlite::map_sqlite)?;
    Ok(FileConflict {
        conflict_id: column_u64(row, 0)?,
        workspace_id: WorkspaceId::new(column_text(row, 1)?)?,
        normalized_path: column_text(row, 2)?,
        expected_generation: column_u64(row, 3)?,
        actual_generation: column_u64(row, 4)?,
        agent_id: AgentId::new(column_text(row, 5)?)?,
        session_id: SessionId::new(column_text(row, 6)?)?,
        job_id: match job_id {
            Some(id) => Some(JobId::new(id)?),
            None => None,
        },
        state: ConflictState::from_name(&state)
            .ok_or_else(|| TaseError::internal(format!("unknown state of a conflict {state:?}")))?,
        detected_at_ms: column_u64(row, 9)?,
    })
}

fn upsert_session(conn: &Connection, row: &AgentRegistration) -> Result<(), TaseError> {
    conn.execute(
        "INSERT INTO local_agent_sessions
           (session_id, agent_id, provider, instance_id, workspace_id, owner_logon_sid_hash,
            granted_scopes_json, capabilities_json, state, lease_epoch, heartbeat_at_ms, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(session_id) DO UPDATE SET
           agent_id = ?2, provider = ?3, instance_id = ?4, workspace_id = ?5,
           owner_logon_sid_hash = ?6, granted_scopes_json = ?7, capabilities_json = ?8,
           state = ?9, lease_epoch = ?10, heartbeat_at_ms = ?11, created_at_ms = ?12",
        rusqlite::params![
            row.session_id.as_str(),
            row.agent_id.as_str(),
            row.provider,
            row.instance_id,
            row.workspace_id.as_str(),
            row.owner_logon_sid_hash,
            encode_str_list(&row.granted_scopes),
            encode_str_list(&row.capabilities),
            row.state.as_str(),
            row.lease_epoch as i64,
            row.heartbeat_at_ms as i64,
            row.created_at_ms as i64
        ],
    )
    .map_err(sqlite::map_sqlite)?;
    Ok(())
}

fn upsert_job(conn: &Connection, row: &Job) -> Result<(), TaseError> {
    conn.execute(
        "INSERT INTO local_jobs
           (job_id, job_type, workspace_id, agent_id, session_id, idempotency_key, request_digest,
            base_snapshot_digest, status, attempt, lease_epoch, worker_id, result_digest,
            commit_intent_digest, committed_ref, committed_oid, error_code, priority,
            created_at_ms, updated_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)
         ON CONFLICT(job_id) DO UPDATE SET
           job_type = ?2, workspace_id = ?3, agent_id = ?4, session_id = ?5, idempotency_key = ?6,
           request_digest = ?7, base_snapshot_digest = ?8, status = ?9, attempt = ?10,
           lease_epoch = ?11, worker_id = ?12, result_digest = ?13, commit_intent_digest = ?14,
           committed_ref = ?15, committed_oid = ?16, error_code = ?17, priority = ?18,
           created_at_ms = ?19, updated_at_ms = ?20",
        rusqlite::params![
            row.job_id.as_str(),
            row.job_type,
            row.workspace_id.as_str(),
            row.agent_id.as_str(),
            row.session_id.as_str(),
            row.idempotency_key,
            row.request_digest,
            row.base_snapshot_digest,
            row.status.as_str(),
            row.attempt as i64,
            row.lease_epoch as i64,
            row.worker_id,
            row.result_digest,
            row.commit_intent_digest,
            row.committed_ref,
            row.committed_oid,
            row.error_code.map(|code| code.as_str()),
            row.priority as i64,
            row.created_at_ms as i64,
            row.updated_at_ms as i64
        ],
    )
    .map_err(sqlite::map_sqlite)?;
    Ok(())
}

fn upsert_lease(conn: &Connection, row: &Lease) -> Result<(), TaseError> {
    conn.execute(
        "INSERT INTO local_leases
           (lease_id, resource, holder, session_id, fencing_token, acquired_at_ms, expires_at_ms,
            released_at_ms, state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(lease_id) DO UPDATE SET
           resource = ?2, holder = ?3, session_id = ?4, fencing_token = ?5, acquired_at_ms = ?6,
           expires_at_ms = ?7, released_at_ms = ?8, state = ?9",
        rusqlite::params![
            row.lease_id,
            row.resource,
            row.holder,
            row.session_id.as_ref().map(|id| id.as_str()),
            row.fencing_token as i64,
            row.acquired_at_ms as i64,
            row.expires_at_ms as i64,
            row.released_at_ms.map(|value| value as i64),
            row.state.as_str()
        ],
    )
    .map_err(sqlite::map_sqlite)?;
    Ok(())
}

fn upsert_idempotency(conn: &Connection, row: &IdempotencyRecord) -> Result<(), TaseError> {
    conn.execute(
        "INSERT INTO local_idempotency
           (scope, idempotency_key, request_digest, response_digest, job_id, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(scope, idempotency_key) DO UPDATE SET
           request_digest = ?3, response_digest = ?4, job_id = ?5, created_at_ms = ?6",
        rusqlite::params![
            row.scope,
            row.idempotency_key,
            row.request_digest,
            row.response_digest,
            row.job_id.as_str(),
            row.created_at_ms as i64
        ],
    )
    .map_err(sqlite::map_sqlite)?;
    Ok(())
}

fn upsert_file_generation(conn: &Connection, row: &FileGeneration) -> Result<(), TaseError> {
    conn.execute(
        "INSERT INTO local_workspace_versions
           (workspace_id, normalized_path, buffer_generation, file_content_digest, updated_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(workspace_id, normalized_path) DO UPDATE SET
           buffer_generation = ?3, file_content_digest = ?4, updated_at_ms = ?5",
        rusqlite::params![
            row.workspace_id.as_str(),
            row.normalized_path,
            row.generation as i64,
            row.content_digest,
            row.updated_at_ms as i64
        ],
    )
    .map_err(sqlite::map_sqlite)?;
    Ok(())
}

fn insert_file_conflict(conn: &Connection, row: &FileConflict) -> Result<(), TaseError> {
    conn.execute(
        "INSERT INTO local_file_conflicts
           (conflict_id, workspace_id, normalized_path, expected_generation, actual_generation,
            agent_id, session_id, job_id, state, detected_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(conflict_id) DO UPDATE SET
           expected_generation = ?4, actual_generation = ?5, agent_id = ?6, session_id = ?7,
           job_id = ?8, state = ?9, detected_at_ms = ?10",
        rusqlite::params![
            row.conflict_id as i64,
            row.workspace_id.as_str(),
            row.normalized_path,
            row.expected_generation as i64,
            row.actual_generation as i64,
            row.agent_id.as_str(),
            row.session_id.as_str(),
            row.job_id.as_ref().map(|id| id.as_str()),
            row.state.as_str(),
            row.detected_at_ms as i64
        ],
    )
    .map_err(sqlite::map_sqlite)?;
    Ok(())
}

fn list_unpublished_outbox(conn: &Connection) -> Result<Vec<OutboxEvent>, TaseError> {
    read_all(
        conn,
        "SELECT event_id, job_id, payload_digest, published_at_ms FROM local_outbox
          WHERE published_at_ms IS NULL ORDER BY event_id",
        rusqlite::params![],
        |row| {
            let published: Option<i64> = row.get(3).map_err(sqlite::map_sqlite)?;
            Ok(OutboxEvent {
                event_id: row.get(0).map_err(sqlite::map_sqlite)?,
                job_id: JobId::new(column_text(row, 1)?)?,
                payload_digest: column_text(row, 2)?,
                published_at_ms: published.map(|value| value as u64),
            })
        },
    )
}

fn list_audit_events(conn: &Connection) -> Result<Vec<AuditEvent>, TaseError> {
    read_all(
        conn,
        "SELECT seq, job_id, agent_id, session_id, workspace_id, event_type, payload_digest, created_at_ms
           FROM local_audit_events ORDER BY seq",
        rusqlite::params![],
        |row| {
            Ok(AuditEvent {
                seq: row.get(0).map_err(sqlite::map_sqlite)?,
                job_id: row.get(1).map_err(sqlite::map_sqlite)?,
                agent_id: column_text(row, 2)?,
                session_id: column_text(row, 3)?,
                workspace_id: column_text(row, 4)?,
                event_type: column_text(row, 5)?,
                payload_digest: column_text(row, 6)?,
                created_at_ms: column_u64(row, 7)?,
            })
        },
    )
}

// ---------------------------------------------------------------------------------------------
// The journal
// ---------------------------------------------------------------------------------------------

/// The kind of a row of the journal.
pub(crate) const KIND_SESSION: &str = "session";
pub(crate) const KIND_JOB: &str = "job";
pub(crate) const KIND_LEASE: &str = "lease";
pub(crate) const KIND_IDEMPOTENCY: &str = "idempotency";
pub(crate) const KIND_FILE_GENERATION: &str = "file_generation";
pub(crate) const KIND_FILE_CONFLICT: &str = "file_conflict";

fn journal_session(
    conn: &Connection,
    row: &AgentRegistration,
    now_ms: u64,
) -> Result<(), TaseError> {
    sqlite::append_journal(conn, KIND_SESSION, &session_to_json(row), now_ms)
}

fn journal_job(conn: &Connection, row: &Job, now_ms: u64) -> Result<(), TaseError> {
    sqlite::append_journal(conn, KIND_JOB, &job_to_json(row), now_ms)
}

fn journal_lease(conn: &Connection, row: &Lease, now_ms: u64) -> Result<(), TaseError> {
    sqlite::append_journal(conn, KIND_LEASE, &lease_to_json(row), now_ms)
}

fn journal_idempotency(
    conn: &Connection,
    row: &IdempotencyRecord,
    now_ms: u64,
) -> Result<(), TaseError> {
    sqlite::append_journal(conn, KIND_IDEMPOTENCY, &idempotency_to_json(row), now_ms)
}

fn journal_file_generation(
    conn: &Connection,
    row: &FileGeneration,
    now_ms: u64,
) -> Result<(), TaseError> {
    sqlite::append_journal(conn, KIND_FILE_GENERATION, &file_generation_to_json(row), now_ms)
}

fn journal_file_conflict(
    conn: &Connection,
    row: &FileConflict,
    now_ms: u64,
) -> Result<(), TaseError> {
    sqlite::append_journal(conn, KIND_FILE_CONFLICT, &file_conflict_to_json(row), now_ms)
}

// ---------------------------------------------------------------------------------------------
// The counters
// ---------------------------------------------------------------------------------------------

/// Gives the next number of a counter and grows it by one.
///
/// The call must be inside a write transaction: the number is taken and the counter is grown in one step, so two
/// writers never get the same number.
pub(crate) fn next_counter(conn: &Connection, name: &str) -> Result<u64, TaseError> {
    conn.execute(
        "INSERT INTO local_counters (name, value) VALUES (?1, 1)
         ON CONFLICT(name) DO UPDATE SET value = value + 1",
        rusqlite::params![name],
    )
    .map_err(sqlite::map_sqlite)?;
    let value: i64 = conn
        .query_row(
            "SELECT value FROM local_counters WHERE name = ?1",
            rusqlite::params![name],
            |row| row.get(0),
        )
        .map_err(sqlite::map_sqlite)?;
    Ok(value as u64)
}

// ---------------------------------------------------------------------------------------------
// The helpers
// ---------------------------------------------------------------------------------------------

/// The scope of an idempotency key: the workspace and the agent, as the unique constraint of the design book says.
#[must_use]
pub fn idempotency_scope(workspace_id: &WorkspaceId, agent_id: &AgentId) -> String {
    format!("{}|{}", workspace_id.as_str(), agent_id.as_str())
}

/// Takes back the leases of a resource that ran out of time.
fn reclaim_expired_of(
    conn: &Connection,
    resource: &str,
    now_ms: u64,
) -> Result<Vec<Lease>, TaseError> {
    let sql = format!(
        "SELECT {LEASE_COLUMNS} FROM local_leases
          WHERE resource = ?1 AND state = 'HELD' AND expires_at_ms <= ?2"
    );
    let mut leases = read_all(
        conn,
        &sql,
        rusqlite::params![resource, now_ms as i64],
        read_lease,
    )?;
    for lease in &mut leases {
        lease.state = LeaseState::Expired;
        upsert_lease(conn, lease)?;
        journal_lease(conn, lease, now_ms)?;
    }
    Ok(leases)
}

/// Encodes a list of texts as the JSON text the table holds.
fn encode_str_list(items: &[String]) -> String {
    Json::strs(items).to_json_string()
}

/// Decodes the JSON text of a list of texts.
fn decode_str_list(text: &str) -> Result<Vec<String>, TaseError> {
    let value = parse(text, Limits::DEFAULT)
        .map_err(|error| TaseError::internal(format!("the stored list is not JSON: {error}")))?;
    match value {
        Json::Arr(items) => items
            .into_iter()
            .map(|item| {
                item.as_str().map(str::to_string).ok_or_else(|| {
                    TaseError::internal("the stored list holds a value that is not a text")
                })
            })
            .collect(),
        _ => Err(TaseError::internal("the stored list is not an array")),
    }
}

// ---------------------------------------------------------------------------------------------
// The row images
// ---------------------------------------------------------------------------------------------

/// The row image of a session.
#[must_use]
pub fn session_to_json(row: &AgentRegistration) -> Json {
    Json::obj(vec![
        ("session_id", Json::str(row.session_id.as_str())),
        ("agent_id", Json::str(row.agent_id.as_str())),
        ("provider", Json::str(&row.provider)),
        ("instance_id", Json::str(&row.instance_id)),
        ("workspace_id", Json::str(row.workspace_id.as_str())),
        ("owner_logon_sid_hash", Json::str(&row.owner_logon_sid_hash)),
        ("granted_scopes", Json::strs(&row.granted_scopes)),
        ("capabilities", Json::strs(&row.capabilities)),
        ("state", Json::str(row.state.as_str())),
        ("lease_epoch", Json::int(row.lease_epoch as i64)),
        ("heartbeat_at_ms", Json::int(row.heartbeat_at_ms as i64)),
        ("created_at_ms", Json::int(row.created_at_ms as i64)),
    ])
}

/// The row image of a job.
#[must_use]
pub fn job_to_json(row: &Job) -> Json {
    Json::obj(vec![
        ("job_id", Json::str(row.job_id.as_str())),
        ("job_type", Json::str(&row.job_type)),
        ("workspace_id", Json::str(row.workspace_id.as_str())),
        ("agent_id", Json::str(row.agent_id.as_str())),
        ("session_id", Json::str(row.session_id.as_str())),
        ("idempotency_key", Json::str(&row.idempotency_key)),
        ("request_digest", Json::str(&row.request_digest)),
        ("base_snapshot_digest", Json::str(&row.base_snapshot_digest)),
        ("status", Json::str(row.status.as_str())),
        ("attempt", Json::int(row.attempt as i64)),
        ("lease_epoch", Json::int(row.lease_epoch as i64)),
        ("worker_id", optional_str(&row.worker_id)),
        ("result_digest", optional_str(&row.result_digest)),
        ("commit_intent_digest", optional_str(&row.commit_intent_digest)),
        ("committed_ref", optional_str(&row.committed_ref)),
        ("committed_oid", optional_str(&row.committed_oid)),
        (
            "error_code",
            match row.error_code {
                Some(code) => Json::str(code.as_str()),
                None => Json::Null,
            },
        ),
        ("priority", Json::int(row.priority as i64)),
        ("created_at_ms", Json::int(row.created_at_ms as i64)),
        ("updated_at_ms", Json::int(row.updated_at_ms as i64)),
    ])
}

/// The row image of a lease.
#[must_use]
pub fn lease_to_json(row: &Lease) -> Json {
    Json::obj(vec![
        ("lease_id", Json::str(&row.lease_id)),
        ("resource", Json::str(&row.resource)),
        ("holder", Json::str(&row.holder)),
        (
            "session_id",
            match &row.session_id {
                Some(id) => Json::str(id.as_str()),
                None => Json::Null,
            },
        ),
        ("fencing_token", Json::int(row.fencing_token as i64)),
        ("acquired_at_ms", Json::int(row.acquired_at_ms as i64)),
        ("expires_at_ms", Json::int(row.expires_at_ms as i64)),
        (
            "released_at_ms",
            match row.released_at_ms {
                Some(value) => Json::int(value as i64),
                None => Json::Null,
            },
        ),
        ("state", Json::str(row.state.as_str())),
    ])
}

/// The row image of an idempotency record.
#[must_use]
pub fn idempotency_to_json(row: &IdempotencyRecord) -> Json {
    Json::obj(vec![
        ("scope", Json::str(&row.scope)),
        ("idempotency_key", Json::str(&row.idempotency_key)),
        ("request_digest", Json::str(&row.request_digest)),
        ("response_digest", Json::str(&row.response_digest)),
        ("job_id", Json::str(row.job_id.as_str())),
        ("created_at_ms", Json::int(row.created_at_ms as i64)),
    ])
}

/// The row image of a generation of a file.
#[must_use]
pub fn file_generation_to_json(row: &FileGeneration) -> Json {
    Json::obj(vec![
        ("workspace_id", Json::str(row.workspace_id.as_str())),
        ("normalized_path", Json::str(&row.normalized_path)),
        ("generation", Json::int(row.generation as i64)),
        ("content_digest", Json::str(&row.content_digest)),
        ("updated_at_ms", Json::int(row.updated_at_ms as i64)),
    ])
}

/// The row image of a conflict of a file.
#[must_use]
pub fn file_conflict_to_json(row: &FileConflict) -> Json {
    Json::obj(vec![
        ("conflict_id", Json::int(row.conflict_id as i64)),
        ("workspace_id", Json::str(row.workspace_id.as_str())),
        ("normalized_path", Json::str(&row.normalized_path)),
        ("expected_generation", Json::int(row.expected_generation as i64)),
        ("actual_generation", Json::int(row.actual_generation as i64)),
        ("agent_id", Json::str(row.agent_id.as_str())),
        ("session_id", Json::str(row.session_id.as_str())),
        (
            "job_id",
            match &row.job_id {
                Some(id) => Json::str(id.as_str()),
                None => Json::Null,
            },
        ),
        ("state", Json::str(row.state.as_str())),
        ("detected_at_ms", Json::int(row.detected_at_ms as i64)),
    ])
}

fn optional_str(value: &Option<String>) -> Json {
    match value {
        Some(text) => Json::str(text),
        None => Json::Null,
    }
}

/// The field of a row image, or an internal failure when it is not there.
pub(crate) fn json_field<'a>(value: &'a Json, key: &str) -> Result<&'a Json, TaseError> {
    value.get(key).ok_or_else(|| {
        TaseError::internal(format!("the row image has no field {key:?}"))
    })
}

/// The text of a field of a row image.
pub(crate) fn json_str(value: &Json, key: &str) -> Result<String, TaseError> {
    json_field(value, key)?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| TaseError::internal(format!("the field {key:?} is not a text")))
}

/// The number of a field of a row image.
pub(crate) fn json_u64(value: &Json, key: &str) -> Result<u64, TaseError> {
    json_field(value, key)?
        .as_i64()
        .and_then(|number| u64::try_from(number).ok())
        .ok_or_else(|| TaseError::internal(format!("the field {key:?} is not a number")))
}

/// The text of a field of a row image, or `None` when the field is null.
pub(crate) fn json_opt_str(value: &Json, key: &str) -> Result<Option<String>, TaseError> {
    let field = json_field(value, key)?;
    if field.is_null() {
        return Ok(None);
    }
    field
        .as_str()
        .map(|text| Some(text.to_string()))
        .ok_or_else(|| TaseError::internal(format!("the field {key:?} is not a text")))
}

/// The number of a field of a row image, or `None` when the field is null.
pub(crate) fn json_opt_u64(value: &Json, key: &str) -> Result<Option<u64>, TaseError> {
    let field = json_field(value, key)?;
    if field.is_null() {
        return Ok(None);
    }
    field
        .as_i64()
        .and_then(|number| u64::try_from(number).ok())
        .map(Some)
        .ok_or_else(|| TaseError::internal(format!("the field {key:?} is not a number")))
}

/// The list of texts of a field of a row image.
pub(crate) fn json_str_list(value: &Json, key: &str) -> Result<Vec<String>, TaseError> {
    let items = json_field(value, key)?
        .as_array()
        .ok_or_else(|| TaseError::internal(format!("the field {key:?} is not an array")))?;
    items
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| TaseError::internal(format!("the field {key:?} holds a value that is not a text")))
        })
        .collect()
}

/// The row image of a session, read back.
pub fn session_from_json(value: &Json) -> Result<AgentRegistration, TaseError> {
    let state = json_str(value, "state")?;
    Ok(AgentRegistration {
        session_id: SessionId::new(json_str(value, "session_id")?)?,
        agent_id: AgentId::new(json_str(value, "agent_id")?)?,
        provider: json_str(value, "provider")?,
        instance_id: json_str(value, "instance_id")?,
        workspace_id: WorkspaceId::new(json_str(value, "workspace_id")?)?,
        owner_logon_sid_hash: json_str(value, "owner_logon_sid_hash")?,
        granted_scopes: json_str_list(value, "granted_scopes")?,
        capabilities: json_str_list(value, "capabilities")?,
        state: SessionState::from_name(&state)
            .ok_or_else(|| TaseError::internal(format!("unknown state of a session {state:?}")))?,
        lease_epoch: json_u64(value, "lease_epoch")?,
        heartbeat_at_ms: json_u64(value, "heartbeat_at_ms")?,
        created_at_ms: json_u64(value, "created_at_ms")?,
    })
}

/// The row image of a job, read back.
pub fn job_from_json(value: &Json) -> Result<Job, TaseError> {
    let status = json_str(value, "status")?;
    let error_code = json_opt_str(value, "error_code")?;
    Ok(Job {
        job_id: JobId::new(json_str(value, "job_id")?)?,
        job_type: json_str(value, "job_type")?,
        workspace_id: WorkspaceId::new(json_str(value, "workspace_id")?)?,
        agent_id: AgentId::new(json_str(value, "agent_id")?)?,
        session_id: SessionId::new(json_str(value, "session_id")?)?,
        idempotency_key: json_str(value, "idempotency_key")?,
        request_digest: json_str(value, "request_digest")?,
        base_snapshot_digest: json_str(value, "base_snapshot_digest")?,
        status: JobStatus::from_name(&status)
            .ok_or_else(|| TaseError::internal(format!("unknown state of a job {status:?}")))?,
        attempt: json_u64(value, "attempt")? as u32,
        lease_epoch: json_u64(value, "lease_epoch")?,
        worker_id: json_opt_str(value, "worker_id")?,
        result_digest: json_opt_str(value, "result_digest")?,
        commit_intent_digest: json_opt_str(value, "commit_intent_digest")?,
        committed_ref: json_opt_str(value, "committed_ref")?,
        committed_oid: json_opt_str(value, "committed_oid")?,
        error_code: error_code
            .as_deref()
            .and_then(ErrorCode::from_str),
        priority: json_u64(value, "priority")? as u8,
        created_at_ms: json_u64(value, "created_at_ms")?,
        updated_at_ms: json_u64(value, "updated_at_ms")?,
    })
}

/// The row image of a lease, read back.
pub fn lease_from_json(value: &Json) -> Result<Lease, TaseError> {
    let state = json_str(value, "state")?;
    let session_id = json_opt_str(value, "session_id")?;
    Ok(Lease {
        lease_id: json_str(value, "lease_id")?,
        resource: json_str(value, "resource")?,
        holder: json_str(value, "holder")?,
        session_id: match session_id {
            Some(id) => Some(SessionId::new(id)?),
            None => None,
        },
        fencing_token: json_u64(value, "fencing_token")?,
        acquired_at_ms: json_u64(value, "acquired_at_ms")?,
        expires_at_ms: json_u64(value, "expires_at_ms")?,
        released_at_ms: json_opt_u64(value, "released_at_ms")?,
        state: LeaseState::from_name(&state)
            .ok_or_else(|| TaseError::internal(format!("unknown state of a lease {state:?}")))?,
    })
}

/// The row image of an idempotency record, read back.
pub fn idempotency_from_json(value: &Json) -> Result<IdempotencyRecord, TaseError> {
    Ok(IdempotencyRecord {
        scope: json_str(value, "scope")?,
        idempotency_key: json_str(value, "idempotency_key")?,
        request_digest: json_str(value, "request_digest")?,
        response_digest: json_str(value, "response_digest")?,
        job_id: JobId::new(json_str(value, "job_id")?)?,
        created_at_ms: json_u64(value, "created_at_ms")?,
    })
}

/// The row image of a generation of a file, read back.
pub fn file_generation_from_json(value: &Json) -> Result<FileGeneration, TaseError> {
    Ok(FileGeneration {
        workspace_id: WorkspaceId::new(json_str(value, "workspace_id")?)?,
        normalized_path: json_str(value, "normalized_path")?,
        generation: json_u64(value, "generation")?,
        content_digest: json_str(value, "content_digest")?,
        updated_at_ms: json_u64(value, "updated_at_ms")?,
    })
}

/// The row image of a conflict of a file, read back.
pub fn file_conflict_from_json(value: &Json) -> Result<FileConflict, TaseError> {
    let state = json_str(value, "state")?;
    let job_id = json_opt_str(value, "job_id")?;
    Ok(FileConflict {
        conflict_id: json_u64(value, "conflict_id")?,
        workspace_id: WorkspaceId::new(json_str(value, "workspace_id")?)?,
        normalized_path: json_str(value, "normalized_path")?,
        expected_generation: json_u64(value, "expected_generation")?,
        actual_generation: json_u64(value, "actual_generation")?,
        agent_id: AgentId::new(json_str(value, "agent_id")?)?,
        session_id: SessionId::new(json_str(value, "session_id")?)?,
        job_id: match job_id {
            Some(id) => Some(JobId::new(id)?),
            None => None,
        },
        state: ConflictState::from_name(&state)
            .ok_or_else(|| TaseError::internal(format!("unknown state of a conflict {state:?}")))?,
        detected_at_ms: json_u64(value, "detected_at_ms")?,
    })
}

// ---------------------------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------------------------

/// The helpers the tests of the crate share.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// The number that makes the name of a directory of a test unique.
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// A directory of the disk that only one test uses.
    pub fn temp_dir(name: &str) -> PathBuf {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "tase-local-store-{name}-{}-{nanos}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("the directory is made");
        path
    }

    /// Removes a directory of the disk that a test made.
    pub fn remove_dir(path: &Path) {
        let _ = std::fs::remove_dir_all(path);
    }

    /// A store of the disk, with the steps of the schema applied.
    pub fn new_store(dir: &Path) -> Store {
        let mut store = Store::open(dir.join("hub.sqlite3")).expect("the store opens");
        migrations::apply(&mut store).expect("the steps run");
        store
    }

    /// Opens a session of an agent and gives its id back.
    pub fn new_session(
        store: &mut Store,
        session_id: &str,
        agent_id: &str,
        now_ms: u64,
    ) -> SessionId {
        let session = SessionId::new(session_id).expect("the id of the session is legal");
        store
            .open_session(NewSession {
                session_id: session.clone(),
                agent_id: AgentId::new(agent_id).expect("the id of the agent is legal"),
                provider: "local".into(),
                instance_id: "instance_1".into(),
                workspace_id: WorkspaceId::new("ws_1").expect("the id of the workspace is legal"),
                owner_logon_sid_hash: "sid_hash".into(),
                granted_scopes: vec!["code:write".into(), "code:read".into()],
                capabilities: vec!["plan".into(), "apply".into()],
                now_ms,
            })
            .expect("the session is opened");
        session
    }

    /// The job of a session, registered with the given key and digest.
    pub fn new_job(
        store: &mut Store,
        session_id: &SessionId,
        idempotency_key: &str,
        request_digest: &str,
        now_ms: u64,
    ) -> JobId {
        store
            .submit_job(NewJob {
                job_type: "apply".into(),
                workspace_id: WorkspaceId::new("ws_1").expect("the id of the workspace is legal"),
                agent_id: AgentId::new("agent_1").expect("the id of the agent is legal"),
                session_id: session_id.clone(),
                idempotency_key: idempotency_key.into(),
                request_digest: request_digest.into(),
                base_snapshot_digest: "base_digest".into(),
                priority: 5,
                now_ms,
            })
            .expect("the job is registered")
            .job_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{new_job, new_session, new_store, temp_dir};

    // -- the sessions ------------------------------------------------------------------------

    #[test]
    fn a_session_is_opened_and_read_back() {
        let dir = temp_dir("session-open");
        let mut store = new_store(&dir);
        let session = new_session(&mut store, "sess_1", "agent_1", 1_000);
        let row = store
            .get_session(&session)
            .expect("the session is read")
            .expect("the session is there");
        assert_eq!(row.agent_id.as_str(), "agent_1");
        assert_eq!(row.provider, "local");
        assert_eq!(row.instance_id, "instance_1");
        assert_eq!(row.workspace_id.as_str(), "ws_1");
        assert_eq!(row.state, SessionState::Connected);
        assert_eq!(row.created_at_ms, 1_000);
        assert_eq!(row.heartbeat_at_ms, 1_000);
        assert_eq!(row.granted_scopes, vec!["code:write", "code:read"]);
        assert_eq!(row.capabilities, vec!["plan", "apply"]);
        test_support::remove_dir(&dir);
    }

    #[test]
    fn a_session_that_is_already_known_is_refused() {
        let dir = temp_dir("session-twice");
        let mut store = new_store(&dir);
        let _ = new_session(&mut store, "sess_1", "agent_1", 1_000);
        let error = store
            .open_session(NewSession {
                session_id: SessionId::new("sess_1").unwrap(),
                agent_id: AgentId::new("agent_2").unwrap(),
                provider: "local".into(),
                instance_id: "instance_2".into(),
                workspace_id: WorkspaceId::new("ws_1").unwrap(),
                owner_logon_sid_hash: "sid_hash".into(),
                granted_scopes: vec![],
                capabilities: vec![],
                now_ms: 2_000,
            })
            .expect_err("the second registration must be refused");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        test_support::remove_dir(&dir);
    }

    #[test]
    fn a_heartbeat_moves_the_time_of_the_session() {
        let dir = temp_dir("session-heartbeat");
        let mut store = new_store(&dir);
        let session = new_session(&mut store, "sess_1", "agent_1", 1_000);
        store.heartbeat(&session, 5_000).expect("the heartbeat is written");
        let row = store.get_session(&session).unwrap().unwrap();
        assert_eq!(row.heartbeat_at_ms, 5_000);
        assert_eq!(row.created_at_ms, 1_000);
        test_support::remove_dir(&dir);
    }

    #[test]
    fn a_heartbeat_of_an_unknown_session_is_not_found() {
        let dir = temp_dir("session-heartbeat-missing");
        let mut store = new_store(&dir);
        let error = store
            .heartbeat(&SessionId::new("sess_nope").unwrap(), 1_000)
            .expect_err("the heartbeat must fail");
        assert_eq!(error.code, ErrorCode::NotFound);
        test_support::remove_dir(&dir);
    }

    #[test]
    fn a_heartbeat_of_a_closed_session_is_unauthenticated() {
        let dir = temp_dir("session-heartbeat-closed");
        let mut store = new_store(&dir);
        let session = new_session(&mut store, "sess_1", "agent_1", 1_000);
        store.unregister_session(&session).expect("the session is closed");
        let error = store
            .heartbeat(&session, 2_000)
            .expect_err("the heartbeat must fail");
        assert_eq!(error.code, ErrorCode::Unauthenticated);
        test_support::remove_dir(&dir);
    }

    #[test]
    fn unregistering_a_session_closes_it() {
        let dir = temp_dir("session-unregister");
        let mut store = new_store(&dir);
        let session = new_session(&mut store, "sess_1", "agent_1", 1_000);
        store.unregister_session(&session).expect("the session is closed");
        let row = store.get_session(&session).unwrap().unwrap();
        assert_eq!(row.state, SessionState::Disconnected);
        test_support::remove_dir(&dir);
    }

    #[test]
    fn unregistering_twice_changes_nothing() {
        let dir = temp_dir("session-unregister-twice");
        let mut store = new_store(&dir);
        let session = new_session(&mut store, "sess_1", "agent_1", 1_000);
        store.unregister_session(&session).expect("the session is closed");
        store.unregister_session(&session).expect("the second close is accepted");
        let row = store.get_session(&session).unwrap().unwrap();
        assert_eq!(row.state, SessionState::Disconnected);
        test_support::remove_dir(&dir);
    }

    #[test]
    fn the_sessions_are_listed() {
        let dir = temp_dir("session-list");
        let mut store = new_store(&dir);
        let _ = new_session(&mut store, "sess_1", "agent_1", 1_000);
        let _ = new_session(&mut store, "sess_2", "agent_2", 2_000);
        let rows = store.list_sessions().expect("the sessions are listed");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].session_id.as_str(), "sess_1");
        assert_eq!(rows[1].session_id.as_str(), "sess_2");
        test_support::remove_dir(&dir);
    }

    // -- the jobs ----------------------------------------------------------------------------

    #[test]
    fn a_job_is_registered_and_read_back() {
        let dir = temp_dir("job-register");
        let mut store = new_store(&dir);
        let session = new_session(&mut store, "sess_1", "agent_1", 1_000);
        let job_id = new_job(&mut store, &session, "key_1", "dig_1", 1_100);
        let job = store.get_job(&job_id).unwrap().expect("the job is there");
        assert_eq!(job.job_type, "apply");
        assert_eq!(job.status, JobStatus::Queued);
        assert_eq!(job.attempt, 0);
        assert_eq!(job.idempotency_key, "key_1");
        assert_eq!(job.request_digest, "dig_1");
        assert_eq!(job.workspace_id.as_str(), "ws_1");
        assert_eq!(job.agent_id.as_str(), "agent_1");
        assert_eq!(job.created_at_ms, 1_100);
        test_support::remove_dir(&dir);
    }

    #[test]
    fn the_same_key_with_the_same_digest_gives_the_same_job() {
        let dir = temp_dir("job-idempotent");
        let mut store = new_store(&dir);
        let session = new_session(&mut store, "sess_1", "agent_1", 1_000);
        let first = store
            .submit_job(NewJob {
                job_type: "apply".into(),
                workspace_id: WorkspaceId::new("ws_1").unwrap(),
                agent_id: AgentId::new("agent_1").unwrap(),
                session_id: session.clone(),
                idempotency_key: "key_1".into(),
                request_digest: "dig_1".into(),
                base_snapshot_digest: "base".into(),
                priority: 5,
                now_ms: 1_100,
            })
            .expect("the job is registered");
        assert!(!first.deduplicated);
        let second = store
            .submit_job(NewJob {
                job_type: "apply".into(),
                workspace_id: WorkspaceId::new("ws_1").unwrap(),
                agent_id: AgentId::new("agent_1").unwrap(),
                session_id: session.clone(),
                idempotency_key: "key_1".into(),
                request_digest: "dig_1".into(),
                base_snapshot_digest: "base".into(),
                priority: 5,
                now_ms: 1_200,
            })
            .expect("the job is registered again");
        assert!(second.deduplicated);
        assert_eq!(first.job_id, second.job_id);
        assert_eq!(store.list_jobs().unwrap().len(), 1);
        test_support::remove_dir(&dir);
    }

    #[test]
    fn the_same_key_with_another_digest_is_a_conflict() {
        let dir = temp_dir("job-idempotency-mismatch");
        let mut store = new_store(&dir);
        let session = new_session(&mut store, "sess_1", "agent_1", 1_000);
        let _ = new_job(&mut store, &session, "key_1", "dig_1", 1_100);
        let error = store
            .submit_job(NewJob {
                job_type: "apply".into(),
                workspace_id: WorkspaceId::new("ws_1").unwrap(),
                agent_id: AgentId::new("agent_1").unwrap(),
                session_id: session,
                idempotency_key: "key_1".into(),
                request_digest: "dig_2".into(),
                base_snapshot_digest: "base".into(),
                priority: 5,
                now_ms: 1_200,
            })
            .expect_err("the second digest must be refused");
        assert_eq!(error.code, ErrorCode::IdempotencyMismatch);
        assert_eq!(store.list_jobs().unwrap().len(), 1);
        test_support::remove_dir(&dir);
    }

    #[test]
    fn the_scope_of_a_key_is_the_workspace_and_the_agent() {
        let dir = temp_dir("job-scope");
        let mut store = new_store(&dir);
        let session = new_session(&mut store, "sess_1", "agent_1", 1_000);
        let _ = new_job(&mut store, &session, "key_1", "dig_1", 1_100);
        let scope = idempotency_scope(
            &WorkspaceId::new("ws_1").unwrap(),
            &AgentId::new("agent_1").unwrap(),
        );
        let record = store
            .lookup_idempotency(&scope, "key_1")
            .unwrap()
            .expect("the record is there");
        assert_eq!(record.request_digest, "dig_1");
        assert_eq!(record.idempotency_key, "key_1");
        assert_eq!(store.list_idempotency().unwrap().len(), 1);
        test_support::remove_dir(&dir);
    }

    #[test]
    fn a_job_of_an_unknown_session_is_not_found() {
        let dir = temp_dir("job-no-session");
        let mut store = new_store(&dir);
        let error = store
            .submit_job(NewJob {
                job_type: "apply".into(),
                workspace_id: WorkspaceId::new("ws_1").unwrap(),
                agent_id: AgentId::new("agent_1").unwrap(),
                session_id: SessionId::new("sess_nope").unwrap(),
                idempotency_key: "key_1".into(),
                request_digest: "dig_1".into(),
                base_snapshot_digest: "base".into(),
                priority: 5,
                now_ms: 1_000,
            })
            .expect_err("the job must be refused");
        assert_eq!(error.code, ErrorCode::NotFound);
        test_support::remove_dir(&dir);
    }

    #[test]
    fn a_job_goes_from_one_state_to_the_next() {
        let dir = temp_dir("job-transition");
        let mut store = new_store(&dir);
        let session = new_session(&mut store, "sess_1", "agent_1", 1_000);
        let job_id = new_job(&mut store, &session, "key_1", "dig_1", 1_100);
        let moved = store
            .transition_job(&job_id, JobStatus::Queued, JobStatus::Running, 0, 1_200)
            .expect("the job moves");
        assert!(moved);
        let job = store.get_job(&job_id).unwrap().unwrap();
        assert_eq!(job.status, JobStatus::Running);
        assert_eq!(job.updated_at_ms, 1_200);
        test_support::remove_dir(&dir);
    }

    #[test]
    fn a_job_with_another_state_does_not_move() {
        let dir = temp_dir("job-transition-state");
        let mut store = new_store(&dir);
        let session = new_session(&mut store, "sess_1", "agent_1", 1_000);
        let job_id = new_job(&mut store, &session, "key_1", "dig_1", 1_100);
        let moved = store
            .transition_job(&job_id, JobStatus::Verifying, JobStatus::Running, 0, 1_200)
            .expect("the move is refused, not the call");
        assert!(!moved);
        assert_eq!(
            store.get_job(&job_id).unwrap().unwrap().status,
            JobStatus::Queued
        );
        test_support::remove_dir(&dir);
    }

    #[test]
    fn a_worker_with_an_old_epoch_does_not_move_the_job() {
        let dir = temp_dir("job-transition-epoch");
        let mut store = new_store(&dir);
        let session = new_session(&mut store, "sess_1", "agent_1", 1_000);
        let job_id = new_job(&mut store, &session, "key_1", "dig_1", 1_100);
        let _ = store
            .lease_job(&job_id, "worker_1", 10_000, 1_200)
            .expect("the job is leased");
        let job = store.get_job(&job_id).unwrap().unwrap();
        assert_eq!(job.lease_epoch, 1);
        // the worker that still believes in the epoch 0 may not move the job
        let moved = store
            .transition_job(&job_id, JobStatus::Running, JobStatus::Verifying, 0, 1_300)
            .expect("the move is refused, not the call");
        assert!(!moved);
        assert_eq!(
            store.get_job(&job_id).unwrap().unwrap().status,
            JobStatus::Running
        );
        test_support::remove_dir(&dir);
    }

    #[test]
    fn a_finished_job_keeps_its_terminal_state() {
        let dir = temp_dir("job-finish");
        let mut store = new_store(&dir);
        let session = new_session(&mut store, "sess_1", "agent_1", 1_000);
        let job_id = new_job(&mut store, &session, "key_1", "dig_1", 1_100);
        let finished = store
            .finish_job(&job_id, JobStatus::Committed, Some("dig_out".into()), None, 1_200)
            .expect("the job is finished");
        assert!(finished);
        let again = store
            .finish_job(&job_id, JobStatus::Failed, None, Some(ErrorCode::ValidationFailed), 1_300)
            .expect("the second notification is accepted");
        assert!(!again);
        let job = store.get_job(&job_id).unwrap().unwrap();
        assert_eq!(job.status, JobStatus::Committed);
        assert_eq!(job.result_digest.as_deref(), Some("dig_out"));
        test_support::remove_dir(&dir);
    }

    #[test]
    fn the_jobs_of_a_state_are_listed() {
        let dir = temp_dir("job-list");
        let mut store = new_store(&dir);
        let session = new_session(&mut store, "sess_1", "agent_1", 1_000);
        let first = new_job(&mut store, &session, "key_1", "dig_1", 1_100);
        let _ = new_job(&mut store, &session, "key_2", "dig_2", 1_110);
        store
            .transition_job(&first, JobStatus::Queued, JobStatus::Running, 0, 1_200)
            .unwrap();
        assert_eq!(store.list_jobs_by_status(JobStatus::Queued).unwrap().len(), 1);
        assert_eq!(
            store.list_jobs_by_status(JobStatus::Running).unwrap().len(),
            1
        );
        assert_eq!(store.list_jobs().unwrap().len(), 2);
        test_support::remove_dir(&dir);
    }

    #[test]
    fn the_outbox_holds_the_job_and_may_be_published() {
        let dir = temp_dir("outbox");
        let mut store = new_store(&dir);
        let session = new_session(&mut store, "sess_1", "agent_1", 1_000);
        let _ = new_job(&mut store, &session, "key_1", "dig_1", 1_100);
        let events = store.list_unpublished_outbox().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].payload_digest, "dig_1");
        assert!(store.mark_outbox_published(events[0].event_id, 1_200).unwrap());
        assert!(!store.mark_outbox_published(events[0].event_id, 1_300).unwrap());
        assert!(store.list_unpublished_outbox().unwrap().is_empty());
        test_support::remove_dir(&dir);
    }

    #[test]
    fn the_audit_trail_holds_the_events() {
        let dir = temp_dir("audit");
        let mut store = new_store(&dir);
        let seq = store
            .audit(AuditRecord {
                event_type: "RECOVERED".into(),
                agent_id: "agent_1".into(),
                session_id: "sess_1".into(),
                workspace_id: "ws_1".into(),
                job_id: None,
                payload_digest: "dig".into(),
                now_ms: 1_000,
            })
            .expect("the event is written");
        assert!(seq > 0);
        let events = store.list_audit_events().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "RECOVERED");
        assert_eq!(events[0].agent_id, "agent_1");
        assert_eq!(events[0].created_at_ms, 1_000);
        test_support::remove_dir(&dir);
    }

    // -- the leases --------------------------------------------------------------------------

    #[test]
    fn a_lease_is_taken_and_held() {
        let dir = temp_dir("lease-take");
        let mut store = new_store(&dir);
        let lease = store
            .acquire_lease(NewLease {
                resource: "res_1".into(),
                holder: "worker_1".into(),
                session_id: None,
                ttl_ms: 5_000,
                now_ms: 1_000,
            })
            .expect("the lease is taken");
        assert_eq!(lease.holder, "worker_1");
        assert_eq!(lease.state, LeaseState::Held);
        assert_eq!(lease.acquired_at_ms, 1_000);
        assert_eq!(lease.expires_at_ms, 6_000);
        assert!(lease.fencing_token > 0);
        assert!(lease.is_valid_at(2_000));
        assert!(!lease.is_valid_at(7_000));
        test_support::remove_dir(&dir);
    }

    #[test]
    fn a_resource_that_is_held_is_rate_limited() {
        let dir = temp_dir("lease-taken");
        let mut store = new_store(&dir);
        let _ = store
            .acquire_lease(NewLease {
                resource: "res_1".into(),
                holder: "worker_1".into(),
                session_id: None,
                ttl_ms: 5_000,
                now_ms: 1_000,
            })
            .expect("the lease is taken");
        let error = store
            .acquire_lease(NewLease {
                resource: "res_1".into(),
                holder: "worker_2".into(),
                session_id: None,
                ttl_ms: 5_000,
                now_ms: 1_100,
            })
            .expect_err("the second holder must be refused");
        assert_eq!(error.code, ErrorCode::RateLimited);
        assert!(error.code.retryable());
        assert_eq!(store.list_leases().unwrap().len(), 1);
        test_support::remove_dir(&dir);
    }

    #[test]
    fn a_lease_that_ran_out_may_be_taken_again() {
        let dir = temp_dir("lease-expired");
        let mut store = new_store(&dir);
        let first = store
            .acquire_lease(NewLease {
                resource: "res_1".into(),
                holder: "worker_1".into(),
                session_id: None,
                ttl_ms: 1_000,
                now_ms: 1_000,
            })
            .expect("the lease is taken");
        let second = store
            .acquire_lease(NewLease {
                resource: "res_1".into(),
                holder: "worker_2".into(),
                session_id: None,
                ttl_ms: 1_000,
                now_ms: 3_000,
            })
            .expect("the lease is taken again");
        assert_ne!(first.lease_id, second.lease_id);
        assert!(second.fencing_token > first.fencing_token);
        let old = store.get_lease(&first.lease_id).unwrap().unwrap();
        assert_eq!(old.state, LeaseState::Expired);
        test_support::remove_dir(&dir);
    }

    #[test]
    fn the_fencing_token_only_grows() {
        let dir = temp_dir("lease-fencing");
        let mut store = new_store(&dir);
        let mut previous = 0;
        for index in 0..5 {
            let lease = store
                .acquire_lease(NewLease {
                    resource: format!("res_{index}"),
                    holder: "worker_1".into(),
                    session_id: None,
                    ttl_ms: 5_000,
                    now_ms: 1_000,
                })
                .expect("the lease is taken");
            assert!(lease.fencing_token > previous, "the token must grow");
            previous = lease.fencing_token;
        }
        // a lease that is taken again after the end of the first one grows the token too
        let again = store
            .acquire_lease(NewLease {
                resource: "res_0".into(),
                holder: "worker_2".into(),
                session_id: None,
                ttl_ms: 5_000,
                now_ms: 100_000,
            })
            .expect("the lease is taken again");
        assert!(again.fencing_token > previous);
        test_support::remove_dir(&dir);
    }

    #[test]
    fn a_lease_is_renewed() {
        let dir = temp_dir("lease-renew");
        let mut store = new_store(&dir);
        let lease = store
            .acquire_lease(NewLease {
                resource: "res_1".into(),
                holder: "worker_1".into(),
                session_id: None,
                ttl_ms: 5_000,
                now_ms: 1_000,
            })
            .expect("the lease is taken");
        let renewed = store
            .renew_lease(&lease.lease_id, "worker_1", 20_000, 2_000)
            .expect("the lease is renewed");
        assert_eq!(renewed.expires_at_ms, 20_000);
        assert_eq!(renewed.fencing_token, lease.fencing_token);
        test_support::remove_dir(&dir);
    }

    #[test]
    fn a_lease_of_another_holder_may_not_be_renewed() {
        let dir = temp_dir("lease-renew-other");
        let mut store = new_store(&dir);
        let lease = store
            .acquire_lease(NewLease {
                resource: "res_1".into(),
                holder: "worker_1".into(),
                session_id: None,
                ttl_ms: 5_000,
                now_ms: 1_000,
            })
            .expect("the lease is taken");
        let error = store
            .renew_lease(&lease.lease_id, "worker_2", 20_000, 2_000)
            .expect_err("the renewal must be refused");
        assert_eq!(error.code, ErrorCode::PermissionDenied);
        test_support::remove_dir(&dir);
    }

    #[test]
    fn a_lease_that_ran_out_may_not_be_renewed() {
        let dir = temp_dir("lease-renew-expired");
        let mut store = new_store(&dir);
        let lease = store
            .acquire_lease(NewLease {
                resource: "res_1".into(),
                holder: "worker_1".into(),
                session_id: None,
                ttl_ms: 1_000,
                now_ms: 1_000,
            })
            .expect("the lease is taken");
        let error = store
            .renew_lease(&lease.lease_id, "worker_1", 20_000, 5_000)
            .expect_err("the renewal must be refused");
        assert_eq!(error.code, ErrorCode::WorkerUnavailable);
        assert!(error.code.retryable());
        test_support::remove_dir(&dir);
    }

    #[test]
    fn a_lease_is_released() {
        let dir = temp_dir("lease-release");
        let mut store = new_store(&dir);
        let lease = store
            .acquire_lease(NewLease {
                resource: "res_1".into(),
                holder: "worker_1".into(),
                session_id: None,
                ttl_ms: 5_000,
                now_ms: 1_000,
            })
            .expect("the lease is taken");
        let released = store
            .release_lease(&lease.lease_id, "worker_1")
            .expect("the lease is released");
        assert_eq!(released.state, LeaseState::Released);
        // the resource is free again
        let _ = store
            .acquire_lease(NewLease {
                resource: "res_1".into(),
                holder: "worker_2".into(),
                session_id: None,
                ttl_ms: 5_000,
                now_ms: 2_000,
            })
            .expect("the lease is taken again");
        test_support::remove_dir(&dir);
    }

    #[test]
    fn the_expired_leases_are_taken_back() {
        let dir = temp_dir("lease-reclaim");
        let mut store = new_store(&dir);
        let _ = store
            .acquire_lease(NewLease {
                resource: "res_1".into(),
                holder: "worker_1".into(),
                session_id: None,
                ttl_ms: 1_000,
                now_ms: 1_000,
            })
            .expect("the lease is taken");
        let expired = store.reclaim_expired_leases(5_000).expect("the leases are taken back");
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].state, LeaseState::Expired);
        assert!(store.reclaim_expired_leases(5_000).unwrap().is_empty());
        test_support::remove_dir(&dir);
    }

    #[test]
    fn a_job_that_is_leased_counts_the_attempt_and_the_epoch() {
        let dir = temp_dir("lease-job");
        let mut store = new_store(&dir);
        let session = new_session(&mut store, "sess_1", "agent_1", 1_000);
        let job_id = new_job(&mut store, &session, "key_1", "dig_1", 1_100);
        let lease = store
            .lease_job(&job_id, "worker_1", 5_000, 1_200)
            .expect("the job is leased");
        assert_eq!(lease.resource, job_id.as_str());
        let job = store.get_job(&job_id).unwrap().unwrap();
        assert_eq!(job.status, JobStatus::Running);
        assert_eq!(job.attempt, 1);
        assert_eq!(job.lease_epoch, 1);
        assert_eq!(job.worker_id.as_deref(), Some("worker_1"));
        test_support::remove_dir(&dir);
    }

    #[test]
    fn a_job_that_is_leased_may_not_be_leased_again() {
        let dir = temp_dir("lease-job-twice");
        let mut store = new_store(&dir);
        let session = new_session(&mut store, "sess_1", "agent_1", 1_000);
        let job_id = new_job(&mut store, &session, "key_1", "dig_1", 1_100);
        let _ = store
            .lease_job(&job_id, "worker_1", 5_000, 1_200)
            .expect("the job is leased");
        let error = store
            .lease_job(&job_id, "worker_2", 5_000, 1_300)
            .expect_err("the second holder must be refused");
        assert_eq!(error.code, ErrorCode::RateLimited);
        test_support::remove_dir(&dir);
    }

    #[test]
    fn two_holders_at_the_same_time_only_one_wins() {
        let dir = temp_dir("lease-concurrent");
        let path = dir.join("hub.sqlite3");
        {
            let mut store = Store::open(&path).expect("the store opens");
            migrations::apply(&mut store).expect("the steps run");
        }
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut handles = Vec::new();
        for holder in ["worker_a", "worker_b"] {
            let path = path.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                let mut store = Store::open(&path).expect("the store opens");
                barrier.wait();
                store.acquire_lease(NewLease {
                    resource: "res_1".into(),
                    holder: holder.into(),
                    session_id: None,
                    ttl_ms: 60_000,
                    now_ms: 1_000,
                })
            }));
        }
        let results: Vec<Result<Lease, TaseError>> = handles
            .into_iter()
            .map(|handle| handle.join().expect("the thread ends"))
            .collect();
        let winners = results.iter().filter(|result| result.is_ok()).count();
        assert_eq!(winners, 1, "only one holder may win");
        let error = results
            .iter()
            .find_map(|result| result.as_ref().err())
            .expect("there is a loser");
        assert_eq!(error.code, ErrorCode::RateLimited);

        let store = Store::open(&path).expect("the store opens");
        assert_eq!(store.list_leases().unwrap().len(), 1);
        test_support::remove_dir(&dir);
    }

    // -- the files ---------------------------------------------------------------------------

    #[test]
    fn a_file_grows_from_one_generation_to_the_next() {
        let dir = temp_dir("file-generation");
        let mut store = new_store(&dir);
        let session = new_session(&mut store, "sess_1", "agent_1", 1_000);
        let workspace = WorkspaceId::new("ws_1").unwrap();
        let mut previous = 0;
        for index in 0..3 {
            let row = store
                .record_file_transaction(NewFileTransaction {
                    workspace_id: workspace.clone(),
                    normalized_path: "src/main.rs".into(),
                    expected_generation: Some(previous),
                    content_digest: format!("dig_{index}"),
                    agent_id: AgentId::new("agent_1").unwrap(),
                    session_id: session.clone(),
                    job_id: None,
                    now_ms: 1_000 + index,
                })
                .expect("the file is written");
            assert_eq!(row.generation, previous + 1);
            previous = row.generation;
        }
        assert_eq!(store.file_generation(&workspace, "src/main.rs").unwrap(), 3);
        test_support::remove_dir(&dir);
    }

    #[test]
    fn two_writers_of_the_same_file_meet_a_conflict() {
        let dir = temp_dir("file-conflict");
        let mut store = new_store(&dir);
        let session = new_session(&mut store, "sess_1", "agent_1", 1_000);
        let workspace = WorkspaceId::new("ws_1").unwrap();
        store
            .record_file_transaction(NewFileTransaction {
                workspace_id: workspace.clone(),
                normalized_path: "src/main.rs".into(),
                expected_generation: Some(0),
                content_digest: "dig_a".into(),
                agent_id: AgentId::new("agent_1").unwrap(),
                session_id: session.clone(),
                job_id: None,
                now_ms: 1_100,
            })
            .expect("the first write works");
        let error = store
            .record_file_transaction(NewFileTransaction {
                workspace_id: workspace.clone(),
                normalized_path: "src/main.rs".into(),
                expected_generation: Some(0),
                content_digest: "dig_b".into(),
                agent_id: AgentId::new("agent_1").unwrap(),
                session_id: session,
                job_id: None,
                now_ms: 1_200,
            })
            .expect_err("the second write must meet a conflict");
        assert_eq!(error.code, ErrorCode::SymbolConflict);
        // the generation did not move
        assert_eq!(store.file_generation(&workspace, "src/main.rs").unwrap(), 1);
        let conflicts = store.list_file_conflicts().unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].expected_generation, 0);
        assert_eq!(conflicts[0].actual_generation, 1);
        assert_eq!(conflicts[0].state, ConflictState::Open);
        test_support::remove_dir(&dir);
    }

    #[test]
    fn a_conflict_is_detected_without_being_written() {
        let dir = temp_dir("file-detect");
        let mut store = new_store(&dir);
        let session = new_session(&mut store, "sess_1", "agent_1", 1_000);
        let workspace = WorkspaceId::new("ws_1").unwrap();
        store
            .record_file_transaction(NewFileTransaction {
                workspace_id: workspace.clone(),
                normalized_path: "src/main.rs".into(),
                expected_generation: Some(0),
                content_digest: "dig_a".into(),
                agent_id: AgentId::new("agent_1").unwrap(),
                session_id: session.clone(),
                job_id: None,
                now_ms: 1_100,
            })
            .expect("the first write works");
        let conflict = store
            .detect_file_conflict(&workspace, "src/main.rs", 0)
            .expect("the conflict is looked for")
            .expect("there is a conflict");
        assert_eq!(conflict.conflict_id, 0);
        assert_eq!(conflict.actual_generation, 1);
        // nothing was written down
        assert!(store.list_file_conflicts().unwrap().is_empty());
        assert!(
            store
                .detect_file_conflict(&workspace, "src/main.rs", 1)
                .expect("the conflict is looked for")
                .is_none()
        );
        test_support::remove_dir(&dir);
    }

    // -- the row images ---------------------------------------------------------------------

    #[test]
    fn the_row_images_go_through_the_json() {
        let session = AgentRegistration {
            session_id: SessionId::new("sess_1").unwrap(),
            agent_id: AgentId::new("agent_1").unwrap(),
            provider: "local".into(),
            instance_id: "i1".into(),
            workspace_id: WorkspaceId::new("ws_1").unwrap(),
            owner_logon_sid_hash: "sid".into(),
            granted_scopes: vec!["code:write".into()],
            capabilities: vec!["plan".into()],
            state: SessionState::Connected,
            lease_epoch: 2,
            heartbeat_at_ms: 10,
            created_at_ms: 5,
        };
        let image = session_to_json(&session);
        assert_eq!(session_from_json(&image).unwrap(), session);

        let job = Job {
            job_id: JobId::new("job_000000000001").unwrap(),
            job_type: "apply".into(),
            workspace_id: WorkspaceId::new("ws_1").unwrap(),
            agent_id: AgentId::new("agent_1").unwrap(),
            session_id: SessionId::new("sess_1").unwrap(),
            idempotency_key: "key_1".into(),
            request_digest: "dig_1".into(),
            base_snapshot_digest: "base".into(),
            status: JobStatus::Verifying,
            attempt: 3,
            lease_epoch: 4,
            worker_id: Some("worker_1".into()),
            result_digest: Some("dig_out".into()),
            commit_intent_digest: None,
            committed_ref: None,
            committed_oid: None,
            error_code: Some(ErrorCode::ValidationFailed),
            priority: 7,
            created_at_ms: 11,
            updated_at_ms: 12,
        };
        assert_eq!(job_from_json(&job_to_json(&job)).unwrap(), job);

        let lease = Lease {
            lease_id: "lease_000000000001".into(),
            resource: "res_1".into(),
            holder: "worker_1".into(),
            session_id: Some(SessionId::new("sess_1").unwrap()),
            fencing_token: 5,
            acquired_at_ms: 100,
            expires_at_ms: 200,
            released_at_ms: None,
            state: LeaseState::Held,
        };
        assert_eq!(lease_from_json(&lease_to_json(&lease)).unwrap(), lease);

        let record = IdempotencyRecord {
            scope: "ws_1|agent_1".into(),
            idempotency_key: "key_1".into(),
            request_digest: "dig_1".into(),
            response_digest: "dig_1".into(),
            job_id: JobId::new("job_000000000001").unwrap(),
            created_at_ms: 50,
        };
        assert_eq!(
            idempotency_from_json(&idempotency_to_json(&record)).unwrap(),
            record
        );

        let generation = FileGeneration {
            workspace_id: WorkspaceId::new("ws_1").unwrap(),
            normalized_path: "src/main.rs".into(),
            generation: 3,
            content_digest: "dig_a".into(),
            updated_at_ms: 60,
        };
        assert_eq!(
            file_generation_from_json(&file_generation_to_json(&generation)).unwrap(),
            generation
        );

        let conflict = FileConflict {
            conflict_id: 9,
            workspace_id: WorkspaceId::new("ws_1").unwrap(),
            normalized_path: "src/main.rs".into(),
            expected_generation: 1,
            actual_generation: 2,
            agent_id: AgentId::new("agent_1").unwrap(),
            session_id: SessionId::new("sess_1").unwrap(),
            job_id: Some(JobId::new("job_000000000001").unwrap()),
            state: ConflictState::Confirmed,
            detected_at_ms: 70,
        };
        assert_eq!(
            file_conflict_from_json(&file_conflict_to_json(&conflict)).unwrap(),
            conflict
        );
    }

    #[test]
    fn a_row_image_without_a_field_is_refused() {
        let image = pime_core::json::Json::obj(vec![("job_id", pime_core::json::Json::str("job_1"))]);
        let error = job_from_json(&image).expect_err("the row image is incomplete");
        assert_eq!(error.code, ErrorCode::InternalError);
    }

    #[test]
    fn the_names_of_the_states_are_the_ones_of_the_design_book() {
        assert_eq!(SessionState::Connected.as_str(), "CONNECTED");
        assert_eq!(SessionState::Disconnected.as_str(), "DISCONNECTED");
        assert_eq!(SessionState::Expired.as_str(), "EXPIRED");
        assert_eq!(SessionState::from_name("EXPIRED"), Some(SessionState::Expired));
        assert_eq!(SessionState::from_name("nope"), None);

        assert_eq!(JobStatus::Queued.as_str(), "QUEUED");
        assert_eq!(JobStatus::Running.as_str(), "RUNNING");
        assert_eq!(JobStatus::Verifying.as_str(), "VERIFYING");
        assert_eq!(JobStatus::Committed.as_str(), "COMMITTED");
        assert_eq!(JobStatus::Failed.as_str(), "FAILED");
        assert_eq!(JobStatus::Cancelled.as_str(), "CANCELLED");
        assert_eq!(JobStatus::Conflict.as_str(), "CONFLICT");
        assert_eq!(JobStatus::from_name("CONFLICT"), Some(JobStatus::Conflict));
        assert_eq!(JobStatus::from_name("nope"), None);

        assert_eq!(LeaseState::Held.as_str(), "HELD");
        assert_eq!(LeaseState::from_name("RELEASED"), Some(LeaseState::Released));
        assert_eq!(LeaseState::from_name("nope"), None);

        assert_eq!(ConflictState::Open.as_str(), "OPEN");
        assert_eq!(ConflictState::from_name("RESOLVED"), Some(ConflictState::Resolved));
        assert_eq!(ConflictState::from_name("nope"), None);
    }

    #[test]
    fn the_terminal_states_are_the_ones_of_the_design_book() {
        assert!(JobStatus::Committed.is_terminal());
        assert!(JobStatus::Failed.is_terminal());
        assert!(JobStatus::Cancelled.is_terminal());
        assert!(JobStatus::Conflict.is_terminal());
        assert!(!JobStatus::Queued.is_terminal());
        assert!(!JobStatus::Running.is_terminal());
        assert!(!JobStatus::Verifying.is_terminal());
        assert!(JobStatus::Running.is_active());
        assert!(JobStatus::Verifying.is_active());
        assert!(!JobStatus::Queued.is_active());
    }
}
