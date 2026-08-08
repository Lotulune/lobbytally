use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{StorageError, StorageResult};
use crate::models::DataRefreshStatus;

pub fn load_cursor(conn: &Connection, cursor_key: &str) -> StorageResult<Option<String>> {
    conn.query_row(
        "SELECT cursor_json FROM source_cursors WHERE cursor_key = ?1",
        params![cursor_key],
        |row| row.get(0),
    )
    .optional()
    .map_err(StorageError::from)
}

pub fn save_cursor(
    conn: &Connection,
    cursor_key: &str,
    source: &str,
    cursor_json: &str,
    now_ms: i64,
) -> StorageResult<()> {
    if cursor_key.trim().is_empty() || source.trim().is_empty() {
        return Err(StorageError::validation(
            "cursor_key and source are required",
        ));
    }
    serde_json::from_str::<serde_json::Value>(cursor_json)
        .map_err(|_| StorageError::validation("cursor_json must be valid JSON"))?;
    conn.execute(
        "INSERT INTO source_cursors (
            cursor_key, source, cursor_json, last_success_at_ms, updated_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?4)
         ON CONFLICT(cursor_key) DO UPDATE SET
            source = excluded.source,
            cursor_json = excluded.cursor_json,
            last_success_at_ms = excluded.last_success_at_ms,
            updated_at_ms = excluded.updated_at_ms",
        params![cursor_key, source, cursor_json, now_ms],
    )?;
    Ok(())
}

pub fn start_run(
    conn: &Connection,
    source: &str,
    task_type: &str,
    parser_version: &str,
    notes: Option<&str>,
    now_ms: i64,
) -> StorageResult<i64> {
    for (label, value) in [
        ("source", source),
        ("task_type", task_type),
        ("parser_version", parser_version),
    ] {
        if value.trim().is_empty() {
            return Err(StorageError::validation(format!("{label} is required")));
        }
    }
    // A process killed or replaced mid-run cannot finalize its audit row. Do
    // not leave such historical rows looking perpetually active forever.
    // Only one invocation of a given source/task is supported at a time.
    conn.execute(
        "UPDATE source_runs SET
             status = 'failed', finished_at_ms = ?1,
             error_category = COALESCE(error_category, 'interrupted'),
             notes = CASE
                 WHEN notes IS NULL OR TRIM(notes) = '' THEN 'superseded by a newer run'
                 ELSE notes || '; superseded by a newer run'
             END
         WHERE source = ?2 AND task_type = ?3 AND status = 'running'",
        params![now_ms, source, task_type],
    )?;
    conn.execute(
        "INSERT INTO source_runs (
            source, task_type, status, started_at_ms, parser_version, notes
         ) VALUES (?1, ?2, 'running', ?3, ?4, ?5)",
        params![source, task_type, now_ms, parser_version, notes],
    )?;
    Ok(conn.last_insert_rowid())
}

#[allow(clippy::too_many_arguments)]
pub fn finish_run(
    conn: &Connection,
    run_id: i64,
    status: &str,
    request_count: i64,
    success_count: i64,
    error_category: Option<&str>,
    notes: Option<&str>,
    now_ms: i64,
) -> StorageResult<()> {
    if !matches!(status, "succeeded" | "failed" | "partial") {
        return Err(StorageError::validation(
            "source run final status must be succeeded, failed, or partial",
        ));
    }
    if request_count < 0 || success_count < 0 {
        return Err(StorageError::validation(
            "source run counters must be non-negative",
        ));
    }
    let changed = conn.execute(
        "UPDATE source_runs SET
            status = ?1, finished_at_ms = ?2, request_count = ?3,
            success_count = ?4, error_category = ?5, notes = ?6
         WHERE run_id = ?7 AND status = 'running'",
        params![
            status,
            now_ms,
            request_count,
            success_count,
            error_category,
            notes,
            run_id
        ],
    )?;
    if changed != 1 {
        return Err(StorageError::conflict(format!(
            "source run {run_id} is missing or already finalized"
        )));
    }
    Ok(())
}

pub fn ensure_data_refresh_tasks(conn: &Connection, now_ms: i64) -> StorageResult<()> {
    for task_name in [
        "catalog_sync",
        "candidate_top_refresh",
        "candidate_continuation",
        "enrichment",
        "quality_check",
        "retrieval_sync",
        "pipeline_snapshot",
    ] {
        conn.execute(
            "INSERT OR IGNORE INTO data_refresh_state (
                task_name, last_success_at_ms, next_run_at_ms, last_error_category,
                cursor_value, coverage_ratio, updated_at_ms
             ) VALUES (?1, NULL, ?2, NULL, NULL, NULL, ?2)",
            params![task_name, now_ms],
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn update_data_refresh_status(
    conn: &Connection,
    task_name: &str,
    last_success_at_ms: Option<i64>,
    next_run_at_ms: Option<i64>,
    last_error_category: Option<&str>,
    cursor_value: Option<&str>,
    coverage_ratio: Option<f64>,
    now_ms: i64,
) -> StorageResult<()> {
    if task_name.trim().is_empty() || task_name.len() > 80 {
        return Err(StorageError::validation(
            "data refresh task name is invalid",
        ));
    }
    if let Some(ratio) = coverage_ratio
        && (!ratio.is_finite() || !(0.0..=1.0).contains(&ratio))
    {
        return Err(StorageError::validation("data coverage ratio is invalid"));
    }
    conn.execute(
        "INSERT INTO data_refresh_state (
            task_name, last_success_at_ms, next_run_at_ms, last_error_category,
            cursor_value, coverage_ratio, updated_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(task_name) DO UPDATE SET
            last_success_at_ms = excluded.last_success_at_ms,
            next_run_at_ms = excluded.next_run_at_ms,
            last_error_category = excluded.last_error_category,
            cursor_value = excluded.cursor_value,
            coverage_ratio = excluded.coverage_ratio,
            updated_at_ms = excluded.updated_at_ms",
        params![
            task_name,
            last_success_at_ms,
            next_run_at_ms,
            last_error_category,
            cursor_value,
            coverage_ratio,
            now_ms
        ],
    )?;
    Ok(())
}

pub fn data_refresh_status(conn: &Connection) -> StorageResult<Vec<DataRefreshStatus>> {
    let mut statement = conn.prepare(
        "SELECT task_name, last_success_at_ms, next_run_at_ms, last_error_category,
                cursor_value, coverage_ratio, updated_at_ms
         FROM data_refresh_state ORDER BY task_name",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(DataRefreshStatus {
            task_name: row.get(0)?,
            last_success_at_ms: row.get(1)?,
            next_run_at_ms: row.get(2)?,
            last_error_category: row.get(3)?,
            cursor_value: row.get(4)?,
            coverage_ratio: row.get(5)?,
            updated_at_ms: row.get(6)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}
