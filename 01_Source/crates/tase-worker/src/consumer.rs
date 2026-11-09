//! The consumer of the queue: it takes a task, it runs it, it tells the queue what happened and it waits before it
//! tries again (detailed design §5.2 and §6.2).
//!
//! # The three doors of the consumer
//!
//! The consumer is small on purpose and it is made of three traits:
//!
//! * [`TaskSource`] - the queue. It gives the task that may run now and it takes the report of the worker back.
//!   The source owns the lease of the task: the token it hands out is the fencing token of that lease.
//! * [`TaskExecutor`] - the part of the worker that runs the job (the runner and the verifier).
//! * [`RetryPolicy`] - how many times a task may be tried and how long the worker waits between two tries.
//!
//! # The rule of the retries
//!
//! A task that fails is tried again only when the failure may be repaired by trying again: the business codes of the
//! contract know which ones those are ([`tase_contract::ErrorCode::retryable`]). A validation that failed or a
//! conflict that was found is not repaired by running the same job again: the job ends with the code of the failure,
//! and a retry is a new job (detailed design §6.3).
//!
//! The wait between two tries grows: the first wait is the base of the policy, and every wait after it is the wait
//! before multiplied by the factor of the policy, until the biggest wait of the policy is reached. The wait is
//! computed from the number of the tries, never from the clock, so a test knows exactly when the next try happens.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use tase_contract::ids::JobId;
use tase_contract::{ErrorCode, TaseError};
use tase_local_store::{Job, JobStatus, Lease, Store};

use crate::lease::LeaseTable;

/// A task of the queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    /// The id of the task: the id of the job of the store.
    pub task_id: String,
    /// How many times the task was tried already.
    pub attempt: u32,
    /// The fencing token of the lease of the task.
    ///
    /// The token is the proof that the worker is still the holder of the task: a worker whose lease ran out carries
    /// an old token, and the queue refuses its report.
    pub fencing_token: u64,
    /// The key that makes the task unique.
    pub idempotency_key: String,
    /// The digest of the request of the task.
    pub request_digest: String,
}

/// What the executor of a task answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskResult {
    /// The task is done.
    Succeeded {
        /// The digest of the result of the task.
        result_digest: String,
    },
    /// The task failed.
    Failed {
        /// The business code of the failure.
        code: ErrorCode,
        /// The message of the failure, for the log.
        message: String,
    },
}

impl TaskResult {
    /// Whether the task is done.
    pub fn is_success(&self) -> bool {
        matches!(self, TaskResult::Succeeded { .. })
    }

    /// The business code of the result: `None` when the task is done.
    pub fn code(&self) -> Option<ErrorCode> {
        match self {
            TaskResult::Succeeded { .. } => None,
            TaskResult::Failed { code, .. } => Some(*code),
        }
    }
}

/// The executor of a task: the part of the worker that runs the job.
pub trait TaskExecutor {
    /// Runs the task and answers what happened.
    fn execute(&mut self, task: &Task, now_ms: u64) -> Result<TaskResult, TaseError>;
}

/// The report of a task: what the worker tells the queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskReport {
    /// The id of the task.
    pub task_id: String,
    /// How many times the task was tried.
    pub attempt: u32,
    /// The fencing token of the lease of the task.
    pub fencing_token: u64,
    /// What happened.
    pub result: TaskResult,
    /// When the task may be tried again; `None` when it may not.
    pub retry_at_ms: Option<u64>,
}

/// The queue of the tasks.
pub trait TaskSource {
    /// The next task that may run now, or `None` when there is none.
    fn next_task(&mut self, now_ms: u64) -> Option<Task>;

    /// The worker tells the queue what happened.
    ///
    /// The report carries the fencing token of the lease of the task: a queue that hands the task to somebody else in
    /// the meantime refuses the report, because the worker that reports is old.
    fn report(&mut self, report: &TaskReport, now_ms: u64) -> Result<(), TaseError>;
}

impl TaskSource for Box<dyn TaskSource> {
    fn next_task(&mut self, now_ms: u64) -> Option<Task> {
        (**self).next_task(now_ms)
    }

    fn report(&mut self, report: &TaskReport, now_ms: u64) -> Result<(), TaseError> {
        (**self).report(report, now_ms)
    }
}

impl TaskExecutor for Box<dyn TaskExecutor> {
    fn execute(&mut self, task: &Task, now_ms: u64) -> Result<TaskResult, TaseError> {
        (**self).execute(task, now_ms)
    }
}

/// The rule of the retries of a task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    /// How many times a task may be tried.
    pub max_attempts: u32,
    /// The wait of the first retry.
    pub base_delay_ms: u64,
    /// The biggest wait of a retry.
    pub max_delay_ms: u64,
    /// The factor of the wait between two retries.
    pub multiplier: u32,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy {
            max_attempts: 3,
            base_delay_ms: 1_000,
            max_delay_ms: 60_000,
            multiplier: 2,
        }
    }
}

impl RetryPolicy {
    /// The wait before the next try of a task that was tried `attempt` times.
    ///
    /// The wait of the first retry is the base of the policy; every wait after it is the wait before multiplied by the
    /// factor of the policy, and it never grows past the biggest wait of the policy.
    pub fn delay_for(&self, attempt: u32) -> u64 {
        let mut delay = self.base_delay_ms;
        let factor = u64::from(self.multiplier.max(1));
        for _ in 1..attempt {
            delay = delay.saturating_mul(factor);
            if delay >= self.max_delay_ms {
                return self.max_delay_ms;
            }
        }
        delay.min(self.max_delay_ms)
    }

    /// Whether the task may be tried again after `attempt` tries.
    pub fn may_retry(&self, attempt: u32) -> bool {
        attempt < self.max_attempts
    }
}

/// What the consumer did with a task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConsumeOutcome {
    /// There was no task to run.
    Idle,
    /// The task is done.
    Done {
        /// The id of the task.
        task_id: String,
    },
    /// The task failed and will be tried again.
    Retry {
        /// The id of the task.
        task_id: String,
        /// How many times the task was tried.
        attempt: u32,
        /// When the task may be tried again.
        retry_at_ms: u64,
    },
    /// The task failed for good.
    Failed {
        /// The id of the task.
        task_id: String,
        /// How many times the task was tried.
        attempt: u32,
        /// The business code of the failure.
        code: ErrorCode,
    },
    /// The queue refused the result: the lease of the task was taken back while the task ran.
    Rejected {
        /// The id of the task.
        task_id: String,
        /// Why the queue refused the result.
        reason: String,
    },
}

impl ConsumeOutcome {
    /// Whether the consumer ran a task.
    pub fn is_busy(&self) -> bool {
        !matches!(self, ConsumeOutcome::Idle)
    }
}

/// The consumer of the queue.
pub struct Consumer<S, E> {
    source: S,
    executor: E,
    policy: RetryPolicy,
}

impl<S: TaskSource, E: TaskExecutor> Consumer<S, E> {
    /// A consumer with the rule of the retries.
    pub fn new(source: S, executor: E, policy: RetryPolicy) -> Consumer<S, E> {
        Consumer {
            source,
            executor,
            policy,
        }
    }

    /// The rule of the retries of the consumer.
    pub fn policy(&self) -> &RetryPolicy {
        &self.policy
    }

    /// The queue of the consumer.
    pub fn source(&self) -> &S {
        &self.source
    }

    /// The queue of the consumer, to change it.
    pub fn source_mut(&mut self) -> &mut S {
        &mut self.source
    }

    /// Takes one task, runs it and tells the queue what happened.
    ///
    /// The consumer never waits: it answers `Idle` when there is nothing to do, and the caller decides how long it
    /// waits before it asks again.
    pub fn take_one(&mut self, now_ms: u64) -> Result<ConsumeOutcome, TaseError> {
        let Some(task) = self.source.next_task(now_ms) else {
            return Ok(ConsumeOutcome::Idle);
        };

        let result = match self.executor.execute(&task, now_ms) {
            Ok(result) => result,
            Err(error) => TaskResult::Failed {
                code: error.code,
                message: error.message,
            },
        };

        let attempt = task.attempt;
        let retry_at_ms = match &result {
            TaskResult::Succeeded { .. } => None,
            TaskResult::Failed { code, .. } => {
                if code.retryable() && self.policy.may_retry(attempt) {
                    Some(now_ms.saturating_add(self.policy.delay_for(attempt)))
                } else {
                    None
                }
            }
        };

        let report = TaskReport {
            task_id: task.task_id.clone(),
            attempt,
            fencing_token: task.fencing_token,
            result: result.clone(),
            retry_at_ms,
        };

        if let Err(error) = self.source.report(&report, now_ms) {
            // the queue refused the result: the worker that reports is old, and its write is not accepted
            return Ok(ConsumeOutcome::Rejected {
                task_id: task.task_id,
                reason: error.message,
            });
        }

        Ok(match (&result, retry_at_ms) {
            (TaskResult::Succeeded { .. }, _) => ConsumeOutcome::Done {
                task_id: task.task_id,
            },
            (_, Some(retry_at_ms)) => ConsumeOutcome::Retry {
                task_id: task.task_id,
                attempt,
                retry_at_ms,
            },
            (TaskResult::Failed { code, .. }, None) => ConsumeOutcome::Failed {
                task_id: task.task_id,
                attempt,
                code: *code,
            },
        })
    }
}

/// A task that waits in the queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedTask {
    /// The id of the task.
    pub task_id: String,
    /// The key that makes the task unique.
    pub idempotency_key: String,
    /// The digest of the request of the task.
    pub request_digest: String,
    /// The moment the task may be taken.
    pub ready_at_ms: u64,
    /// How many times the task was tried.
    pub attempt: u32,
}

impl QueuedTask {
    /// A task that may be taken now.
    pub fn new(task_id: &str, idempotency_key: &str, request_digest: &str) -> QueuedTask {
        QueuedTask {
            task_id: task_id.to_string(),
            idempotency_key: idempotency_key.to_string(),
            request_digest: request_digest.to_string(),
            ready_at_ms: 0,
            attempt: 0,
        }
    }
}

/// The tasks that wait and the leases of the tasks.
///
/// The queue is shared by the workers: the leases live in it, so that two workers never take the same task.
#[derive(Clone, Debug, Default)]
pub struct TaskQueue {
    queue: VecDeque<QueuedTask>,
    leases: LeaseTable,
}

impl TaskQueue {
    /// A queue without a task.
    pub fn new() -> TaskQueue {
        TaskQueue {
            queue: VecDeque::new(),
            leases: LeaseTable::new(),
        }
    }

    /// Puts a task in the queue.
    pub fn push(&mut self, task: QueuedTask) {
        self.queue.push_back(task);
    }

    /// How many tasks wait.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Whether no task waits.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// The leases of the tasks of the queue.
    pub fn leases(&self) -> &LeaseTable {
        &self.leases
    }
}

/// The queue of the worker when the jobs do not live in the store.
///
/// The queue and its leases live behind a lock, so that several workers may share them: this is the queue of the
/// simulation of the concurrent workers of detailed design §14.
#[derive(Clone, Debug)]
pub struct MemoryTaskSource {
    queue: Arc<Mutex<TaskQueue>>,
    holder: String,
    ttl_ms: u64,
    held: Option<QueuedTask>,
    reports: Vec<TaskReport>,
}

impl MemoryTaskSource {
    /// A worker of a queue that is not shared with anybody.
    pub fn new(holder: &str, ttl_ms: u64) -> MemoryTaskSource {
        MemoryTaskSource::with_queue(Arc::new(Mutex::new(TaskQueue::new())), holder, ttl_ms)
    }

    /// A worker of a queue that is shared with the other workers.
    pub fn with_queue(queue: Arc<Mutex<TaskQueue>>, holder: &str, ttl_ms: u64) -> MemoryTaskSource {
        MemoryTaskSource {
            queue,
            holder: holder.to_string(),
            ttl_ms,
            held: None,
            reports: Vec::new(),
        }
    }

    /// The queue of the worker.
    pub fn queue(&self) -> Arc<Mutex<TaskQueue>> {
        Arc::clone(&self.queue)
    }

    /// The reports the worker gave back.
    pub fn reports(&self) -> &[TaskReport] {
        &self.reports
    }

    /// The lock of the queue.
    fn lock(&self) -> MutexGuard<'_, TaskQueue> {
        match self.queue.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl TaskSource for MemoryTaskSource {
    fn next_task(&mut self, now_ms: u64) -> Option<Task> {
        // a worker runs one task at a time
        if self.held.is_some() {
            return None;
        }
        // the lock of the queue ends with the block: the worker owns the task it took
        let taken = {
            let mut queue = self.lock();
            let position = queue
                .queue
                .iter()
                .position(|task| task.ready_at_ms <= now_ms)?;
            let mut task = queue
                .queue
                .remove(position)
                .expect("the position was found in the queue");
            let lease = match queue
                .leases
                .acquire(&task.task_id, &self.holder, self.ttl_ms, now_ms)
            {
                Ok(lease) => lease,
                Err(_) => {
                    // somebody else holds the task: it goes back into the queue and the worker waits
                    queue.queue.push_front(task);
                    return None;
                }
            };
            task.attempt = task.attempt.saturating_add(1);
            Some((task, lease))
        };

        let (task, lease) = taken?;
        self.held = Some(task.clone());
        Some(Task {
            task_id: task.task_id,
            attempt: task.attempt,
            fencing_token: lease.fencing_token,
            idempotency_key: task.idempotency_key,
            request_digest: task.request_digest,
        })
    }

    fn report(&mut self, report: &TaskReport, now_ms: u64) -> Result<(), TaseError> {
        let held = self.held.take();
        // the task that goes back into the queue when the try failed
        let mut again: Option<QueuedTask> = None;
        if let Some(held) = &held
            && held.task_id == report.task_id
            && let Some(retry_at_ms) = report.retry_at_ms
        {
            let mut task = held.clone();
            task.ready_at_ms = retry_at_ms;
            again = Some(task);
        }

        {
            let mut queue = self.lock();
            // the fencing: a worker whose lease was taken back may not write the result of the task
            let checked = queue
                .leases
                .check(&report.task_id, report.fencing_token, now_ms);
            match checked {
                Ok(()) => {
                    let _released =
                        queue
                            .leases
                            .release(&report.task_id, &self.holder, report.fencing_token);
                    if let Some(task) = again {
                        queue.queue.push_back(task);
                    }
                }
                Err(error) => {
                    // the worker is old: the task goes back into the queue, and the lease of the holder that took it
                    // keeps it from running twice
                    if let Some(held) = held {
                        queue.queue.push_back(held);
                    }
                    return Err(error.to_error());
                }
            }
        }
        self.reports.push(report.clone());
        Ok(())
    }
}

/// The queue of the worker when the jobs live in the store of the local hub.
///
/// The claim of a job is the transaction of the store: the job is only given to one holder, the number of the tries
/// grows, the epoch of the job grows and the fencing token of the lease grows (detailed design §6.2).
pub struct StoreTaskSource {
    store: Store,
    holder: String,
    ttl_ms: u64,
    policy: RetryPolicy,
    held: Option<(JobId, Lease)>,
}

impl StoreTaskSource {
    /// A worker of the jobs of the store.
    pub fn new(store: Store, holder: &str, ttl_ms: u64, policy: RetryPolicy) -> StoreTaskSource {
        StoreTaskSource {
            store,
            holder: holder.to_string(),
            ttl_ms,
            policy,
            held: None,
        }
    }

    /// The store of the queue.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The store of the queue, to change it.
    pub fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }

    /// Whether the job may be taken now.
    ///
    /// A job that was never tried is ready at once; a job that failed waits for the window of the backoff of the
    /// policy, counted from the moment it went back into the queue.
    fn is_ready(&self, job: &Job, now_ms: u64) -> bool {
        if job.attempt == 0 {
            return true;
        }
        let ready_at_ms = job
            .updated_at_ms
            .saturating_add(self.policy.delay_for(job.attempt));
        ready_at_ms <= now_ms
    }

    /// The refusal of a worker that is not the holder of the job any more.
    fn stale_job(&self, task_id: &str) -> TaseError {
        TaseError::permission_denied(format!(
            "the job {task_id} is not the one of this worker any more"
        ))
    }
}

impl TaskSource for StoreTaskSource {
    fn next_task(&mut self, now_ms: u64) -> Option<Task> {
        let mut jobs = self.store.list_jobs_by_status(JobStatus::Queued).ok()?;
        jobs.sort_by(|left, right| {
            left.created_at_ms
                .cmp(&right.created_at_ms)
                .then_with(|| left.job_id.as_str().cmp(right.job_id.as_str()))
        });
        let job = jobs.into_iter().find(|job| self.is_ready(job, now_ms))?;
        let lease = self
            .store
            .lease_job(&job.job_id, &self.holder, self.ttl_ms, now_ms)
            .ok()?;
        let job = self.store.get_job(&job.job_id).ok()??;

        self.held = Some((job.job_id.clone(), lease.clone()));
        Some(Task {
            task_id: job.job_id.as_str().to_string(),
            attempt: job.attempt,
            fencing_token: lease.fencing_token,
            idempotency_key: job.idempotency_key.clone(),
            request_digest: job.request_digest.clone(),
        })
    }

    fn report(&mut self, report: &TaskReport, now_ms: u64) -> Result<(), TaseError> {
        let job_id = JobId::new(report.task_id.clone())?;
        let job = self
            .store
            .get_job(&job_id)?
            .ok_or_else(|| TaseError::not_found(format!("there is no job {}", report.task_id)))?;

        // The epoch of the job is the fencing of the store: a worker whose lease ran out may not move the job. The
        // move to a state that is not final is the check of the epoch; the final state is written by `finish_job`,
        // which refuses a job that is already final.
        match &report.result {
            TaskResult::Succeeded { result_digest } => {
                let moved = self.store.transition_job(
                    &job_id,
                    JobStatus::Running,
                    JobStatus::Verifying,
                    job.lease_epoch,
                    now_ms,
                )?;
                if !moved {
                    return Err(self.stale_job(&report.task_id));
                }
                self.store.finish_job(
                    &job_id,
                    JobStatus::Committed,
                    Some(result_digest.clone()),
                    None,
                    now_ms,
                )?;
            }
            TaskResult::Failed { code, .. } if report.retry_at_ms.is_some() => {
                // the job goes back into the queue and waits for the window of the backoff
                let moved = self.store.transition_job(
                    &job_id,
                    JobStatus::Running,
                    JobStatus::Queued,
                    job.lease_epoch,
                    now_ms,
                )?;
                if !moved {
                    return Err(self.stale_job(&report.task_id));
                }
            }
            TaskResult::Failed { code, .. } => {
                let moved = self.store.transition_job(
                    &job_id,
                    JobStatus::Running,
                    JobStatus::Verifying,
                    job.lease_epoch,
                    now_ms,
                )?;
                if !moved {
                    return Err(self.stale_job(&report.task_id));
                }
                self.store
                    .finish_job(&job_id, JobStatus::Failed, None, Some(*code), now_ms)?;
            }
        }

        // the lease of the job is given back: another worker may take the job when it is ready
        if let Some((_, lease)) = self.held.take() {
            let _released = self.store.release_lease(&lease.lease_id, &self.holder);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lease::LeaseError;

    /// An executor that answers what the test tells it to answer.
    struct ScriptedExecutor {
        answers: Vec<Result<TaskResult, TaseError>>,
        calls: Vec<String>,
    }

    impl ScriptedExecutor {
        fn new(answers: Vec<Result<TaskResult, TaseError>>) -> ScriptedExecutor {
            ScriptedExecutor {
                answers,
                calls: Vec::new(),
            }
        }
    }

    impl TaskExecutor for ScriptedExecutor {
        fn execute(&mut self, task: &Task, _now_ms: u64) -> Result<TaskResult, TaseError> {
            self.calls.push(task.task_id.clone());
            if self.answers.len() == 1 {
                return self.answers[0].clone();
            }
            if self.answers.is_empty() {
                return Ok(TaskResult::Succeeded {
                    result_digest: "sha256:done".to_string(),
                });
            }
            self.answers.remove(0)
        }
    }

    /// An executor that takes the lease of the task back while the task runs.
    struct ThiefExecutor {
        queue: Arc<Mutex<TaskQueue>>,
    }

    impl TaskExecutor for ThiefExecutor {
        fn execute(&mut self, task: &Task, _now_ms: u64) -> Result<TaskResult, TaseError> {
            let mut guard = self
                .queue
                .lock()
                .expect("the lock of the queue is not poisoned");
            let _released = guard
                .leases
                .release(&task.task_id, "worker-1", task.fencing_token);
            let _stolen = guard
                .leases
                .acquire(&task.task_id, "worker-2", 30_000, 1_000)
                .expect("the lease is taken back");
            Ok(TaskResult::Succeeded {
                result_digest: "sha256:done".to_string(),
            })
        }
    }

    fn retryable() -> Result<TaskResult, TaseError> {
        Ok(TaskResult::Failed {
            code: ErrorCode::WorkerUnavailable,
            message: "the host is not there".to_string(),
        })
    }

    fn business_failure() -> Result<TaskResult, TaseError> {
        Ok(TaskResult::Failed {
            code: ErrorCode::ValidationFailed,
            message: "the test did not pass".to_string(),
        })
    }

    fn done() -> Result<TaskResult, TaseError> {
        Ok(TaskResult::Succeeded {
            result_digest: "sha256:done".to_string(),
        })
    }

    fn policy() -> RetryPolicy {
        RetryPolicy {
            max_attempts: 3,
            base_delay_ms: 1_000,
            max_delay_ms: 8_000,
            multiplier: 2,
        }
    }

    /// A directory of the disk that only this test uses.
    fn temp_dir(name: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "tase-worker-{name}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("the directory of the test is made");
        path
    }

    fn queue_with(tasks: usize) -> Arc<Mutex<TaskQueue>> {
        let queue = Arc::new(Mutex::new(TaskQueue::new()));
        {
            let mut guard = queue.lock().expect("the lock of the queue is not poisoned");
            for index in 0..tasks {
                guard.push(QueuedTask::new(
                    &format!("job_{index:012}"),
                    &format!("key-{index}"),
                    &format!("sha256:request-{index}"),
                ));
            }
        }
        queue
    }

    #[test]
    fn the_wait_of_the_retries_grows_and_is_capped() {
        let policy = policy();
        assert_eq!(policy.delay_for(1), 1_000);
        assert_eq!(policy.delay_for(2), 2_000);
        assert_eq!(policy.delay_for(3), 4_000);
        assert_eq!(policy.delay_for(4), 8_000);
        assert_eq!(policy.delay_for(9), 8_000);
    }

    #[test]
    fn a_task_that_succeeds_is_reported_as_done() {
        let queue = queue_with(1);
        let source = MemoryTaskSource::with_queue(Arc::clone(&queue), "worker-1", 30_000);
        let mut consumer = Consumer::new(source, ScriptedExecutor::new(vec![done()]), policy());

        let outcome = consumer.take_one(1_000).expect("the task runs");
        assert_eq!(
            outcome,
            ConsumeOutcome::Done {
                task_id: "job_000000000000".to_string()
            }
        );
        let reports = consumer.source().reports();
        assert_eq!(reports.len(), 1);
        assert!(reports[0].result.is_success());
        assert_eq!(reports[0].retry_at_ms, None);
    }

    #[test]
    fn an_empty_queue_answers_that_there_is_nothing_to_do() {
        let source = MemoryTaskSource::new("worker-1", 30_000);
        let mut consumer = Consumer::new(source, ScriptedExecutor::new(vec![]), policy());
        assert_eq!(
            consumer.take_one(1_000).expect("the consumer answers"),
            ConsumeOutcome::Idle
        );
    }

    #[test]
    fn a_task_that_fails_is_tried_again_after_the_backoff() {
        let queue = queue_with(1);
        let source = MemoryTaskSource::with_queue(Arc::clone(&queue), "worker-1", 30_000);
        let mut consumer = Consumer::new(
            source,
            ScriptedExecutor::new(vec![retryable(), retryable()]),
            policy(),
        );

        let outcome = consumer.take_one(1_000).expect("the task runs");
        assert_eq!(
            outcome,
            ConsumeOutcome::Retry {
                task_id: "job_000000000000".to_string(),
                attempt: 1,
                retry_at_ms: 2_000,
            }
        );

        // the task is not taken again before the window of the backoff is over
        let outcome = consumer.take_one(1_500).expect("the consumer answers");
        assert_eq!(outcome, ConsumeOutcome::Idle);

        // and it is taken again when the window is over
        let outcome = consumer.take_one(2_000).expect("the task runs");
        assert_eq!(
            outcome,
            ConsumeOutcome::Retry {
                task_id: "job_000000000000".to_string(),
                attempt: 2,
                retry_at_ms: 4_000,
            }
        );
    }

    #[test]
    fn a_task_that_failed_too_many_times_is_not_tried_again() {
        let queue = queue_with(1);
        let source = MemoryTaskSource::with_queue(Arc::clone(&queue), "worker-1", 30_000);
        let mut consumer = Consumer::new(
            source,
            ScriptedExecutor::new(vec![retryable(), retryable(), retryable()]),
            policy(),
        );

        assert!(matches!(
            consumer.take_one(1_000).expect("the task runs"),
            ConsumeOutcome::Retry { attempt: 1, .. }
        ));
        assert!(matches!(
            consumer.take_one(2_000).expect("the task runs"),
            ConsumeOutcome::Retry { attempt: 2, .. }
        ));
        assert_eq!(
            consumer.take_one(4_000).expect("the task runs"),
            ConsumeOutcome::Failed {
                task_id: "job_000000000000".to_string(),
                attempt: 3,
                code: ErrorCode::WorkerUnavailable,
            }
        );
    }

    #[test]
    fn a_failure_that_may_not_be_repaired_is_not_tried_again() {
        let queue = queue_with(1);
        let source = MemoryTaskSource::with_queue(Arc::clone(&queue), "worker-1", 30_000);
        let mut consumer = Consumer::new(
            source,
            ScriptedExecutor::new(vec![business_failure()]),
            policy(),
        );

        assert_eq!(
            consumer.take_one(1_000).expect("the task runs"),
            ConsumeOutcome::Failed {
                task_id: "job_000000000000".to_string(),
                attempt: 1,
                code: ErrorCode::ValidationFailed,
            }
        );
    }

    #[test]
    fn the_result_of_a_task_whose_lease_was_taken_back_is_refused() {
        let queue = queue_with(1);
        let source = MemoryTaskSource::with_queue(Arc::clone(&queue), "worker-1", 30_000);
        let mut consumer = Consumer::new(source, ScriptedExecutor::new(vec![done()]), policy());

        // the worker takes the task
        let task = consumer
            .source_mut()
            .next_task(1_000)
            .expect("the task is taken");
        assert_eq!(task.attempt, 1);

        // the lease of the task runs out and another worker takes it back
        {
            let mut guard = queue.lock().expect("the lock of the queue is not poisoned");
            let _expired = guard.leases.reclaim_expired(31_001);
            let _second = guard
                .leases
                .acquire(&task.task_id, "worker-2", 30_000, 31_001)
                .expect("the lease is taken back");
        }

        // the worker that comes back late may not write the result of the task
        let report = TaskReport {
            task_id: task.task_id.clone(),
            attempt: task.attempt,
            fencing_token: task.fencing_token,
            result: TaskResult::Succeeded {
                result_digest: "sha256:done".to_string(),
            },
            retry_at_ms: None,
        };
        let error = consumer
            .source_mut()
            .report(&report, 31_002)
            .expect_err("the report of an old worker is refused");
        assert_eq!(error.code, ErrorCode::PermissionDenied);
    }

    #[test]
    fn the_consumer_refuses_to_report_the_result_of_an_old_worker() {
        let queue = queue_with(1);
        let source = MemoryTaskSource::with_queue(Arc::clone(&queue), "worker-1", 30_000);
        // while the task runs, another worker takes the lease back
        let executor = ThiefExecutor {
            queue: Arc::clone(&queue),
        };
        let mut consumer = Consumer::new(source, executor, policy());

        let outcome = consumer.take_one(1_000).expect("the task runs");
        assert_eq!(
            outcome,
            ConsumeOutcome::Rejected {
                task_id: "job_000000000000".to_string(),
                reason: "the token 1 of job_000000000000 is old; the current token is 2"
                    .to_string(),
            }
        );
    }

    #[test]
    fn thirty_two_workers_never_take_the_same_task() {
        let queue = queue_with(32);
        let mut workers: Vec<MemoryTaskSource> = (0..32)
            .map(|index| {
                MemoryTaskSource::with_queue(Arc::clone(&queue), &format!("worker-{index}"), 30_000)
            })
            .collect();

        let mut taken: Vec<String> = Vec::new();
        for worker in &mut workers {
            let task = worker.next_task(1_000).expect("the task is taken");
            taken.push(task.task_id);
        }

        taken.sort();
        taken.dedup();
        assert_eq!(taken.len(), 32, "every worker took a task of its own");

        // the queue is empty now: the tasks that are held are not given a second time
        for worker in &mut workers {
            assert!(
                worker.next_task(1_000).is_none(),
                "a task that is held is not given again"
            );
        }
    }

    #[test]
    fn the_lease_of_a_task_grows_with_every_take() {
        let queue = queue_with(1);
        let mut source = MemoryTaskSource::with_queue(Arc::clone(&queue), "worker-1", 30_000);
        let first = source.next_task(1_000).expect("the task is taken");
        let report = TaskReport {
            task_id: first.task_id.clone(),
            attempt: first.attempt,
            fencing_token: first.fencing_token,
            result: TaskResult::Failed {
                code: ErrorCode::WorkerUnavailable,
                message: "the host is not there".to_string(),
            },
            retry_at_ms: Some(2_000),
        };
        source
            .report(&report, 1_000)
            .expect("the report is accepted");

        let second = source.next_task(2_000).expect("the task is taken again");
        assert_eq!(second.attempt, 2);
        assert!(
            second.fencing_token > first.fencing_token,
            "the token of the second take is greater than the token of the first one"
        );
    }

    #[test]
    fn the_queue_of_the_memory_keeps_the_leases_of_the_tasks() {
        let queue = queue_with(1);
        let mut source = MemoryTaskSource::with_queue(Arc::clone(&queue), "worker-1", 30_000);
        let task = source.next_task(1_000).expect("the task is taken");
        let guard = queue.lock().expect("the lock of the queue is not poisoned");
        let lease = guard
            .leases
            .lease_of(&task.task_id)
            .expect("the lease is there");
        assert_eq!(lease.fencing_token, task.fencing_token);
        assert_eq!(lease.holder, "worker-1");
    }

    #[test]
    fn the_refusal_of_a_stale_token_carries_the_two_tokens() {
        let mut leases = LeaseTable::new();
        let first = leases
            .acquire("job_1", "worker-1", 1_000, 0)
            .expect("the lease is taken");
        let _released = leases
            .release("job_1", "worker-1", first.fencing_token)
            .expect("the lease is given back");
        let second = leases
            .acquire("job_1", "worker-2", 1_000, 2_000)
            .expect("the lease is taken again");
        let refused = leases
            .check("job_1", first.fencing_token, 2_500)
            .expect_err("the write is refused");
        assert!(matches!(
            refused,
            LeaseError::StaleToken { presented, current, .. }
                if presented == first.fencing_token && current == second.fencing_token
        ));
    }

    #[test]
    fn the_queue_of_the_store_gives_the_job_to_the_worker_and_waits_for_the_backoff() {
        let dir = temp_dir("store-source");
        let mut store = Store::open(dir.join("worker.sqlite3")).expect("the store opens");
        tase_local_store::migrations::apply(&mut store).expect("the steps run");

        let session_id =
            tase_contract::ids::SessionId::new("session-1").expect("the session id is valid");
        store
            .open_session(tase_local_store::NewSession {
                session_id: session_id.clone(),
                agent_id: tase_contract::ids::AgentId::new("agent-1")
                    .expect("the agent id is valid"),
                provider: "test".to_string(),
                instance_id: "instance-1".to_string(),
                workspace_id: tase_contract::ids::WorkspaceId::new("workspace-1")
                    .expect("the workspace id is valid"),
                owner_logon_sid_hash: "sha256:logon".to_string(),
                granted_scopes: vec!["tase.code.apply".to_string()],
                capabilities: Vec::new(),
                now_ms: 1_000,
            })
            .expect("the session is registered");
        let submission = store
            .submit_job(tase_local_store::NewJob {
                job_type: "tase.code.apply".to_string(),
                workspace_id: tase_contract::ids::WorkspaceId::new("workspace-1")
                    .expect("the workspace id is valid"),
                agent_id: tase_contract::ids::AgentId::new("agent-1")
                    .expect("the agent id is valid"),
                session_id,
                idempotency_key: "key-1".to_string(),
                request_digest: "sha256:request".to_string(),
                base_snapshot_digest: "sha256:base".to_string(),
                priority: 0,
                now_ms: 1_000,
            })
            .expect("the job is registered");

        let mut source = StoreTaskSource::new(store, "worker-1", 30_000, policy());
        let task = source.next_task(1_000).expect("the job is taken");
        assert_eq!(task.task_id, submission.job_id.as_str());
        assert_eq!(task.attempt, 1);
        assert_eq!(task.idempotency_key, "key-1");

        // the job failed and waits for the window of the backoff
        let report = TaskReport {
            task_id: task.task_id.clone(),
            attempt: task.attempt,
            fencing_token: task.fencing_token,
            result: TaskResult::Failed {
                code: ErrorCode::WorkerUnavailable,
                message: "the host is not there".to_string(),
            },
            retry_at_ms: Some(2_000),
        };
        source
            .report(&report, 1_000)
            .expect("the report is accepted");

        assert!(
            source.next_task(1_500).is_none(),
            "the job waits for the window of the backoff"
        );
        let again = source.next_task(2_000).expect("the job is taken again");
        assert_eq!(again.attempt, 2);
        assert!(
            again.fencing_token > task.fencing_token,
            "the token of the second take is greater than the token of the first one"
        );

        drop(source);
        std::fs::remove_dir_all(&dir).expect("the directory of the test is removed");
    }
}
