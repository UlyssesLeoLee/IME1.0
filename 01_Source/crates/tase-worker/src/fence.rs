//! The fence of an apply: the lock of the worker and the live epoch of the lease of the job (detailed design §5.5
//! and §17.6).
//!
//! # Why the worker has a fence of its own
//!
//! The executor does not own the lock of the commit coordinator: it asks for it through the [`Fence`] trait of the
//! executor, so that the epoch of the lease of the job is compared **while the lock is held** and a worker whose
//! lease ran out writes nothing at all. The worker is the side that owns the lock: the lock is the lock of the store
//! of the worker process, and the live epoch is the epoch of the row of the job in that store.
//!
//! # The shape
//!
//! | the executor asks for | the worker answers |
//! |---|---|
//! | [`Fence::lock`] - the lock of the target | the lock of the store of the worker process |
//! | [`FenceHandle::live_epoch`] - the epoch as it is now | the epoch of the job in the store, read under the lock |
//!
//! The epoch that the executor compares is the epoch the caller got when it took the job, and not the epoch it reads
//! when it reports: an epoch that is read at the last moment is the epoch of whoever holds the job now, and it fences
//! nothing (detailed design §6.2).

use std::sync::{Mutex, MutexGuard};

use tase_contract::TaseError;
use tase_contract::ids::JobId;
use tase_executor::apply::{Fence, FenceHandle};
use tase_local_store::Store;

/// The fence of an apply that reads the epoch of the job from the store of the local hub.
///
/// The lock is the lock of the store of the worker process: it is taken before the epoch is read and it is given
/// back when the handle of the lock is dropped. The epoch that the executor compares is therefore the epoch of the
/// job as it is now, and not the epoch the caller saw before it waited for the lock.
pub struct StoreFence<'a> {
    store: &'a Mutex<Store>,
    job_id: JobId,
}

/// The handle of the lock of the fence: it holds the lock of the store while the epoch is read.
struct StoreFenceHandle<'a> {
    store: MutexGuard<'a, Store>,
    job_id: JobId,
}

impl<'a> StoreFence<'a> {
    /// The fence of the job, over the store of the worker.
    pub fn new(store: &'a Mutex<Store>, job_id: &JobId) -> StoreFence<'a> {
        StoreFence {
            store,
            job_id: job_id.clone(),
        }
    }
}

impl Fence for StoreFence<'_> {
    fn lock(&mut self) -> Result<Box<dyn FenceHandle + '_>, TaseError> {
        let store = self.store.lock().map_err(|_| {
            TaseError::internal("the lock of the store of the worker is poisoned")
        })?;
        Ok(Box::new(StoreFenceHandle {
            store,
            job_id: self.job_id.clone(),
        }))
    }
}

impl FenceHandle for StoreFenceHandle<'_> {
    fn live_epoch(&mut self) -> Result<u64, TaseError> {
        let job = self.store.get_job(&self.job_id)?.ok_or_else(|| {
            TaseError::not_found(format!("there is no job {}", self.job_id.as_str()))
        })?;
        Ok(job.lease_epoch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tase_contract::ids::{AgentId, SessionId, WorkspaceId};
    use tase_local_store::{NewJob, NewSession};

    /// A store of the disk with a session and a job, and the id of the job.
    fn store_with_a_job() -> (Mutex<Store>, JobId) {
        let mut store = Store::open_in_memory().expect("the store opens");
        tase_local_store::migrations::apply(&mut store).expect("the steps run");
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
                now_ms: 1_000,
            })
            .expect("the session is registered");
        let submission = store
            .submit_job(NewJob {
                job_type: "tase.code.apply".to_string(),
                workspace_id: WorkspaceId::new("workspace-1").expect("the workspace id is valid"),
                agent_id: AgentId::new("agent-1").expect("the agent id is valid"),
                session_id,
                idempotency_key: "key-1".to_string(),
                request_digest: "sha256:request".to_string(),
                base_snapshot_digest: "sha256:base".to_string(),
                priority: 0,
                now_ms: 1_000,
            })
            .expect("the job is registered");
        (Mutex::new(store), submission.job_id)
    }

    #[test]
    fn the_fence_reads_the_epoch_of_the_job_under_the_lock() {
        let (store, job_id) = store_with_a_job();
        {
            let _lease = store
                .lock()
                .unwrap()
                .lease_job(&job_id, "worker-1", 30_000, 1_000)
                .expect("the job is leased");
        }
        let mut fence = StoreFence::new(&store, &job_id);

        let mut handle = fence.lock().expect("the lock is taken");
        assert_eq!(handle.live_epoch().expect("the epoch is read"), 1);
    }

    #[test]
    fn the_fence_reads_the_epoch_that_moved_while_the_worker_worked() {
        let (store, job_id) = store_with_a_job();
        {
            let _lease = store
                .lock()
                .unwrap()
                .lease_job(&job_id, "worker-1", 30_000, 1_000)
                .expect("the job is leased");
        }
        // the lease of the job runs out and another worker takes the job back: the epoch of the job moves
        {
            let _lease = store
                .lock()
                .unwrap()
                .lease_job(&job_id, "worker-2", 30_000, 31_001)
                .expect("the job is taken back");
        }
        let mut fence = StoreFence::new(&store, &job_id);

        let mut handle = fence.lock().expect("the lock is taken");
        assert_eq!(handle.live_epoch().expect("the epoch is read"), 2);
    }

    #[test]
    fn the_fence_of_a_job_that_is_gone_is_not_found() {
        let (store, _) = store_with_a_job();
        let job_id = JobId::new("job_000000000999").expect("the job id is valid");
        let mut fence = StoreFence::new(&store, &job_id);

        let mut handle = fence.lock().expect("the lock is taken");
        let error = handle
            .live_epoch()
            .expect_err("the job that is gone has no epoch");
        assert_eq!(error.code, tase_contract::ErrorCode::NotFound);
    }
}
