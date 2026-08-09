use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::error::{StorageError, StorageResult};
use crate::models::GameIngestionTask;

pub const INITIAL_STAGE: &str = "store_details";
const LANE_NAME: &str = "integrated_game_ingestion";
const DETERMINISTIC_FAILURE_LIMIT: i64 = 3;
const TRANSIENT_FAILURE_LIMIT: i64 = 8;
const MAX_ERROR_SUMMARY_CHARS: usize = 512;

pub fn queue_new_app(
    conn: &Connection,
    app_id: u32,
    source: &str,
    now_ms: i64,
) -> StorageResult<bool> {
    let inserted = conn.execute(
        "INSERT INTO game_ingestion_queue (
            app_id, source, priority, stage, status, stage_failure_attempts,
            total_failure_attempts, lease_count, next_attempt_at_ms,
            enrichment_profile, profile_version, discovered_at_ms, updated_at_ms
         )
         SELECT ?1, ?2, 0, ?3, 'pending', 0, 0, 0, ?4,
                CASE
                    WHEN app_type IN ('demo', 'playtest') THEN 'basic_demo'
                    WHEN release_state IN ('upcoming', 'coming_soon') THEN 'basic_upcoming'
                    ELSE 'full_released'
                END,
                1, ?4, ?4
         FROM apps WHERE app_id = ?1
         ON CONFLICT(app_id) DO NOTHING",
        params![app_id, source, INITIAL_STAGE, now_ms],
    )?;
    Ok(inserted == 1)
}

pub fn pending_app_ids(conn: &Connection) -> StorageResult<Vec<u32>> {
    let mut statement = conn.prepare(
        "SELECT app_id
         FROM game_ingestion_queue
         WHERE status IN ('pending', 'retry')
         ORDER BY priority DESC, discovered_at_ms, app_id",
    )?;
    let rows = statement.query_map([], |row| row.get(0))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn recover_leases(conn: &Connection, now_ms: i64) -> StorageResult<usize> {
    conn.execute(
        "UPDATE game_ingestion_queue
         SET lease_owner = NULL, lease_expires_at_ms = NULL, updated_at_ms = ?1
         WHERE lease_owner IS NOT NULL OR lease_expires_at_ms IS NOT NULL",
        [now_ms],
    )
    .map_err(Into::into)
}

pub fn claim_tasks(
    conn: &Connection,
    owner: &str,
    limit: i64,
    lease_ms: i64,
    now_ms: i64,
) -> StorageResult<Vec<GameIngestionTask>> {
    validate_lease(owner, limit, lease_ms)?;
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let lane_paused: bool = tx.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM game_ingestion_lane_state
             WHERE lane = ?1 AND pause_until_ms > ?2
         )",
        params![LANE_NAME, now_ms],
        |row| row.get(0),
    )?;
    if lane_paused {
        tx.commit()?;
        return Ok(Vec::new());
    }
    let mut statement = tx.prepare(
        "SELECT app_id
         FROM game_ingestion_queue
         WHERE status IN ('pending', 'retry')
           AND next_attempt_at_ms <= ?1
           AND (lease_expires_at_ms IS NULL OR lease_expires_at_ms <= ?1)
         ORDER BY priority DESC, next_attempt_at_ms, discovered_at_ms, app_id
         LIMIT ?2",
    )?;
    let app_ids = statement
        .query_map(params![now_ms, limit], |row| row.get::<_, u32>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);

    let lease_expires_at_ms = now_ms.saturating_add(lease_ms);
    let mut claimed = Vec::with_capacity(app_ids.len());
    for app_id in app_ids {
        let changed = tx.execute(
            "UPDATE game_ingestion_queue
             SET lease_owner = ?1, lease_expires_at_ms = ?2,
                 lease_count = lease_count + 1, updated_at_ms = ?3
             WHERE app_id = ?4
               AND status IN ('pending', 'retry')
               AND next_attempt_at_ms <= ?3
               AND (lease_expires_at_ms IS NULL OR lease_expires_at_ms <= ?3)",
            params![owner, lease_expires_at_ms, now_ms, app_id],
        )?;
        if changed == 1
            && let Some(task) = get_task(&tx, app_id)?
        {
            claimed.push(task);
        }
    }
    tx.commit()?;
    Ok(claimed)
}

pub fn advance_stage(
    conn: &Connection,
    app_id: u32,
    owner: &str,
    now_ms: i64,
) -> StorageResult<GameIngestionTask> {
    if owner.trim().is_empty() {
        return Err(StorageError::validation("lease owner is required"));
    }
    let task = require_active_lease(conn, app_id, owner, now_ms)?;
    let next_stage = match task.stage.as_str() {
        "store_details" => "review_summary",
        "review_summary" => "popular_reviews",
        "popular_reviews" => "ccu",
        "ccu" => "complete",
        "complete" => return Ok(task),
        stage => {
            return Err(StorageError::validation(format!(
                "unknown game ingestion stage {stage}"
            )));
        }
    };
    let complete = next_stage == "complete";
    let changed = conn.execute(
        "UPDATE game_ingestion_queue
         SET stage = ?1,
             status = CASE WHEN ?2 = 1 THEN 'complete' ELSE 'pending' END,
             stage_failure_attempts = 0,
             next_attempt_at_ms = ?3,
             last_error_category = NULL,
             last_error_summary = NULL,
             dead_at_ms = NULL,
             lease_owner = CASE WHEN ?2 = 1 THEN NULL ELSE lease_owner END,
             lease_expires_at_ms = CASE WHEN ?2 = 1 THEN NULL ELSE lease_expires_at_ms END,
             updated_at_ms = ?3
         WHERE app_id = ?4 AND lease_owner = ?5
           AND lease_expires_at_ms IS NOT NULL AND lease_expires_at_ms > ?3",
        params![next_stage, i64::from(complete), now_ms, app_id, owner],
    )?;
    if changed != 1 {
        return Err(StorageError::lease(format!(
            "game ingestion lease for app {app_id} is no longer active"
        )));
    }
    get_task(conn, app_id)?.ok_or_else(|| StorageError::not_found(format!("app {app_id} queue")))
}

pub fn retry_stage(
    conn: &Connection,
    app_id: u32,
    owner: &str,
    error_category: &str,
    error_summary: &str,
    retry_delay_ms: i64,
    now_ms: i64,
) -> StorageResult<GameIngestionTask> {
    validate_failure(owner, error_category, retry_delay_ms)?;
    if is_global_failure(error_category) {
        return Err(StorageError::validation(
            "global auth/config failures must pause the lane",
        ));
    }
    let task = require_active_lease(conn, app_id, owner, now_ms)?;
    let failure_attempts = task.stage_failure_attempts.saturating_add(1);
    let dead = failure_attempts >= failure_limit(error_category);
    let summary = bounded_summary(error_summary);
    let changed = conn.execute(
        "UPDATE game_ingestion_queue
         SET status = CASE WHEN ?1 = 1 THEN 'dead' ELSE 'retry' END,
             stage_failure_attempts = ?2,
             total_failure_attempts = total_failure_attempts + 1,
             next_attempt_at_ms = CASE WHEN ?1 = 1 THEN ?3 ELSE ?4 END,
             last_error_category = ?5,
             last_error_summary = ?6,
             dead_at_ms = CASE WHEN ?1 = 1 THEN ?3 ELSE NULL END,
             lease_owner = NULL, lease_expires_at_ms = NULL, updated_at_ms = ?3
         WHERE app_id = ?7 AND lease_owner = ?8
           AND lease_expires_at_ms IS NOT NULL AND lease_expires_at_ms > ?3",
        params![
            i64::from(dead),
            failure_attempts,
            now_ms,
            now_ms.saturating_add(retry_delay_ms),
            error_category,
            summary,
            app_id,
            owner
        ],
    )?;
    if changed != 1 {
        return Err(StorageError::lease(format!(
            "game ingestion lease for app {app_id} is no longer active"
        )));
    }
    get_task(conn, app_id)?.ok_or_else(|| StorageError::not_found(format!("app {app_id} queue")))
}

pub fn pause_lane(
    conn: &Connection,
    owner: &str,
    error_category: &str,
    error_summary: &str,
    retry_delay_ms: i64,
    now_ms: i64,
) -> StorageResult<usize> {
    validate_failure(owner, error_category, retry_delay_ms)?;
    if !is_global_failure(error_category) {
        return Err(StorageError::validation(
            "only auth/config failures may pause an ingestion lane",
        ));
    }
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let requested_pause_until_ms = now_ms.saturating_add(retry_delay_ms);
    tx.execute(
        "INSERT INTO game_ingestion_lane_state(
             lane, pause_until_ms, last_error_category, last_error_summary,
             paused_at_ms, updated_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         ON CONFLICT(lane) DO UPDATE SET
             pause_until_ms = MAX(
                 game_ingestion_lane_state.pause_until_ms,
                 excluded.pause_until_ms
             ),
             last_error_category = excluded.last_error_category,
             last_error_summary = excluded.last_error_summary,
             paused_at_ms = excluded.paused_at_ms,
             updated_at_ms = excluded.updated_at_ms",
        params![
            LANE_NAME,
            requested_pause_until_ms,
            error_category,
            bounded_summary(error_summary),
            now_ms
        ],
    )?;
    let effective_pause_until_ms: i64 = tx.query_row(
        "SELECT pause_until_ms FROM game_ingestion_lane_state WHERE lane = ?1",
        [LANE_NAME],
        |row| row.get(0),
    )?;
    let paused_tasks = tx.execute(
        "UPDATE game_ingestion_queue
         SET status = 'retry', next_attempt_at_ms = ?1,
             last_error_category = ?2, last_error_summary = ?3,
             lease_owner = NULL, lease_expires_at_ms = NULL, updated_at_ms = ?4
         WHERE lease_owner = ?5",
        params![
            effective_pause_until_ms,
            error_category,
            bounded_summary(error_summary),
            now_ms,
            owner
        ],
    )?;
    tx.commit()?;
    Ok(paused_tasks)
}

pub fn requeue_dead_task(
    conn: &Connection,
    app_id: u32,
    stage: &str,
    operator: &str,
    reason: &str,
    now_ms: i64,
) -> StorageResult<bool> {
    validate_requeue(stage, operator, reason)?;
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let current = get_task(&tx, app_id)?
        .ok_or_else(|| StorageError::not_found(format!("app {app_id} queue")))?;
    if current.stage != stage {
        return Err(StorageError::validation(format!(
            "app {app_id} is at stage {}, not {stage}",
            current.stage
        )));
    }
    if current.status != "dead" {
        return Ok(false);
    }
    tx.execute(
        "INSERT INTO game_ingestion_requeue_audit(
             app_id, stage, previous_status, operator, reason, requeued_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            app_id,
            stage,
            current.status,
            operator.trim(),
            reason.trim(),
            now_ms
        ],
    )?;
    tx.execute(
        "UPDATE game_ingestion_queue
         SET status = 'pending', stage_failure_attempts = 0,
             next_attempt_at_ms = ?1, last_error_category = NULL,
             last_error_summary = NULL, dead_at_ms = NULL,
             lease_owner = NULL, lease_expires_at_ms = NULL, updated_at_ms = ?1
         WHERE app_id = ?2 AND stage = ?3 AND status = 'dead'",
        params![now_ms, app_id, stage],
    )?;
    tx.commit()?;
    Ok(true)
}

pub fn stage_observed(
    conn: &Connection,
    app_id: u32,
    stage: &str,
    country_code: &str,
    language: &str,
) -> StorageResult<bool> {
    let observed = match stage {
        "store_details" => conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM store_detail_refresh_state
                WHERE app_id = ?1 AND country_code = ?2 AND language = ?3
             )",
            params![app_id, country_code, language],
            |row| row.get(0),
        )?,
        "review_summary" => conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM review_snapshots WHERE app_id = ?1)",
            [app_id],
            |row| row.get(0),
        )?,
        "popular_reviews" => conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM popular_review_refresh_state WHERE app_id = ?1)",
            [app_id],
            |row| row.get(0),
        )?,
        "ccu" => conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM player_snapshots WHERE app_id = ?1)",
            [app_id],
            |row| row.get(0),
        )?,
        "complete" => true,
        _ => {
            return Err(StorageError::validation(format!(
                "unknown game ingestion stage {stage}"
            )));
        }
    };
    Ok(observed)
}

fn failure_limit(category: &str) -> i64 {
    match category {
        "invalid_payload" | "parse_changed" | "response_too_large" | "not_found" => {
            DETERMINISTIC_FAILURE_LIMIT
        }
        _ => TRANSIENT_FAILURE_LIMIT,
    }
}

fn is_global_failure(category: &str) -> bool {
    matches!(category, "auth" | "config")
}

fn bounded_summary(summary: &str) -> String {
    summary.chars().take(MAX_ERROR_SUMMARY_CHARS).collect()
}

fn validate_failure(owner: &str, category: &str, retry_delay_ms: i64) -> StorageResult<()> {
    if owner.trim().is_empty() {
        return Err(StorageError::validation("lease owner is required"));
    }
    if category.trim().is_empty() {
        return Err(StorageError::validation("error category is required"));
    }
    if !(1..=7 * 24 * 60 * 60 * 1000).contains(&retry_delay_ms) {
        return Err(StorageError::validation(
            "retry_delay_ms must be between 1 and 604800000",
        ));
    }
    Ok(())
}

fn validate_requeue(stage: &str, operator: &str, reason: &str) -> StorageResult<()> {
    if !matches!(
        stage,
        "store_details" | "review_summary" | "popular_reviews" | "ccu"
    ) {
        return Err(StorageError::validation("a requeueable stage is required"));
    }
    if operator.trim().is_empty() || operator.trim().chars().count() > 128 {
        return Err(StorageError::validation(
            "operator must contain between 1 and 128 characters",
        ));
    }
    if reason.trim().is_empty() || reason.trim().chars().count() > 512 {
        return Err(StorageError::validation(
            "reason must contain between 1 and 512 characters",
        ));
    }
    Ok(())
}

fn validate_lease(owner: &str, limit: i64, lease_ms: i64) -> StorageResult<()> {
    if owner.trim().is_empty() {
        return Err(StorageError::validation("lease owner is required"));
    }
    if !(1..=100).contains(&limit) {
        return Err(StorageError::validation(
            "claim limit must be between 1 and 100",
        ));
    }
    if !(1_000..=24 * 60 * 60 * 1000).contains(&lease_ms) {
        return Err(StorageError::validation(
            "lease_ms must be between 1000 and 86400000",
        ));
    }
    Ok(())
}

fn require_active_lease(
    conn: &Connection,
    app_id: u32,
    owner: &str,
    now_ms: i64,
) -> StorageResult<GameIngestionTask> {
    let task = get_task(conn, app_id)?
        .ok_or_else(|| StorageError::not_found(format!("app {app_id} queue")))?;
    if task.lease_owner.as_deref() != Some(owner)
        || task
            .lease_expires_at_ms
            .is_none_or(|expires| expires <= now_ms)
    {
        return Err(StorageError::lease(format!(
            "game ingestion lease for app {app_id} is not owned by {owner}"
        )));
    }
    Ok(task)
}

fn get_task(conn: &Connection, app_id: u32) -> StorageResult<Option<GameIngestionTask>> {
    conn.query_row(
        "SELECT app_id, source, priority, stage, status, stage_failure_attempts,
                total_failure_attempts, lease_count, next_attempt_at_ms,
                last_error_category, last_error_summary, lease_owner,
                lease_expires_at_ms, enrichment_profile, profile_version, dead_at_ms
         FROM game_ingestion_queue WHERE app_id = ?1",
        [app_id],
        |row| {
            Ok(GameIngestionTask {
                app_id: row.get(0)?,
                source: row.get(1)?,
                priority: row.get(2)?,
                stage: row.get(3)?,
                status: row.get(4)?,
                stage_failure_attempts: row.get(5)?,
                total_failure_attempts: row.get(6)?,
                lease_count: row.get(7)?,
                next_attempt_at_ms: row.get(8)?,
                last_error_category: row.get(9)?,
                last_error_summary: row.get(10)?,
                lease_owner: row.get(11)?,
                lease_expires_at_ms: row.get(12)?,
                enrichment_profile: row.get(13)?,
                profile_version: row.get(14)?,
                dead_at_ms: row.get(15)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}
