//! The reconciliation of the store after a crash (detailed design §17.6).
//!
//! When the hub dies, the store keeps whatever the last transactions wrote. Three kinds of rows may then lie:
//!
//! * a **session** whose heartbeats stopped: the agent is gone, the session must not be `CONNECTED` any more;
//! * a **lease** whose time ran out: the holder is a zombie, the lease must be given back;
//! * a **job** that is `RUNNING` or `VERIFYING` while its lease is gone: nobody works on it any more, it must go
//!   back to the queue;
//! * a **conflict of a file** that was seen but never judged: the file may have moved on (the conflict is confirmed
//!   and the job that met it is a `CONFLICT`), or it may have come back (the conflict is closed).
//!
//! The reconciliation runs in one transaction: the state is either the state before or the state after, never half
//! of it. Every change is written in the journal and in the audit trail, with the events `RECOVERED` and `ORPHANED`
//! of the design book.

use tase_contract::{ErrorCode, TaseError};

use crate::{
    AuditRecord, ConflictState, JobStatus, LeaseState, SessionState, Store, job_to_json, lease_to_json,
    session_to_json,
};

/// The event of the audit trail that says a thing was taken back.
pub const EVENT_RECOVERED: &str = "RECOVERED";
/// The event of the audit trail that says a thing was left behind.
pub const EVENT_ORPHANED: &str = "ORPHANED";

/// The default time a session may stay without a heartbeat.
pub const DEFAULT_SESSION_TIMEOUT_MS: u64 = 30_000;

/// The options of a reconciliation.
#[derive(Clone, Copy, Debug)]
pub struct ReconcileOptions {
    /// The time the reconciliation starts at.
    pub now_ms: u64,
    /// How long a session may stay without a heartbeat before the reconciliation gives up on it.
    pub session_timeout_ms: u64,
}

impl Default for ReconcileOptions {
    fn default() -> Self {
        ReconcileOptions {
            now_ms: 0,
            session_timeout_ms: DEFAULT_SESSION_TIMEOUT_MS,
        }
    }
}

/// What a reconciliation did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    /// The sessions that were given up on.
    pub expired_sessions: u32,
    /// The leases that ran out of time.
    pub expired_leases: u32,
    /// The jobs that went back to the queue.
    pub requeued_jobs: u32,
    /// The conflicts that were confirmed.
    pub confirmed_conflicts: u32,
    /// The conflicts that were closed.
    pub resolved_conflicts: u32,
    /// The lines that were written in the audit trail.
    pub audit_events: u32,
}

/// Runs the reconciliation of the store at the given time.
///
/// The sessions that stopped sending heartbeats are given up on, the leases that ran out of time are taken back,
/// the jobs whose lease is gone go back to the queue and the conflicts of the files are judged.
pub fn reconcile(store: &mut Store, now_ms: u64) -> Result<ReconcileReport, TaseError> {
    reconcile_with(
        store,
        ReconcileOptions {
            now_ms,
            session_timeout_ms: DEFAULT_SESSION_TIMEOUT_MS,
        },
    )
}

/// Runs the reconciliation of the store with the given options.
pub fn reconcile_with(
    store: &mut Store,
    options: ReconcileOptions,
) -> Result<ReconcileReport, TaseError> {
    let tx = store.transaction()?;
    let mut report = ReconcileReport::default();

    // 1. the sessions that stopped sending heartbeats
    let cutoff = options
        .now_ms
        .saturating_sub(options.session_timeout_ms);
    let sessions = crate::read_all(
        &tx,
        "SELECT session_id FROM local_agent_sessions
          WHERE state = 'CONNECTED' AND heartbeat_at_ms <= ?1 ORDER BY session_id",
        rusqlite::params![cutoff as i64],
        |row| row.get::<_, String>(0).map_err(crate::sqlite::map_sqlite),
    )?;
    for session_id in sessions {
        if let Some(mut row) = crate::row_session(&tx, &session_id)? {
            row.state = SessionState::Expired;
            crate::upsert_session(&tx, &row)?;
            crate::sqlite::append_journal(
                &tx,
                crate::KIND_SESSION,
                &session_to_json(&row),
                options.now_ms,
            )?;
            audit(
                &tx,
                event(
                    EVENT_ORPHANED,
                    row.agent_id.as_str(),
                    row.session_id.as_str(),
                    row.workspace_id.as_str(),
                    None,
                    &format!("session {} expired", row.session_id.as_str()),
                    options.now_ms,
                ),
            )?;
            report.expired_sessions += 1;
            report.audit_events += 1;
        }
    }

    // 2. the leases that ran out of time
    let leases = crate::expired_leases(&tx, options.now_ms)?;
    for mut lease in leases {
        lease.state = LeaseState::Expired;
        crate::upsert_lease(&tx, &lease)?;
        crate::sqlite::append_journal(
            &tx,
            crate::KIND_LEASE,
            &lease_to_json(&lease),
            options.now_ms,
        )?;
        audit(
            &tx,
            event(
                EVENT_RECOVERED,
                &lease.holder,
                lease.session_id.as_ref().map_or("", |id| id.as_str()),
                "",
                None,
                &format!("lease {} of {} expired", lease.lease_id, lease.resource),
                options.now_ms,
            ),
        )?;
        report.expired_leases += 1;
        report.audit_events += 1;
    }

    // 3. the jobs whose lease is gone go back to the queue
    let jobs = crate::list_jobs_by_status(&tx, JobStatus::Running)?
        .into_iter()
        .chain(crate::list_jobs_by_status(&tx, JobStatus::Verifying)?)
        .collect::<Vec<_>>();
    for mut job in jobs {
        let held = crate::row_held_lease(&tx, job.job_id.as_str())?;
        let alive = held.as_ref().is_some_and(|lease| lease.is_valid_at(options.now_ms));
        if alive {
            continue;
        }
        job.status = JobStatus::Queued;
        job.worker_id = None;
        job.updated_at_ms = options.now_ms;
        crate::upsert_job(&tx, &job)?;
        crate::sqlite::append_journal(
            &tx,
            crate::KIND_JOB,
            &job_to_json(&job),
            options.now_ms,
        )?;
        audit(
            &tx,
            event(
                EVENT_RECOVERED,
                job.agent_id.as_str(),
                job.session_id.as_str(),
                job.workspace_id.as_str(),
                Some(job.job_id.as_str()),
                &format!("the job {} went back to the queue", job.job_id.as_str()),
                options.now_ms,
            ),
        )?;
        report.requeued_jobs += 1;
        report.audit_events += 1;
    }

    // 4. the conflicts of the files are judged
    let conflicts = crate::list_file_conflicts(&tx)?;
    for mut conflict in conflicts {
        if conflict.state != ConflictState::Open {
            continue;
        }
        let current = crate::row_file_generation(
            &tx,
            conflict.workspace_id.as_str(),
            &conflict.normalized_path,
        )?
        .map(|row| row.generation)
        .unwrap_or(0);
        if current == conflict.expected_generation {
            conflict.state = ConflictState::Resolved;
            crate::insert_file_conflict(&tx, &conflict)?;
            crate::sqlite::append_journal(
                &tx,
                crate::KIND_FILE_CONFLICT,
                &crate::file_conflict_to_json(&conflict),
                options.now_ms,
            )?;
            report.resolved_conflicts += 1;
        } else {
            conflict.state = ConflictState::Confirmed;
            conflict.actual_generation = current;
            crate::insert_file_conflict(&tx, &conflict)?;
            crate::sqlite::append_journal(
                &tx,
                crate::KIND_FILE_CONFLICT,
                &crate::file_conflict_to_json(&conflict),
                options.now_ms,
            )?;
            if let Some(job_id) = conflict.job_id.clone()
                && let Some(mut job) = crate::row_job(&tx, job_id.as_str())?
                && !job.status.is_terminal()
            {
                job.status = JobStatus::Conflict;
                job.error_code = Some(ErrorCode::SymbolConflict);
                job.updated_at_ms = options.now_ms;
                crate::upsert_job(&tx, &job)?;
                crate::sqlite::append_journal(
                    &tx,
                    crate::KIND_JOB,
                    &job_to_json(&job),
                    options.now_ms,
                )?;
            }
            audit(
                &tx,
                event(
                    EVENT_RECOVERED,
                    conflict.agent_id.as_str(),
                    conflict.session_id.as_str(),
                    conflict.workspace_id.as_str(),
                    conflict.job_id.as_ref().map(|id| id.as_str()),
                    &format!(
                        "the file {} is at the generation {} and not at the generation {}",
                        conflict.normalized_path, current, conflict.expected_generation
                    ),
                    options.now_ms,
                ),
            )?;
            report.confirmed_conflicts += 1;
            report.audit_events += 1;
        }
    }

    // 5. the run is written down
    tx.execute(
        "INSERT INTO local_reconcile_log
           (started_at_ms, expired_sessions, expired_leases, requeued_jobs,
            confirmed_conflicts, resolved_conflicts, detail)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            options.now_ms as i64,
            report.expired_sessions as i64,
            report.expired_leases as i64,
            report.requeued_jobs as i64,
            report.confirmed_conflicts as i64,
            report.resolved_conflicts as i64,
            format!(
                "{{\"expired_sessions\":{},\"expired_leases\":{},\"requeued_jobs\":{},\
                  \"confirmed_conflicts\":{},\"resolved_conflicts\":{},\"audit_events\":{}}}",
                report.expired_sessions,
                report.expired_leases,
                report.requeued_jobs,
                report.confirmed_conflicts,
                report.resolved_conflicts,
                report.audit_events
            )
        ],
    )?;

    tx.commit()?;
    Ok(report)
}

/// Writes a line in the audit trail, inside the transaction of the reconciliation.
fn audit(tx: &rusqlite::Connection, record: AuditRecord) -> Result<(), TaseError> {
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
    )
    .map_err(crate::sqlite::map_sqlite)?;
    Ok(())
}

/// Builds the line of the audit trail of an event.
fn event(
    event_type: &str,
    agent_id: &str,
    session_id: &str,
    workspace_id: &str,
    job_id: Option<&str>,
    payload: &str,
    now_ms: u64,
) -> AuditRecord {
    AuditRecord {
        event_type: event_type.to_string(),
        agent_id: agent_id.to_string(),
        session_id: session_id.to_string(),
        workspace_id: workspace_id.to_string(),
        job_id: job_id.map(str::to_string),
        payload_digest: pime_core::sha256::sha256_hex(payload),
        now_ms,
    }
}

/// The lines of the log of the reconciliations.
pub fn reconcile_log(store: &Store) -> Result<Vec<ReconcileLogEntry>, TaseError> {
    crate::read_all(
        store.connection(),
        "SELECT reconcile_id, started_at_ms, expired_sessions, expired_leases, requeued_jobs,
                confirmed_conflicts, resolved_conflicts, detail
           FROM local_reconcile_log ORDER BY reconcile_id",
        rusqlite::params![],
        |row| {
            Ok(ReconcileLogEntry {
                reconcile_id: row.get(0).map_err(crate::sqlite::map_sqlite)?,
                started_at_ms: row.get::<_, i64>(1).map_err(crate::sqlite::map_sqlite)? as u64,
                expired_sessions: row.get::<_, i64>(2).map_err(crate::sqlite::map_sqlite)? as u32,
                expired_leases: row.get::<_, i64>(3).map_err(crate::sqlite::map_sqlite)? as u32,
                requeued_jobs: row.get::<_, i64>(4).map_err(crate::sqlite::map_sqlite)? as u32,
                confirmed_conflicts: row.get::<_, i64>(5).map_err(crate::sqlite::map_sqlite)? as u32,
                resolved_conflicts: row.get::<_, i64>(6).map_err(crate::sqlite::map_sqlite)? as u32,
                detail: row.get(7).map_err(crate::sqlite::map_sqlite)?,
            })
        },
    )
}

/// One line of the log of the reconciliations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconcileLogEntry {
    /// The number of the run.
    pub reconcile_id: i64,
    /// The time the run started at.
    pub started_at_ms: u64,
    /// The sessions that were given up on.
    pub expired_sessions: u32,
    /// The leases that ran out of time.
    pub expired_leases: u32,
    /// The jobs that went back to the queue.
    pub requeued_jobs: u32,
    /// The conflicts that were confirmed.
    pub confirmed_conflicts: u32,
    /// The conflicts that were closed.
    pub resolved_conflicts: u32,
    /// The detail of the run, as JSON.
    pub detail: String,
}

/// The row image of a conflict of a file, for the callers that build a state.
#[must_use]
pub fn conflict_image(row: &crate::FileConflict) -> pime_core::json::Json {
    crate::file_conflict_to_json(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{new_session, new_store, temp_dir};
    use tase_contract::{AgentId, JobId, SessionId, WorkspaceId};

    /// A store of the disk, migrated, with a session and a job.
    fn store_with_job(dir: &std::path::Path) -> (Store, JobId) {
        let mut store = new_store(dir);
        let session = new_session(&mut store, "sess_1", "agent_1", 100);
        let job = store
            .submit_job(crate::NewJob {
                job_type: "apply".into(),
                workspace_id: WorkspaceId::new("ws_1").unwrap(),
                agent_id: AgentId::new("agent_1").unwrap(),
                session_id: session,
                idempotency_key: "key_1".into(),
                request_digest: "dig_1".into(),
                base_snapshot_digest: "base_1".into(),
                priority: 5,
                now_ms: 100,
            })
            .expect("the job is registered");
        (store, job.job_id)
    }

    #[test]
    fn a_session_without_heartbeat_is_given_up_on() {
        let dir = temp_dir("reconcile-session");
        let (mut store, _) = store_with_job(&dir);
        // the last heartbeat was at 100 and the timeout is 30 000: the session is old at 40 000
        let report = reconcile(&mut store, 40_000).expect("the reconciliation runs");
        assert_eq!(report.expired_sessions, 1);
        let session = store
            .get_session(&SessionId::new("sess_1").unwrap())
            .expect("the session is read")
            .expect("the session is there");
        assert_eq!(session.state, SessionState::Expired);
        crate::test_support::remove_dir(&dir);
    }

    #[test]
    fn a_session_with_a_heartbeat_is_kept() {
        let dir = temp_dir("reconcile-session-alive");
        let (mut store, _) = store_with_job(&dir);
        store
            .heartbeat(&SessionId::new("sess_1").unwrap(), 39_000)
            .expect("the heartbeat is written");
        let report = reconcile(&mut store, 40_000).expect("the reconciliation runs");
        assert_eq!(report.expired_sessions, 0);
        let session = store
            .get_session(&SessionId::new("sess_1").unwrap())
            .expect("the session is read")
            .expect("the session is there");
        assert_eq!(session.state, SessionState::Connected);
        crate::test_support::remove_dir(&dir);
    }

    #[test]
    fn a_lease_that_ran_out_is_taken_back_and_the_job_goes_back_to_the_queue() {
        let dir = temp_dir("reconcile-lease");
        let (mut store, job_id) = store_with_job(&dir);
        let lease = store
            .lease_job(&job_id, "worker_1", 1_000, 200)
            .expect("the job is leased");
        let job = store.get_job(&job_id).expect("the job is read").unwrap();
        assert_eq!(job.status, JobStatus::Running);
        assert_eq!(job.attempt, 1);

        let report = reconcile(&mut store, 5_000).expect("the reconciliation runs");
        assert_eq!(report.expired_leases, 1);
        assert_eq!(report.requeued_jobs, 1);

        let lease = store.get_lease(&lease.lease_id).expect("the lease is read").unwrap();
        assert_eq!(lease.state, LeaseState::Expired);
        let job = store.get_job(&job_id).expect("the job is read").unwrap();
        assert_eq!(job.status, JobStatus::Queued);
        assert_eq!(job.worker_id, None);
        crate::test_support::remove_dir(&dir);
    }

    #[test]
    fn a_job_with_a_live_lease_is_left_alone() {
        let dir = temp_dir("reconcile-lease-alive");
        let (mut store, job_id) = store_with_job(&dir);
        store
            .lease_job(&job_id, "worker_1", 100_000, 200)
            .expect("the job is leased");
        let report = reconcile(&mut store, 5_000).expect("the reconciliation runs");
        assert_eq!(report.expired_leases, 0);
        assert_eq!(report.requeued_jobs, 0);
        let job = store.get_job(&job_id).expect("the job is read").unwrap();
        assert_eq!(job.status, JobStatus::Running);
        crate::test_support::remove_dir(&dir);
    }

    #[test]
    fn a_conflict_is_confirmed_and_its_job_becomes_a_conflict() {
        let dir = temp_dir("reconcile-conflict");
        let (mut store, job_id) = store_with_job(&dir);
        let workspace = WorkspaceId::new("ws_1").unwrap();
        let agent = AgentId::new("agent_1").unwrap();
        let session = SessionId::new("sess_1").unwrap();
        store
            .record_file_transaction(crate::NewFileTransaction {
                workspace_id: workspace.clone(),
                normalized_path: "src/main.rs".into(),
                expected_generation: Some(0),
                content_digest: "dig_a".into(),
                agent_id: agent.clone(),
                session_id: session.clone(),
                job_id: Some(job_id.clone()),
                now_ms: 300,
            })
            .expect("the first write works");
        let error = store
            .record_file_transaction(crate::NewFileTransaction {
                workspace_id: workspace.clone(),
                normalized_path: "src/main.rs".into(),
                expected_generation: Some(0),
                content_digest: "dig_b".into(),
                agent_id: agent.clone(),
                session_id: session.clone(),
                job_id: Some(job_id.clone()),
                now_ms: 400,
            })
            .expect_err("the second write must meet a conflict");
        assert_eq!(error.code, ErrorCode::SymbolConflict);

        let report = reconcile(&mut store, 5_000).expect("the reconciliation runs");
        assert_eq!(report.confirmed_conflicts, 1);
        assert_eq!(report.resolved_conflicts, 0);

        let conflicts = store.list_file_conflicts().expect("the conflicts are read");
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].state, ConflictState::Confirmed);
        assert_eq!(conflicts[0].actual_generation, 1);

        let job = store.get_job(&job_id).expect("the job is read").unwrap();
        assert_eq!(job.status, JobStatus::Conflict);
        assert_eq!(job.error_code, Some(ErrorCode::SymbolConflict));
        crate::test_support::remove_dir(&dir);
    }

    #[test]
    fn a_conflict_that_is_gone_again_is_closed() {
        let dir = temp_dir("reconcile-conflict-resolved");
        let (mut store, _) = store_with_job(&dir);
        let workspace = WorkspaceId::new("ws_1").unwrap();
        let agent = AgentId::new("agent_1").unwrap();
        let session = SessionId::new("sess_1").unwrap();
        store
            .record_file_transaction(crate::NewFileTransaction {
                workspace_id: workspace.clone(),
                normalized_path: "src/main.rs".into(),
                expected_generation: Some(0),
                content_digest: "dig_a".into(),
                agent_id: agent.clone(),
                session_id: session.clone(),
                job_id: None,
                now_ms: 300,
            })
            .expect("the first write works");
        let _ = store.record_file_transaction(crate::NewFileTransaction {
            workspace_id: workspace.clone(),
            normalized_path: "src/main.rs".into(),
            expected_generation: Some(0),
            content_digest: "dig_b".into(),
            agent_id: agent.clone(),
            session_id: session.clone(),
            job_id: None,
            now_ms: 400,
        });
        // the file comes back to the generation the writer expected
        crate::upsert_file_generation(
            store.connection(),
            &crate::FileGeneration {
                workspace_id: workspace.clone(),
                normalized_path: "src/main.rs".into(),
                generation: 0,
                content_digest: String::new(),
                updated_at_ms: 500,
            },
        )
        .expect("the generation is written");

        let report = reconcile(&mut store, 5_000).expect("the reconciliation runs");
        assert_eq!(report.resolved_conflicts, 1);
        assert_eq!(report.confirmed_conflicts, 0);
        let conflicts = store.list_file_conflicts().expect("the conflicts are read");
        assert_eq!(conflicts[0].state, ConflictState::Resolved);
        crate::test_support::remove_dir(&dir);
    }

    #[test]
    fn the_reconciliation_writes_an_audit_trail_and_a_log() {
        let dir = temp_dir("reconcile-audit");
        let (mut store, job_id) = store_with_job(&dir);
        store
            .lease_job(&job_id, "worker_1", 1_000, 200)
            .expect("the job is leased");
        let report = reconcile(&mut store, 40_000).expect("the reconciliation runs");
        assert!(report.audit_events >= 2);

        let events = store.list_audit_events().expect("the events are read");
        assert!(events.iter().any(|event| event.event_type == EVENT_ORPHANED));
        assert!(events.iter().any(|event| event.event_type == EVENT_RECOVERED));

        let log = reconcile_log(&store).expect("the log is read");
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].expired_sessions, 1);
        assert_eq!(log[0].expired_leases, 1);
        assert_eq!(log[0].requeued_jobs, 1);
        crate::test_support::remove_dir(&dir);
    }

    #[test]
    fn the_reconciliation_may_be_run_again_without_a_change() {
        let dir = temp_dir("reconcile-twice");
        let (mut store, job_id) = store_with_job(&dir);
        store
            .lease_job(&job_id, "worker_1", 1_000, 200)
            .expect("the job is leased");
        let first = reconcile(&mut store, 5_000).expect("the reconciliation runs");
        assert_eq!(first.expired_leases, 1);
        let second = reconcile(&mut store, 5_000).expect("the reconciliation runs again");
        assert_eq!(second.expired_leases, 0);
        assert_eq!(second.expired_sessions, 0);
        assert_eq!(second.requeued_jobs, 0);
        crate::test_support::remove_dir(&dir);
    }

    #[test]
    fn a_job_in_a_terminal_state_is_not_touched_by_the_reconciliation() {
        let dir = temp_dir("reconcile-terminal");
        let (mut store, job_id) = store_with_job(&dir);
        store
            .finish_job(&job_id, JobStatus::Committed, Some("dig".into()), None, 300)
            .expect("the job is finished");
        store
            .lease_job(&job_id, "worker_1", 1_000, 200)
            .expect_err("a job in a terminal state may not be leased");
        let report = reconcile(&mut store, 5_000).expect("the reconciliation runs");
        assert_eq!(report.requeued_jobs, 0);
        let job = store.get_job(&job_id).expect("the job is read").unwrap();
        assert_eq!(job.status, JobStatus::Committed);
        crate::test_support::remove_dir(&dir);
    }
}
