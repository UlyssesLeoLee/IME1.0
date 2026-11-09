//! The fair scheduler of the Hub: the adapter between the seam of §17.5 and `tase-agent-scheduler`.
//!
//! The detailed design §17.5 asks for two fairnesses at the same time, and they are two different
//! kinds of fairness:
//!
//! * **between the classes**: the five layered queues of the design book (`interactive_candidate`,
//!   `preview_read`, `prepare`, `verify`, `commit`), so that a flood of cheap reads does not drown
//!   the verification work;
//! * **between the agents**: inside a class, the agents share the work by their weight (the deficit
//!   round robin of §17.5).
//!
//! Both live in `tase-agent-scheduler`. This module is the **adapter** that plugs that crate into
//! the seam of [`JobScheduler`]; the Hub writes no second queue and no second round robin of its
//! own.
//!
//! What the adapter adds, and what it loses:
//!
//! * **the job stays whole.** A [`Task`] of the scheduler carries the id of the job, the agent, the
//!   class, the cost and the instant the task was queued. The Hub hands out a [`JobRequest`], which
//!   also carries the session, the workspace, the kind of the job and the priority. The adapter
//!   keeps the `job_id → JobRequest` map and gives the whole request back when the task comes out of
//!   the queue.
//! * **the class comes from the kind of the job.** The Hub does not name a class: it names a kind of
//!   job (`apply`, `preview`, ...). [`work_class_of`] maps the kind to the class of the design book;
//!   a kind that no table knows goes to [`DEFAULT_WORK_CLASS`].
//! * **the lease comes with the dispatch.** A job that leaves the queue takes a lease of the
//!   [`LeaseRegistry`] of the scheduler, and the lease carries the fencing token that refuses the
//!   late write of a holder that was replaced (detailed design §17.6).
//!
//! The rules of the module:
//!
//! * **the Hub never reads the clock.** Every decision of the adapter takes the instant of the
//!   caller (`now_ms`), exactly like the seam does;
//! * **nothing is lost.** A job that cannot be handed out (because it is already leased, or because
//!   it is done) is refused, it is never dropped silently;
//! * **the round robin of the Hub stays.** [`crate::server::RoundRobinScheduler`] is the degradation
//!   of the design book and it is still there; the configuration says which one the Hub uses.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use tase_agent_scheduler::{
    FenceCheck, Lease, LeaseRegistry, Scheduler, Task, Timestamp, WorkClass,
};
use tase_contract::{AgentId, JobId, TaseError};

use crate::server::{JobRequest, JobScheduler};

/// The class of a kind of job that no table knows.
///
/// The kind of a job is a free text in the Hub (`JobRequest::job_type`), so a kind that nobody
/// knows must land somewhere. It lands in [`WorkClass::Prepare`], and not in another class, for
/// three reasons:
///
/// * the preparation is the class the pipeline of §17.5 really runs (`run_preparation_in_isolated_worker`);
/// * a job that nobody knows must not jump in front of the work of the keyboard, so it is not an
///   [`WorkClass::InteractiveCandidate`];
/// * a job that nobody knows must not open the commit window of §17.5, so it is not an
///   [`WorkClass::Commit`].
pub const DEFAULT_WORK_CLASS: WorkClass = WorkClass::Prepare;

/// The cost of a class in the credits of the round robin.
///
/// The credits of the deficit round robin are the unit of the work: a cheap read costs one, and the
/// work that touches the compiler and the ref costs more. The table is the one of the design book,
/// read from the cheapest to the most expensive.
pub const fn cost_of(class: WorkClass) -> u32 {
    match class {
        WorkClass::InteractiveCandidate => 1,
        WorkClass::PreviewRead => 1,
        WorkClass::Prepare => 2,
        WorkClass::Verify => 4,
        WorkClass::Commit => 8,
    }
}

/// The quantum of the round robin of the adapter: the cost of the most expensive class.
///
/// The quantum is the number of credits that one visit of the round robin grants to an agent. The
/// adapter makes it the cost of the heaviest class, so that one visit always pays for one task of
/// any class: no task ever waits for a credit that no visit will bring. The share of the agents
/// then follows their weights, because the credits of a visit are `weight × quantum`.
const QUANTUM: u32 = cost_of(WorkClass::Commit);

/// The class of a kind of job (detailed design §17.5).
///
/// The table is the one of the design book:
///
/// | kind of the job | class |
/// |---|---|
/// | `candidate`, `interactive_candidate` | [`WorkClass::InteractiveCandidate`] |
/// | `preview`, `preview_read` | [`WorkClass::PreviewRead`] |
/// | `prepare` | [`WorkClass::Prepare`] |
/// | `verify` | [`WorkClass::Verify`] |
/// | `commit` | [`WorkClass::Commit`] |
/// | `apply` | [`WorkClass::Prepare`] |
/// | anything else | [`DEFAULT_WORK_CLASS`] |
///
/// The kind is normalized before it is looked up: it is put in lower case and the `.`, the `-` and
/// the `:` become `_`. The whole name is looked up first; a name that has more than one segment is
/// then looked up by its last segment, so `tase.code.apply` lands on `apply` and `tase.code.verify`
/// on `verify`.
///
/// `apply` is a preparation and not a commit: an apply job is the work of an agent on a change, and
/// the commit window of §17.5 is for the job that pushes the ref.
#[must_use]
pub fn work_class_of(job_type: &str) -> WorkClass {
    let normalized = normalize(job_type);
    if let Some(class) = class_of_name(&normalized) {
        return class;
    }
    // The namespaced kinds: "tase.code.apply" -> "apply".
    let last = normalized.rsplit('_').next().unwrap_or_default();
    class_of_name(last).unwrap_or(DEFAULT_WORK_CLASS)
}

/// Puts a kind of job in the shape the table is written in.
fn normalize(job_type: &str) -> String {
    job_type
        .chars()
        .map(|character| match character {
            '.' | '-' | ':' => '_',
            other => other.to_ascii_lowercase(),
        })
        .collect()
}

/// The class of a name of the table.
fn class_of_name(name: &str) -> Option<WorkClass> {
    match name {
        "interactive_candidate" | "candidate" => Some(WorkClass::InteractiveCandidate),
        "preview_read" | "preview" => Some(WorkClass::PreviewRead),
        "prepare" => Some(WorkClass::Prepare),
        "verify" => Some(WorkClass::Verify),
        "commit" => Some(WorkClass::Commit),
        "apply" => Some(WorkClass::Prepare),
        _ => None,
    }
}

/// The scheduler of the Hub that the configuration asks for (detailed design §17.5).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SchedulerKind {
    /// The fair scheduler of `tase-agent-scheduler`: the five classes and the weighted round robin.
    ///
    /// This is the default: the design book asks for the fairness of §17.5, and this is the crate
    /// that owns it.
    #[default]
    Fair,
    /// The round robin of the Hub: the degradation where every agent takes turns.
    ///
    /// It stays in the code because the design book writes it down as the simplification of the
    /// fairness, and a Hub that cannot use the scheduler crate still has to run the agents.
    RoundRobin,
}

impl SchedulerKind {
    /// The name of the kind; the same in the configuration and in the log.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            SchedulerKind::Fair => "fair",
            SchedulerKind::RoundRobin => "round-robin",
        }
    }

    /// The kind of a name; `None` when the name is not one of ours.
    #[must_use]
    pub fn from_name(name: &str) -> Option<SchedulerKind> {
        match name {
            "fair" => Some(SchedulerKind::Fair),
            "round-robin" | "round_robin" => Some(SchedulerKind::RoundRobin),
            _ => None,
        }
    }
}

/// The fair scheduler of the Hub: the five classes, the weighted round robin and the leases of
/// `tase-agent-scheduler`, behind the seam of [`JobScheduler`].
///
/// The adapter owns three things of its own, and nothing else:
///
/// * the `job_id → JobRequest` map, because the scheduler carries only the id of a job;
/// * the set of the jobs that run, because the scheduler forgets a task as soon as it hands it out;
/// * the mapping of the kind of a job to the class of the design book.
#[derive(Debug)]
pub struct FairScheduler {
    /// The queues of the scheduler: the five classes and the deficit round robin over the agents.
    scheduler: Scheduler,
    /// The leases of the jobs that run, with their fencing tokens.
    leases: LeaseRegistry,
    /// The whole request of every job the adapter knows, so that a task can be given back whole.
    requests: HashMap<JobId, JobRequest>,
    /// The jobs that were handed out and are not done yet.
    running: HashSet<JobId>,
    /// The most jobs that may run at the same time.
    max_running: usize,
    /// How long the lease of a job lasts.
    lease_ttl: Duration,
    /// The weight an agent gets when nobody says otherwise.
    default_weight: u32,
}

impl FairScheduler {
    /// A fair scheduler that lets at most `max_running` jobs run at the same time.
    #[must_use]
    pub fn new(max_running: u32) -> FairScheduler {
        FairScheduler {
            scheduler: Scheduler::new().with_quantum(QUANTUM),
            leases: LeaseRegistry::new(),
            requests: HashMap::new(),
            running: HashSet::new(),
            max_running: max_running as usize,
            lease_ttl: Duration::from_millis(crate::defaults::JOB_LEASE_MS),
            default_weight: 1,
        }
    }

    /// The same scheduler with another lease time; a lease of zero would be refused at the door.
    #[must_use]
    pub fn with_lease_ttl(mut self, lease_ttl: Duration) -> FairScheduler {
        self.lease_ttl = if lease_ttl.is_zero() {
            Duration::from_millis(1)
        } else {
            lease_ttl
        };
        self
    }

    /// The same scheduler with another weight for the agents nobody weighs.
    #[must_use]
    pub fn with_default_weight(mut self, weight: u32) -> FairScheduler {
        self.default_weight = weight.max(1);
        self
    }

    /// The weight of an agent, or `None` when the agent is not registered.
    ///
    /// The weight is the share of the agent inside a class: the round robin grants the agent
    /// `weight × quantum` credits at every visit.
    #[must_use]
    pub fn weight_of(&self, agent_id: &AgentId) -> Option<u32> {
        self.scheduler.weight_of(agent_id)
    }

    /// The lease of a job, or `None` when the job has no lease.
    #[must_use]
    pub fn lease_of(&self, job_id: &JobId) -> Option<&Lease> {
        self.leases.current(job_id)
    }

    /// The verdict of the fence for a holder that wants to write at `now_ms`.
    ///
    /// The verdict is the one of the scheduler crate: the write of a holder whose lease is over is
    /// refused, and the write of a holder that was replaced is refused too (a zombie).
    #[must_use]
    pub fn check_lease(&self, lease: &Lease, now_ms: u64) -> FenceCheck {
        self.leases.check(lease, Timestamp::from_millis(now_ms))
    }

    /// Whether the holder of a lease may write at `now_ms`.
    #[must_use]
    pub fn may_write(&self, lease: &Lease, now_ms: u64) -> bool {
        self.leases.may_write(lease, Timestamp::from_millis(now_ms))
    }

    /// Renews the lease of a job; only the holder of the current lease may do it.
    pub fn renew_lease(
        &mut self,
        lease: &Lease,
        now_ms: u64,
    ) -> Result<Lease, TaseError> {
        self.leases
            .renew(lease, self.lease_ttl, Timestamp::from_millis(now_ms))
    }

    /// Gives the lease of a job back; only the holder of the current lease may do it.
    pub fn release_lease(&mut self, lease: &Lease, now_ms: u64) -> Result<(), TaseError> {
        self.leases.release(lease, Timestamp::from_millis(now_ms))
    }

    /// Takes back the leases that ran out of time at `now_ms` and gives them to the caller.
    ///
    /// The reclaim does not give a new token: the next lease of the job takes one, and it is bigger
    /// than the token of the lease that was reclaimed.
    pub fn expire_leases(&mut self, now_ms: u64) -> Vec<Lease> {
        self.leases.expire(Timestamp::from_millis(now_ms))
    }

    /// Says that an agent may run jobs and with which weight.
    ///
    /// A weight of zero is refused by the scheduler crate, and the adapter does not change it: an
    /// agent that asks for no share at all is a mistake of the caller, not a decision of the Hub.
    pub fn set_agent_weight(&mut self, agent_id: &AgentId, weight: u32) -> Result<(), TaseError> {
        self.scheduler.register_agent(agent_id.clone(), weight)
    }

    /// Makes sure the agent of a job is known by the scheduler.
    ///
    /// An agent that the Hub has not weighed yet gets the default weight: the Hub must not refuse
    /// the job of an agent that only registered itself after the Hub was started.
    fn ensure_agent(&mut self, agent_id: &AgentId) -> Result<(), TaseError> {
        if self.scheduler.weight_of(agent_id).is_none() {
            self.scheduler
                .register_agent(agent_id.clone(), self.default_weight)?;
        }
        Ok(())
    }

    /// Hands a task out, if the job of the task may run now.
    ///
    /// The job must not be leased already: a job whose lease is not over runs with its first
    /// holder, and the holder that was replaced must not run the job a second time.
    fn dispatch(&mut self, task: &Task, now: Timestamp) -> Option<JobRequest> {
        let request = self.requests.get(&task.job_id).cloned()?;
        if self
            .leases
            .current(&request.job_id)
            .is_some_and(|lease| lease.is_valid_at(now))
        {
            return None;
        }
        let lease = self.leases.acquire(
            request.job_id.clone(),
            request.agent_id.clone(),
            Some(request.session_id.clone()),
            self.lease_ttl,
            now,
        );
        if lease.is_err() {
            return None;
        }
        self.running.insert(request.job_id.clone());
        Some(request)
    }
}

impl JobScheduler for FairScheduler {
    fn enqueue(&mut self, job: JobRequest) -> Result<(), TaseError> {
        self.ensure_agent(&job.agent_id)?;
        let class = work_class_of(&job.job_type);
        let task = Task::new(
            job.job_id.clone(),
            job.agent_id.clone(),
            class,
            Timestamp::from_millis(job.enqueued_at_ms),
        )
        .with_cost(cost_of(class));
        self.scheduler.submit(task)?;
        self.requests.insert(job.job_id.clone(), job);
        Ok(())
    }

    fn next_job(&mut self, now_ms: u64) -> Option<JobRequest> {
        if self.running.len() >= self.max_running {
            return None;
        }
        let now = Timestamp::from_millis(now_ms);
        while let Some(task) = self.scheduler.next_task(now) {
            // A job that is done is not run again, and a job whose lease cannot be taken is not
            // handed out: the task is refused, it is not lost.
            if let Some(request) = self.dispatch(&task, now) {
                return Some(request);
            }
        }
        None
    }

    fn complete(&mut self, job_id: &JobId, now_ms: u64) {
        self.running.remove(job_id);
        self.requests.remove(job_id);
        if let Some(lease) = self.leases.current(job_id).cloned() {
            // The lease of a job that is done is given back. A lease that ran out of time is not
            // given back: it waits for the reclaim, and the registry keeps its fencing token.
            let _ = self
                .leases
                .release(&lease, Timestamp::from_millis(now_ms));
        }
    }

    /// The token of the lease of a job: the fence of the split brain.
    ///
    /// The Hub hands this token to the worker that takes the job and checks it when the worker
    /// reports: the store does not check the epoch of a job, so the token of the lease is the only
    /// thing that refuses the report of a holder that was replaced (detailed design §17.6).
    fn lease_token(&self, job_id: &JobId) -> Option<u64> {
        self.leases.current(job_id).map(|lease| lease.token.as_u64())
    }

    fn queued(&self) -> usize {
        self.scheduler.len()
    }

    fn running(&self) -> usize {
        self.running.len()
    }

    fn register_agent(&mut self, agent_id: &AgentId, weight: u32) -> Result<(), TaseError> {
        self.set_agent_weight(agent_id, weight)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tase_agent_scheduler::FencingToken;
    use tase_contract::{SessionId, WorkspaceId};

    /// An agent of the tests.
    fn agent(name: &str) -> AgentId {
        AgentId::new(name).expect("a legal agent")
    }

    /// The request of a job of the tests.
    fn request(agent: &AgentId, name: &str, job_type: &str) -> JobRequest {
        JobRequest {
            job_id: JobId::new(name).expect("a legal job"),
            job_type: job_type.to_string(),
            agent_id: agent.clone(),
            session_id: SessionId::new("sess_01").expect("a legal session"),
            workspace_id: WorkspaceId::new("ws_01").expect("a legal workspace"),
            priority: 0,
            enqueued_at_ms: 0,
        }
    }

    #[test]
    fn the_five_kinds_of_job_go_to_their_class() {
        assert_eq!(
            work_class_of("candidate"),
            WorkClass::InteractiveCandidate,
            "the candidate of the keyboard comes first"
        );
        assert_eq!(
            work_class_of("interactive_candidate"),
            WorkClass::InteractiveCandidate
        );
        assert_eq!(work_class_of("preview"), WorkClass::PreviewRead);
        assert_eq!(work_class_of("preview_read"), WorkClass::PreviewRead);
        assert_eq!(work_class_of("prepare"), WorkClass::Prepare);
        assert_eq!(work_class_of("verify"), WorkClass::Verify);
        assert_eq!(work_class_of("commit"), WorkClass::Commit);

        // The namespaced kinds of the repository land on the same classes.
        assert_eq!(work_class_of("tase.code.apply"), WorkClass::Prepare);
        assert_eq!(work_class_of("tase.code.verify"), WorkClass::Verify);
        assert_eq!(work_class_of("tase.code.commit"), WorkClass::Commit);
        assert_eq!(work_class_of("tase.code.preview"), WorkClass::PreviewRead);
        assert_eq!(
            work_class_of("tase.code.candidate"),
            WorkClass::InteractiveCandidate
        );

        // An apply job is the preparation of a change, not the commit of it.
        assert_eq!(work_class_of("apply"), WorkClass::Prepare);
    }

    #[test]
    fn a_kind_of_job_that_is_not_known_goes_to_the_default_class() {
        for kind in ["", "mystery", "tase.code.something", "42"] {
            assert_eq!(
                work_class_of(kind),
                DEFAULT_WORK_CLASS,
                "the kind {kind:?} must not panic and must not be refused"
            );
        }
        assert_eq!(DEFAULT_WORK_CLASS, WorkClass::Prepare);
    }

    #[test]
    fn the_class_of_the_job_decides_how_soon_it_runs() {
        let mut scheduler = FairScheduler::new(8);
        let agent = agent("agent_01");
        scheduler.set_agent_weight(&agent, 1).expect("the agent is registered");

        // The commit is queued first: it is the class of the job, not the order of the queue, that
        // decides when a job runs.
        scheduler
            .enqueue(request(&agent, "job_commit", "commit"))
            .expect("queued");
        scheduler
            .enqueue(request(&agent, "job_prepare", "prepare"))
            .expect("queued");
        scheduler
            .enqueue(request(&agent, "job_candidate", "candidate"))
            .expect("queued");
        scheduler
            .enqueue(request(&agent, "job_preview", "preview"))
            .expect("queued");
        scheduler
            .enqueue(request(&agent, "job_verify", "verify"))
            .expect("queued");

        let mut order: Vec<String> = Vec::new();
        while let Some(job) = scheduler.next_job(0) {
            order.push(job.job_id.as_str().to_string());
        }
        assert_eq!(
            order,
            [
                "job_candidate",
                "job_preview",
                "job_prepare",
                "job_verify",
                "job_commit"
            ]
        );
    }

    #[test]
    fn the_share_of_the_agents_follows_their_weights() {
        let mut scheduler = FairScheduler::new(1_000);
        let heavy = agent("agent_heavy");
        let light = agent("agent_light");
        scheduler.set_agent_weight(&heavy, 3).expect("the agent is registered");
        scheduler.set_agent_weight(&light, 1).expect("the agent is registered");
        assert_eq!(scheduler.weight_of(&heavy), Some(3));
        assert_eq!(scheduler.weight_of(&light), Some(1));

        // Both queues stay full over the window of the measure: the share is a property of the
        // window in which every agent still has work to do.
        for index in 0..300 {
            scheduler
                .enqueue(request(&heavy, &format!("job_heavy_{index}"), "apply"))
                .expect("queued");
        }
        for index in 0..100 {
            scheduler
                .enqueue(request(&light, &format!("job_light_{index}"), "apply"))
                .expect("queued");
        }
        assert_eq!(scheduler.queued(), 400);

        let mut heavy_count = 0usize;
        let mut light_count = 0usize;
        for _ in 0..400 {
            let taken = scheduler.next_job(0).expect("a job runs");
            if taken.agent_id == heavy {
                heavy_count += 1;
            } else {
                light_count += 1;
            }
        }
        let total = heavy_count + light_count;
        assert_eq!(total, 400, "every job of the window is handed out");

        // The ideal share of the heavy agent is 3/4 of the tasks. The deviation of a deficit round
        // robin over a window is bounded by one round of the weights, so 1% is a wide margin.
        let ideal = total * 3 / 4;
        let deviation = heavy_count as i64 - ideal as i64;
        assert!(
            deviation.abs() <= (total as i64) / 100,
            "the share of the heavy agent is off by {deviation} over {total} tasks"
        );
        let ideal_light = total / 4;
        let deviation_light = light_count as i64 - ideal_light as i64;
        assert!(
            deviation_light.abs() <= (total as i64) / 100,
            "the share of the light agent is off by {deviation_light} over {total} tasks"
        );
        assert!(
            light_count > 0,
            "the light agent is never starved by the heavy one"
        );
    }

    #[test]
    fn a_job_that_runs_takes_a_lease_and_the_old_token_is_refused() {
        let mut scheduler = FairScheduler::new(4);
        let agent = agent("agent_01");
        scheduler.set_agent_weight(&agent, 1).expect("the agent is registered");
        scheduler
            .enqueue(request(&agent, "job_01", "apply"))
            .expect("queued");

        let taken = scheduler.next_job(0).expect("a job runs");
        let lease = scheduler
            .lease_of(&taken.job_id)
            .expect("the job has a lease")
            .clone();
        assert_eq!(lease.agent_id, agent);
        assert_eq!(lease.token, FencingToken::FIRST);
        assert!(scheduler.may_write(&lease, 0), "the holder may write");

        // The lease runs out of time: the write of the old holder is refused.
        let later = crate::defaults::JOB_LEASE_MS + 1;
        assert!(!scheduler.may_write(&lease, later));
        assert_eq!(scheduler.check_lease(&lease, later), FenceCheck::Expired);
        assert!(
            scheduler
                .check_lease(&lease, later)
                .to_error()
                .is_some_and(|error| error.code == tase_contract::ErrorCode::WorkerUnavailable)
        );

        // The lease is taken back and the job is queued again: the new holder takes a bigger token.
        assert_eq!(scheduler.expire_leases(later).len(), 1);
        scheduler
            .enqueue(request(&agent, "job_01", "apply"))
            .expect("queued");
        let again = scheduler.next_job(later).expect("the job runs again");
        let new_lease = scheduler
            .lease_of(&again.job_id)
            .expect("the job has a lease")
            .clone();
        assert!(
            new_lease.token > lease.token,
            "the token of a new lease is always bigger"
        );

        // The old holder is a zombie: its write is refused.
        assert_eq!(scheduler.check_lease(&lease, later), FenceCheck::StaleToken);
        assert!(!scheduler.may_write(&lease, later));
        assert!(
            scheduler
                .check_lease(&lease, later)
                .to_error()
                .is_some_and(|error| error.code == tase_contract::ErrorCode::PermissionDenied)
        );
        assert!(scheduler.may_write(&new_lease, later));
    }

    #[test]
    fn a_job_that_is_done_is_not_handed_out_again() {
        let mut scheduler = FairScheduler::new(4);
        let agent = agent("agent_01");
        scheduler.set_agent_weight(&agent, 1).expect("the agent is registered");
        scheduler
            .enqueue(request(&agent, "job_01", "apply"))
            .expect("queued");
        scheduler
            .enqueue(request(&agent, "job_02", "apply"))
            .expect("queued");
        assert_eq!(scheduler.queued(), 2);
        assert_eq!(scheduler.running(), 0);

        let taken = scheduler.next_job(0).expect("a job runs");
        assert_eq!(scheduler.queued(), 1, "the job left the queue");
        assert_eq!(scheduler.running(), 1, "the job runs");

        scheduler.complete(&taken.job_id, 0);
        assert_eq!(scheduler.running(), 0, "the job is done");
        assert_eq!(scheduler.queued(), 1, "the other job still waits");

        // The job that is done does not come back, and the other one does.
        let second = scheduler.next_job(0).expect("a job runs");
        assert_ne!(second.job_id, taken.job_id);
        assert_eq!(scheduler.running(), 1);
        scheduler.complete(&second.job_id, 0);
        assert_eq!(scheduler.queued(), 0);
        assert_eq!(scheduler.running(), 0);
        assert!(scheduler.next_job(0).is_none(), "there is nothing left");

        // The lease of the job that is done is given back: its holder may not write any more.
        assert!(scheduler.lease_of(&taken.job_id).is_none());
    }

    #[test]
    fn a_job_that_is_queued_again_may_run_again() {
        // The recovery of §17.6 puts the jobs that were not finished back into the queue: a job
        // that is queued again runs again, and it takes a bigger fencing token than the lease it
        // had before.
        let mut scheduler = FairScheduler::new(4);
        let agent = agent("agent_01");
        scheduler.set_agent_weight(&agent, 1).expect("the agent is registered");
        scheduler
            .enqueue(request(&agent, "job_01", "apply"))
            .expect("queued");
        let taken = scheduler.next_job(0).expect("a job runs");
        let first = scheduler
            .lease_of(&taken.job_id)
            .expect("the job has a lease")
            .clone();
        scheduler.complete(&taken.job_id, 0);

        scheduler
            .enqueue(request(&agent, "job_01", "apply"))
            .expect("queued");
        let again = scheduler.next_job(0).expect("the job runs again");
        let second = scheduler
            .lease_of(&again.job_id)
            .expect("the job has a lease")
            .clone();
        assert_eq!(again.job_id, taken.job_id);
        assert!(
            second.token > first.token,
            "the token of a new lease is always bigger"
        );
        assert_eq!(scheduler.running(), 1, "the job runs once");
    }

    #[test]
    fn a_kind_of_job_that_is_not_known_does_not_panic_the_scheduler() {
        let mut scheduler = FairScheduler::new(2);
        let agent = agent("agent_01");
        scheduler
            .enqueue(request(&agent, "job_01", "tase.code.something"))
            .expect("queued");
        let taken = scheduler.next_job(0).expect("a job runs");
        assert_eq!(taken.job_type, "tase.code.something");
        assert_eq!(scheduler.queued(), 0);
        assert_eq!(scheduler.running(), 1);
    }

    #[test]
    fn the_scheduler_takes_at_most_the_jobs_it_may_run() {
        let mut scheduler = FairScheduler::new(1);
        let agent = agent("agent_01");
        scheduler
            .enqueue(request(&agent, "job_01", "apply"))
            .expect("queued");
        scheduler
            .enqueue(request(&agent, "job_02", "apply"))
            .expect("queued");
        assert!(scheduler.next_job(0).is_some());
        assert!(scheduler.next_job(0).is_none(), "the hub runs one job at a time");
    }

    #[test]
    fn an_agent_that_is_not_weighed_yet_still_may_run() {
        let mut scheduler = FairScheduler::new(2);
        let agent = agent("agent_01");
        // The agent never registered itself with the scheduler: the job is not refused.
        scheduler
            .enqueue(request(&agent, "job_01", "apply"))
            .expect("queued");
        assert_eq!(scheduler.weight_of(&agent), Some(1));
        assert!(scheduler.next_job(0).is_some());
    }

    #[test]
    fn the_two_schedulers_of_the_hub_have_a_name() {
        assert_eq!(SchedulerKind::default(), SchedulerKind::Fair);
        assert_eq!(SchedulerKind::Fair.as_str(), "fair");
        assert_eq!(SchedulerKind::RoundRobin.as_str(), "round-robin");
        assert_eq!(SchedulerKind::from_name("fair"), Some(SchedulerKind::Fair));
        assert_eq!(
            SchedulerKind::from_name("round-robin"),
            Some(SchedulerKind::RoundRobin)
        );
        assert_eq!(
            SchedulerKind::from_name("round_robin"),
            Some(SchedulerKind::RoundRobin)
        );
        assert_eq!(SchedulerKind::from_name("nonsense"), None);
    }
}
