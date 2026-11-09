//! TASE worker: the resident worker of TASE - the consumer of the queue, the lease with its fencing token, the
//! runner that hands a job to the executor, the verifier of the gates and the recovery after a crash (detailed design
//! §14, S3 "Worker & Queue").
//!
//! The worker is the part of TASE that **runs the jobs**. The planner decides what a change is, the registry decides
//! whether it may be made and the executor turns a plan into a diff, an edit and a commit; the worker is the process
//! that takes a job out of the queue, that keeps it while it runs, that refuses to write when it is not the holder any
//! more, that checks the result and that puts the state back together when it dies.
//!
//! # What the crate holds
//!
//! * [`lease`] - the deadline of a job, the fencing token that grows with every lease and the guard that refuses the
//!   write of a holder that came back too late;
//! * [`consumer`] - the consumer of the queue: it takes a task, it runs it, it tells the queue what happened and it
//!   waits before it tries again;
//! * [`runner`] - the runner of a job: the diff of the executor, the apply to the host editor and the commit;
//! * [`verifier`] - the gates of the sandbox, the assertions of the acceptance file and the tokens and the cost of
//!   the job;
//! * [`recovery`] - what happens after a crash: the leases that ran out, the jobs that go back into the queue, the
//!   idempotency key that must not run the same job twice and the snapshot of the state;
//! * [`main`] - the skeleton of the resident process: the configuration, the loop of the jobs and the stop in an
//!   orderly way.
//!
//! # What the crate covers of detailed design §14
//!
//! Detailed design §14 is the build order of TASE; its fourth step is the step of this crate:
//!
//! | step of §14 | what this crate does |
//! |---|---|
//! | S3 "Worker & Queue：租约、fencing、outbox、故障恢复与 32 并发模拟" | the leases and the fencing ([`lease`]), the queue ([`consumer`]), the recovery ([`recovery`]) and the simulation of the concurrent workers (the test of [`consumer`] that lets thirty two workers compete for the same jobs) |
//! | S2 "Apply & Git CAS" | the worker **calls** it: the diff, the apply, the rollback and the compare-and-swap come from `tase-executor` ([`runner`]) |
//! | S5 "TS & Bench" | the gates and the metrics of the acceptance are here ([`verifier`]); the benchmark itself is not |
//! | the "禁止条件" of §14 ("不得为了满足性能目标跳过 `Document` 一致性、编译/测试验证、授权审批或 Git CAS") | the worker never announces a job that was not verified: the runner writes, the verifier checks, and the commit comes after the checks ([`runner`], [`verifier`]) |
//!
//! The other paragraphs of the design book that the worker implements live in the modules: the pseudo code of
//! `worker_apply` of §5.2 is the runner and the consumer, the atomic claim of the lease of §6.2 is the lease, the
//! rules of the sandbox of §7.2 are the allowlist of the verifier, the table of the failure modes of §13 is the
//! recovery, and the fields of the acceptance file of §10.1 are the assertions of the verifier.
//!
//! # What the crate does NOT cover, and why
//!
//! * **The distributed scheduler.** The worker of this repository is one process on one machine. The queue of the
//!   jobs is the table of the local store, and the claim of a job is a transaction of SQLite, not a lease of a broker
//!   of a cluster. A worker of a cluster would need a durable queue that is not there yet, and the design book itself
//!   puts the Postgres reference DDL of §6.1 in the "no counterpart" column of the mapping document (§7-G7).
//! * **The fairness between the agents.** The fair rotation of the agents, the quota of a session and the share of
//!   the budget are the job of `tase-agent-scheduler` (the tenth crate of the mapping document). The worker asks the
//!   queue for "a task that may run now" and does not know nor care which agent asked first; the order of the agents
//!   is decided above it and never inside it.
//! * **The sandbox itself.** The verifier of this crate checks the *command* (the program is in the allowlist, the
//!   directory is inside the workspace, the arguments are a vector and never a shell string) and reads the answer of
//!   the door to the sandbox. The isolation of the process, the limit of the resources and the denial of the network
//!   of §7.2 are the job of the operating system and of the runner of the commands, which this repository does not
//!   have yet.
//! * **The door to the host.** The apply of the worker hands the edits to the `DocumentEditAdapter` of the executor
//!   and waits for the editor. In this repository that door is **not** carried by the named pipe of the hub: the only
//!   process that can reach the focused window is the one that speaks the bridge of the text service (the
//!   `WM_COPYDATA` bridge of `pime-agent`), so the **client side** holds the adapter and the worker holds the plan
//!   and the diff. The worker therefore never writes into a document by itself: the binary of this crate runs with a
//!   host that refuses every job instead of pretending that it wrote something, and the round trip
//!   "plan → preview → apply" ends in the client that asked for it.
//! * **The decision of the rebase.** The rebase decision of §5.3 needs the symbol index of the new head, and the
//!   symbol index belongs to `tase-semantic`. The worker does not depend on the semantic layer: it asks a
//!   [`runner::RebaseGate`], and the host that owns the index answers it with the decision of `tase-executor`.
//! * **The outbox.** The outbox of S3 is written in the same transaction as the job and is read by the relay of the
//!   hub; the worker neither writes nor publishes it.
//!
//! # The rules of the crate
//!
//! * **The time is injected.** Every deadline of the worker - the TTL of a lease, the window of a backoff, the moment
//!   a lease runs out - comes from the clock of the caller ([`lease::Clock`]). The core of the crate never reads the
//!   wall clock, so a test moves the time by hand and never waits.
//! * **The executor is not reinvented.** The diff, the apply, the rollback, the rebase and the compare-and-swap come
//!   from `tase-executor`: this crate writes no second diff and no second commit logic.
//! * **The old holder never writes.** A worker whose lease ran out is told that it is old, and its write is refused
//!   ([`lease::LeaseTable::check`], and the epoch of the job in the store).
//! * **A job that is not verified is not committed.** The runner writes, the verifier checks, and the commit comes
//!   after the checks (detailed design §14: "不得因为'模板生成成功'直接宣布'任务成功'").
//! * **No third-party dependency, no network.** The crate never goes on the network; the tests write only in the
//!   temporary directory of the test that made it.

#![forbid(unsafe_code)]
// The module of the skeleton of the worker is `src/main.rs` (the table of the crates of the mapping document). The
// file is a module of the library and not the root of a binary: the autodiscovery of the binaries is off and the only
// binary of the crate is `src/bin/tase-worker.rs`. The lint of rustc does not know that.
#![allow(special_module_name)]

pub mod consumer;
pub mod hub_link;
pub mod lease;
// The mapping document asks for `src/main.rs` as the module of the skeleton of the worker (the fifth row of the table
// of the crates). The file is a module of the library and not the root of a binary: the autodiscovery of the binaries
// is off and the only binary of the crate is `src/bin/tase-worker.rs`. The lint of rustc does not know that.
#[allow(special_module_name)]
pub mod main;
pub mod recovery;
pub mod runner;
pub mod verifier;

pub use consumer::{
    ConsumeOutcome, Consumer, MemoryTaskSource, QueuedTask, RetryPolicy, StoreTaskSource, Task,
    TaskExecutor, TaskQueue, TaskReport, TaskResult, TaskSource,
};
pub use hub_link::{
    COMPLETE_METHOD, HubJobRunner, HubLink, LinkOutcome, PreviewRunner, TAKE_METHOD, TakenJob,
    WORKER_CAPABILITIES, preview_to_json, run_request_of,
};
pub use lease::{
    Clock, FenceGuard, Lease, LeaseError, LeaseGuard, LeaseState, LeaseTable, LogicalClock,
    SystemClock, Unguarded,
};
pub use main::{Shutdown, StopSignal, Worker, WorkerConfig};
pub use recovery::{
    CommitEvidence, IdempotencyLedger, PrepareOutcome, RecoveryAction, RecoveryJob, RecoveryPolicy,
    RefEvidence,
};
pub use runner::{DocumentInput, RebaseGate, RunOutcome, RunRequest, Runner};
pub use verifier::{CommandResult, CommandRunner, Gate, SandboxCommand, Verification, Verifier};

/// The version of the worker.
pub const WORKER_VERSION: &str = "1.1.0";
