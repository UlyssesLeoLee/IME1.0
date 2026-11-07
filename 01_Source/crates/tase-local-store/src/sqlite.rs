//! The SQLite connection of the local store: the WAL mode, the pragmas, the transaction boundary and the mapping of
//! a failure of SQLite into a business code of the contract (detailed design §18.1, §6.3).
//!
//! The store is SQLite in WAL mode, as the design book asks. SQLite itself is public domain and `rusqlite` is MIT,
//! so the dependency adds no licence obligation to the repository; the `bundled` feature compiles the SQLite
//! amalgamation into the library, so the crate needs no system SQLite and behaves the same on every target.
//!
//! The crate never reads the clock: every timestamp comes from the caller, so a test is deterministic and a
//! recovery can be replayed.

use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};
use std::path::{Path, PathBuf};
use std::time::Duration;

use pime_core::json::Json;
use tase_contract::TaseError;

/// The default time a connection waits for the lock of another writer.
pub const DEFAULT_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// The parameters of a statement, as the `params!` macro of `rusqlite` builds them.
pub type SqlParams<'a> = &'a [&'a dyn rusqlite::ToSql];

/// The journal mode of the database.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JournalMode {
    /// The write-ahead log of the design book (§18.1): a reader never blocks the writer.
    Wal,
    /// The rollback journal: the whole file is locked while a writer works.
    Delete,
    /// The in-memory journal: only for a database that lives in memory.
    Memory,
}

impl JournalMode {
    /// The name of the mode, as SQLite spells it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            JournalMode::Wal => "wal",
            JournalMode::Delete => "delete",
            JournalMode::Memory => "memory",
        }
    }

    /// The mode of a name that SQLite reported.
    ///
    /// The names that SQLite accepts for the rollback journal (`truncate`, `persist`, `off`) all mean the same
    /// thing for the store, so they are all read as [`JournalMode::Delete`].
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "wal" => Some(JournalMode::Wal),
            "delete" | "truncate" | "persist" | "off" => Some(JournalMode::Delete),
            "memory" => Some(JournalMode::Memory),
            _ => None,
        }
    }
}

/// The durability of the writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Synchronous {
    /// Every write goes to the disk before the answer is given.
    Full,
    /// The WAL is written to the disk at every checkpoint; a power cut may lose the last transactions.
    Normal,
}

impl Synchronous {
    /// The name of the mode, as SQLite spells it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Synchronous::Full => "full",
            Synchronous::Normal => "normal",
        }
    }
}

/// The options of a store.
#[derive(Clone, Debug)]
pub struct StoreOptions {
    /// The journal mode. The design book asks for WAL.
    pub journal_mode: JournalMode,
    /// How long a connection waits for the lock of another writer.
    pub busy_timeout: Duration,
    /// Whether the foreign keys are checked.
    pub foreign_keys: bool,
    /// The durability of the writes.
    pub synchronous: Synchronous,
}

impl Default for StoreOptions {
    fn default() -> Self {
        StoreOptions {
            journal_mode: JournalMode::Wal,
            busy_timeout: DEFAULT_BUSY_TIMEOUT,
            foreign_keys: true,
            synchronous: Synchronous::Normal,
        }
    }
}

/// The database of the local store.
///
/// The store owns the connection: it opens it, it sets the pragmas of the design book and it hands out the write
/// transactions. The connection is `Send` but not `Sync`, so a store belongs to one thread at a time; two processes
/// (or two threads with one connection each) share the database through the locks of SQLite.
pub struct Store {
    conn: Connection,
    path: Option<PathBuf>,
    options: StoreOptions,
}

impl Store {
    /// Opens a database of the disk, creating it when it does not exist yet.
    ///
    /// The parent directory is created when it is missing, so the Hub may name a path under a directory it has not
    /// made yet.
    pub fn open(path: impl AsRef<Path>) -> Result<Store, TaseError> {
        Self::open_with(path, StoreOptions::default())
    }

    /// Opens a database of the disk with the given options.
    pub fn open_with(path: impl AsRef<Path>, options: StoreOptions) -> Result<Store, TaseError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent).map_err(|error| {
                TaseError::internal(format!("the directory of the store cannot be made: {error}"))
            })?;
        }
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(map_sqlite)?;
        let store = Store {
            conn,
            path: Some(path.to_path_buf()),
            options,
        };
        store.apply_pragmas()?;
        Ok(store)
    }

    /// Opens a database that lives in memory.
    ///
    /// A database in memory has no WAL: SQLite keeps the journal in memory as well, so the mode of the store is
    /// [`JournalMode::Memory`]. The tests that check the WAL mode use a database of the disk.
    pub fn open_in_memory() -> Result<Store, TaseError> {
        let conn = Connection::open_in_memory().map_err(map_sqlite)?;
        let options = StoreOptions {
            journal_mode: JournalMode::Memory,
            ..StoreOptions::default()
        };
        let store = Store {
            conn,
            path: None,
            options,
        };
        store.apply_pragmas()?;
        Ok(store)
    }

    /// The pragmas of the design book: the journal mode, the foreign keys and the busy timeout.
    fn apply_pragmas(&self) -> Result<(), TaseError> {
        self.conn
            .busy_timeout(self.options.busy_timeout)
            .map_err(map_sqlite)?;
        self.conn
            .pragma_update(None, "journal_mode", self.options.journal_mode.as_str())
            .map_err(map_sqlite)?;
        self.conn
            .pragma_update(None, "foreign_keys", self.options.foreign_keys)
            .map_err(map_sqlite)?;
        self.conn
            .pragma_update(None, "synchronous", self.options.synchronous.as_str())
            .map_err(map_sqlite)?;
        Ok(())
    }

    /// The path of the database, or `None` when it lives in memory.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The options the store was opened with.
    #[must_use]
    pub fn options(&self) -> &StoreOptions {
        &self.options
    }

    /// The connection, for the parts that need it (the migrations, the reconciliation).
    #[must_use]
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// The journal mode SQLite really uses.
    ///
    /// A database in memory answers `memory`; a database of the disk answers `wal` when the WAL was asked for.
    pub fn journal_mode(&self) -> Result<JournalMode, TaseError> {
        let name: String = self
            .conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .map_err(map_sqlite)?;
        JournalMode::from_name(&name).ok_or_else(|| {
            TaseError::internal(format!("SQLite reports an unknown journal mode {name:?}"))
        })
    }

    /// Whether the foreign keys are checked.
    pub fn foreign_keys_enabled(&self) -> Result<bool, TaseError> {
        self.conn
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .map_err(map_sqlite)
    }

    /// The busy timeout of the connection, in milliseconds.
    #[must_use]
    pub fn busy_timeout_ms(&self) -> u64 {
        self.options.busy_timeout.as_millis() as u64
    }

    /// Whether a table of the schema is in the database.
    ///
    /// The hub uses it to know whether the store was migrated before it starts to work.
    pub fn has_table(&self, name: &str) -> Result<bool, TaseError> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                rusqlite::params![name],
                |row| row.get(0),
            )
            .map_err(map_sqlite)?;
        Ok(count > 0)
    }

    /// Begins a write transaction.
    ///
    /// The transaction is `IMMEDIATE`: the write lock is taken at the beginning, so two writers cannot both read a
    /// free row and then both write it. Every write of the store that has to be atomic (the claim of a lease, the
    /// registration of a job) goes through this call.
    pub fn transaction(&mut self) -> Result<WriteTx<'_>, TaseError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite)?;
        Ok(WriteTx { tx })
    }

    /// Runs a statement outside a transaction.
    pub fn execute(&self, sql: &str, params: SqlParams<'_>) -> Result<usize, TaseError> {
        self.conn.execute(sql, params).map_err(map_sqlite)
    }

    /// Reads one row.
    pub fn query_row<T>(
        &self,
        sql: &str,
        params: SqlParams<'_>,
        row: impl FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<T, TaseError> {
        self.conn.query_row(sql, params, row).map_err(map_sqlite)
    }

    /// Reads every row of a statement.
    pub fn query_map<T>(
        &self,
        sql: &str,
        params: SqlParams<'_>,
        row: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<Vec<T>, TaseError> {
        let mut statement = self.conn.prepare(sql).map_err(map_sqlite)?;
        let rows = statement
            .query_map(params, row)
            .map_err(map_sqlite)?
            .collect::<rusqlite::Result<Vec<T>>>()
            .map_err(map_sqlite)?;
        Ok(rows)
    }
}

/// A write transaction of the store.
///
/// The transaction holds the write lock of the database until it is committed or rolled back; dropping it without a
/// commit rolls it back.
pub struct WriteTx<'a> {
    tx: Transaction<'a>,
}

impl WriteTx<'_> {
    /// Runs a statement inside the transaction.
    pub fn execute(&self, sql: &str, params: SqlParams<'_>) -> Result<usize, TaseError> {
        self.tx.execute(sql, params).map_err(map_sqlite)
    }

    /// Reads one row inside the transaction.
    pub fn query_row<T>(
        &self,
        sql: &str,
        params: SqlParams<'_>,
        row: impl FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<T, TaseError> {
        self.tx.query_row(sql, params, row).map_err(map_sqlite)
    }

    /// Reads every row of a statement inside the transaction.
    pub fn query_map<T>(
        &self,
        sql: &str,
        params: SqlParams<'_>,
        row: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<Vec<T>, TaseError> {
        let mut statement = self.tx.prepare(sql).map_err(map_sqlite)?;
        let rows = statement
            .query_map(params, row)
            .map_err(map_sqlite)?
            .collect::<rusqlite::Result<Vec<T>>>()
            .map_err(map_sqlite)?;
        Ok(rows)
    }

    /// Commits the transaction.
    pub fn commit(self) -> Result<(), TaseError> {
        self.tx.commit().map_err(map_sqlite)
    }

    /// Rolls the transaction back.
    pub fn rollback(self) -> Result<(), TaseError> {
        self.tx.rollback().map_err(map_sqlite)
    }
}

/// The transaction of the store is used as a connection, so that the helpers of the crate work with both.
impl std::ops::Deref for WriteTx<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        &self.tx
    }
}

/// Appends one entry to the journal of the store.
///
/// The journal is the row image of a change: it says which kind of row changed and what the row became. The
/// snapshot of the store is a state plus the sequence of the journal, so the state can be rebuilt from the snapshot
/// and the entries that came after it.
pub(crate) fn append_journal(
    conn: &Connection,
    kind: &str,
    payload: &Json,
    now_ms: u64,
) -> Result<(), TaseError> {
    conn.execute(
        "INSERT INTO local_journal (kind, payload, created_at_ms) VALUES (?1, ?2, ?3)",
        rusqlite::params![kind, payload.to_json_string(), now_ms as i64],
    )
    .map_err(map_sqlite)?;
    Ok(())
}

/// Maps a failure of SQLite into a business code of the contract.
///
/// A lock that lasts longer than the busy timeout is a failure another writer may recover from (`WORKER_UNAVAILABLE`),
/// a constraint that SQLite refuses is a bad argument (`INVALID_ARGUMENT`), and everything else is an internal
/// failure.
pub(crate) fn map_sqlite(error: rusqlite::Error) -> TaseError {
    match error {
        rusqlite::Error::SqliteFailure(inner, _) => match inner.code {
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked => {
                TaseError::worker_unavailable("the local store is locked by another writer")
            }
            rusqlite::ErrorCode::ConstraintViolation => {
                TaseError::invalid_argument("the local store refused the row (constraint)")
            }
            rusqlite::ErrorCode::ReadOnly => {
                TaseError::worker_unavailable("the local store is read-only")
            }
            _ => TaseError::internal(format!("the local store failed: {inner}")),
        },
        other => TaseError::internal(format!("the local store failed: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_store_in_memory_opens_and_answers_its_pragmas() {
        let store = Store::open_in_memory().expect("the store opens");
        assert_eq!(store.journal_mode().expect("the mode"), JournalMode::Memory);
        assert!(store.foreign_keys_enabled().expect("the foreign keys"));
        assert!(store.path().is_none());
        assert_eq!(store.busy_timeout_ms(), 5_000);
    }

    #[test]
    fn a_store_of_the_disk_uses_the_wal() {
        let dir = crate::test_support::temp_dir("sqlite-wal");
        let path = dir.join("hub.sqlite3");
        let store = Store::open(&path).expect("the store opens");
        assert_eq!(store.journal_mode().expect("the mode"), JournalMode::Wal);
        assert!(store.path().is_some());
        crate::test_support::remove_dir(&dir);
    }

    #[test]
    fn a_transaction_is_rolled_back_when_it_is_dropped() {
        let mut store = Store::open_in_memory().expect("the store opens");
        store
            .execute(
                "CREATE TABLE t (a INTEGER NOT NULL)",
                rusqlite::params![],
            )
            .expect("the table is made");
        {
            let tx = store.transaction().expect("the transaction opens");
            tx.execute("INSERT INTO t (a) VALUES (1)", rusqlite::params![])
                .expect("the row is written");
            tx.rollback().expect("the transaction rolls back");
        }
        let count: i64 = store
            .query_row("SELECT COUNT(*) FROM t", rusqlite::params![], |row| {
                row.get(0)
            })
            .expect("the count is read");
        assert_eq!(count, 0);
    }

    #[test]
    fn a_transaction_is_kept_when_it_is_committed() {
        let mut store = Store::open_in_memory().expect("the store opens");
        store
            .execute(
                "CREATE TABLE t (a INTEGER NOT NULL)",
                rusqlite::params![],
            )
            .expect("the table is made");
        {
            let tx = store.transaction().expect("the transaction opens");
            tx.execute("INSERT INTO t (a) VALUES (1)", rusqlite::params![])
                .expect("the row is written");
            tx.commit().expect("the transaction commits");
        }
        let count: i64 = store
            .query_row("SELECT COUNT(*) FROM t", rusqlite::params![], |row| {
                row.get(0)
            })
            .expect("the count is read");
        assert_eq!(count, 1);
    }

    #[test]
    fn the_names_of_the_journal_modes_are_the_ones_of_sqlite() {
        assert_eq!(JournalMode::Wal.as_str(), "wal");
        assert_eq!(JournalMode::from_name("WAL"), Some(JournalMode::Wal));
        assert_eq!(JournalMode::from_name("delete"), Some(JournalMode::Delete));
        assert_eq!(
            JournalMode::from_name("truncate"),
            Some(JournalMode::Delete)
        );
        assert_eq!(JournalMode::from_name("memory"), Some(JournalMode::Memory));
        assert_eq!(JournalMode::from_name("nonsense"), None);
    }

    #[test]
    fn a_locked_database_is_a_recoverable_failure() {
        let error = map_sqlite(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_LOCKED),
            None,
        ));
        assert_eq!(error.code, tase_contract::ErrorCode::WorkerUnavailable);
        assert!(error.code.retryable());
    }

    #[test]
    fn a_refused_row_is_a_bad_argument() {
        let error = map_sqlite(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT),
            None,
        ));
        assert_eq!(error.code, tase_contract::ErrorCode::InvalidArgument);
    }

    #[test]
    fn an_unknown_failure_is_an_internal_one() {
        let error = map_sqlite(rusqlite::Error::InvalidQuery);
        assert_eq!(error.code, tase_contract::ErrorCode::InternalError);
    }
}
