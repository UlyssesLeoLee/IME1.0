//! The skeleton of the resident worker: the configuration, the loop of the jobs and the stop in an orderly way
//! (detailed design §14, S3, and §17.2).
//!
//! # What a resident worker is
//!
//! A worker of TASE is a process that stays alive: it takes a job out of the queue, it runs it, it tells the queue
//! what happened and it waits for the next job. This module is the skeleton of that process:
//!
//! * [`WorkerConfig`] - the configuration of the worker: the deadline of the lease of a job, the heartbeat, the wait
//!   between two tries and the number of the tries;
//! * [`StopSignal`] - the door of the stop: the worker is asked to stop and it stops **between two jobs**, never in
//!   the middle of one;
//! * [`Worker`] - the loop: it takes one job at a time, it never waits inside the core of the worker (the wait is
//!   the business of the caller, which is the only place that may sleep), and it answers what it did when it stops.
//!
//! # The rules of the configuration
//!
//! The heartbeat has to be shorter than the lease: a worker that is alive keeps its lease, and a worker that is not
//! heard from loses it (detailed design §12: "设 `heartbeat_seconds < lease_seconds`").
//!
//! # The time
//!
//! The loop asks the clock of the caller for the time; the core of the worker never reads the wall clock. The binary
//! of the worker uses the clock of the machine and sleeps between two jobs; the tests use the logical clock and never
//! wait.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tase_contract::TaseError;

use crate::consumer::{ConsumeOutcome, Consumer, RetryPolicy, TaskExecutor, TaskSource};
use crate::lease::Clock;

/// The configuration of a worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerConfig {
    /// The id of the worker: the holder of the leases of its jobs.
    pub worker_id: String,
    /// How long the lease of a job is good for.
    pub lease_ttl_ms: u64,
    /// How often the worker tells the queue that it is alive.
    pub heartbeat_ms: u64,
    /// How long the worker waits when there is nothing to do.
    pub poll_interval_ms: u64,
    /// How many times a job may be tried.
    pub max_attempts: u32,
    /// The wait of the first retry of a job.
    pub backoff_base_ms: u64,
    /// The biggest wait of a retry of a job.
    pub backoff_max_ms: u64,
    /// The factor of the wait between two retries of a job.
    pub backoff_multiplier: u32,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        WorkerConfig {
            worker_id: "tase-worker-1".to_string(),
            lease_ttl_ms: 30_000,
            heartbeat_ms: 10_000,
            poll_interval_ms: 500,
            max_attempts: 3,
            backoff_base_ms: 1_000,
            backoff_max_ms: 60_000,
            backoff_multiplier: 2,
        }
    }
}

impl WorkerConfig {
    /// Checks the configuration.
    ///
    /// The heartbeat has to be shorter than the lease, a job has to be tried at least once and the biggest wait of a
    /// retry may not be shorter than the first one.
    pub fn validate(&self) -> Result<(), TaseError> {
        if self.heartbeat_ms >= self.lease_ttl_ms {
            return Err(TaseError::invalid_argument(format!(
                "the heartbeat of {} ms has to be shorter than the lease of {} ms",
                self.heartbeat_ms, self.lease_ttl_ms
            )));
        }
        if self.max_attempts == 0 {
            return Err(TaseError::invalid_argument(
                "a job has to be tried at least once",
            ));
        }
        if self.backoff_max_ms < self.backoff_base_ms {
            return Err(TaseError::invalid_argument(
                "the biggest wait of a retry may not be shorter than the first one",
            ));
        }
        Ok(())
    }

    /// The rule of the retries of the worker.
    pub fn retry_policy(&self) -> RetryPolicy {
        RetryPolicy {
            max_attempts: self.max_attempts,
            base_delay_ms: self.backoff_base_ms,
            max_delay_ms: self.backoff_max_ms,
            multiplier: self.backoff_multiplier,
        }
    }
}

/// The door of the stop of a worker.
///
/// The signal is shared: the process that supervises the worker and the worker itself hold the same one.
#[derive(Clone, Debug, Default)]
pub struct StopSignal {
    flag: Arc<AtomicBool>,
}

impl StopSignal {
    /// A signal that asks for nothing.
    pub fn new() -> StopSignal {
        StopSignal {
            flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Asks the worker to stop.
    pub fn stop(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    /// Whether the worker was asked to stop.
    pub fn is_stopped(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

/// What the worker did when it stopped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Shutdown {
    /// How many jobs the worker ran.
    pub processed: u32,
    /// Whether the worker stopped because it was asked to.
    pub stopped: bool,
}

/// The worker: the loop that takes the jobs of the queue and runs them.
pub struct Worker<S, E> {
    config: WorkerConfig,
    consumer: Consumer<S, E>,
    stop: StopSignal,
}

impl<S: TaskSource, E: TaskExecutor> Worker<S, E> {
    /// A worker with its configuration, its queue and the door of its stop.
    pub fn new(config: WorkerConfig, consumer: Consumer<S, E>, stop: StopSignal) -> Worker<S, E> {
        Worker {
            config,
            consumer,
            stop,
        }
    }

    /// The configuration of the worker.
    pub fn config(&self) -> &WorkerConfig {
        &self.config
    }

    /// The door of the stop of the worker.
    pub fn stop_signal(&self) -> &StopSignal {
        &self.stop
    }

    /// Runs one job of the queue.
    pub fn step(&mut self, now_ms: u64) -> Result<ConsumeOutcome, TaseError> {
        self.consumer.take_one(now_ms)
    }

    /// Runs the jobs until the queue is empty.
    ///
    /// The worker stops between two jobs: a job that runs is never interrupted.
    pub fn drain(&mut self, clock: &dyn Clock) -> Result<Shutdown, TaseError> {
        let mut processed = 0u32;
        loop {
            if self.stop.is_stopped() {
                return Ok(Shutdown {
                    processed,
                    stopped: true,
                });
            }
            let outcome = self.step(clock.now_ms())?;
            if !outcome.is_busy() {
                return Ok(Shutdown {
                    processed,
                    stopped: false,
                });
            }
            processed = processed.saturating_add(1);
        }
    }

    /// Runs the jobs until the worker is asked to stop.
    ///
    /// The worker waits **between** two jobs, never inside one: the wait is the business of `on_idle`, which is the
    /// only place of the worker that may sleep.
    pub fn run(
        &mut self,
        clock: &dyn Clock,
        mut on_idle: impl FnMut(u64),
    ) -> Result<Shutdown, TaseError> {
        let mut processed = 0u32;
        loop {
            if self.stop.is_stopped() {
                return Ok(Shutdown {
                    processed,
                    stopped: true,
                });
            }
            let now_ms = clock.now_ms();
            let outcome = self.step(now_ms)?;
            if outcome.is_busy() {
                processed = processed.saturating_add(1);
            } else {
                on_idle(now_ms);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tase_contract::{ErrorCode, TaseError};

    use super::*;
    use crate::consumer::{MemoryTaskSource, QueuedTask, Task, TaskResult};
    use crate::lease::LogicalClock;

    /// An executor that answers that every task is done.
    struct AlwaysDone;

    impl TaskExecutor for AlwaysDone {
        fn execute(&mut self, _task: &Task, _now_ms: u64) -> Result<TaskResult, TaseError> {
            Ok(TaskResult::Succeeded {
                result_digest: "sha256:done".to_string(),
            })
        }
    }

    /// An executor that refuses every task.
    struct AlwaysUnavailable;

    impl TaskExecutor for AlwaysUnavailable {
        fn execute(&mut self, _task: &Task, _now_ms: u64) -> Result<TaskResult, TaseError> {
            Err(TaseError::worker_unavailable("the host is not there"))
        }
    }

    fn source_with(tasks: usize) -> MemoryTaskSource {
        let queue = Arc::new(Mutex::new(crate::consumer::TaskQueue::new()));
        {
            let mut guard = queue.lock().expect("the lock of the queue is not poisoned");
            for index in 0..tasks {
                guard.push(QueuedTask::new(
                    &format!("job_{index:012}"),
                    &format!("key-{index}"),
                    "sha256:request",
                ));
            }
        }
        MemoryTaskSource::with_queue(queue, "worker-1", 30_000)
    }

    fn worker_of(
        source: MemoryTaskSource,
        stop: StopSignal,
    ) -> Worker<MemoryTaskSource, AlwaysDone> {
        let config = WorkerConfig::default();
        let consumer = Consumer::new(source, AlwaysDone, config.retry_policy());
        Worker::new(config, consumer, stop)
    }

    #[test]
    fn the_configuration_of_the_default_worker_is_valid() {
        let config = WorkerConfig::default();
        assert!(config.validate().is_ok());
        assert_eq!(config.retry_policy().max_attempts, 3);
    }

    #[test]
    fn the_configuration_refuses_a_heartbeat_that_is_not_shorter_than_the_lease() {
        let mut config = WorkerConfig::default();
        config.heartbeat_ms = config.lease_ttl_ms;
        let error = config.validate().expect_err("the configuration is refused");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn the_configuration_refuses_a_job_that_is_never_tried() {
        let config = WorkerConfig {
            max_attempts: 0,
            ..WorkerConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn the_worker_runs_the_jobs_of_the_queue_and_stops_when_it_is_asked_to() {
        let stop = StopSignal::new();
        let mut worker = worker_of(source_with(2), stop.clone());
        let clock = LogicalClock::new(1_000);

        // the worker is asked to stop while it waits
        let signal = stop.clone();
        let shutdown = worker
            .run(&clock, move |_now_ms| signal.stop())
            .expect("the worker runs");

        assert!(shutdown.stopped, "the worker stopped when it was asked to");
        assert_eq!(shutdown.processed, 2, "the two jobs of the queue ran");
    }

    #[test]
    fn the_worker_drains_the_queue() {
        let stop = StopSignal::new();
        let mut worker = worker_of(source_with(3), stop);
        let clock = LogicalClock::new(1_000);

        let shutdown = worker.drain(&clock).expect("the worker runs");

        assert!(
            !shutdown.stopped,
            "the worker stopped because the queue was empty"
        );
        assert_eq!(shutdown.processed, 3);
    }

    #[test]
    fn the_worker_runs_one_job_at_a_time() {
        let stop = StopSignal::new();
        let mut worker = worker_of(source_with(1), stop);

        let outcome = worker.step(1_000).expect("the job runs");
        assert!(outcome.is_busy());
        let outcome = worker.step(1_001).expect("the worker answers");
        assert!(!outcome.is_busy(), "the queue is empty");
    }

    #[test]
    fn a_job_that_the_host_refuses_is_tried_again() {
        let stop = StopSignal::new();
        let config = WorkerConfig::default();
        let consumer = Consumer::new(source_with(1), AlwaysUnavailable, config.retry_policy());
        let mut worker = Worker::new(config, consumer, stop);

        let outcome = worker.step(1_000).expect("the job runs");
        assert!(matches!(outcome, ConsumeOutcome::Retry { .. }));
    }
}
