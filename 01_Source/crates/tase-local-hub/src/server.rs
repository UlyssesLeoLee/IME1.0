//! The Hub itself: the loop that takes the connections and the state that the loop keeps.
//!
//! The Hub is the resident process of a logon session (detailed design §17.1). This module is the
//! runtime of it:
//!
//! * [`Hub`] takes the connections of the namespace, authorizes them, holds the sessions, arbitrates
//!   the writes and hands the jobs to the scheduler;
//! * [`JobScheduler`] is the seam of the scheduler (detailed design §17.5): the Hub gives it the jobs
//!   and it gives back the one that may run. `tase-agent-scheduler` will be plugged in here; the Hub
//!   does not write a second queue of its own;
//! * the recovery (detailed design §17.6) puts the jobs that were not finished back into the queue
//!   after a restart, out of the store.
//!
//! The rules of the module:
//!
//! * **the Hub is the single writer of its state.** The state lives behind one lock, so two agents
//!   that write at the same time are ordered; the connections are served by one thread each, and the
//!   state is the only thing they share.
//! * **the back pressure is explicit.** More connections than the Hub takes are refused with
//!   [`ErrorCode::RateLimited`]; more jobs than the Hub runs are refused with
//!   [`ErrorCode::WorkerUnavailable`]. Nothing is dropped silently.
//! * **a disconnect gives everything back.** The leases of the session, the write locks of the session
//!   and the jobs of the session are released when the connection goes: a crashed agent leaves no
//!   deadlock behind.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use pime_core::json::{Json, Limits};
use tase_contract::{AgentId, JobId, SessionId, TaseError, WorkspaceId};
use tase_local_ipc::{
    AclPolicy, Connection, FrameError, FrameHeader, FrameKind, PeerIdentity, Timeouts, Transport,
};
use tase_local_store::{Job, JobStatus, NewFileTransaction, NewJob, NewSession, Store};

use crate::auth::authorize;
use crate::bootstrap::{AcceptedConnection, BoxedTransport, HubConfig, PipeListener, SessionIdentity};
use crate::scheduler::{FairScheduler, SchedulerKind};
use crate::session::{AgentSession, SessionRegistry};
use crate::telemetry::{HubEvent, Telemetry, redact};
use crate::workspace::{Clock, SystemClock, WorkspaceRegistry};
use crate::HUB_CAPABILITIES;

/// How long the Hub waits for a frame before it looks at its stop flag again.
const POLL_MS: u64 = 250;

/// The limits of the JSON of a frame.
const JSON_LIMITS: Limits = Limits {
    max_bytes: tase_local_ipc::MAX_FRAME_BYTES,
    max_depth: 32,
};

/// A job that the scheduler must run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobRequest {
    /// The id of the job.
    pub job_id: JobId,
    /// The kind of the job (`apply`, `preview`, `verify`, ...).
    pub job_type: String,
    /// The agent that asked for it.
    pub agent_id: AgentId,
    /// The session that asked for it.
    pub session_id: SessionId,
    /// The workspace it works in.
    pub workspace_id: WorkspaceId,
    /// The priority of the job inside the queue of its agent.
    pub priority: u8,
    /// The time the job was put in the queue.
    pub enqueued_at_ms: u64,
}

impl JobRequest {
    /// The job of a row of the store.
    #[must_use]
    pub fn from_job(job: &Job, enqueued_at_ms: u64) -> JobRequest {
        JobRequest {
            job_id: job.job_id.clone(),
            job_type: job.job_type.clone(),
            agent_id: job.agent_id.clone(),
            session_id: job.session_id.clone(),
            workspace_id: job.workspace_id.clone(),
            priority: job.priority,
            enqueued_at_ms,
        }
    }
}

/// The seam of the scheduler (detailed design §17.5).
///
/// The Hub does not decide which job runs: it gives the job to the scheduler and asks it for the one
/// that may run. `tase-agent-scheduler` (the fair queues, the quotas, the leases and the budgets) is
/// plugged in here; the trait is the whole contract between the two.
pub trait JobScheduler: Send {
    /// Puts a job in the queue of its agent.
    fn enqueue(&mut self, job: JobRequest) -> Result<(), TaseError>;

    /// Takes the job that may run now, if there is one.
    fn next_job(&mut self, now_ms: u64) -> Option<JobRequest>;

    /// Says that a job is done.
    fn complete(&mut self, job_id: &JobId, now_ms: u64);

    /// The number of the jobs that wait.
    fn queued(&self) -> usize;

    /// The number of the jobs that run now.
    fn running(&self) -> usize;

    /// Says that an agent may run jobs, and with which weight.
    ///
    /// The weight is the share of the agent inside a class: the fair scheduler of
    /// `tase-agent-scheduler` grants the agent `weight × quantum` credits at every visit of the
    /// round robin. The default does nothing, because the round robin of the Hub gives every agent
    /// the same turn and needs no weight.
    fn register_agent(&mut self, _agent_id: &AgentId, _weight: u32) -> Result<(), TaseError> {
        Ok(())
    }
}

/// The scheduler of the Hub when nobody brought one: a round robin over the agents.
///
/// It is the fairness of the detailed design §17.5 in its simplest form: every agent has its own
/// queue, and the agents take turns. `tase-agent-scheduler` replaces it with the weighted round robin
/// and the quotas of the design book; the trait is the seam.
#[derive(Debug)]
pub struct RoundRobinScheduler {
    agents: Vec<(AgentId, VecDeque<JobRequest>)>,
    running: usize,
    max_running: u32,
}

impl RoundRobinScheduler {
    /// A scheduler that lets at most `max_running` jobs run at the same time.
    #[must_use]
    pub fn new(max_running: u32) -> RoundRobinScheduler {
        RoundRobinScheduler {
            agents: Vec::new(),
            running: 0,
            max_running,
        }
    }
}

impl JobScheduler for RoundRobinScheduler {
    fn enqueue(&mut self, job: JobRequest) -> Result<(), TaseError> {
        let agent = job.agent_id.clone();
        if let Some((_, queue)) = self.agents.iter_mut().find(|(owner, _)| owner == &agent) {
            queue.push_back(job);
            return Ok(());
        }
        let mut queue = VecDeque::new();
        queue.push_back(job);
        self.agents.push((agent, queue));
        Ok(())
    }

    fn next_job(&mut self, _now_ms: u64) -> Option<JobRequest> {
        if self.running >= self.max_running as usize {
            return None;
        }
        for _ in 0..self.agents.len() {
            let job = self.agents[0].1.pop_front();
            match job {
                Some(job) => {
                    if self.agents[0].1.is_empty() {
                        self.agents.remove(0);
                    } else {
                        self.agents.rotate_left(1);
                    }
                    self.running += 1;
                    return Some(job);
                }
                None => {
                    self.agents.rotate_left(1);
                }
            }
        }
        None
    }

    fn complete(&mut self, _job_id: &JobId, _now_ms: u64) {
        self.running = self.running.saturating_sub(1);
    }

    fn queued(&self) -> usize {
        self.agents.iter().map(|(_, queue)| queue.len()).sum()
    }

    fn running(&self) -> usize {
        self.running
    }
}

/// A connection of the Hub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionInfo {
    /// The number of the connection; it is also the session id of the IPC layer.
    pub connection_id: u64,
    /// The session of the agent; the registration fills the provider, the instance and the
    /// capabilities.
    pub session: AgentSession,
    /// The name of the peer.
    pub peer: String,
    /// The time the connection was opened.
    pub opened_at_ms: u64,
    /// The number of the events that were pushed to this connection.
    pub last_event_seq: u64,
    /// Whether the peer asked for the stream of the events.
    pub subscribed: bool,
}

/// What the Hub knows about a job that is not finished.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobRecord {
    /// The id of the job.
    pub job_id: JobId,
    /// The kind of the job.
    pub job_type: String,
    /// The agent that asked for it.
    pub agent_id: AgentId,
    /// The session that asked for it.
    pub session_id: SessionId,
    /// The workspace it works in.
    pub workspace_id: WorkspaceId,
    /// The idempotency key of the job.
    pub idempotency_key: String,
    /// The digest of the request.
    pub request_digest: String,
    /// The state of the job.
    pub status: String,
    /// The time the job was registered.
    pub created_at_ms: u64,
    /// The time the job was touched the last time.
    pub updated_at_ms: u64,
}

impl JobRecord {
    /// The record of a row of the store.
    #[must_use]
    pub fn from_job(job: &Job) -> JobRecord {
        JobRecord {
            job_id: job.job_id.clone(),
            job_type: job.job_type.clone(),
            agent_id: job.agent_id.clone(),
            session_id: job.session_id.clone(),
            workspace_id: job.workspace_id.clone(),
            idempotency_key: job.idempotency_key.clone(),
            request_digest: job.request_digest.clone(),
            status: job.status.as_str().to_string(),
            created_at_ms: job.created_at_ms,
            updated_at_ms: job.updated_at_ms,
        }
    }
}

/// The record of an idempotency key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdempotencyEntry {
    /// The digest of the request that took the key.
    pub request_digest: String,
    /// The job the key belongs to.
    pub job_id: JobId,
    /// The state of the job.
    pub status: String,
}

/// What a recovery did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    /// The sessions that were given up on.
    pub expired_sessions: u32,
    /// The leases that ran out of time.
    pub expired_leases: u32,
    /// The jobs that the reconciliation put back into the queue.
    pub requeued_jobs: u32,
    /// The jobs that the Hub put back into the queue of the scheduler.
    pub recovered_jobs: u32,
}

/// The state of the Hub.
pub struct HubState {
    /// The sessions of the agents.
    pub sessions: SessionRegistry,
    /// The workspaces: the write locks and the generation of the files.
    pub workspaces: WorkspaceRegistry,
    /// The events of the Hub.
    pub telemetry: Telemetry,
    /// The scheduler of the jobs.
    pub scheduler: Box<dyn JobScheduler>,
    /// The persistence of the state, when the Hub has one.
    pub store: Option<Store>,
    /// The connections that are open.
    pub connections: Vec<ConnectionInfo>,
    next_connection_id: u64,
    jobs: HashMap<String, JobRecord>,
    idempotency: HashMap<String, IdempotencyEntry>,
}

impl std::fmt::Debug for HubState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubState")
            .field("sessions", &self.sessions.len())
            .field("workspaces", &self.workspaces)
            .field("connections", &self.connections.len())
            .field("jobs", &self.jobs.len())
            .finish()
    }
}

impl HubState {
    /// A state without a session and without a job.
    fn new(clock: Arc<dyn Clock>, config: &HubConfig) -> HubState {
        HubState {
            sessions: SessionRegistry::new(),
            workspaces: WorkspaceRegistry::new(clock, crate::defaults::FILE_LEASE_MS),
            telemetry: Telemetry::new(),
            scheduler: build_scheduler(config),
            store: None,
            connections: Vec::new(),
            next_connection_id: 1,
            jobs: HashMap::new(),
            idempotency: HashMap::new(),
        }
    }

    /// The connection of a session.
    fn connection_of(&self, session_id: &SessionId) -> Option<&ConnectionInfo> {
        self.connections
            .iter()
            .find(|connection| &connection.session.session_id == session_id)
    }

    /// The connection of a number.
    fn connection_mut(&mut self, connection_id: u64) -> Option<&mut ConnectionInfo> {
        self.connections
            .iter_mut()
            .find(|connection| connection.connection_id == connection_id)
    }

    /// The jobs that are not finished.
    fn unfinished_jobs(&self) -> usize {
        self.jobs
            .values()
            .filter(|job| !is_terminal(&job.status))
            .count()
    }
}

/// Whether a state of a job is the end of it.
fn is_terminal(status: &str) -> bool {
    matches!(status, "COMMITTED" | "FAILED" | "CANCELLED" | "CONFLICT")
}

/// The scheduler of the configuration of the Hub (detailed design §17.5).
///
/// The default is the fair scheduler of `tase-agent-scheduler`: the five classes of the design
/// book and the weighted round robin over the agents. The round robin of the Hub stays as the
/// degradation that the design book allows, and the configuration says which one is used.
fn build_scheduler(config: &HubConfig) -> Box<dyn JobScheduler> {
    match config.scheduler {
        SchedulerKind::Fair => Box::new(FairScheduler::new(config.max_concurrent_jobs)),
        SchedulerKind::RoundRobin => {
            Box::new(RoundRobinScheduler::new(config.max_concurrent_jobs))
        }
    }
}

/// The parts of the Hub that every thread shares.
struct HubInner {
    config: HubConfig,
    acl: AclPolicy,
    state: Mutex<HubState>,
    clock: Mutex<Arc<dyn Clock>>,
    capabilities: Vec<String>,
    timeouts: Timeouts,
}

impl HubInner {
    /// The time of the caller.
    fn now_ms(&self) -> u64 {
        let clock = self.clock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        clock.now_ms()
    }

    /// The state of the Hub, under its lock.
    fn lock(&self) -> MutexGuard<'_, HubState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Takes a connection of the namespace.
    ///
    /// The peer is authorized first (the same user, of the same logon session, of the same
    /// terminal-services session), and the limit of the connections is checked after: a Hub that is
    /// full refuses the connection with [`ErrorCode::RateLimited`] and says so, it does not drop it.
    fn register_connection(
        &self,
        peer: &PeerIdentity,
        now_ms: u64,
    ) -> Result<ConnectionInfo, TaseError> {
        let mut state = self.lock();
        if state.connections.len() >= self.config.max_connections as usize {
            return Err(TaseError::rate_limited(format!(
                "the hub takes {} connections at the same time",
                self.config.max_connections
            )));
        }
        let session = authorize(&self.acl, peer)?;
        let connection_id = state.next_connection_id;
        state.next_connection_id += 1;
        let info = ConnectionInfo {
            connection_id,
            session,
            peer: peer_name(peer),
            opened_at_ms: now_ms,
            last_event_seq: 0,
            subscribed: false,
        };
        state.connections.push(info.clone());
        state.telemetry.push(HubEvent::ConnectionOpened {
            connection_id,
            peer: redact(&info.peer),
            at_ms: now_ms,
        });
        Ok(info)
    }

    /// Ends a connection: the session, its leases and its write locks go with it.
    fn close_connection(&self, connection_id: u64, reason: &str) {
        let now_ms = self.now_ms();
        let mut state = self.lock();
        let index = state
            .connections
            .iter()
            .position(|connection| connection.connection_id == connection_id);
        let Some(index) = index else {
            return;
        };
        let connection = state.connections.remove(index);
        state.sessions.disconnect(&connection.session.session_id);
        state.workspaces.release_session(&connection.session.session_id);
        state.telemetry.push(HubEvent::ConnectionClosed {
            connection_id,
            reason: redact(reason),
            at_ms: now_ms,
        });
    }

    /// Answers a request of a session.
    ///
    /// The method may be written with or without the `tase.` prefix: the design book writes
    /// `tase.agent.register`, and the shorthand of the task is `agent.register`; both are the same
    /// method.
    fn dispatch(
        &self,
        session_id: &SessionId,
        method: &str,
        params: &Json,
        now_ms: u64,
    ) -> Result<Json, TaseError> {
        let name = method.strip_prefix("tase.").unwrap_or(method);
        match name {
            "agent.register" => self.agent_register(session_id, params, now_ms),
            "agent.heartbeat" => self.agent_heartbeat(session_id, now_ms),
            "agent.unregister" => self.agent_unregister(session_id, now_ms),
            "job.enqueue" => self.job_enqueue(session_id, params, now_ms),
            "job.status" => self.job_status(params),
            "local.status" => self.local_status(),
            "local.events" => self.local_events(params),
            "workspace.snapshot" => self.workspace_snapshot(params),
            "workspace.acquire" => self.workspace_acquire(session_id, params, now_ms),
            "workspace.release" => self.workspace_release(session_id, params),
            "workspace.write" => self.workspace_write(session_id, params, now_ms),
            _ => Err(TaseError::not_found(format!(
                "the hub has no method {method}"
            ))),
        }
    }

    fn agent_register(
        &self,
        session_id: &SessionId,
        params: &Json,
        now_ms: u64,
    ) -> Result<Json, TaseError> {
        let provider = string_param(params, &["provider", "client"])?;
        let instance = optional_string_param(params, &["instance", "instance_id"]).unwrap_or_default();
        let workspace = WorkspaceId::new(string_param(params, &["workspace_id", "workspace"])?)?;
        let capabilities = string_list_param(params, &["capabilities", "protocol_capabilities"]);
        let requested = string_list_param(params, &["requested_scopes", "scopes"]);
        let granted: Vec<String> = requested
            .iter()
            .filter(|scope| HUB_CAPABILITIES.contains(&scope.as_str()))
            .cloned()
            .collect();

        let mut state = self.lock();
        if state.sessions.get(session_id).is_some() {
            return Err(TaseError::invalid_argument(format!(
                "the session {session_id} is already registered"
            )));
        }
        let connection = state
            .connection_of(session_id)
            .ok_or_else(|| TaseError::not_found(format!("the session {session_id} is not known")))?;
        let mut session = connection.session.clone();
        session.provider = provider.clone();
        session.instance = instance.clone();
        session.capabilities = capabilities.clone();

        state.sessions.register(
            session.clone(),
            workspace.clone(),
            granted.clone(),
            now_ms,
            crate::defaults::SESSION_LEASE_MS,
        )?;

        // The scheduler knows the agent and the share it gets in the queue of its class
        // (detailed design §17.5): the fair scheduler weighs the agents, the round robin of the
        // Hub does not need a weight.
        state
            .scheduler
            .register_agent(&session.agent, self.config.agent_weight)?;

        if let Some(store) = state.store.as_mut() {
            store.open_session(NewSession {
                session_id: session_id.clone(),
                agent_id: session.agent.clone(),
                provider: provider.clone(),
                instance_id: instance.clone(),
                workspace_id: workspace.clone(),
                owner_logon_sid_hash: owner_hash(&self.acl.required_logon_sid),
                granted_scopes: granted.clone(),
                capabilities: capabilities.clone(),
                now_ms,
            })?;
        }

        state.telemetry.push(HubEvent::AgentRegistered {
            session_id: session_id.as_str().to_string(),
            agent_id: session.agent.as_str().to_string(),
            provider: provider.clone(),
            instance: instance.clone(),
            at_ms: now_ms,
        });

        Ok(Json::obj(vec![
            ("session_id", Json::str(session_id.as_str())),
            ("agent_id", Json::str(session.agent.as_str())),
            ("provider", Json::str(&provider)),
            ("instance", Json::str(&instance)),
            ("workspace_id", Json::str(workspace.as_str())),
            ("granted_scopes", Json::strs(&granted)),
            ("capabilities", Json::strs(&capabilities)),
            (
                "heartbeat_interval_ms",
                Json::int(crate::defaults::HEARTBEAT_INTERVAL_MS as i64),
            ),
            (
                "session_expires_at_ms",
                Json::int(now_ms.saturating_add(crate::defaults::SESSION_LEASE_MS) as i64),
            ),
        ]))
    }

    fn agent_heartbeat(&self, session_id: &SessionId, now_ms: u64) -> Result<Json, TaseError> {
        let mut state = self.lock();
        let expires = state
            .sessions
            .heartbeat(session_id, now_ms, crate::defaults::SESSION_LEASE_MS)?;
        if let Some(store) = state.store.as_mut() {
            store.heartbeat(session_id, now_ms)?;
        }
        Ok(Json::obj(vec![
            ("session_id", Json::str(session_id.as_str())),
            ("lease_expires_at_ms", Json::int(expires as i64)),
            ("last_heartbeat_ms", Json::int(now_ms as i64)),
        ]))
    }

    fn agent_unregister(&self, session_id: &SessionId, _now_ms: u64) -> Result<Json, TaseError> {
        let mut state = self.lock();
        let entry = state.sessions.unregister(session_id)?;
        let released = state.workspaces.release_session(session_id);
        let active_jobs = entry.inflight_jobs;
        if let Some(store) = state.store.as_mut() {
            store.unregister_session(session_id)?;
        }
        Ok(Json::obj(vec![
            ("session_id", Json::str(session_id.as_str())),
            ("accepted", Json::Bool(true)),
            ("active_jobs", Json::int(active_jobs as i64)),
            ("released_locks", Json::int(released.len() as i64)),
        ]))
    }

    fn job_enqueue(
        &self,
        session_id: &SessionId,
        params: &Json,
        now_ms: u64,
    ) -> Result<Json, TaseError> {
        let idempotency_key = string_param(params, &["idempotency_key", "key"])?;
        let job_type = optional_string_param(params, &["job_type", "kind"])
            .unwrap_or_else(|| "apply".to_string());
        let request_digest = optional_string_param(params, &["request_digest", "digest"])
            .unwrap_or_else(|| pime_core::sha256::sha256_hex(&params.to_json_string()));
        let base_snapshot_digest =
            optional_string_param(params, &["base_snapshot_digest"]).unwrap_or_default();
        let priority = params
            .get("priority")
            .and_then(Json::as_i64)
            .and_then(|number| u8::try_from(number).ok())
            .unwrap_or(0);

        let mut state = self.lock();
        let entry = state
            .sessions
            .get(session_id)
            .ok_or_else(|| TaseError::not_found(format!("the session {session_id} is not known")))?
            .clone();
        let agent_id = entry.session.agent.clone();
        let workspace = entry.workspace.clone();
        let scope = format!(
            "{}|{idempotency_key}",
            tase_local_store::idempotency_scope(&workspace, &agent_id)
        );

        // the rule of the idempotency of the contract: the same key with the same digest gives the
        // same job, the same key with another digest is a conflict
        if let Some(known) = state.idempotency.get(&scope) {
            if known.request_digest != request_digest {
                return Err(TaseError::idempotency_mismatch(format!(
                    "the idempotency key {idempotency_key} was already used with another digest"
                )));
            }
            return Ok(Json::obj(vec![
                ("job_id", Json::str(known.job_id.as_str())),
                ("status", Json::str(&known.status)),
                ("deduplicated", Json::Bool(true)),
            ]));
        }

        // the back pressure of the queue: the Hub says that it is full, it does not drop the job
        if state.unfinished_jobs() >= self.config.max_concurrent_jobs as usize {
            return Err(TaseError::worker_unavailable(format!(
                "the hub runs at most {} jobs at the same time",
                self.config.max_concurrent_jobs
            )));
        }

        let number = state.jobs.len() + 1;
        let job_id = match state.store.as_mut() {
            Some(store) => store
                .submit_job(NewJob {
                    job_type: job_type.clone(),
                    workspace_id: workspace.clone(),
                    agent_id: agent_id.clone(),
                    session_id: session_id.clone(),
                    idempotency_key: idempotency_key.clone(),
                    request_digest: request_digest.clone(),
                    base_snapshot_digest: base_snapshot_digest.clone(),
                    priority,
                    now_ms,
                })?
                .job_id,
            None => JobId::new(format!("job_{number:012}"))?,
        };

        let record = JobRecord {
            job_id: job_id.clone(),
            job_type: job_type.clone(),
            agent_id: agent_id.clone(),
            session_id: session_id.clone(),
            workspace_id: workspace.clone(),
            idempotency_key: idempotency_key.clone(),
            request_digest: request_digest.clone(),
            status: JobStatus::Queued.as_str().to_string(),
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };
        state.jobs.insert(job_id.as_str().to_string(), record);
        state.idempotency.insert(
            scope,
            IdempotencyEntry {
                request_digest: request_digest.clone(),
                job_id: job_id.clone(),
                status: JobStatus::Queued.as_str().to_string(),
            },
        );
        state.scheduler.enqueue(JobRequest {
            job_id: job_id.clone(),
            job_type: job_type.clone(),
            agent_id: agent_id.clone(),
            session_id: session_id.clone(),
            workspace_id: workspace.clone(),
            priority,
            enqueued_at_ms: now_ms,
        })?;
        state.telemetry.push(HubEvent::JobEnqueued {
            job_id: job_id.as_str().to_string(),
            session_id: session_id.as_str().to_string(),
            agent_id: agent_id.as_str().to_string(),
            workspace: workspace.as_str().to_string(),
            at_ms: now_ms,
        });
        Ok(Json::obj(vec![
            ("job_id", Json::str(job_id.as_str())),
            ("status", Json::str(JobStatus::Queued.as_str())),
            ("deduplicated", Json::Bool(false)),
        ]))
    }

    fn job_status(&self, params: &Json) -> Result<Json, TaseError> {
        let job_id = JobId::new(string_param(params, &["job_id", "id"])?)?;
        let state = self.lock();
        if let Some(store) = state.store.as_ref()
            && let Some(job) = store.get_job(&job_id)?
        {
            return Ok(Json::obj(vec![
                ("job_id", Json::str(job.job_id.as_str())),
                ("status", Json::str(job.status.as_str())),
                ("job_type", Json::str(&job.job_type)),
                ("agent_id", Json::str(job.agent_id.as_str())),
                ("session_id", Json::str(job.session_id.as_str())),
                ("workspace_id", Json::str(job.workspace_id.as_str())),
                ("created_at_ms", Json::int(job.created_at_ms as i64)),
                ("updated_at_ms", Json::int(job.updated_at_ms as i64)),
            ]));
        }
        let record = state
            .jobs
            .get(job_id.as_str())
            .ok_or_else(|| TaseError::not_found(format!("the job {job_id} is not known")))?;
        Ok(Json::obj(vec![
            ("job_id", Json::str(record.job_id.as_str())),
            ("status", Json::str(&record.status)),
            ("job_type", Json::str(&record.job_type)),
            ("agent_id", Json::str(record.agent_id.as_str())),
            ("session_id", Json::str(record.session_id.as_str())),
            ("workspace_id", Json::str(record.workspace_id.as_str())),
            ("created_at_ms", Json::int(record.created_at_ms as i64)),
            ("updated_at_ms", Json::int(record.updated_at_ms as i64)),
        ]))
    }

    fn local_status(&self) -> Result<Json, TaseError> {
        let state = self.lock();
        Ok(Json::obj(vec![
            ("connections", Json::int(state.connections.len() as i64)),
            ("max_connections", Json::int(self.config.max_connections as i64)),
            ("sessions", Json::int(state.sessions.len() as i64)),
            ("queued_jobs", Json::int(state.scheduler.queued() as i64)),
            ("running_jobs", Json::int(state.scheduler.running() as i64)),
            (
                "max_concurrent_jobs",
                Json::int(self.config.max_concurrent_jobs as i64),
            ),
            ("locks", Json::int(state.workspaces.len() as i64)),
            ("events", Json::int(state.telemetry.len() as i64)),
        ]))
    }

    fn local_events(&self, params: &Json) -> Result<Json, TaseError> {
        let since_seq = params
            .get("since_seq")
            .and_then(Json::as_i64)
            .and_then(|number| u64::try_from(number).ok())
            .unwrap_or(0);
        let state = self.lock();
        let events: Vec<Json> = state
            .telemetry
            .since(since_seq.saturating_add(1))
            .iter()
            .map(|event| event.to_json())
            .collect();
        Ok(Json::obj(vec![
            ("events", Json::Arr(events)),
            ("next_seq", Json::int(state.telemetry.len() as i64 + 1)),
        ]))
    }

    fn workspace_snapshot(&self, params: &Json) -> Result<Json, TaseError> {
        let workspace = WorkspaceId::new(string_param(params, &["workspace_id", "workspace"])?)?;
        let path = optional_string_param(params, &["path", "document"]);
        let state = self.lock();
        let locks: Vec<Json> = state
            .workspaces
            .locks()
            .iter()
            .filter(|lock| lock.workspace == workspace.as_str())
            .map(|lock| {
                Json::obj(vec![
                    ("path", Json::str(&lock.path)),
                    ("agent_id", Json::str(lock.agent.as_str())),
                    ("generation", Json::int(lock.generation as i64)),
                    ("expires_at_ms", Json::int(lock.expires_at_ms as i64)),
                ])
            })
            .collect();
        let generation = match &path {
            Some(path) => {
                let in_memory = state.workspaces.generation(workspace.as_str(), path);
                let durable = match state.store.as_ref() {
                    Some(store) => store.file_generation(&workspace, path)?,
                    None => 0,
                };
                in_memory.max(durable)
            }
            None => state
                .workspaces
                .locks()
                .iter()
                .filter(|lock| lock.workspace == workspace.as_str())
                .map(|lock| lock.generation)
                .max()
                .unwrap_or(0),
        };
        let digest = pime_core::sha256::sha256_hex(&format!(
            "{}|{}|{generation}",
            workspace.as_str(),
            path.clone().unwrap_or_default()
        ));
        Ok(Json::obj(vec![
            ("workspace_id", Json::str(workspace.as_str())),
            ("generation", Json::int(generation as i64)),
            ("snapshot_digest", Json::str(&digest)),
            ("locks", Json::Arr(locks)),
        ]))
    }

    fn workspace_acquire(
        &self,
        session_id: &SessionId,
        params: &Json,
        now_ms: u64,
    ) -> Result<Json, TaseError> {
        let workspace = WorkspaceId::new(string_param(params, &["workspace_id", "workspace"])?)?;
        let path = string_param(params, &["path", "document"])?;
        let mut state = self.lock();
        let agent = state
            .sessions
            .get(session_id)
            .ok_or_else(|| TaseError::not_found(format!("the session {session_id} is not known")))?
            .session
            .agent
            .clone();
        match state.workspaces.acquire_at(
            workspace.as_str(),
            &path,
            &agent,
            Some(session_id),
            now_ms,
        ) {
            Ok(lock) => Ok(Json::obj(vec![
                ("workspace_id", Json::str(workspace.as_str())),
                ("path", Json::str(&lock.path)),
                ("agent_id", Json::str(lock.agent.as_str())),
                ("generation", Json::int(lock.generation as i64)),
                ("expires_at_ms", Json::int(lock.expires_at_ms as i64)),
            ])),
            Err(error) => {
                state.telemetry.push(HubEvent::LockDenied {
                    workspace: workspace.as_str().to_string(),
                    path: path.clone(),
                    agent_id: agent.as_str().to_string(),
                    at_ms: now_ms,
                });
                Err(error)
            }
        }
    }

    fn workspace_release(
        &self,
        session_id: &SessionId,
        params: &Json,
    ) -> Result<Json, TaseError> {
        let workspace = WorkspaceId::new(string_param(params, &["workspace_id", "workspace"])?)?;
        let path = string_param(params, &["path", "document"])?;
        let mut state = self.lock();
        let agent = state
            .sessions
            .get(session_id)
            .ok_or_else(|| TaseError::not_found(format!("the session {session_id} is not known")))?
            .session
            .agent
            .clone();
        state
            .workspaces
            .release(workspace.as_str(), &path, &agent)?;
        Ok(Json::obj(vec![
            ("workspace_id", Json::str(workspace.as_str())),
            ("path", Json::str(&path)),
            ("released", Json::Bool(true)),
        ]))
    }

    fn workspace_write(
        &self,
        session_id: &SessionId,
        params: &Json,
        now_ms: u64,
    ) -> Result<Json, TaseError> {
        let workspace = WorkspaceId::new(string_param(params, &["workspace_id", "workspace"])?)?;
        let path = string_param(params, &["path", "document"])?;
        let content_digest = string_param(params, &["content_digest", "digest"])?;
        let expected_generation = params
            .get("expected_generation")
            .and_then(Json::as_i64)
            .and_then(|number| u64::try_from(number).ok());
        let mut state = self.lock();
        let agent = state
            .sessions
            .get(session_id)
            .ok_or_else(|| TaseError::not_found(format!("the session {session_id} is not known")))?
            .session
            .agent
            .clone();
        let generation = match state.store.as_mut() {
            Some(store) => store
                .record_file_transaction(NewFileTransaction {
                    workspace_id: workspace.clone(),
                    normalized_path: path.clone(),
                    expected_generation,
                    content_digest,
                    agent_id: agent,
                    session_id: session_id.clone(),
                    job_id: None,
                    now_ms,
                })?
                .generation,
            None => {
                if let Some(expected) = expected_generation
                    && state.workspaces.generation(workspace.as_str(), &path) != expected
                {
                    return Err(TaseError::symbol_conflict(format!(
                        "the file {path} of the workspace {} is not at the generation {expected}",
                        workspace.as_str()
                    )));
                }
                state.workspaces.bump_generation(workspace.as_str(), &path)
            }
        };
        state
            .workspaces
            .set_generation(workspace.as_str(), &path, generation);
        Ok(Json::obj(vec![
            ("workspace_id", Json::str(workspace.as_str())),
            ("path", Json::str(&path)),
            ("generation", Json::int(generation as i64)),
        ]))
    }
}

/// The Hub: the resident process of a logon session.
pub struct Hub {
    inner: Arc<HubInner>,
    listener: Option<Box<dyn PipeListener>>,
    stopped: Arc<AtomicBool>,
}

impl Hub {
    /// Builds a Hub for the identity of the caller.
    pub fn new(config: HubConfig, identity: &dyn SessionIdentity) -> Result<Hub, TaseError> {
        config.validate()?;
        let acl = AclPolicy::new(
            &identity.logon_sid(),
            &identity.user_sid(),
            identity.session_id(),
        );
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let state = HubState::new(Arc::clone(&clock), &config);
        let inner = HubInner {
            config,
            acl,
            state: Mutex::new(state),
            clock: Mutex::new(clock),
            capabilities: HUB_CAPABILITIES.iter().map(|name| name.to_string()).collect(),
            timeouts: Timeouts::default(),
        };
        Ok(Hub {
            inner: Arc::new(inner),
            listener: None,
            stopped: Arc::new(AtomicBool::new(false)),
        })
    }

    /// The Hub with a clock of the caller; a test drives the time by hand.
    #[must_use]
    pub fn with_clock(self, clock: Arc<dyn Clock>) -> Hub {
        {
            let mut state = self.inner.lock();
            state.workspaces =
                WorkspaceRegistry::new(Arc::clone(&clock), crate::defaults::FILE_LEASE_MS);
        }
        {
            let mut slot = self.inner.clock.lock().unwrap_or_else(|e| e.into_inner());
            *slot = clock;
        }
        self
    }

    /// The Hub with a store; the migrations of the schema are applied.
    pub fn with_store(self, mut store: Store) -> Result<Hub, TaseError> {
        tase_local_store::migrations::apply(&mut store)?;
        self.inner.lock().store = Some(store);
        Ok(self)
    }

    /// The Hub with the scheduler of the caller.
    #[must_use]
    pub fn with_scheduler(self, scheduler: Box<dyn JobScheduler>) -> Hub {
        self.inner.lock().scheduler = scheduler;
        self
    }

    /// The Hub with the listener of the pipe of the caller.
    #[must_use]
    pub fn with_listener(self, listener: Box<dyn PipeListener>) -> Hub {
        Hub {
            inner: self.inner,
            listener: Some(listener),
            stopped: self.stopped,
        }
    }

    /// The configuration of the Hub.
    #[must_use]
    pub fn config(&self) -> &HubConfig {
        &self.inner.config
    }

    /// The access control of the Hub.
    #[must_use]
    pub fn acl(&self) -> &AclPolicy {
        &self.inner.acl
    }

    /// The state of the Hub, under its lock.
    #[must_use = "the state of the hub is only good while its lock is held"]
    pub fn state(&self) -> MutexGuard<'_, HubState> {
        self.inner.lock()
    }

    /// The time of the caller.
    #[must_use]
    pub fn now_ms(&self) -> u64 {
        self.inner.now_ms()
    }

    /// Asks the Hub to stop; the loop of the pipe ends at the next poll.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }

    /// Whether the Hub was asked to stop.
    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    /// The flag that says whether the Hub was asked to stop.
    ///
    /// The caller may hold it and set it: the signal handler of the console of the binary stops the
    /// Hub that way, without touching the state of the Hub.
    #[must_use]
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stopped)
    }

    /// Takes a connection of the namespace.
    pub fn register_connection(
        &self,
        peer: &PeerIdentity,
        now_ms: u64,
    ) -> Result<ConnectionInfo, TaseError> {
        self.inner.register_connection(peer, now_ms)
    }

    /// Ends a connection: the session, its leases and its write locks go with it.
    pub fn close_connection(&self, connection_id: u64, reason: &str) {
        self.inner.close_connection(connection_id, reason);
    }

    /// Answers a request of a session.
    ///
    /// The method may be written with or without the `tase.` prefix: the design book writes
    /// `tase.agent.register`, and the shorthand of the task is `agent.register`; both are the same
    /// method.
    pub fn handle_message(
        &self,
        session_id: &SessionId,
        method: &str,
        params: &Json,
        now_ms: u64,
    ) -> Result<Json, TaseError> {
        self.inner.dispatch(session_id, method, params, now_ms)
    }

    /// Puts the jobs that were not finished back into the queue (detailed design §17.6).
    ///
    /// The reconciliation of the store gives the sessions that stopped their heartbeats up and takes
    /// the leases that ran out of time back; the jobs that are not finished go back into the queue of
    /// the scheduler.
    pub fn recover(&self, now_ms: u64) -> Result<RecoveryReport, TaseError> {
        let mut report = RecoveryReport::default();
        let mut state = self.inner.lock();
        let jobs: Vec<Job> = {
            let Some(store) = state.store.as_mut() else {
                return Ok(report);
            };
            let reconciliation = tase_local_store::reconcile::reconcile(store, now_ms)?;
            report.expired_sessions = reconciliation.expired_sessions;
            report.expired_leases = reconciliation.expired_leases;
            report.requeued_jobs = reconciliation.requeued_jobs;
            let mut jobs = Vec::new();
            for status in [JobStatus::Queued, JobStatus::Running, JobStatus::Verifying] {
                jobs.extend(store.list_jobs_by_status(status)?);
            }
            jobs
        };
        for job in jobs {
            state
                .jobs
                .insert(job.job_id.as_str().to_string(), JobRecord::from_job(&job));
            state.scheduler.enqueue(JobRequest::from_job(&job, now_ms))?;
            report.recovered_jobs += 1;
        }
        Ok(report)
    }

    /// The loop of the Hub: it takes the connections and serves them.
    ///
    /// Every connection is served by a thread of its own; the state of the Hub is behind one lock, so
    /// the threads are ordered by it.
    pub fn run(&mut self) -> Result<(), TaseError> {
        let mut listener = match self.listener.take() {
            Some(listener) => listener,
            None => {
                return Err(TaseError::internal(
                    "the hub has no pipe to listen on".to_string(),
                ));
            }
        };
        while !self.is_stopped() {
            match listener.accept(Duration::from_millis(POLL_MS)) {
                Ok(Some(accepted)) => {
                    let inner = Arc::clone(&self.inner);
                    let stopped = Arc::clone(&self.stopped);
                    std::thread::spawn(move || {
                        serve_connection(&inner, accepted, &stopped);
                    });
                }
                Ok(None) => {}
                Err(error) => {
                    self.degraded(&error.message);
                    break;
                }
            }
        }
        Ok(())
    }

    /// Records that the Hub runs on less than it should.
    pub fn degraded(&self, reason: &str) {
        let now_ms = self.now_ms();
        self.inner.lock().telemetry.push(HubEvent::Degraded {
            reason: redact(reason),
            at_ms: now_ms,
        });
    }
}

/// The name of a peer: the user and the terminal-services session, never a secret.
fn peer_name(peer: &PeerIdentity) -> String {
    format!("{}@{}", peer.user_sid, peer.session_id)
}

/// The hash of the logon of the owner: an audit group, never a proof of an identity.
fn owner_hash(logon_sid: &str) -> String {
    pime_core::sha256::sha256_hex(logon_sid)
}

/// A string of the parameters of a request.
fn string_param(params: &Json, names: &[&str]) -> Result<String, TaseError> {
    for name in names {
        if let Some(text) = params.get(name).and_then(Json::as_str) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
    }
    Err(TaseError::invalid_argument(format!(
        "the request has none of the fields {names:?}"
    )))
}

/// A string of the parameters of a request, when there is one.
fn optional_string_param(params: &Json, names: &[&str]) -> Option<String> {
    for name in names {
        if let Some(text) = params.get(name).and_then(Json::as_str) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// A list of strings of the parameters of a request.
fn string_list_param(params: &Json, names: &[&str]) -> Vec<String> {
    for name in names {
        if let Some(list) = params.get(name).and_then(Json::as_array) {
            return list
                .iter()
                .filter_map(Json::as_str)
                .map(str::to_string)
                .collect();
        }
    }
    Vec::new()
}

/// Serves one connection of the namespace.
fn serve_connection(inner: &Arc<HubInner>, accepted: AcceptedConnection, stopped: &Arc<AtomicBool>) {
    let Some(peer) = accepted.peer else {
        // a peer whose identity is not known is refused: it cannot be told apart from anybody
        return;
    };
    let now_ms = inner.now_ms();
    let connection = match inner.register_connection(&peer, now_ms) {
        Ok(connection) => connection,
        Err(error) => {
            let mut transport = accepted.transport;
            let payload = error.message.clone();
            let _ = write_frame(&mut transport, FrameKind::Close, 0, 0, payload.as_bytes());
            return;
        }
    };
    let session_id = connection.session.session_id.clone();

    // the handshake of the protocol belongs to the IPC crate: the Hub only gives it the session
    let mut hub_side = Connection::new(accepted.transport, inner.capabilities.clone(), inner.timeouts);
    if hub_side.accept_handshake(connection.connection_id).is_err() {
        inner.close_connection(connection.connection_id, "the handshake failed");
        return;
    }
    let mut transport = hub_side.into_transport();

    loop {
        if stopped.load(Ordering::SeqCst) {
            break;
        }
        let deadline = Instant::now() + Duration::from_millis(POLL_MS);
        let (header, payload) = match read_frame(&mut transport, deadline) {
            Ok(frame) => frame,
            Err(FrameError::Timeout) => continue,
            Err(_) => break,
        };
        match header.kind {
            FrameKind::Request => {
                let request = match parse_json(&payload) {
                    Ok(request) => request,
                    Err(_) => break,
                };
                let method = request
                    .get("method")
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_string();
                let params = request
                    .get("params")
                    .cloned()
                    .unwrap_or(Json::Obj(Vec::new()));
                let id = request
                    .get("id")
                    .cloned()
                    .unwrap_or(Json::int(header.sequence as i64));
                let answer = inner.dispatch(&session_id, &method, &params, inner.now_ms());
                let response = match answer {
                    Ok(result) => ok_response(&id, result),
                    Err(error) => error_response(&id, &error),
                };
                if write_frame(
                    &mut transport,
                    FrameKind::Response,
                    connection.connection_id,
                    header.sequence,
                    response.as_bytes(),
                )
                .is_err()
                {
                    break;
                }
                let name = method.strip_prefix("tase.").unwrap_or(&method);
                if name == "local.events" {
                    let mut state = inner.lock();
                    if let Some(connection) = state.connection_mut(connection.connection_id) {
                        connection.subscribed = true;
                    }
                }
                if push_events(inner, &mut transport, connection.connection_id).is_err() {
                    break;
                }
            }
            FrameKind::Ping => {
                if write_frame(
                    &mut transport,
                    FrameKind::Pong,
                    connection.connection_id,
                    header.sequence,
                    &[],
                )
                .is_err()
                {
                    break;
                }
            }
            FrameKind::Close => break,
            _ => break,
        }
    }
    inner.close_connection(connection.connection_id, "the connection ended");
}

/// Pushes the events of the Hub to the connections that asked for them.
fn push_events(
    inner: &Arc<HubInner>,
    transport: &mut BoxedTransport,
    connection_id: u64,
) -> Result<(), TaseError> {
    let mut state = inner.lock();
    let Some(connection) = state.connection_mut(connection_id) else {
        return Ok(());
    };
    if !connection.subscribed {
        return Ok(());
    }
    let last_seq = connection.last_event_seq;
    let events: Vec<Json> = state
        .telemetry
        .since(last_seq.saturating_add(1))
        .iter()
        .map(|event| event.to_json())
        .collect();
    if events.is_empty() {
        return Ok(());
    }
    let count = events.len() as u64;
    for event in events {
        let payload = event.to_json_string();
        write_frame(
            transport,
            FrameKind::Event,
            connection_id,
            0,
            payload.as_bytes(),
        )
        .map_err(crate::bootstrap::map_frame_error)?;
    }
    if let Some(connection) = state.connection_mut(connection_id) {
        connection.last_event_seq += count;
    }
    Ok(())
}

/// Reads one frame of a transport.
fn read_frame(
    transport: &mut BoxedTransport,
    deadline: Instant,
) -> Result<(FrameHeader, Vec<u8>), FrameError> {
    let mut prefix = [0u8; 4];
    read_exact(transport, &mut prefix, deadline)?;
    let body = tase_local_ipc::framing::body_len(&prefix)?;
    let mut buffer = vec![0u8; 4 + body];
    buffer[..4].copy_from_slice(&prefix);
    read_exact(transport, &mut buffer[4..], deadline)?;
    tase_local_ipc::framing::decode(&buffer)
}

/// Reads exactly the bytes of a buffer.
fn read_exact(
    transport: &mut BoxedTransport,
    buf: &mut [u8],
    deadline: Instant,
) -> Result<(), FrameError> {
    let mut got = 0;
    while got < buf.len() {
        let read = transport.recv_until(&mut buf[got..], deadline)?;
        if read == 0 {
            return Err(FrameError::Closed);
        }
        got += read;
    }
    Ok(())
}

/// Writes one frame on a transport.
fn write_frame(
    transport: &mut BoxedTransport,
    kind: FrameKind,
    session_id: u64,
    sequence: u64,
    payload: &[u8],
) -> Result<(), FrameError> {
    let header = FrameHeader {
        kind,
        session_id,
        sequence,
        payload_len: payload.len() as u32,
    };
    let bytes = tase_local_ipc::framing::encode(&header, payload)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    transport.send_until(&bytes, deadline)?;
    Ok(())
}

/// The JSON of a payload of a frame.
fn parse_json(payload: &[u8]) -> Result<Json, FrameError> {
    let text = std::str::from_utf8(payload).map_err(|_| FrameError::Malformed)?;
    pime_core::json::parse(text, JSON_LIMITS).map_err(|_| FrameError::Malformed)
}

/// The answer of a request that worked.
fn ok_response(id: &Json, result: Json) -> String {
    Json::obj(vec![
        ("jsonrpc", Json::str("2.0")),
        ("id", id.clone()),
        ("result", result),
    ])
    .to_json_string()
}

/// The answer of a request that failed.
fn error_response(id: &Json, error: &TaseError) -> String {
    Json::obj(vec![
        ("jsonrpc", Json::str("2.0")),
        ("id", id.clone()),
        (
            "error",
            Json::obj(vec![
                ("code", Json::int(i64::from(error.code.http_status()))),
                ("message", Json::str(&error.message)),
                (
                    "data",
                    Json::obj(vec![
                        ("business_code", Json::str(error.code.as_str())),
                        ("retryable", Json::Bool(error.code.retryable())),
                    ]),
                ),
            ]),
        ),
    ])
    .to_json_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::StaticSessionIdentity;
    use crate::session::SessionState;
    use crate::telemetry::HubEventKind;
    use tase_contract::ErrorCode;
    use crate::workspace::ManualClock;

    const LOGON: &str = "S-1-5-5-1000-1000";
    const USER: &str = "S-1-5-21-1-2-3-1001";

    fn identity() -> StaticSessionIdentity {
        StaticSessionIdentity::new(LOGON, USER, 3)
    }

    fn config() -> HubConfig {
        HubConfig {
            max_connections: 2,
            max_concurrent_jobs: 2,
            ..HubConfig::default()
        }
    }

    fn hub() -> (Hub, ManualClock) {
        let clock = ManualClock::new(1000);
        let hub = Hub::new(config(), &identity())
            .expect("the hub is built")
            .with_clock(Arc::new(clock.clone()))
            .with_scheduler(Box::new(RoundRobinScheduler::new(2)));
        (hub, clock)
    }

    fn peer() -> PeerIdentity {
        PeerIdentity::new(USER, LOGON, 3, 42)
    }

    /// A session that is connected and registered.
    fn registered(hub: &Hub, now_ms: u64) -> (ConnectionInfo, SessionId) {
        let connection = hub
            .register_connection(&peer(), now_ms)
            .expect("the connection is accepted");
        let params = Json::obj(vec![
            ("provider", Json::str("claude")),
            ("instance", Json::str("inst-1")),
            ("workspace_id", Json::str("ws_1")),
            ("capabilities", Json::strs(&["catalog:read"])),
            ("requested_scopes", Json::strs(&["catalog:read", "code:write"])),
        ]);
        hub.handle_message(
            &connection.session.session_id,
            "agent.register",
            &params,
            now_ms,
        )
        .expect("the agent registers");
        let session_id = connection.session.session_id.clone();
        (connection, session_id)
    }

    #[test]
    fn an_agent_registers_itself() {
        let (hub, _clock) = hub();
        let now = 1000;
        let connection = hub
            .register_connection(&peer(), now)
            .expect("the connection is accepted");
        let params = Json::obj(vec![
            ("provider", Json::str("claude")),
            ("instance", Json::str("inst-1")),
            ("workspace_id", Json::str("ws_1")),
            ("requested_scopes", Json::strs(&["catalog:read", "code:write"])),
        ]);
        let answer = hub
            .handle_message(
                &connection.session.session_id,
                "tase.agent.register",
                &params,
                now,
            )
            .expect("the agent registers");
        assert_eq!(
            answer.get("session_id").and_then(Json::as_str),
            Some(connection.session.session_id.as_str())
        );
        assert_eq!(
            answer
                .get("granted_scopes")
                .and_then(Json::as_array)
                .map(|list| list.len()),
            Some(2)
        );
        assert_eq!(hub.state().sessions.len(), 1);
    }

    #[test]
    fn a_scope_that_the_hub_does_not_offer_is_not_granted() {
        let (hub, _clock) = hub();
        let now = 1000;
        let connection = hub
            .register_connection(&peer(), now)
            .expect("the connection is accepted");
        let params = Json::obj(vec![
            ("provider", Json::str("claude")),
            ("workspace_id", Json::str("ws_1")),
            ("requested_scopes", Json::strs(&["catalog:read", "root:everything"])),
        ]);
        let answer = hub
            .handle_message(&connection.session.session_id, "agent.register", &params, now)
            .expect("the agent registers");
        let granted = answer
            .get("granted_scopes")
            .and_then(Json::as_array)
            .expect("the scopes are there");
        assert_eq!(granted.len(), 1);
        assert_eq!(granted[0].as_str(), Some("catalog:read"));
    }

    #[test]
    fn an_agent_that_registers_twice_is_refused() {
        let (hub, _clock) = hub();
        let now = 1000;
        let (_connection, session) = registered(&hub, now);
        let params = Json::obj(vec![
            ("provider", Json::str("claude")),
            ("workspace_id", Json::str("ws_1")),
        ]);
        let error = hub
            .handle_message(&session, "agent.register", &params, now)
            .expect_err("the second registration is refused");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn the_same_key_with_the_same_digest_gives_the_same_job() {
        let (hub, _clock) = hub();
        let now = 1000;
        let (_connection, session) = registered(&hub, now);
        let params = Json::obj(vec![
            ("idempotency_key", Json::str("key-1")),
            ("job_type", Json::str("apply")),
            ("request_digest", Json::str("digest-1")),
        ]);
        let first = hub
            .handle_message(&session, "job.enqueue", &params, now)
            .expect("the job is queued");
        let second = hub
            .handle_message(&session, "job.enqueue", &params, now)
            .expect("the job is queued again");
        assert_eq!(
            first.get("job_id").and_then(Json::as_str),
            second.get("job_id").and_then(Json::as_str)
        );
        assert_eq!(second.get("deduplicated").and_then(Json::as_bool), Some(true));
    }

    #[test]
    fn the_same_key_with_another_digest_is_a_conflict() {
        let (hub, _clock) = hub();
        let now = 1000;
        let (_connection, session) = registered(&hub, now);
        let first = Json::obj(vec![
            ("idempotency_key", Json::str("key-1")),
            ("request_digest", Json::str("digest-1")),
        ]);
        hub.handle_message(&session, "job.enqueue", &first, now)
            .expect("the job is queued");
        let second = Json::obj(vec![
            ("idempotency_key", Json::str("key-1")),
            ("request_digest", Json::str("digest-2")),
        ]);
        let error = hub
            .handle_message(&session, "job.enqueue", &second, now)
            .expect_err("the job is refused");
        assert_eq!(error.code, ErrorCode::IdempotencyMismatch);
    }

    #[test]
    fn the_state_of_a_job_is_right() {
        let (hub, _clock) = hub();
        let now = 1000;
        let (_connection, session) = registered(&hub, now);
        let params = Json::obj(vec![
            ("idempotency_key", Json::str("key-1")),
            ("request_digest", Json::str("digest-1")),
        ]);
        let answer = hub
            .handle_message(&session, "job.enqueue", &params, now)
            .expect("the job is queued");
        let job_id = answer.get("job_id").and_then(Json::as_str).expect("the job");
        let status = hub
            .handle_message(
                &session,
                "job.status",
                &Json::obj(vec![("job_id", Json::str(job_id))]),
                now,
            )
            .expect("the state of the job");
        assert_eq!(status.get("status").and_then(Json::as_str), Some("QUEUED"));
        assert_eq!(status.get("job_id").and_then(Json::as_str), Some(job_id));
    }

    #[test]
    fn the_state_of_a_job_that_is_not_there_is_refused() {
        let (hub, _clock) = hub();
        let now = 1000;
        let (_connection, session) = registered(&hub, now);
        let error = hub
            .handle_message(
                &session,
                "job.status",
                &Json::obj(vec![("job_id", Json::str("job_999"))]),
                now,
            )
            .expect_err("the job is not there");
        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn the_queue_of_the_hub_is_bounded() {
        let (hub, _clock) = hub();
        let now = 1000;
        let (_connection, session) = registered(&hub, now);
        for index in 0..2 {
            let params = Json::obj(vec![
                ("idempotency_key", Json::str(&format!("key-{index}"))),
                ("request_digest", Json::str(&format!("digest-{index}"))),
            ]);
            hub.handle_message(&session, "job.enqueue", &params, now)
                .expect("the job is queued");
        }
        let params = Json::obj(vec![
            ("idempotency_key", Json::str("key-3")),
            ("request_digest", Json::str("digest-3")),
        ]);
        let error = hub
            .handle_message(&session, "job.enqueue", &params, now)
            .expect_err("the job is refused");
        assert_eq!(error.code, ErrorCode::WorkerUnavailable);
    }

    #[test]
    fn the_number_of_the_connections_is_bounded() {
        let (hub, _clock) = hub();
        let now = 1000;
        hub.register_connection(&peer(), now)
            .expect("the first connection is accepted");
        hub.register_connection(&peer(), now)
            .expect("the second connection is accepted");
        let error = hub
            .register_connection(&peer(), now)
            .expect_err("the third connection is refused");
        assert_eq!(error.code, ErrorCode::RateLimited);
    }

    #[test]
    fn a_peer_of_another_session_is_refused() {
        let (hub, _clock) = hub();
        let mut other = peer();
        other.logon_sid = "S-1-5-5-2000-2000".to_string();
        let error = hub
            .register_connection(&other, 1000)
            .expect_err("the peer is refused");
        assert_eq!(error.code, ErrorCode::PermissionDenied);
    }

    #[test]
    fn a_disconnect_releases_the_locks_of_the_session() {
        let (hub, _clock) = hub();
        let now = 1000;
        let (connection, session) = registered(&hub, now);
        let params = Json::obj(vec![
            ("workspace_id", Json::str("ws_1")),
            ("path", Json::str("src/main.rs")),
        ]);
        hub.handle_message(&session, "workspace.acquire", &params, now)
            .expect("the lock is taken");
        assert_eq!(hub.state().workspaces.len(), 1);
        hub.close_connection(connection.connection_id, "the peer left");
        assert_eq!(hub.state().workspaces.len(), 0);
        assert_eq!(
            hub.state()
                .sessions
                .get(&session)
                .expect("the session is there")
                .state,
            SessionState::Disconnected
        );
    }

    #[test]
    fn a_disconnect_releases_the_leases_of_the_session() {
        let (hub, _clock) = hub();
        let now = 1000;
        let (connection, session) = registered(&hub, now);
        let answer = hub
            .handle_message(&session, "agent.heartbeat", &Json::Obj(Vec::new()), now)
            .expect("the heartbeat is accepted");
        assert!(
            answer
                .get("lease_expires_at_ms")
                .and_then(Json::as_i64)
                .unwrap_or(0)
                > now as i64
        );
        hub.close_connection(connection.connection_id, "the peer left");
        assert_eq!(
            hub.state()
                .sessions
                .get(&session)
                .expect("the session is there")
                .state,
            SessionState::Disconnected
        );
    }

    #[test]
    fn an_agent_unregisters_itself() {
        let (hub, _clock) = hub();
        let now = 1000;
        let (_connection, session) = registered(&hub, now);
        let answer = hub
            .handle_message(&session, "agent.unregister", &Json::Obj(Vec::new()), now)
            .expect("the agent leaves");
        assert_eq!(answer.get("accepted").and_then(Json::as_bool), Some(true));
        assert_eq!(hub.state().sessions.len(), 0);
    }

    #[test]
    fn the_second_agent_of_a_file_is_refused_and_the_refusal_is_written_down() {
        let (hub, _clock) = hub();
        let now = 1000;
        let (_first, first_session) = registered(&hub, now);
        let params = Json::obj(vec![
            ("workspace_id", Json::str("ws_1")),
            ("path", Json::str("src/main.rs")),
        ]);
        hub.handle_message(&first_session, "workspace.acquire", &params, now)
            .expect("the lock is taken");
        // another agent, on another connection
        let connection = hub
            .register_connection(&peer(), now)
            .expect("the connection is accepted");
        let register = Json::obj(vec![
            ("provider", Json::str("codex")),
            ("instance", Json::str("inst-2")),
            ("workspace_id", Json::str("ws_1")),
        ]);
        hub.handle_message(
            &connection.session.session_id,
            "agent.register",
            &register,
            now,
        )
        .expect("the agent registers");
        let error = hub
            .handle_message(
                &connection.session.session_id,
                "workspace.acquire",
                &params,
                now,
            )
            .expect_err("the lock is refused");
        assert_eq!(error.code, ErrorCode::SymbolConflict);
        let kinds: Vec<&str> = hub
            .state()
            .telemetry
            .events()
            .iter()
            .map(HubEvent::kind)
            .map(HubEventKind::as_str)
            .collect();
        assert!(kinds.contains(&"lock_denied"), "{kinds:?}");
    }

    #[test]
    fn the_state_of_the_hub_is_reported() {
        let (hub, _clock) = hub();
        let now = 1000;
        let (_connection, _session) = registered(&hub, now);
        let answer = hub
            .handle_message(
                &SessionId::new("sess_none").expect("a legal id"),
                "local.status",
                &Json::Obj(Vec::new()),
                now,
            )
            .expect("the state is reported");
        assert_eq!(answer.get("sessions").and_then(Json::as_i64), Some(1));
        assert_eq!(answer.get("connections").and_then(Json::as_i64), Some(1));
        assert_eq!(answer.get("max_connections").and_then(Json::as_i64), Some(2));
    }

    #[test]
    fn the_events_are_reported_in_their_order() {
        let (hub, _clock) = hub();
        let now = 1000;
        let (_connection, session) = registered(&hub, now);
        let answer = hub
            .handle_message(&session, "local.events", &Json::Obj(Vec::new()), now)
            .expect("the events are reported");
        let events = answer
            .get("events")
            .and_then(Json::as_array)
            .expect("the events are there");
        assert!(events.len() >= 2, "{events:?}");
        assert_eq!(
            events[0].get("kind").and_then(Json::as_str),
            Some("connection_opened")
        );
        assert_eq!(
            events[1].get("kind").and_then(Json::as_str),
            Some("agent_registered")
        );
    }

    #[test]
    fn a_method_that_the_hub_does_not_have_is_refused() {
        let (hub, _clock) = hub();
        let now = 1000;
        let (_connection, session) = registered(&hub, now);
        let error = hub
            .handle_message(&session, "does.not.exist", &Json::Obj(Vec::new()), now)
            .expect_err("the method is refused");
        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn a_method_of_a_session_that_is_not_there_is_refused() {
        let (hub, _clock) = hub();
        let error = hub
            .handle_message(
                &SessionId::new("sess_ghost").expect("a legal id"),
                "agent.heartbeat",
                &Json::Obj(Vec::new()),
                1000,
            )
            .expect_err("the session is not there");
        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn the_hub_reloads_the_jobs_that_were_not_finished() {
        let directory = temp_dir("recovery");
        let path = directory.join("hub.db");
        let store = Store::open(&path).expect("the store is open");
        let hub = Hub::new(config(), &identity())
            .expect("the hub is built")
            .with_clock(Arc::new(ManualClock::new(1000)))
            .with_store(store)
            .expect("the store is opened");
        let now = 1000;
        let (_connection, session) = registered(&hub, now);
        let params = Json::obj(vec![
            ("idempotency_key", Json::str("key-1")),
            ("request_digest", Json::str("digest-1")),
        ]);
        let answer = hub
            .handle_message(&session, "job.enqueue", &params, now)
            .expect("the job is queued");
        let job_id = answer.get("job_id").and_then(Json::as_str).expect("the job");
        drop(hub);

        // the hub comes back with the same store
        let store = Store::open(&path).expect("the store is open again");
        let hub = Hub::new(config(), &identity())
            .expect("the hub is built")
            .with_clock(Arc::new(ManualClock::new(2000)))
            .with_store(store)
            .expect("the store is opened");
        let report = hub.recover(2000).expect("the recovery runs");
        assert_eq!(report.recovered_jobs, 1);
        assert_eq!(hub.state().scheduler.queued(), 1);
        let (_connection, session) = registered(&hub, 2000);
        let status = hub
            .handle_message(
                &session,
                "job.status",
                &Json::obj(vec![("job_id", Json::str(job_id))]),
                2000,
            )
            .expect("the job is known again");
        assert_eq!(status.get("status").and_then(Json::as_str), Some("QUEUED"));
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn the_generation_of_a_file_survives_the_restart() {
        let directory = temp_dir("generation");
        let path = directory.join("hub.db");
        let store = Store::open(&path).expect("the store is open");
        let hub = Hub::new(config(), &identity())
            .expect("the hub is built")
            .with_clock(Arc::new(ManualClock::new(1000)))
            .with_store(store)
            .expect("the store is opened");
        let now = 1000;
        let (_connection, session) = registered(&hub, now);
        let write = Json::obj(vec![
            ("workspace_id", Json::str("ws_1")),
            ("path", Json::str("src/main.rs")),
            ("content_digest", Json::str("sha256:abc")),
        ]);
        let answer = hub
            .handle_message(&session, "workspace.write", &write, now)
            .expect("the write is recorded");
        assert_eq!(answer.get("generation").and_then(Json::as_i64), Some(1));
        drop(hub);

        let store = Store::open(&path).expect("the store is open again");
        let hub = Hub::new(config(), &identity())
            .expect("the hub is built")
            .with_clock(Arc::new(ManualClock::new(2000)))
            .with_store(store)
            .expect("the store is opened");
        let (_connection, session) = registered(&hub, 2000);
        let snapshot = hub
            .handle_message(
                &session,
                "workspace.snapshot",
                &Json::obj(vec![
                    ("workspace_id", Json::str("ws_1")),
                    ("path", Json::str("src/main.rs")),
                ]),
                2000,
            )
            .expect("the snapshot is taken");
        assert_eq!(snapshot.get("generation").and_then(Json::as_i64), Some(1));
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_write_that_is_based_on_an_old_generation_is_refused() {
        let (hub, _clock) = hub();
        let now = 1000;
        let (_connection, session) = registered(&hub, now);
        let write = Json::obj(vec![
            ("workspace_id", Json::str("ws_1")),
            ("path", Json::str("src/main.rs")),
            ("content_digest", Json::str("sha256:abc")),
            ("expected_generation", Json::int(0)),
        ]);
        hub.handle_message(&session, "workspace.write", &write, now)
            .expect("the first write is recorded");
        let error = hub
            .handle_message(&session, "workspace.write", &write, now)
            .expect_err("the second write is refused");
        assert_eq!(error.code, ErrorCode::SymbolConflict);
    }

    #[test]
    fn the_scheduler_takes_the_jobs_of_the_agents_in_turn() {
        let mut scheduler = RoundRobinScheduler::new(2);
        let first = AgentId::new("agent_one").expect("a legal id");
        let second = AgentId::new("agent_two").expect("a legal id");
        let job = |agent: &AgentId, name: &str| JobRequest {
            job_id: JobId::new(name).expect("a legal id"),
            job_type: "apply".to_string(),
            agent_id: agent.clone(),
            session_id: SessionId::new("sess_one").expect("a legal id"),
            workspace_id: WorkspaceId::new("ws_1").expect("a legal id"),
            priority: 0,
            enqueued_at_ms: 0,
        };
        scheduler.enqueue(job(&first, "job_1")).expect("queued");
        scheduler.enqueue(job(&second, "job_2")).expect("queued");
        assert_eq!(scheduler.queued(), 2);
        let taken = scheduler.next_job(0).expect("a job is taken");
        assert_eq!(taken.job_id.as_str(), "job_1");
        let taken = scheduler.next_job(0).expect("a job is taken");
        assert_eq!(taken.job_id.as_str(), "job_2");
        scheduler.complete(&taken.job_id, 0);
        assert_eq!(scheduler.running(), 1);
    }

    #[test]
    fn the_scheduler_does_not_give_more_jobs_than_it_may_run() {
        let mut scheduler = RoundRobinScheduler::new(1);
        let agent = AgentId::new("agent_one").expect("a legal id");
        for index in 0..2 {
            scheduler
                .enqueue(JobRequest {
                    job_id: JobId::new(format!("job_{index}")).expect("a legal id"),
                    job_type: "apply".to_string(),
                    agent_id: agent.clone(),
                    session_id: SessionId::new("sess_one").expect("a legal id"),
                    workspace_id: WorkspaceId::new("ws_1").expect("a legal id"),
                    priority: 0,
                    enqueued_at_ms: 0,
                })
                .expect("queued");
        }
        assert!(scheduler.next_job(0).is_some());
        assert!(scheduler.next_job(0).is_none());
    }

    #[test]
    fn the_answer_of_a_request_has_the_shape_of_a_json_rpc_answer() {
        let answer = ok_response(&Json::int(7), Json::obj(vec![("a", Json::int(1))]));
        assert!(answer.contains("\"jsonrpc\":\"2.0\""));
        assert!(answer.contains("\"id\":7"));
        assert!(answer.contains("\"result\""));
        let failure = error_response(&Json::int(7), &TaseError::rate_limited("full"));
        assert!(failure.contains("\"error\""));
        assert!(failure.contains("RATE_LIMITED"));
    }

    #[test]
    fn the_hub_can_be_stopped() {
        let (hub, _clock) = hub();
        assert!(!hub.is_stopped());
        hub.stop();
        assert!(hub.is_stopped());
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!("tase-local-hub-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("the directory is made");
        directory
    }
}
