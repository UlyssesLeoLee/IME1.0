//! The migrations of the local store: a numbered, forward-only list of steps (detailed design §18.1).
//!
//! A step is applied once and only once. The table `local_schema_migrations` holds the version and the name of every
//! step that ran, and `PRAGMA user_version` holds the version of the schema, so a store that is opened again does
//! not run a step twice and a store that is opened by an older build does not break.
//!
//! The steps only go forward: a step never undoes what an earlier step did. The design book asks for the schema of
//! §18.1; the steps below are that schema plus the columns the local hub needs (the type and the priority of a job,
//! the provider and the capabilities of an agent, the generation of a file).

use tase_contract::TaseError;

use crate::sqlite::{map_sqlite, Store};

/// One step of the schema.
#[derive(Clone, Copy, Debug)]
pub struct Migration {
    /// The version of the schema after the step. The versions start at 1 and grow by one.
    pub version: u32,
    /// The name of the step, for the table of the applied steps.
    pub name: &'static str,
    /// The SQL of the step.
    pub sql: &'static str,
}

/// The first step: the tables of the design book §18.1, with the columns the local hub needs.
const BASE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS local_counters (
  name  TEXT PRIMARY KEY,
  value INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS local_agent_sessions (
  session_id          TEXT PRIMARY KEY,
  agent_id            TEXT NOT NULL,
  provider            TEXT NOT NULL DEFAULT '',
  instance_id         TEXT NOT NULL DEFAULT '',
  workspace_id        TEXT NOT NULL,
  owner_logon_sid_hash TEXT NOT NULL,
  granted_scopes_json TEXT NOT NULL DEFAULT '[]',
  capabilities_json   TEXT NOT NULL DEFAULT '[]',
  state               TEXT NOT NULL CHECK (state IN ('CONNECTED','DISCONNECTED','EXPIRED')),
  lease_epoch         INTEGER NOT NULL DEFAULT 0,
  heartbeat_at_ms     INTEGER NOT NULL,
  created_at_ms       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_local_sessions_agent
  ON local_agent_sessions(agent_id, state);
CREATE INDEX IF NOT EXISTS idx_local_sessions_workspace
  ON local_agent_sessions(workspace_id, state);

CREATE TABLE IF NOT EXISTS local_jobs (
  job_id               TEXT PRIMARY KEY,
  job_type             TEXT NOT NULL,
  workspace_id         TEXT NOT NULL,
  agent_id             TEXT NOT NULL,
  session_id           TEXT NOT NULL,
  idempotency_key      TEXT NOT NULL,
  request_digest       TEXT NOT NULL,
  base_snapshot_digest TEXT NOT NULL DEFAULT '',
  status               TEXT NOT NULL,
  attempt              INTEGER NOT NULL DEFAULT 0,
  lease_epoch          INTEGER NOT NULL DEFAULT 0,
  worker_id            TEXT,
  result_digest        TEXT,
  commit_intent_digest TEXT,
  committed_ref        TEXT,
  committed_oid        TEXT,
  error_code           TEXT,
  created_at_ms        INTEGER NOT NULL,
  updated_at_ms        INTEGER NOT NULL,
  UNIQUE (workspace_id, agent_id, idempotency_key),
  FOREIGN KEY (session_id) REFERENCES local_agent_sessions(session_id)
);
CREATE INDEX IF NOT EXISTS idx_local_jobs_state
  ON local_jobs(status, updated_at_ms);
CREATE INDEX IF NOT EXISTS idx_local_jobs_session
  ON local_jobs(session_id, status);

CREATE TABLE IF NOT EXISTS local_leases (
  lease_id      TEXT PRIMARY KEY,
  resource      TEXT NOT NULL,
  holder        TEXT NOT NULL,
  session_id    TEXT,
  fencing_token INTEGER NOT NULL,
  acquired_at_ms INTEGER NOT NULL,
  expires_at_ms  INTEGER NOT NULL,
  released_at_ms INTEGER,
  state          TEXT NOT NULL CHECK (state IN ('HELD','RELEASED','EXPIRED'))
);
-- At most one holder at a time: the index is the guarantee that a second claim of the same resource fails.
CREATE UNIQUE INDEX IF NOT EXISTS idx_local_leases_one_holder
  ON local_leases(resource) WHERE state = 'HELD';
CREATE INDEX IF NOT EXISTS idx_local_leases_holder
  ON local_leases(holder, state);

CREATE TABLE IF NOT EXISTS local_idempotency (
  scope            TEXT NOT NULL,
  idempotency_key  TEXT NOT NULL,
  request_digest   TEXT NOT NULL,
  response_digest  TEXT NOT NULL,
  job_id           TEXT NOT NULL,
  created_at_ms    INTEGER NOT NULL,
  PRIMARY KEY (scope, idempotency_key)
);

-- The generation of every file of a workspace (design §18.1, `local_workspace_versions`).
CREATE TABLE IF NOT EXISTS local_workspace_versions (
  workspace_id        TEXT NOT NULL,
  normalized_path     TEXT NOT NULL,
  buffer_generation   INTEGER NOT NULL DEFAULT 0,
  file_content_digest TEXT NOT NULL DEFAULT '',
  updated_at_ms       INTEGER NOT NULL,
  PRIMARY KEY (workspace_id, normalized_path)
);

CREATE TABLE IF NOT EXISTS local_file_conflicts (
  conflict_id        INTEGER PRIMARY KEY,
  workspace_id       TEXT NOT NULL,
  normalized_path    TEXT NOT NULL,
  expected_generation INTEGER NOT NULL,
  actual_generation   INTEGER NOT NULL,
  agent_id           TEXT NOT NULL,
  session_id         TEXT NOT NULL,
  job_id             TEXT,
  state              TEXT NOT NULL CHECK (state IN ('OPEN','CONFIRMED','RESOLVED')),
  detected_at_ms     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_local_file_conflicts_state
  ON local_file_conflicts(state, workspace_id);

CREATE TABLE IF NOT EXISTS local_outbox (
  event_id        INTEGER PRIMARY KEY AUTOINCREMENT,
  job_id          TEXT NOT NULL,
  payload_digest  TEXT NOT NULL,
  published_at_ms INTEGER,
  FOREIGN KEY (job_id) REFERENCES local_jobs(job_id)
);

CREATE TABLE IF NOT EXISTS local_audit_events (
  seq             INTEGER PRIMARY KEY AUTOINCREMENT,
  job_id          TEXT,
  agent_id        TEXT NOT NULL,
  session_id      TEXT NOT NULL,
  workspace_id    TEXT NOT NULL,
  event_type      TEXT NOT NULL,
  payload_digest  TEXT NOT NULL,
  created_at_ms   INTEGER NOT NULL
);

-- The journal is the row image of every change: it is what makes a snapshot plus the entries that came after it
-- enough to rebuild the state of the store.
CREATE TABLE IF NOT EXISTS local_journal (
  seq            INTEGER PRIMARY KEY AUTOINCREMENT,
  kind           TEXT NOT NULL,
  payload        TEXT NOT NULL,
  created_at_ms  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS local_snapshots (
  snapshot_id  TEXT PRIMARY KEY,
  journal_seq  INTEGER NOT NULL,
  digest       TEXT NOT NULL,
  payload      TEXT NOT NULL,
  taken_at_ms   INTEGER NOT NULL
);
"#;

/// The second step: the priority of a job, so that a queue may order the jobs of one agent.
const JOB_PRIORITY: &str = r#"
ALTER TABLE local_jobs ADD COLUMN priority INTEGER NOT NULL DEFAULT 5;
CREATE INDEX IF NOT EXISTS idx_local_jobs_priority
  ON local_jobs(priority, created_at_ms);
"#;

/// The third step: the log of the reconciliations, so that a recovery is written down and not only done.
const RECONCILE_LOG: &str = r#"
CREATE TABLE IF NOT EXISTS local_reconcile_log (
  reconcile_id       INTEGER PRIMARY KEY AUTOINCREMENT,
  started_at_ms      INTEGER NOT NULL,
  expired_sessions   INTEGER NOT NULL DEFAULT 0,
  expired_leases     INTEGER NOT NULL DEFAULT 0,
  requeued_jobs      INTEGER NOT NULL DEFAULT 0,
  confirmed_conflicts INTEGER NOT NULL DEFAULT 0,
  resolved_conflicts INTEGER NOT NULL DEFAULT 0,
  detail             TEXT NOT NULL DEFAULT '{}'
);
"#;

/// The steps of the schema, in the order they must run.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "base_schema",
        sql: BASE_SCHEMA,
    },
    Migration {
        version: 2,
        name: "job_priority",
        sql: JOB_PRIORITY,
    },
    Migration {
        version: 3,
        name: "reconcile_log",
        sql: RECONCILE_LOG,
    },
];

/// The version of the schema the crate expects.
#[must_use]
pub fn latest_version() -> u32 {
    MIGRATIONS
        .iter()
        .map(|migration| migration.version)
        .max()
        .unwrap_or(0)
}

/// The name of a version, or `None` when the version is not a step of the crate.
#[must_use]
pub fn migration_name(version: u32) -> Option<&'static str> {
    MIGRATIONS
        .iter()
        .find(|migration| migration.version == version)
        .map(|migration| migration.name)
}

/// Applies every step that did not run yet, and returns the versions that ran.
pub fn apply(store: &mut Store) -> Result<Vec<u32>, TaseError> {
    apply_up_to(store, latest_version())
}

/// Applies the steps up to `target` that did not run yet, and returns the versions that ran.
///
/// A step that already ran is skipped, so the call may be repeated as often as the caller likes: it changes
/// nothing and reports nothing the second time.
pub fn apply_up_to(store: &mut Store, target: u32) -> Result<Vec<u32>, TaseError> {
    ensure_bookkeeping(store)?;
    let applied = applied_versions(store)?;
    let mut ran = Vec::new();
    for migration in MIGRATIONS {
        if migration.version > target {
            break;
        }
        if applied.contains(&migration.version) {
            continue;
        }
        run_migration(store, migration)?;
        ran.push(migration.version);
    }
    Ok(ran)
}

/// The versions of the steps that ran, in the order they ran.
pub fn applied_versions(store: &Store) -> Result<Vec<u32>, TaseError> {
    ensure_bookkeeping(store)?;
    let versions = store.query_map(
        "SELECT version FROM local_schema_migrations ORDER BY version",
        rusqlite::params![],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(versions.into_iter().map(|version| version as u32).collect())
}

/// The version of the schema of the store: the version of the last step that ran, or `0` for a store that was never
/// migrated.
pub fn current_version(store: &Store) -> Result<u32, TaseError> {
    Ok(applied_versions(store)?.into_iter().max().unwrap_or(0))
}

/// The version SQLite itself holds (`PRAGMA user_version`).
///
/// The table of the applied steps is the truth; this is the value that another tool may read.
pub fn sqlite_user_version(store: &Store) -> Result<u32, TaseError> {
    let version: i64 = store
        .connection()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(map_sqlite)?;
    Ok(version as u32)
}

/// Makes the table of the applied steps, when it is not there yet.
fn ensure_bookkeeping(store: &Store) -> Result<(), TaseError> {
    store.execute(
        "CREATE TABLE IF NOT EXISTS local_schema_migrations (
           version      INTEGER PRIMARY KEY,
           name         TEXT NOT NULL,
           applied_at_ms INTEGER NOT NULL
         )",
        rusqlite::params![],
    )?;
    Ok(())
}

/// Runs one step inside a transaction, together with the line of the bookkeeping that says it ran.
fn run_migration(store: &mut Store, migration: &Migration) -> Result<(), TaseError> {
    let tx = store.transaction()?;
    tx.execute_batch(migration.sql).map_err(map_sqlite)?;
    tx.execute(
        "INSERT INTO local_schema_migrations (version, name, applied_at_ms) VALUES (?1, ?2, ?3)",
        rusqlite::params![
            migration.version as i64,
            migration.name,
            crate::clock_ms()
        ],
    )?;
    // The value of the pragma cannot be a bound parameter, and the version is a number of this crate, not a text
    // of the caller.
    tx.execute_batch(&format!("PRAGMA user_version = {};", migration.version))
        .map_err(map_sqlite)?;
    tx.commit()?;
    Ok(())
}

/// The tables of the schema, in the order they must be emptied by a restore: the children first, so that the
/// foreign keys are never in the way. The rows are put back in the opposite order.
pub(crate) const STATE_TABLES: &[&str] = &[
    "local_file_conflicts",
    "local_workspace_versions",
    "local_idempotency",
    "local_leases",
    "local_jobs",
    "local_agent_sessions",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::temp_dir;

    /// A store of the disk with the steps applied.
    fn migrated(dir: &std::path::Path, name: &str) -> Store {
        let mut store = Store::open(dir.join(name)).expect("the store opens");
        apply(&mut store).expect("the steps run");
        store
    }

    #[test]
    fn an_empty_database_goes_to_the_latest_version() {
        let dir = temp_dir("migrate-empty");
        let mut store = Store::open(dir.join("hub.sqlite3")).expect("the store opens");
        assert_eq!(current_version(&store).expect("the version"), 0);
        let ran = apply(&mut store).expect("the steps run");
        assert_eq!(ran, vec![1, 2, 3]);
        assert_eq!(
            current_version(&store).expect("the version"),
            latest_version()
        );
        crate::test_support::remove_dir(&dir);
    }

    #[test]
    fn the_steps_may_be_repeated_without_a_change() {
        let dir = temp_dir("migrate-repeat");
        let mut store = Store::open(dir.join("hub.sqlite3")).expect("the store opens");
        assert_eq!(apply(&mut store).expect("the steps run"), vec![1, 2, 3]);
        assert!(apply(&mut store).expect("the steps run again").is_empty());
        assert!(apply(&mut store).expect("the steps run again").is_empty());
        assert_eq!(
            current_version(&store).expect("the version"),
            latest_version()
        );
        assert_eq!(
            applied_versions(&store).expect("the versions"),
            vec![1, 2, 3]
        );
        crate::test_support::remove_dir(&dir);
    }

    #[test]
    fn an_old_database_goes_up_to_the_latest_version() {
        let dir = temp_dir("migrate-upgrade");
        let path = dir.join("hub.sqlite3");

        // a store of the first step only
        let mut store = Store::open(&path).expect("the store opens");
        assert_eq!(apply_up_to(&mut store, 1).expect("the step runs"), vec![1]);
        assert_eq!(current_version(&store).expect("the version"), 1);
        assert_eq!(sqlite_user_version(&store).expect("the pragma"), 1);

        // a job of the old schema, which the new steps must not touch
        store
            .execute(
                "INSERT INTO local_agent_sessions
                   (session_id, agent_id, workspace_id, owner_logon_sid_hash, state, heartbeat_at_ms, created_at_ms)
                 VALUES ('sess_1', 'agent_1', 'ws_1', 'sid', 'CONNECTED', 1, 1)",
                rusqlite::params![],
            )
            .expect("the session is written");
        store
            .execute(
                "INSERT INTO local_jobs
                   (job_id, job_type, workspace_id, agent_id, session_id, idempotency_key,
                    request_digest, status, created_at_ms, updated_at_ms)
                 VALUES ('job_1', 'apply', 'ws_1', 'agent_1', 'sess_1', 'key_1', 'dig', 'QUEUED', 1, 1)",
                rusqlite::params![],
            )
            .expect("the job is written");
        drop(store);

        // the store is opened again by a build that knows more
        let mut store = Store::open(&path).expect("the store opens again");
        let ran = apply(&mut store).expect("the steps run");
        assert_eq!(ran, vec![2, 3]);
        assert_eq!(
            current_version(&store).expect("the version"),
            latest_version()
        );
        assert_eq!(sqlite_user_version(&store).expect("the pragma"), 3);

        // the row of the old schema is still there
        let count: i64 = store
            .query_row(
                "SELECT COUNT(*) FROM local_jobs WHERE job_id = 'job_1'",
                rusqlite::params![],
                |row| row.get(0),
            )
            .expect("the count is read");
        assert_eq!(count, 1);

        // and the new column has its default
        let priority: i64 = store
            .query_row(
                "SELECT priority FROM local_jobs WHERE job_id = 'job_1'",
                rusqlite::params![],
                |row| row.get(0),
            )
            .expect("the priority is read");
        assert_eq!(priority, 5);

        crate::test_support::remove_dir(&dir);
    }

    #[test]
    fn the_tables_of_the_design_book_are_there() {
        let dir = temp_dir("migrate-tables");
        let store = migrated(&dir, "hub.sqlite3");
        for table in STATE_TABLES {
            assert!(
                store.has_table(table).expect("the table is looked up"),
                "the table {table} must exist"
            );
        }
        for table in [
            "local_counters",
            "local_outbox",
            "local_audit_events",
            "local_journal",
            "local_snapshots",
            "local_reconcile_log",
            "local_schema_migrations",
        ] {
            assert!(
                store.has_table(table).expect("the table is looked up"),
                "the table {table} must exist"
            );
        }
        crate::test_support::remove_dir(&dir);
    }

    #[test]
    fn the_tables_of_the_design_book_keep_their_names() {
        // the names of the reference DDL of the design book §18.1 must not drift
        for name in [
            "local_agent_sessions",
            "local_jobs",
            "local_outbox",
            "local_audit_events",
            "local_workspace_versions",
        ] {
            assert!(
                MIGRATIONS[0].sql.contains(name),
                "the table {name} of the design book must be there"
            );
        }
    }

    #[test]
    fn the_versions_are_the_ones_of_the_crate_and_they_grow_by_one() {
        let mut previous = 0;
        for migration in MIGRATIONS {
            assert_eq!(migration.version, previous + 1, "{}", migration.name);
            previous = migration.version;
            assert!(!migration.name.is_empty());
            assert!(!migration.sql.trim().is_empty());
        }
        assert_eq!(latest_version(), previous);
        assert_eq!(migration_name(1), Some("base_schema"));
        assert_eq!(migration_name(99), None);
    }

    #[test]
    fn the_journal_of_the_leases_forbids_two_holders() {
        let dir = temp_dir("migrate-lease-index");
        let store = migrated(&dir, "hub.sqlite3");
        let sql = "INSERT INTO local_leases
                     (lease_id, resource, holder, fencing_token, acquired_at_ms, expires_at_ms, state)
                   VALUES (?1, 'res_1', ?2, 1, 0, 100, 'HELD')";
        store
            .execute(sql, rusqlite::params!["lease_1", "holder_a"])
            .expect("the first lease is written");
        let error = store
            .execute(sql, rusqlite::params!["lease_2", "holder_b"])
            .expect_err("the second holder must be refused");
        assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);
        crate::test_support::remove_dir(&dir);
    }

    #[test]
    fn the_state_of_a_session_is_limited_to_the_three_states() {
        let dir = temp_dir("migrate-session-state");
        let store = migrated(&dir, "hub.sqlite3");
        let sql = "INSERT INTO local_agent_sessions
                     (session_id, agent_id, workspace_id, owner_logon_sid_hash, state, heartbeat_at_ms, created_at_ms)
                   VALUES (?1, 'agent_1', 'ws_1', 'sid', ?2, 0, 0)";
        for state in ["CONNECTED", "DISCONNECTED", "EXPIRED"] {
            store
                .execute(sql, rusqlite::params![format!("sess_{state}"), state])
                .expect("the session is written");
        }
        let error = store
            .execute(sql, rusqlite::params!["sess_bad", "RUNNING"])
            .expect_err("an unknown state must be refused");
        assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);
        crate::test_support::remove_dir(&dir);
    }
}
