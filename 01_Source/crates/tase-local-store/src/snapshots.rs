//! The snapshots of the state of the store and the restore from them (detailed design §17.6).
//!
//! The store keeps a **journal**: every change writes the row image of what changed, in the same transaction as the
//! change itself. A **snapshot** is the state of the store at one moment, together with the number of the last entry
//! of the journal. The state of any later moment is the snapshot plus the entries of the journal that came after it,
//! so the state of the store can always be rebuilt.
//!
//! The restore empties the tables of the state, puts the rows of the snapshot back and replays the entries of the
//! journal that came after the snapshot. The outbox, the audit trail and the journal itself are not part of the
//! state: they are the streams that tell what happened, not what is true now.

use pime_core::json::{Json, Limits, parse};
use pime_core::sha256::sha256_hex;
use rusqlite::Connection;
use tase_contract::TaseError;

use crate::{
    AgentRegistration, FileConflict, FileGeneration, IdempotencyRecord, Job, Lease, Store,
    file_conflict_from_json, file_conflict_to_json, file_generation_from_json, file_generation_to_json,
    idempotency_from_json, idempotency_to_json, job_from_json, job_to_json, lease_from_json, lease_to_json,
    session_from_json, session_to_json,
};

/// What is known about a snapshot without reading its payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotInfo {
    /// The id of the snapshot.
    pub snapshot_id: String,
    /// The number of the last entry of the journal the snapshot holds.
    pub journal_seq: i64,
    /// The digest of the payload of the snapshot.
    pub digest: String,
    /// The time the snapshot was taken.
    pub taken_at_ms: u64,
}

/// A snapshot with its payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// The id of the snapshot.
    pub snapshot_id: String,
    /// The number of the last entry of the journal the snapshot holds.
    pub journal_seq: i64,
    /// The digest of the payload of the snapshot.
    pub digest: String,
    /// The payload of the snapshot, as JSON.
    pub payload: String,
    /// The time the snapshot was taken.
    pub taken_at_ms: u64,
}

/// What a restore did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RestoreReport {
    /// The id of the snapshot the state was rebuilt from.
    pub snapshot_id: String,
    /// The sessions that were put back.
    pub sessions: usize,
    /// The jobs that were put back.
    pub jobs: usize,
    /// The leases that were put back.
    pub leases: usize,
    /// The idempotency records that were put back.
    pub idempotency: usize,
    /// The generations of the files that were put back.
    pub file_generations: usize,
    /// The conflicts of the files that were put back.
    pub file_conflicts: usize,
    /// The entries of the journal that were replayed after the snapshot.
    pub journal_entries_replayed: usize,
}

/// The state of the store: the rows of the tables that hold what is true now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// The sessions of the agents.
    pub sessions: Vec<AgentRegistration>,
    /// The jobs.
    pub jobs: Vec<Job>,
    /// The leases.
    pub leases: Vec<Lease>,
    /// The idempotency records.
    pub idempotency: Vec<IdempotencyRecord>,
    /// The generations of the files.
    pub file_generations: Vec<FileGeneration>,
    /// The conflicts of the files.
    pub file_conflicts: Vec<FileConflict>,
}

impl State {
    /// Reads the state of the store.
    pub fn load(store: &Store) -> Result<State, TaseError> {
        Self::load_from(store.connection())
    }

    /// Reads the state from a connection.
    pub fn load_from(conn: &Connection) -> Result<State, TaseError> {
        Ok(State {
            sessions: crate::list_sessions(conn)?,
            jobs: crate::list_jobs(conn)?,
            leases: crate::list_leases(conn)?,
            idempotency: crate::list_idempotency(conn)?,
            file_generations: crate::read_all(
                conn,
                "SELECT workspace_id, normalized_path, buffer_generation, file_content_digest, updated_at_ms
                   FROM local_workspace_versions ORDER BY workspace_id, normalized_path",
                rusqlite::params![],
                crate::read_file_generation,
            )?,
            file_conflicts: crate::list_file_conflicts(conn)?,
        })
    }

    /// The state as JSON.
    #[must_use]
    pub fn to_json(&self) -> Json {
        Json::obj(vec![
            (
                "sessions",
                Json::Arr(self.sessions.iter().map(session_to_json).collect()),
            ),
            (
                "jobs",
                Json::Arr(self.jobs.iter().map(job_to_json).collect()),
            ),
            (
                "leases",
                Json::Arr(self.leases.iter().map(lease_to_json).collect()),
            ),
            (
                "idempotency",
                Json::Arr(self.idempotency.iter().map(idempotency_to_json).collect()),
            ),
            (
                "file_generations",
                Json::Arr(
                    self.file_generations
                        .iter()
                        .map(file_generation_to_json)
                        .collect(),
                ),
            ),
            (
                "file_conflicts",
                Json::Arr(self.file_conflicts.iter().map(file_conflict_to_json).collect()),
            ),
        ])
    }

    /// The state that a JSON holds.
    pub fn from_json(value: &Json) -> Result<State, TaseError> {
        Ok(State {
            sessions: array(value, "sessions")?
                .iter()
                .map(session_from_json)
                .collect::<Result<Vec<_>, _>>()?,
            jobs: array(value, "jobs")?
                .iter()
                .map(job_from_json)
                .collect::<Result<Vec<_>, _>>()?,
            leases: array(value, "leases")?
                .iter()
                .map(lease_from_json)
                .collect::<Result<Vec<_>, _>>()?,
            idempotency: array(value, "idempotency")?
                .iter()
                .map(idempotency_from_json)
                .collect::<Result<Vec<_>, _>>()?,
            file_generations: array(value, "file_generations")?
                .iter()
                .map(file_generation_from_json)
                .collect::<Result<Vec<_>, _>>()?,
            file_conflicts: array(value, "file_conflicts")?
                .iter()
                .map(file_conflict_from_json)
                .collect::<Result<Vec<_>, _>>()?,
        })
    }

    /// The number of rows of the state.
    #[must_use]
    pub fn len(&self) -> usize {
        self.sessions.len()
            + self.jobs.len()
            + self.leases.len()
            + self.idempotency.len()
            + self.file_generations.len()
            + self.file_conflicts.len()
    }

    /// Whether the state holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Takes a snapshot of the state of the store.
///
/// The snapshot holds the state of the moment and the number of the last entry of the journal: the state of any
/// later moment is this snapshot plus the entries of the journal that came after it.
pub fn write_snapshot(
    store: &mut Store,
    snapshot_id: &str,
    now_ms: u64,
) -> Result<SnapshotInfo, TaseError> {
    let state = State::load(store)?;
    let payload = state.to_json().to_json_string();
    let digest = sha256_hex(&payload);
    let journal_seq = current_journal_seq(store.connection())?;
    let tx = store.transaction()?;
    tx.execute(
        "INSERT INTO local_snapshots (snapshot_id, journal_seq, digest, payload, taken_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(snapshot_id) DO UPDATE SET
           journal_seq = ?2, digest = ?3, payload = ?4, taken_at_ms = ?5",
        rusqlite::params![snapshot_id, journal_seq, digest, payload, now_ms as i64],
    )?;
    tx.commit()?;
    Ok(SnapshotInfo {
        snapshot_id: snapshot_id.to_string(),
        journal_seq,
        digest,
        taken_at_ms: now_ms,
    })
}

/// Reads a snapshot, or `None` when there is none.
pub fn read_snapshot(store: &Store, snapshot_id: &str) -> Result<Option<Snapshot>, TaseError> {
    crate::read_first(
        store.connection(),
        "SELECT snapshot_id, journal_seq, digest, payload, taken_at_ms
           FROM local_snapshots WHERE snapshot_id = ?1",
        rusqlite::params![snapshot_id],
        |row| {
            Ok(Snapshot {
                snapshot_id: row.get(0).map_err(crate::sqlite::map_sqlite)?,
                journal_seq: row.get(1).map_err(crate::sqlite::map_sqlite)?,
                digest: row.get(2).map_err(crate::sqlite::map_sqlite)?,
                payload: row.get(3).map_err(crate::sqlite::map_sqlite)?,
                taken_at_ms: row.get::<_, i64>(4).map_err(crate::sqlite::map_sqlite)? as u64,
            })
        },
    )
}

/// Every snapshot, without its payload.
pub fn list_snapshots(store: &Store) -> Result<Vec<SnapshotInfo>, TaseError> {
    crate::read_all(
        store.connection(),
        "SELECT snapshot_id, journal_seq, digest, taken_at_ms FROM local_snapshots ORDER BY snapshot_id",
        rusqlite::params![],
        |row| {
            Ok(SnapshotInfo {
                snapshot_id: row.get(0).map_err(crate::sqlite::map_sqlite)?,
                journal_seq: row.get(1).map_err(crate::sqlite::map_sqlite)?,
                digest: row.get(2).map_err(crate::sqlite::map_sqlite)?,
                taken_at_ms: row.get::<_, i64>(3).map_err(crate::sqlite::map_sqlite)? as u64,
            })
        },
    )
}

/// Empties the tables of the state of the store, and the outbox that points at them.
///
/// The journal, the audit trail and the outbox are the streams of what happened. The outbox is emptied with the
/// state, because its events point at the jobs of the state and may not outlive them. The restore uses this to put
/// the state back.
pub fn clear_live_state(store: &mut Store) -> Result<(), TaseError> {
    let tx = store.transaction()?;
    // the events of the outbox point at the jobs: they go first
    tx.execute("DELETE FROM local_outbox", rusqlite::params![])?;
    for table in crate::migrations::STATE_TABLES {
        tx.execute(&format!("DELETE FROM {table}"), rusqlite::params![])?;
    }
    tx.commit()?;
    Ok(())
}

/// Rebuilds the state of the store from a snapshot and the entries of the journal that came after it.
///
/// The state is emptied, the rows of the snapshot are put back and the entries of the journal are replayed, one
/// after the other. The counters of the store are recomputed, so the ids that come after the restore never collide
/// with the ids that were there before.
pub fn restore_from_snapshot(
    store: &mut Store,
    snapshot_id: &str,
) -> Result<RestoreReport, TaseError> {
    let snapshot = read_snapshot(store, snapshot_id)?.ok_or_else(|| {
        TaseError::not_found(format!("there is no snapshot {snapshot_id}"))
    })?;
    let state = State::from_json(
        &parse(&snapshot.payload, Limits::DEFAULT).map_err(|error| {
            TaseError::internal(format!("the payload of the snapshot is not JSON: {error}"))
        })?,
    )?;
    let tx = store.transaction()?;
    // The events of the outbox point at the jobs of the state: the check of the foreign keys waits for the end of
    // the transaction, because the jobs are put back before it ends.
    tx.execute_batch("PRAGMA defer_foreign_keys = ON;")
        .map_err(crate::sqlite::map_sqlite)?;
    for table in crate::migrations::STATE_TABLES {
        tx.execute(&format!("DELETE FROM {table}"), rusqlite::params![])?;
    }
    for session in &state.sessions {
        crate::upsert_session(&tx, session)?;
    }
    for job in &state.jobs {
        crate::upsert_job(&tx, job)?;
    }
    for lease in &state.leases {
        crate::upsert_lease(&tx, lease)?;
    }
    for record in &state.idempotency {
        crate::upsert_idempotency(&tx, record)?;
    }
    for generation in &state.file_generations {
        crate::upsert_file_generation(&tx, generation)?;
    }
    for conflict in &state.file_conflicts {
        crate::insert_file_conflict(&tx, conflict)?;
    }
    let mut replayed = 0;
    for (kind, payload) in journal_entries_after(&tx, snapshot.journal_seq)? {
        apply_journal_entry(&tx, &kind, &payload)?;
        replayed += 1;
    }
    let final_state = State::load_from(&tx)?;
    set_counters(&tx, &final_state)?;
    tx.commit()?;
    Ok(RestoreReport {
        snapshot_id: snapshot.snapshot_id,
        sessions: final_state.sessions.len(),
        jobs: final_state.jobs.len(),
        leases: final_state.leases.len(),
        idempotency: final_state.idempotency.len(),
        file_generations: final_state.file_generations.len(),
        file_conflicts: final_state.file_conflicts.len(),
        journal_entries_replayed: replayed,
    })
}

/// The entries of the journal that came after a number.
fn journal_entries_after(
    conn: &Connection,
    journal_seq: i64,
) -> Result<Vec<(String, String)>, TaseError> {
    crate::read_all(
        conn,
        "SELECT kind, payload FROM local_journal WHERE seq > ?1 ORDER BY seq",
        rusqlite::params![journal_seq],
        |row| {
            Ok((
                row.get::<_, String>(0).map_err(crate::sqlite::map_sqlite)?,
                row.get::<_, String>(1).map_err(crate::sqlite::map_sqlite)?,
            ))
        },
    )
}

/// Puts one row image of the journal back into the tables.
fn apply_journal_entry(
    conn: &Connection,
    kind: &str,
    payload: &str,
) -> Result<(), TaseError> {
    let value = parse(payload, Limits::DEFAULT).map_err(|error| {
        TaseError::internal(format!("the payload of the journal is not JSON: {error}"))
    })?;
    match kind {
        crate::KIND_SESSION => crate::upsert_session(conn, &session_from_json(&value)?)?,
        crate::KIND_JOB => crate::upsert_job(conn, &job_from_json(&value)?)?,
        crate::KIND_LEASE => crate::upsert_lease(conn, &lease_from_json(&value)?)?,
        crate::KIND_IDEMPOTENCY => {
            crate::upsert_idempotency(conn, &idempotency_from_json(&value)?)?
        }
        crate::KIND_FILE_GENERATION => {
            crate::upsert_file_generation(conn, &file_generation_from_json(&value)?)?
        }
        crate::KIND_FILE_CONFLICT => {
            crate::insert_file_conflict(conn, &file_conflict_from_json(&value)?)?
        }
        other => {
            return Err(TaseError::internal(format!(
                "the journal holds an unknown kind {other:?}"
            )));
        }
    }
    Ok(())
}

/// The number of the last entry of the journal.
fn current_journal_seq(conn: &Connection) -> Result<i64, TaseError> {
    conn.query_row(
        "SELECT COALESCE(MAX(seq), 0) FROM local_journal",
        rusqlite::params![],
        |row| row.get(0),
    )
    .map_err(crate::sqlite::map_sqlite)
}

/// Sets the counters of the store, so that the ids that come after a restore never collide.
fn set_counters(conn: &Connection, state: &State) -> Result<(), TaseError> {
    let job_id = max_number(state.jobs.iter().map(|job| job.job_id.as_str()), "job_");
    let lease_id = max_number(state.leases.iter().map(|lease| lease.lease_id.as_str()), "lease_");
    let conflict_id = state
        .file_conflicts
        .iter()
        .map(|conflict| conflict.conflict_id)
        .max()
        .unwrap_or(0);
    let fencing_token = state
        .leases
        .iter()
        .map(|lease| lease.fencing_token)
        .max()
        .unwrap_or(0);
    set_counter(conn, "job_id", job_id)?;
    set_counter(conn, "lease_id", lease_id)?;
    set_counter(conn, "conflict_id", conflict_id)?;
    set_counter(conn, "fencing_token", fencing_token)?;
    Ok(())
}

/// The biggest number of the ids that begin with a prefix.
fn max_number<'a>(ids: impl Iterator<Item = &'a str>, prefix: &str) -> u64 {
    ids.filter_map(|id| id.strip_prefix(prefix))
        .filter_map(|rest| rest.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
}

/// Sets one counter of the store.
fn set_counter(conn: &Connection, name: &str, value: u64) -> Result<(), TaseError> {
    conn.execute(
        "INSERT INTO local_counters (name, value) VALUES (?1, ?2)
         ON CONFLICT(name) DO UPDATE SET value = ?2",
        rusqlite::params![name, value as i64],
    )
    .map_err(crate::sqlite::map_sqlite)?;
    Ok(())
}

/// The array of a field of a JSON object.
fn array<'a>(value: &'a Json, key: &str) -> Result<&'a Vec<Json>, TaseError> {
    crate::json_field(value, key)?
        .as_array()
        .ok_or_else(|| TaseError::internal(format!("the field {key:?} is not an array")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{new_session, new_store, temp_dir};
    use tase_contract::{AgentId, JobId, SessionId, WorkspaceId};

    /// A store of the disk with a session and a job.
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
    fn a_snapshot_holds_the_state_of_the_moment() {
        let dir = temp_dir("snapshot-state");
        let (mut store, _) = store_with_job(&dir);
        let info = write_snapshot(&mut store, "snap_1", 500).expect("the snapshot is written");
        assert_eq!(info.snapshot_id, "snap_1");
        assert_eq!(info.taken_at_ms, 500);
        assert!(info.journal_seq > 0);
        assert!(!info.digest.is_empty());

        let snapshot = read_snapshot(&store, "snap_1")
            .expect("the snapshot is read")
            .expect("the snapshot is there");
        assert_eq!(snapshot.digest, info.digest);
        assert_eq!(snapshot.digest, sha256_hex(&snapshot.payload));
        let state = State::from_json(
            &parse(&snapshot.payload, Limits::DEFAULT).expect("the payload is JSON"),
        )
        .expect("the state is read");
        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.jobs.len(), 1);
        assert_eq!(state.idempotency.len(), 1);
        crate::test_support::remove_dir(&dir);
    }

    #[test]
    fn the_snapshots_are_listed() {
        let dir = temp_dir("snapshot-list");
        let (mut store, _) = store_with_job(&dir);
        write_snapshot(&mut store, "snap_1", 500).expect("the snapshot is written");
        write_snapshot(&mut store, "snap_2", 600).expect("the snapshot is written");
        let list = list_snapshots(&store).expect("the snapshots are listed");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].snapshot_id, "snap_1");
        assert_eq!(list[1].snapshot_id, "snap_2");
        crate::test_support::remove_dir(&dir);
    }

    #[test]
    fn an_unknown_snapshot_is_not_invented() {
        let dir = temp_dir("snapshot-unknown");
        let (store, _) = store_with_job(&dir);
        assert!(
            read_snapshot(&store, "nope")
                .expect("the snapshot is looked up")
                .is_none()
        );
        crate::test_support::remove_dir(&dir);
    }

    #[test]
    fn the_state_is_the_same_after_a_restore() {
        let dir = temp_dir("snapshot-restore");
        let (mut store, job_id) = store_with_job(&dir);
        write_snapshot(&mut store, "snap_1", 500).expect("the snapshot is written");
        let before = State::load(&store).expect("the state is read");

        clear_live_state(&mut store).expect("the state is emptied");
        let emptied = State::load(&store).expect("the state is read");
        assert!(emptied.is_empty());

        let report = restore_from_snapshot(&mut store, "snap_1").expect("the state is rebuilt");
        assert_eq!(report.snapshot_id, "snap_1");
        assert_eq!(report.sessions, 1);
        assert_eq!(report.jobs, 1);

        let after = State::load(&store).expect("the state is read");
        assert_eq!(after, before);
        let job = store.get_job(&job_id).expect("the job is read").unwrap();
        assert_eq!(job.status, crate::JobStatus::Queued);
        crate::test_support::remove_dir(&dir);
    }

    #[test]
    fn the_changes_that_came_after_the_snapshot_are_played_back() {
        let dir = temp_dir("snapshot-replay");
        let (mut store, job_id) = store_with_job(&dir);
        write_snapshot(&mut store, "snap_1", 500).expect("the snapshot is written");

        // after the snapshot: a lease, a write of a file and a new session
        let lease = store
            .lease_job(&job_id, "worker_1", 10_000, 600)
            .expect("the job is leased");
        let session = new_session(&mut store, "sess_2", "agent_2", 700);
        store
            .record_file_transaction(crate::NewFileTransaction {
                workspace_id: WorkspaceId::new("ws_1").unwrap(),
                normalized_path: "src/main.rs".into(),
                expected_generation: Some(0),
                content_digest: "dig_a".into(),
                agent_id: AgentId::new("agent_1").unwrap(),
                session_id: SessionId::new("sess_1").unwrap(),
                job_id: Some(job_id.clone()),
                now_ms: 800,
            })
            .expect("the file is written");
        let before = State::load(&store).expect("the state is read");

        clear_live_state(&mut store).expect("the state is emptied");
        let report = restore_from_snapshot(&mut store, "snap_1").expect("the state is rebuilt");
        assert!(report.journal_entries_replayed >= 3, "the entries must be played back");

        let after = State::load(&store).expect("the state is read");
        assert_eq!(after, before);
        assert_eq!(after.sessions.len(), 2);
        assert_eq!(after.leases.len(), 1);
        assert_eq!(after.file_generations.len(), 1);
        let lease_after = store.get_lease(&lease.lease_id).expect("the lease is read").unwrap();
        assert_eq!(lease_after.fencing_token, lease.fencing_token);
        assert_eq!(lease_after.state, crate::LeaseState::Held);
        let _ = session;
        crate::test_support::remove_dir(&dir);
    }

    #[test]
    fn the_counters_of_the_store_survive_a_restore() {
        let dir = temp_dir("snapshot-counters");
        let (mut store, job_id) = store_with_job(&dir);
        write_snapshot(&mut store, "snap_1", 500).expect("the snapshot is written");
        clear_live_state(&mut store).expect("the state is emptied");
        restore_from_snapshot(&mut store, "snap_1").expect("the state is rebuilt");

        // a new job must not take the id of the old one
        let session = new_session(&mut store, "sess_2", "agent_2", 600);
        let job = store
            .submit_job(crate::NewJob {
                job_type: "apply".into(),
                workspace_id: WorkspaceId::new("ws_1").unwrap(),
                agent_id: AgentId::new("agent_2").unwrap(),
                session_id: session,
                idempotency_key: "key_2".into(),
                request_digest: "dig_2".into(),
                base_snapshot_digest: "base_1".into(),
                priority: 5,
                now_ms: 600,
            })
            .expect("the job is registered");
        assert_ne!(job.job_id.as_str(), job_id.as_str());
        crate::test_support::remove_dir(&dir);
    }

    #[test]
    fn a_restore_of_an_unknown_snapshot_is_not_found() {
        let dir = temp_dir("snapshot-missing");
        let (mut store, _) = store_with_job(&dir);
        let error = restore_from_snapshot(&mut store, "nope").expect_err("the restore must fail");
        assert_eq!(error.code, tase_contract::ErrorCode::NotFound);
        crate::test_support::remove_dir(&dir);
    }

    #[test]
    fn an_empty_store_may_be_snapshotted() {
        let dir = temp_dir("snapshot-empty");
        let mut store = new_store(&dir);
        let info = write_snapshot(&mut store, "snap_1", 500).expect("the snapshot is written");
        assert_eq!(info.journal_seq, 0);
        let report = restore_from_snapshot(&mut store, "snap_1").expect("the state is rebuilt");
        assert_eq!(report.journal_entries_replayed, 0);
        assert!(State::load(&store).expect("the state is read").is_empty());
        crate::test_support::remove_dir(&dir);
    }
}
