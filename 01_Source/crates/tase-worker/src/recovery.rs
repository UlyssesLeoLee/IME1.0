//! The recovery of a worker after a crash: the leases that ran out, the jobs that go back into the queue, the
//! idempotency key that must not run the same job twice and the snapshot of the state (detailed design §5.5, §13 and
//! §17.6).
//!
//! # The three questions of the recovery
//!
//! When a worker comes back after a crash, it has to answer three questions about every job that was in flight:
//!
//! 1. **Did the job write anything?** A job whose commit is already on the ref is finished: the record is completed
//!    and the job does not run again. The evidence is the ref itself, and the trailer of the commit carries the id of
//!    the job (detailed design §5.5).
//! 2. **May the job run again?** A job that wrote nothing may run again, as long as it has tries left. A job whose
//!    attempts are used up is abandoned: a retry is a new job (detailed design §6.3).
//! 3. **Is it the same job?** The idempotency key of the job is the identity of the request. The same key with the
//!    same digest is the same job; the same key with another digest is a different request and is refused
//!    (`IDEMPOTENCY_MISMATCH`, detailed design §2.5).
//!
//! # The state of the store
//!
//! The recovery of the store itself is the job of `tase-local-store`: this module calls its reconciliation (the
//! sessions that stopped, the leases that ran out, the jobs that go back into the queue and the conflicts of the
//! files) and its snapshots (the state of a moment and the journal that came after it).

use tase_contract::TaseError;
use tase_executor::git_tx::GitRepo;
use tase_local_store::reconcile::{ReconcileOptions, ReconcileReport};
use tase_local_store::snapshots::{RestoreReport, SnapshotInfo};
use tase_local_store::{Lease, Store};

/// The state of an idempotency key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgerState {
    /// The job is prepared: it may have been written, but nothing is committed yet.
    Prepared,
    /// The job is committed: it may not run again.
    Committed,
}

/// One entry of the ledger of the idempotency keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerEntry {
    /// The key of the job.
    pub key: String,
    /// The digest of the request of the job.
    pub request_digest: String,
    /// The digest of the result of the job, when it is done.
    pub result_digest: Option<String>,
    /// The state of the key.
    pub state: LedgerState,
}

/// What the ledger says about a key that is prepared again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrepareOutcome {
    /// The key is free: the job may run.
    Fresh,
    /// The key is already known with the same digest: it is the same job.
    Duplicate,
    /// The key is known with another digest: the request is not the same one.
    Mismatch,
}

/// The ledger of the idempotency keys of the jobs of a worker.
///
/// The ledger is the memory of the worker about the jobs it ran. A key that is prepared and not committed is a job
/// that may run again; a key that is committed is a job that never runs again.
#[derive(Clone, Debug, Default)]
pub struct IdempotencyLedger {
    entries: Vec<LedgerEntry>,
}

impl IdempotencyLedger {
    /// A ledger without a key.
    pub fn new() -> IdempotencyLedger {
        IdempotencyLedger {
            entries: Vec::new(),
        }
    }

    /// Prepares a key for a job.
    ///
    /// The same key with the same digest is the same job; the same key with another digest is refused.
    pub fn prepare(&mut self, key: &str, request_digest: &str) -> PrepareOutcome {
        if let Some(entry) = self.entries.iter().find(|entry| entry.key == key) {
            return if entry.request_digest == request_digest {
                PrepareOutcome::Duplicate
            } else {
                PrepareOutcome::Mismatch
            };
        }
        self.entries.push(LedgerEntry {
            key: key.to_string(),
            request_digest: request_digest.to_string(),
            result_digest: None,
            state: LedgerState::Prepared,
        });
        PrepareOutcome::Fresh
    }

    /// Marks a key as committed and gives the digest of the result back.
    ///
    /// A key that is not known is not committed: the call answers `false`.
    pub fn commit(&mut self, key: &str, result_digest: &str) -> bool {
        match self.entries.iter_mut().find(|entry| entry.key == key) {
            Some(entry) => {
                entry.state = LedgerState::Committed;
                entry.result_digest = Some(result_digest.to_string());
                true
            }
            None => false,
        }
    }

    /// The state of a key.
    pub fn state(&self, key: &str) -> Option<LedgerState> {
        self.entry(key).map(|entry| entry.state)
    }

    /// The entry of a key.
    pub fn entry(&self, key: &str) -> Option<&LedgerEntry> {
        self.entries.iter().find(|entry| entry.key == key)
    }

    /// How many keys the ledger holds.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the ledger holds no key.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The evidence of a commit: the part that knows where the ref points at.
pub trait CommitEvidence {
    /// The commit the ref points at, when the ref exists.
    fn head(&self) -> Option<String>;
}

/// The evidence of a commit in a repository.
pub struct RefEvidence<'a> {
    repo: &'a dyn GitRepo,
    ref_name: String,
}

impl<'a> RefEvidence<'a> {
    /// The evidence of the ref of a repository.
    pub fn new(repo: &'a dyn GitRepo, ref_name: &str) -> RefEvidence<'a> {
        RefEvidence {
            repo,
            ref_name: ref_name.to_string(),
        }
    }
}

impl CommitEvidence for RefEvidence<'_> {
    fn head(&self) -> Option<String> {
        self.repo.head(&self.ref_name).ok()
    }
}

/// The job the recovery looks at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryJob {
    /// The id of the job.
    pub job_id: String,
    /// The idempotency key of the job.
    pub idempotency_key: String,
    /// The digest of the request of the job.
    pub request_digest: String,
    /// How many times the job was tried.
    pub attempt: u32,
    /// The commit the worker wrote before it died, when it wrote one.
    pub committed_commit: Option<String>,
}

/// The rule of the recovery of a job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecoveryPolicy {
    /// How many times a job may be tried.
    pub max_attempts: u32,
}

impl Default for RecoveryPolicy {
    fn default() -> Self {
        RecoveryPolicy { max_attempts: 3 }
    }
}

impl RecoveryPolicy {
    /// Whether the job may be tried again after `attempt` tries.
    pub fn may_retry(&self, attempt: u32) -> bool {
        attempt < self.max_attempts
    }
}

/// What the recovery does with a job that was interrupted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryAction {
    /// The job may run again: nothing was committed.
    Replay {
        /// The number of the try that comes.
        attempt: u32,
    },
    /// The job is done: its commit is already there, and it never runs again.
    Complete {
        /// The commit of the job, when it is known.
        commit: Option<String>,
    },
    /// The job may not run again.
    Abandon {
        /// Why the job is not run again.
        reason: String,
    },
}

/// The decision of the recovery about a job.
pub fn plan(
    job: &RecoveryJob,
    ledger: &IdempotencyLedger,
    evidence: &dyn CommitEvidence,
    policy: &RecoveryPolicy,
) -> RecoveryAction {
    let commit = job.committed_commit.clone();
    let committed = commit
        .as_ref()
        .is_some_and(|commit| evidence.head().as_deref() == Some(commit.as_str()));

    // the commit is already on the ref, or the ledger says the job is done: the job never runs again
    if committed || ledger.state(&job.idempotency_key) == Some(LedgerState::Committed) {
        return RecoveryAction::Complete { commit };
    }

    if !policy.may_retry(job.attempt) {
        return RecoveryAction::Abandon {
            reason: format!(
                "the job {} was tried {} times and may not be tried again",
                job.job_id, job.attempt
            ),
        };
    }

    RecoveryAction::Replay {
        attempt: job.attempt.saturating_add(1),
    }
}

/// Runs the reconciliation of the store at the given time.
///
/// The reconciliation takes the leases that ran out back, sends the jobs whose lease is gone back into the queue and
/// judges the conflicts of the files (detailed design §17.6).
pub fn reconcile_store(store: &mut Store, now_ms: u64) -> Result<ReconcileReport, TaseError> {
    tase_local_store::reconcile::reconcile_with(
        store,
        ReconcileOptions {
            now_ms,
            session_timeout_ms: tase_local_store::reconcile::DEFAULT_SESSION_TIMEOUT_MS,
        },
    )
}

/// Takes the leases that ran out of time back.
pub fn reclaim_expired_leases(store: &mut Store, now_ms: u64) -> Result<Vec<Lease>, TaseError> {
    store.reclaim_expired_leases(now_ms)
}

/// Writes a snapshot of the state of the store.
pub fn take_snapshot(
    store: &mut Store,
    snapshot_id: &str,
    now_ms: u64,
) -> Result<SnapshotInfo, TaseError> {
    tase_local_store::snapshots::write_snapshot(store, snapshot_id, now_ms)
}

/// Puts the state of the store back from a snapshot.
pub fn restore_snapshot(store: &mut Store, snapshot_id: &str) -> Result<RestoreReport, TaseError> {
    tase_local_store::snapshots::restore_from_snapshot(store, snapshot_id)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use tase_contract::ids::{AgentId, SessionId, WorkspaceId};
    use tase_executor::git_tx::MemoryGit;
    use tase_local_store::{JobStatus, NewJob, NewSession};

    use super::*;

    /// A directory of the disk that only this test uses.
    fn temp_dir(name: &str) -> PathBuf {
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

    /// A store of the disk, with the steps of the schema applied.
    fn store_of(dir: &Path) -> Store {
        let mut store = Store::open(dir.join("worker.sqlite3")).expect("the store opens");
        tase_local_store::migrations::apply(&mut store).expect("the steps run");
        store
    }

    /// A session and a job of the store.
    fn job_of(store: &mut Store, key: &str, now_ms: u64) -> tase_local_store::Job {
        let session_id = SessionId::new("session-1").expect("the session id is valid");
        store
            .open_session(NewSession {
                session_id: session_id.clone(),
                agent_id: AgentId::new("agent-1").expect("the agent id is valid"),
                provider: "test".to_string(),
                instance_id: "instance-1".to_string(),
                workspace_id: WorkspaceId::new("workspace-1").expect("the workspace id is valid"),
                owner_logon_sid_hash: "sha256:logon".to_string(),
                granted_scopes: vec!["tase.code.apply".to_string()],
                capabilities: Vec::new(),
                now_ms,
            })
            .expect("the session is registered");
        let submission = store
            .submit_job(NewJob {
                job_type: "tase.code.apply".to_string(),
                workspace_id: WorkspaceId::new("workspace-1").expect("the workspace id is valid"),
                agent_id: AgentId::new("agent-1").expect("the agent id is valid"),
                session_id,
                idempotency_key: key.to_string(),
                request_digest: "sha256:request".to_string(),
                base_snapshot_digest: "sha256:base".to_string(),
                priority: 0,
                now_ms,
            })
            .expect("the job is registered");
        store
            .get_job(&submission.job_id)
            .expect("the job is read")
            .expect("the job is there")
    }

    fn job_of_recovery(attempt: u32) -> RecoveryJob {
        RecoveryJob {
            job_id: "job_000000000001".to_string(),
            idempotency_key: "key-1".to_string(),
            request_digest: "sha256:request".to_string(),
            attempt,
            committed_commit: None,
        }
    }

    #[test]
    fn a_key_that_is_free_is_prepared() {
        let mut ledger = IdempotencyLedger::new();
        assert_eq!(ledger.prepare("key-1", "sha256:a"), PrepareOutcome::Fresh);
        assert_eq!(ledger.state("key-1"), Some(LedgerState::Prepared));
        assert_eq!(ledger.len(), 1);
    }

    #[test]
    fn the_same_key_with_the_same_digest_is_the_same_job() {
        let mut ledger = IdempotencyLedger::new();
        assert_eq!(ledger.prepare("key-1", "sha256:a"), PrepareOutcome::Fresh);
        assert_eq!(
            ledger.prepare("key-1", "sha256:a"),
            PrepareOutcome::Duplicate
        );
    }

    #[test]
    fn the_same_key_with_another_digest_is_a_mismatch() {
        let mut ledger = IdempotencyLedger::new();
        assert_eq!(ledger.prepare("key-1", "sha256:a"), PrepareOutcome::Fresh);
        assert_eq!(
            ledger.prepare("key-1", "sha256:b"),
            PrepareOutcome::Mismatch
        );
        assert_eq!(ledger.len(), 1, "the key was not written twice");
    }

    #[test]
    fn a_committed_key_is_never_run_again() {
        let mut ledger = IdempotencyLedger::new();
        let _prepared = ledger.prepare("key-1", "sha256:a");
        assert!(ledger.commit("key-1", "sha256:result"));
        assert_eq!(ledger.state("key-1"), Some(LedgerState::Committed));

        let git = MemoryGit::new();
        let evidence = RefEvidence::new(&git, "refs/heads/main");
        let action = plan(
            &job_of_recovery(1),
            &ledger,
            &evidence,
            &RecoveryPolicy::default(),
        );
        assert_eq!(
            action,
            RecoveryAction::Complete { commit: None },
            "the job of a committed key does not run again"
        );
    }

    #[test]
    fn a_job_that_wrote_nothing_runs_again() {
        let ledger = IdempotencyLedger::new();
        let git = MemoryGit::new();
        let evidence = RefEvidence::new(&git, "refs/heads/main");
        let action = plan(
            &job_of_recovery(1),
            &ledger,
            &evidence,
            &RecoveryPolicy::default(),
        );
        assert_eq!(action, RecoveryAction::Replay { attempt: 2 });
    }

    #[test]
    fn a_job_whose_commit_is_on_the_ref_is_complete() {
        let ledger = IdempotencyLedger::new();
        let mut git = MemoryGit::new();
        git.set_head("refs/heads/main", "commit-9");
        let evidence = RefEvidence::new(&git, "refs/heads/main");

        let mut job = job_of_recovery(1);
        job.committed_commit = Some("commit-9".to_string());
        let action = plan(&job, &ledger, &evidence, &RecoveryPolicy::default());
        assert_eq!(
            action,
            RecoveryAction::Complete {
                commit: Some("commit-9".to_string())
            }
        );
    }

    #[test]
    fn a_job_whose_commit_is_not_on_the_ref_runs_again() {
        let ledger = IdempotencyLedger::new();
        let mut git = MemoryGit::new();
        git.set_head("refs/heads/main", "commit-8");
        let evidence = RefEvidence::new(&git, "refs/heads/main");

        let mut job = job_of_recovery(1);
        job.committed_commit = Some("commit-9".to_string());
        let action = plan(&job, &ledger, &evidence, &RecoveryPolicy::default());
        assert_eq!(action, RecoveryAction::Replay { attempt: 2 });
    }

    #[test]
    fn a_job_that_used_its_tries_is_abandoned() {
        let ledger = IdempotencyLedger::new();
        let git = MemoryGit::new();
        let evidence = RefEvidence::new(&git, "refs/heads/main");
        let action = plan(
            &job_of_recovery(3),
            &ledger,
            &evidence,
            &RecoveryPolicy::default(),
        );
        assert!(matches!(action, RecoveryAction::Abandon { .. }));
    }

    #[test]
    fn the_lease_of_a_job_that_ran_out_is_taken_back_and_the_job_waits_again() {
        let dir = temp_dir("recover");
        let mut store = store_of(&dir);
        let job = job_of(&mut store, "key-1", 1_000);

        let lease = store
            .lease_job(&job.job_id, "worker-1", 30_000, 1_000)
            .expect("the job is leased");
        assert_eq!(lease.fencing_token, 1);

        // the worker dies and comes back after the lease ran out
        let report = reconcile_store(&mut store, 31_001).expect("the reconciliation runs");
        assert!(report.expired_leases >= 1, "the lease was taken back");
        assert!(report.requeued_jobs >= 1, "the job waits again");

        let again = store
            .get_job(&job.job_id)
            .expect("the job is read")
            .expect("the job is there");
        assert_eq!(again.status, JobStatus::Queued);
        assert_eq!(again.attempt, 1);

        // and the job may be taken again, with a greater token
        let second = store
            .lease_job(&job.job_id, "worker-2", 30_000, 31_002)
            .expect("the job is leased again");
        assert!(second.fencing_token > lease.fencing_token);

        // the store holds the file of the database: it has to let go of it before the directory goes away
        drop(store);
        std::fs::remove_dir_all(&dir).expect("the directory of the test is removed");
    }

    #[test]
    fn the_state_of_the_store_is_restored_from_a_snapshot() {
        let dir = temp_dir("snapshot");
        let mut store = store_of(&dir);
        let _job = job_of(&mut store, "key-1", 1_000);

        let snapshot =
            take_snapshot(&mut store, "snapshot-1", 2_000).expect("the snapshot is written");
        assert_eq!(snapshot.snapshot_id, "snapshot-1");

        let report = restore_snapshot(&mut store, "snapshot-1").expect("the state is restored");
        assert_eq!(report.jobs, 1);
        assert_eq!(report.sessions, 1);

        drop(store);
        std::fs::remove_dir_all(&dir).expect("the directory of the test is removed");
    }

    #[test]
    fn the_leases_that_ran_out_are_given_back() {
        let dir = temp_dir("leases");
        let mut store = store_of(&dir);
        let job = job_of(&mut store, "key-1", 1_000);
        let _lease = store
            .lease_job(&job.job_id, "worker-1", 30_000, 1_000)
            .expect("the job is leased");

        let expired =
            reclaim_expired_leases(&mut store, 31_001).expect("the leases are taken back");
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].holder, "worker-1");

        drop(store);
        std::fs::remove_dir_all(&dir).expect("the directory of the test is removed");
    }
}
