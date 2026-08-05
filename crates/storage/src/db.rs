use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::clock::{Clock, SystemClock};
use crate::error::{StorageError, StorageResult};
use crate::migrate::{self, latest_version};

/// How long `/v1/meta` may reuse a previously computed `data_updated_at_ms`.
/// Full recomputation scans many large tables and is far too expensive for every request.
const DATA_UPDATED_CACHE_TTL: Duration = Duration::from_secs(30);

/// SQLite access handle with enforced PRAGMA and single-writer coordination.
#[derive(Clone)]
pub struct Database {
    path: PathBuf,
    write: Arc<Mutex<Connection>>,
    clock: Arc<dyn Clock>,
    /// Cached MAX(updated_at) watermark for public meta freshness.
    data_updated_cache: Arc<Mutex<Option<(Instant, i64)>>>,
}

impl Database {
    pub fn open(path: impl AsRef<Path>) -> StorageResult<Self> {
        Self::open_with_clock(path, Arc::new(SystemClock))
    }

    pub fn open_with_clock(path: impl AsRef<Path>, clock: Arc<dyn Clock>) -> StorageResult<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let conn = open_connection(&path)?;
        apply_pragmas(&conn)?;
        Ok(Self {
            path,
            write: Arc::new(Mutex::new(conn)),
            clock,
            data_updated_cache: Arc::new(Mutex::new(None)),
        })
    }

    pub fn open_in_memory() -> StorageResult<Self> {
        Self::open_in_memory_with_clock(Arc::new(SystemClock))
    }

    pub fn open_in_memory_with_clock(clock: Arc<dyn Clock>) -> StorageResult<Self> {
        let conn = Connection::open_in_memory()?;
        apply_pragmas(&conn)?;
        Ok(Self {
            path: PathBuf::from(":memory:"),
            write: Arc::new(Mutex::new(conn)),
            clock,
            data_updated_cache: Arc::new(Mutex::new(None)),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
    }

    pub fn now_ms(&self) -> i64 {
        self.clock.now_ms()
    }

    pub fn migrate(&self) -> StorageResult<i64> {
        let mut guard = self
            .write
            .lock()
            .map_err(|_| StorageError::migration("write lock poisoned"))?;
        migrate::migrate_to_latest(&mut guard, self.now_ms())
    }

    pub fn schema_version(&self) -> StorageResult<i64> {
        // Prefer a read connection so frequent meta/ready probes never wait on the writer mutex.
        self.with_conn(migrate::current_version)
    }

    /// Invalidate cached public freshness watermark after writes that may advance it.
    pub fn invalidate_data_updated_cache(&self) {
        if let Ok(mut guard) = self.data_updated_cache.lock() {
            *guard = None;
        }
    }

    pub(crate) fn cached_data_updated_at_ms(
        &self,
        compute: impl FnOnce() -> StorageResult<i64>,
    ) -> StorageResult<i64> {
        if let Ok(guard) = self.data_updated_cache.lock()
            && let Some((at, value)) = *guard
            && at.elapsed() < DATA_UPDATED_CACHE_TTL
        {
            return Ok(value);
        }
        let value = compute()?;
        if let Ok(mut guard) = self.data_updated_cache.lock() {
            *guard = Some((Instant::now(), value));
        }
        Ok(value)
    }

    pub fn with_conn<T>(
        &self,
        f: impl FnOnce(&Connection) -> StorageResult<T>,
    ) -> StorageResult<T> {
        if self.path != Path::new(":memory:") {
            let conn = open_read_connection(&self.path)?;
            apply_read_pragmas(&conn)?;
            return f(&conn);
        }
        let guard = self
            .write
            .lock()
            .map_err(|_| StorageError::migration("write lock poisoned"))?;
        f(&guard)
    }

    pub fn with_conn_mut<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> StorageResult<T>,
    ) -> StorageResult<T> {
        let mut guard = self
            .write
            .lock()
            .map_err(|_| StorageError::migration("write lock poisoned"))?;
        let result = f(&mut guard);
        // Any writer path may advance table watermarks used by public meta freshness.
        self.invalidate_data_updated_cache();
        result
    }

    pub fn integrity_check(&self) -> StorageResult<Vec<String>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare("PRAGMA integrity_check")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }

    pub fn assert_ready(&self) -> StorageResult<()> {
        self.readiness_check()?;
        // Full FK scan is O(database size) and can take tens of seconds on production DBs.
        // Keep it on the startup/ops path only — never on `/health/ready`.
        self.with_conn(validate_foreign_keys)?;
        let check = self.integrity_check()?;
        if check != ["ok".to_owned()] {
            return Err(StorageError::migration(format!(
                "integrity_check failed: {check:?}"
            )));
        }
        Ok(())
    }

    /// Lightweight probe for frequent health checks. Full integrity/FK checks stay on startup/ops paths.
    pub fn readiness_check(&self) -> StorageResult<()> {
        let version = self.schema_version()?;
        if version != latest_version() {
            return Err(StorageError::migration(format!(
                "schema version {version} != expected {}",
                latest_version()
            )));
        }
        self.with_conn(|conn| {
            let value: i64 = conn.query_row("SELECT 1", [], |row| row.get(0))?;
            let foreign_keys: i64 = conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
            if value != 1 || foreign_keys != 1 {
                return Err(StorageError::migration(
                    "database connection pragmas are not ready",
                ));
            }
            validate_key_schema(conn)?;
            Ok(())
        })?;
        Ok(())
    }
}

fn open_connection(path: &Path) -> StorageResult<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    Ok(conn)
}

fn open_read_connection(path: &Path) -> StorageResult<Connection> {
    Ok(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?)
}

fn apply_read_pragmas(conn: &Connection) -> StorageResult<()> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000i32)?;
    conn.pragma_update(None, "trusted_schema", "OFF")?;
    conn.pragma_update(None, "synchronous", "FULL")?;
    let mode: String = conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(StorageError::migration(format!(
            "expected read connection journal_mode WAL, got {mode}"
        )));
    }
    Ok(())
}

pub fn apply_pragmas(conn: &Connection) -> StorageResult<()> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000i32)?;
    conn.pragma_update(None, "trusted_schema", "OFF")?;
    // Reasserting WAL on every short-lived worker process can require an
    // exclusive lock even when the database is already in WAL mode.
    let current_mode: String = conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    let mode = if current_mode.eq_ignore_ascii_case("wal")
        || current_mode.eq_ignore_ascii_case("memory")
    {
        current_mode
    } else {
        conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?
    };
    if mode.eq_ignore_ascii_case("wal") || mode.eq_ignore_ascii_case("memory") {
        // memory databases may not use WAL; accept both.
    } else {
        return Err(StorageError::migration(format!(
            "expected journal_mode WAL or memory, got {mode}"
        )));
    }
    conn.pragma_update(None, "synchronous", "FULL")?;
    Ok(())
}

const KEY_SCHEMA_PROBES: &[(&str, &str)] = &[
    (
        "schema_migrations",
        "SELECT version, name, applied_at_ms FROM schema_migrations LIMIT 0",
    ),
    (
        "apps",
        "SELECT app_id, app_type, canonical_name FROM apps LIMIT 0",
    ),
    (
        "anonymous_users",
        "SELECT user_id, access_token_hash, refresh_token_hash,
                access_expires_at_ms, refresh_expires_at_ms
         FROM anonymous_users LIMIT 0",
    ),
    (
        "user_accounts",
        "SELECT user_id, username_normalized, password_hash, password_scheme,
                status, avatar_public_id
         FROM user_accounts LIMIT 0",
    ),
    (
        "account_sessions",
        "SELECT session_id, user_id, access_token_hash, refresh_token_hash,
                expires_at_ms, refresh_expires_at_ms, revoked_at_ms,
                replaced_by_session_id
         FROM account_sessions LIMIT 0",
    ),
    (
        "feedback_events",
        "SELECT feedback_id, user_id, idempotency_key, undone_by,
                request_fingerprint
         FROM feedback_events LIMIT 0",
    ),
    (
        "jobs",
        "SELECT job_id, attempts, max_attempts, status, lease_owner,
                lease_expires_at_ms, completion_idempotency_key
         FROM jobs LIMIT 0",
    ),
    (
        "play_intent_state",
        "SELECT singleton, revision FROM play_intent_state LIMIT 0",
    ),
    (
        "user_ai_credentials",
        "SELECT user_id, mode, encrypted_api_key, key_version
         FROM user_ai_credentials LIMIT 0",
    ),
    (
        "recommendation_runs",
        "SELECT recommendation_run_id, context_hash, candidate_set_hash,
                created_at_ms, expires_at_ms
         FROM recommendation_runs LIMIT 0",
    ),
    (
        "recommendation_items",
        "SELECT recommendation_run_id, app_id, rank, score_components_json,
                recorded_at_ms, expires_at_ms
         FROM recommendation_items LIMIT 0",
    ),
    (
        "recommendation_events",
        "SELECT recommendation_event_id, recommendation_run_id, app_id,
                idempotency_key, created_at_ms, expires_at_ms
         FROM recommendation_events LIMIT 0",
    ),
];

const REQUIRED_PRIMARY_KEYS: &[(&str, &str)] = &[
    ("schema_migrations", "version"),
    ("apps", "app_id"),
    ("anonymous_users", "user_id"),
    ("user_accounts", "user_id"),
    ("account_sessions", "session_id"),
    ("feedback_events", "feedback_id"),
    ("jobs", "job_id"),
    ("play_intent_state", "singleton"),
    ("user_ai_credentials", "user_id"),
    ("recommendation_runs", "recommendation_run_id"),
    ("recommendation_items", "recommendation_run_id"),
    ("recommendation_items", "app_id"),
    ("recommendation_events", "recommendation_event_id"),
];

const REQUIRED_UNIQUE_KEYS: &[(&str, &[&str])] = &[
    ("schema_migrations", &["name"]),
    ("anonymous_users", &["access_token_hash"]),
    ("anonymous_users", &["refresh_token_hash"]),
    ("user_accounts", &["username_normalized"]),
    ("user_accounts", &["avatar_public_id"]),
    ("account_sessions", &["access_token_hash"]),
    ("account_sessions", &["refresh_token_hash"]),
    ("feedback_events", &["user_id", "idempotency_key"]),
    ("jobs", &["idempotency_key"]),
    ("recommendation_items", &["recommendation_run_id", "rank"]),
    (
        "recommendation_events",
        &["recommendation_run_id", "idempotency_key"],
    ),
];

fn validate_key_schema(conn: &Connection) -> StorageResult<()> {
    for (name, probe) in KEY_SCHEMA_PROBES {
        conn.prepare(probe).map_err(|error| {
            StorageError::migration(format!("required {name} schema is unavailable: {error}"))
        })?;
    }
    for (table, column) in REQUIRED_PRIMARY_KEYS {
        let ordinal: Option<i64> = conn
            .query_row(
                "SELECT pk FROM pragma_table_info(?1) WHERE name = ?2",
                params![table, column],
                |row| row.get(0),
            )
            .optional()?;
        if ordinal.is_none_or(|value| value <= 0) {
            return Err(StorageError::migration(format!(
                "required primary key {table}({column}) is missing"
            )));
        }
    }
    for (table, columns) in REQUIRED_UNIQUE_KEYS {
        if !has_unique_key(conn, table, columns)? {
            return Err(StorageError::migration(format!(
                "required unique key {table}({}) is missing",
                columns.join(", ")
            )));
        }
    }
    Ok(())
}

fn has_unique_key(conn: &Connection, table: &str, expected: &[&str]) -> StorageResult<bool> {
    let index_names = {
        let mut statement =
            conn.prepare("SELECT name FROM pragma_index_list(?1) WHERE \"unique\" = 1")?;
        let rows = statement.query_map(params![table], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for index_name in index_names {
        let mut statement =
            conn.prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno ASC")?;
        let rows =
            statement.query_map(params![index_name], |row| row.get::<_, Option<String>>(0))?;
        let columns = rows
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .unwrap_or_default();
        if columns
            .iter()
            .map(String::as_str)
            .eq(expected.iter().copied())
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn validate_foreign_keys(conn: &Connection) -> StorageResult<()> {
    let mut statement = conn.prepare("PRAGMA foreign_key_check")?;
    let mut rows = statement.query([])?;
    if let Some(row) = rows.next()? {
        let table: String = row.get(0)?;
        let row_id: Option<i64> = row.get(1)?;
        let parent: String = row.get(2)?;
        let foreign_key: i64 = row.get(3)?;
        return Err(StorageError::migration(format!(
            "foreign key violation in {table} row {row_id:?}: parent={parent}, constraint={foreign_key}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_an_existing_wal_database_does_not_contend_with_a_writer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wal-open.db");
        let db = Database::open(&path).unwrap();
        db.migrate().unwrap();

        let blocker = Connection::open(&path).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        let second = Database::open(&path);
        blocker.execute_batch("ROLLBACK").unwrap();

        assert!(second.is_ok());
    }

    #[test]
    fn readiness_rejects_missing_key_schema() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        db.with_conn_mut(|conn| {
            conn.execute_batch("DROP TABLE account_sessions")?;
            Ok(())
        })
        .unwrap();
        assert!(matches!(
            db.readiness_check(),
            Err(StorageError::Migration { .. })
        ));
    }

    #[test]
    fn readiness_stays_light_when_foreign_keys_are_violated() {
        // Hot path must not run PRAGMA foreign_key_check (multi-second on large DBs).
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        db.with_conn_mut(|conn| {
            conn.pragma_update(None, "foreign_keys", "OFF")?;
            conn.execute(
                "INSERT INTO play_intent_votes (app_id, user_id, created_at_ms)
                 VALUES (999, 'missing-user', 1)",
                [],
            )?;
            conn.pragma_update(None, "foreign_keys", "ON")?;
            Ok(())
        })
        .unwrap();
        assert!(db.readiness_check().is_ok());
        assert!(matches!(
            db.assert_ready(),
            Err(StorageError::Migration { .. })
        ));
    }
}
