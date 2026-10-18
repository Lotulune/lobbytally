use crate::pg::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::error::{StorageError, StorageResult};
use crate::models::{EnqueueJob, JobRecord};

pub fn enqueue_job(conn: &Connection, job: &EnqueueJob, now_ms: i64) -> StorageResult<i64> {
    if job.idempotency_key.trim().is_empty() {
        return Err(StorageError::validation("idempotency_key is required"));
    }
    if job.idempotency_key.len() > 128 {
        return Err(StorageError::validation(
            "idempotency_key must be at most 128 bytes",
        ));
    }
    if job.source.trim().is_empty()
        || job.task_type.trim().is_empty()
        || job.entity_key.trim().is_empty()
    {
        return Err(StorageError::validation(
            "source, task_type, and entity_key are required",
        ));
    }
    if !(1..=100).contains(&job.max_attempts) {
        return Err(StorageError::validation(
            "max_attempts must be between 1 and 100",
        ));
    }
    if let Some(payload) = &job.payload_json {
        serde_json::from_str::<serde_json::Value>(payload)
            .map_err(|_| StorageError::validation("payload_json must be valid JSON"))?;
    }
    let inserted = conn
        .query_row(
            "INSERT INTO jobs (
            source, task_type, entity_key, priority, attempts, max_attempts,
            due_at_ms, status, idempotency_key, payload_json, created_at_ms, updated_at_ms
         ) VALUES ($1, $2, $3, $4, 0, $5, $6, 'pending', $7, $8, $9, $9)
         ON CONFLICT(idempotency_key) DO NOTHING
         RETURNING job_id",
            params![
                job.source,
                job.task_type,
                job.entity_key,
                job.priority,
                job.max_attempts,
                job.due_at_ms,
                job.idempotency_key,
                job.payload_json,
                now_ms
            ],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(job_id) = inserted {
        return Ok(job_id);
    }
    // The unique key arbitrates concurrent enqueue attempts across instances.
    let existing = get_job_by_idempotency(conn, &job.idempotency_key)?
        .ok_or_else(|| StorageError::conflict("idempotent job disappeared during enqueue"))?;
    if existing.source != job.source
        || existing.task_type != job.task_type
        || existing.entity_key != job.entity_key
        || existing.priority != job.priority
        || existing.max_attempts != job.max_attempts
        || existing.payload_json != job.payload_json
    {
        return Err(StorageError::conflict(
            "idempotency key reused with different job payload",
        ));
    }
    Ok(existing.job_id)
}

/// Atomically enqueue a job unless an equivalent pending or leased job exists.
pub fn enqueue_job_if_no_active(
    conn: &Connection,
    job: &EnqueueJob,
    now_ms: i64,
) -> StorageResult<Option<i64>> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    if has_active_job(&tx, &job.source, &job.task_type, &job.entity_key)? {
        tx.commit()?;
        return Ok(None);
    }
    let job_id = enqueue_job(&tx, job, now_ms)?;
    tx.commit()?;
    Ok(Some(job_id))
}

pub fn lease_jobs(
    conn: &Connection,
    owner: &str,
    limit: i64,
    lease_ms: i64,
    now_ms: i64,
    source_filter: Option<&str>,
) -> StorageResult<Vec<JobRecord>> {
    if owner.trim().is_empty() {
        return Err(StorageError::validation("lease owner is required"));
    }
    if !(1..=100).contains(&limit) {
        return Err(StorageError::validation(
            "lease limit must be between 1 and 100",
        ));
    }
    if !(1_000..=24 * 60 * 60 * 1000).contains(&lease_ms) {
        return Err(StorageError::validation(
            "lease_ms must be between 1000 and 86400000",
        ));
    }

    // Recovery and acquisition share a transaction. SKIP LOCKED and the guarded
    // update arbitrate ownership; unrelated catalog writes must not block claims.
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Deferred)?;

    // A lease consumes an attempt when it is acquired. An expired final
    // attempt must therefore become dead instead of being recycled forever.
    tx.execute(
        "UPDATE jobs
         SET status = CASE WHEN attempts >= max_attempts THEN 'dead' ELSE 'pending' END,
             lease_owner = NULL, lease_expires_at_ms = NULL, updated_at_ms = $1
         WHERE status = 'leased'
           AND (lease_expires_at_ms IS NULL OR lease_expires_at_ms <= $1)",
        params![now_ms],
    )?;
    // Repair pending rows left over by older recovery logic.
    tx.execute(
        "UPDATE jobs
         SET status = 'dead', lease_owner = NULL, lease_expires_at_ms = NULL, updated_at_ms = $1
         WHERE status = 'pending' AND attempts >= max_attempts",
        params![now_ms],
    )?;

    let sql = if source_filter.is_some() {
        "SELECT job_id FROM jobs
         WHERE status = 'pending' AND attempts < max_attempts
           AND due_at_ms <= $1 AND source = $2
         ORDER BY priority ASC, due_at_ms ASC, job_id ASC
         LIMIT $3 FOR UPDATE SKIP LOCKED"
    } else {
        "SELECT job_id FROM jobs
         WHERE status = 'pending' AND attempts < max_attempts AND due_at_ms <= $1
         ORDER BY priority ASC, due_at_ms ASC, job_id ASC
         LIMIT $2 FOR UPDATE SKIP LOCKED"
    };

    let ids: Vec<i64> = if let Some(source) = source_filter {
        let mut stmt = tx.prepare(sql)?;
        let rows = stmt.query_map(params![now_ms, source, limit], |row| row.get(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    } else {
        let mut stmt = tx.prepare(sql)?;
        let rows = stmt.query_map(params![now_ms, limit], |row| row.get(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    let mut leased = Vec::new();
    let expires = now_ms.saturating_add(lease_ms);
    for id in ids {
        let changed = tx.execute(
            "UPDATE jobs
             SET status = 'leased', lease_owner = $1, lease_expires_at_ms = $2,
                 attempts = attempts + 1, updated_at_ms = $3
             WHERE job_id = $4 AND status = 'pending' AND attempts < max_attempts",
            params![owner, expires, now_ms, id],
        )?;
        if changed == 1
            && let Some(job) = get_job(&tx, id)?
        {
            leased.push(job);
        }
    }
    tx.commit()?;
    Ok(leased)
}

/// Return expired leases to the queue without consuming a failure attempt.
/// Live leases belong to independent cloud invocations and must stay intact.
pub fn recover_leased_jobs(
    conn: &Connection,
    now_ms: i64,
    source_filter: Option<&str>,
) -> StorageResult<usize> {
    if source_filter.is_some_and(|source| source.trim().is_empty()) {
        return Err(StorageError::validation(
            "lease recovery source must not be empty",
        ));
    }
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let changed = if let Some(source) = source_filter {
        tx.execute(
            "UPDATE jobs
             SET status = 'pending', attempts = GREATEST(attempts - 1, 0),
                 lease_owner = NULL, lease_expires_at_ms = NULL, updated_at_ms = $1
             WHERE status = 'leased' AND source = $2
               AND (lease_expires_at_ms IS NULL OR lease_expires_at_ms <= $1)",
            params![now_ms, source],
        )?
    } else {
        tx.execute(
            "UPDATE jobs
             SET status = 'pending', attempts = GREATEST(attempts - 1, 0),
                 lease_owner = NULL, lease_expires_at_ms = NULL, updated_at_ms = $1
             WHERE status = 'leased'
               AND (lease_expires_at_ms IS NULL OR lease_expires_at_ms <= $1)",
            params![now_ms],
        )?
    };
    tx.commit()?;
    Ok(changed)
}

/// Return an unfinished bounded batch to the queue without consuming a retry.
pub fn yield_job(
    conn: &Connection,
    job_id: i64,
    owner: &str,
    delay_ms: i64,
    now_ms: i64,
) -> StorageResult<JobRecord> {
    if owner.trim().is_empty() {
        return Err(StorageError::validation("lease owner is required"));
    }
    if !(0..=7 * 24 * 60 * 60 * 1000).contains(&delay_ms) {
        return Err(StorageError::validation(
            "delay_ms must be between 0 and 604800000",
        ));
    }
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let changed = tx.execute(
        "UPDATE jobs
         SET status = 'pending', attempts = GREATEST(attempts - 1, 0),
             due_at_ms = $1, lease_owner = NULL, lease_expires_at_ms = NULL,
             updated_at_ms = $2
         WHERE job_id = $3 AND status = 'leased' AND lease_owner = $4
           AND lease_expires_at_ms IS NOT NULL AND lease_expires_at_ms > $2",
        params![now_ms.saturating_add(delay_ms), now_ms, job_id, owner],
    )?;
    let job =
        get_job(&tx, job_id)?.ok_or_else(|| StorageError::not_found(format!("job {job_id}")))?;
    if changed != 1 {
        return Err(lease_state_error(&job, owner, now_ms, "yielded"));
    }
    tx.commit()?;
    Ok(job)
}

/// Serialize cloud batches while allowing a later invocation to recover a crash.
pub fn try_acquire_worker_lease(
    conn: &Connection,
    owner: &str,
    lease_ms: i64,
    now_ms: i64,
) -> StorageResult<bool> {
    if owner.trim().is_empty() {
        return Err(StorageError::validation("worker lease owner is required"));
    }
    if !(1_000..=24 * 60 * 60 * 1000).contains(&lease_ms) {
        return Err(StorageError::validation(
            "lease_ms must be between 1000 and 86400000",
        ));
    }
    let changed = conn.execute(
        "INSERT INTO worker_leases (
             lease_name, lease_owner, lease_expires_at_ms, updated_at_ms
         ) VALUES ('cloud_batch', $1, $2, $3)
         ON CONFLICT(lease_name) DO UPDATE SET
             lease_owner = excluded.lease_owner,
             lease_expires_at_ms = excluded.lease_expires_at_ms,
             updated_at_ms = excluded.updated_at_ms
         WHERE worker_leases.lease_expires_at_ms <= $3
            OR worker_leases.lease_owner = excluded.lease_owner",
        params![owner, now_ms.saturating_add(lease_ms), now_ms],
    )?;
    Ok(changed == 1)
}

pub fn release_worker_lease(conn: &Connection, owner: &str, now_ms: i64) -> StorageResult<()> {
    if owner.trim().is_empty() {
        return Err(StorageError::validation("worker lease owner is required"));
    }
    conn.execute(
        "UPDATE worker_leases
         SET lease_expires_at_ms = $1, updated_at_ms = $1
         WHERE lease_name = 'cloud_batch' AND lease_owner = $2",
        params![now_ms, owner],
    )?;
    Ok(())
}

pub fn complete_job(
    conn: &Connection,
    job_id: i64,
    owner: &str,
    idempotency_key: &str,
    now_ms: i64,
) -> StorageResult<JobRecord> {
    if owner.trim().is_empty() {
        return Err(StorageError::validation("lease owner is required"));
    }
    if idempotency_key.trim().is_empty() || idempotency_key.len() > 128 {
        return Err(StorageError::validation(
            "completion idempotency_key must be between 1 and 128 bytes",
        ));
    }
    // The conditional UPDATE locks this job and verifies its live owner.
    // Completion must not contend with an unrelated global write transaction.
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Deferred)?;
    let changed = tx.execute(
        "UPDATE jobs
         SET status = 'completed', lease_owner = NULL, lease_expires_at_ms = NULL,
             completion_idempotency_key = $1, updated_at_ms = $2
         WHERE job_id = $3 AND status = 'leased' AND lease_owner = $4
           AND lease_expires_at_ms IS NOT NULL AND lease_expires_at_ms > $2",
        params![idempotency_key, now_ms, job_id, owner],
    )?;
    let job =
        get_job(&tx, job_id)?.ok_or_else(|| StorageError::not_found(format!("job {job_id}")))?;
    if changed == 1 {
        tx.commit()?;
        return Ok(job);
    }
    if job.status == "completed" {
        if job.completion_idempotency_key.as_deref() == Some(idempotency_key) {
            tx.commit()?;
            return Ok(job);
        }
        return Err(StorageError::conflict(
            "job already completed with different idempotency context",
        ));
    }
    Err(lease_state_error(&job, owner, now_ms, "completed"))
}

pub fn fail_job(
    conn: &Connection,
    job_id: i64,
    owner: &str,
    error_category: &str,
    retry_delay_ms: i64,
    now_ms: i64,
) -> StorageResult<JobRecord> {
    if owner.trim().is_empty() {
        return Err(StorageError::validation("lease owner is required"));
    }
    if !matches!(
        error_category,
        "network"
            | "storage"
            | "rate_limited"
            | "auth"
            | "not_found"
            | "parse_changed"
            | "invalid_payload"
    ) {
        return Err(StorageError::validation("unknown stable error_category"));
    }
    if !(1..=7 * 24 * 60 * 60 * 1000).contains(&retry_delay_ms) {
        return Err(StorageError::validation(
            "retry_delay_ms must be between 1 and 604800000",
        ));
    }

    let permanent = matches!(
        error_category,
        "auth" | "not_found" | "invalid_payload" | "parse_changed"
    );
    let due = now_ms.saturating_add(retry_delay_ms.max(1));
    // Ownership and expiry are checked by the row-locking UPDATE itself.
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Deferred)?;
    let changed = tx.execute(
        "UPDATE jobs
         SET status = CASE WHEN $1 = 1 OR attempts >= max_attempts
                           THEN 'dead' ELSE 'pending' END,
             last_error_category = $2,
             due_at_ms = CASE WHEN $1 = 1 OR attempts >= max_attempts
                              THEN due_at_ms ELSE $3 END,
             lease_owner = NULL, lease_expires_at_ms = NULL, updated_at_ms = $4
         WHERE job_id = $5 AND status = 'leased' AND lease_owner = $6
           AND lease_expires_at_ms IS NOT NULL AND lease_expires_at_ms > $4",
        params![
            i64::from(permanent),
            error_category,
            due,
            now_ms,
            job_id,
            owner
        ],
    )?;
    let job =
        get_job(&tx, job_id)?.ok_or_else(|| StorageError::not_found(format!("job {job_id}")))?;
    if changed == 1 {
        tx.commit()?;
        return Ok(job);
    }
    Err(lease_state_error(&job, owner, now_ms, "failed"))
}

pub fn get_job(conn: &Connection, job_id: i64) -> StorageResult<Option<JobRecord>> {
    conn.query_row(
        "SELECT job_id, source, task_type, entity_key, priority, attempts, max_attempts,
                due_at_ms, status, lease_owner, lease_expires_at_ms, idempotency_key,
                completion_idempotency_key, payload_json, last_error_category
         FROM jobs WHERE job_id = $1",
        params![job_id],
        map_job,
    )
    .optional()
    .map_err(StorageError::from)
}

pub fn get_job_by_idempotency(conn: &Connection, key: &str) -> StorageResult<Option<JobRecord>> {
    conn.query_row(
        "SELECT job_id, source, task_type, entity_key, priority, attempts, max_attempts,
                due_at_ms, status, lease_owner, lease_expires_at_ms, idempotency_key,
                completion_idempotency_key, payload_json, last_error_category
         FROM jobs WHERE idempotency_key = $1",
        params![key],
        map_job,
    )
    .optional()
    .map_err(StorageError::from)
}

pub fn count_jobs_by_status(conn: &Connection, status: &str) -> StorageResult<i64> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM jobs WHERE status = $1",
        params![status],
        |row| row.get(0),
    )?;
    Ok(n)
}

/// Whether an equivalent job is still pending or leased.
///
/// Schedulers use this instead of relying solely on time-slot idempotency:
/// a slow worker must not allow a new scheduled job to accumulate behind an
/// older equivalent job.
pub fn has_active_job(
    conn: &Connection,
    source: &str,
    task_type: &str,
    entity_key: &str,
) -> StorageResult<bool> {
    if source.trim().is_empty() || task_type.trim().is_empty() || entity_key.trim().is_empty() {
        return Err(StorageError::validation(
            "source, task_type, and entity_key are required",
        ));
    }
    conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM jobs
             WHERE source = $1
               AND task_type = $2
               AND entity_key = $3
               AND status IN ('pending', 'leased')
         )",
        params![source, task_type, entity_key],
        |row| row.get::<_, bool>(0),
    )
    .map_err(Into::into)
}

fn lease_state_error(job: &JobRecord, owner: &str, now_ms: i64, action: &str) -> StorageError {
    if job.status != "leased" {
        return StorageError::lease(format!(
            "job {} cannot be {action} (status={})",
            job.job_id, job.status
        ));
    }
    if job.lease_owner.as_deref() != Some(owner) {
        return StorageError::lease(format!("job {} is leased by another owner", job.job_id));
    }
    if job
        .lease_expires_at_ms
        .is_none_or(|expires| expires <= now_ms)
    {
        return StorageError::lease(format!("job {} lease expired", job.job_id));
    }
    StorageError::lease(format!("job {} lease state changed", job.job_id))
}

fn map_job(row: &crate::pg::Row<'_>) -> crate::pg::Result<JobRecord> {
    Ok(JobRecord {
        job_id: row.get(0)?,
        source: row.get(1)?,
        task_type: row.get(2)?,
        entity_key: row.get(3)?,
        priority: row.get(4)?,
        attempts: row.get(5)?,
        max_attempts: row.get(6)?,
        due_at_ms: row.get(7)?,
        status: row.get(8)?,
        lease_owner: row.get(9)?,
        lease_expires_at_ms: row.get(10)?,
        idempotency_key: row.get(11)?,
        completion_idempotency_key: row.get(12)?,
        payload_json: row.get(13)?,
        last_error_category: row.get(14)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use std::sync::{Arc, Barrier};

    #[test]
    fn avatar_jobs_progress_while_an_unrelated_global_write_is_open() {
        let writer = Database::open_in_memory().unwrap();
        writer.migrate().unwrap();
        let cleanup = writer.reconnect().unwrap();
        let job_ids = writer
            .with_conn_mut(|conn| {
                ["avatar-complete", "avatar-retry"]
                    .into_iter()
                    .map(|key| {
                        enqueue_job(
                            conn,
                            &EnqueueJob {
                                source: "avatar_cleanup".into(),
                                task_type: "delete_avatar".into(),
                                entity_key: key.into(),
                                priority: 10,
                                due_at_ms: 0,
                                idempotency_key: key.into(),
                                payload_json: None,
                                max_attempts: 3,
                            },
                            0,
                        )
                    })
                    .collect::<StorageResult<Vec<_>>>()
            })
            .unwrap();

        writer
            .with_conn_mut(|conn| {
                let global_write =
                    conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                global_write.execute(
                    "UPDATE feed_snapshot_state SET snapshot_ms = 1 WHERE singleton = 1",
                    [],
                )?;
                cleanup.with_conn_mut(|conn| {
                    let first = lease_jobs(conn, "cleanup", 1, 10_000, 0, Some("avatar_cleanup"))?;
                    assert_eq!(first.len(), 1);
                    assert_eq!(first[0].job_id, job_ids[0]);
                    assert_eq!(first[0].attempts, 1);
                    assert_eq!(
                        complete_job(conn, first[0].job_id, "cleanup", "removed", 1)?.status,
                        "completed"
                    );
                    assert_eq!(
                        complete_job(conn, first[0].job_id, "cleanup", "removed", 1)?.status,
                        "completed"
                    );

                    let second = lease_jobs(conn, "cleanup", 1, 10_000, 0, Some("avatar_cleanup"))?;
                    assert_eq!(second.len(), 1);
                    assert_eq!(second[0].job_id, job_ids[1]);
                    let failed = fail_job(conn, second[0].job_id, "cleanup", "network", 1_000, 1)?;
                    assert_eq!(failed.status, "pending");
                    assert_eq!(failed.attempts, 1);
                    assert_eq!(failed.due_at_ms, 1_001);
                    Ok(())
                })?;
                global_write.rollback()?;
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn concurrent_avatar_claims_skip_locked_jobs_without_duplicate_ownership() {
        let first = Database::open_in_memory().unwrap();
        first.migrate().unwrap();
        let second = first.reconnect().unwrap();
        let job_ids = first
            .with_conn_mut(|conn| {
                ["locked-avatar", "available-avatar"]
                    .into_iter()
                    .map(|key| {
                        enqueue_job(
                            conn,
                            &EnqueueJob {
                                source: "avatar_cleanup".into(),
                                task_type: "delete_avatar".into(),
                                entity_key: key.into(),
                                priority: 10,
                                due_at_ms: 0,
                                idempotency_key: key.into(),
                                payload_json: None,
                                max_attempts: 3,
                            },
                            0,
                        )
                    })
                    .collect::<StorageResult<Vec<_>>>()
            })
            .unwrap();

        first
            .with_conn_mut(|conn| {
                let claim = conn.transaction_with_behavior(TransactionBehavior::Deferred)?;
                claim.query_row(
                    "SELECT job_id FROM jobs WHERE job_id = $1 FOR UPDATE",
                    [job_ids[0]],
                    |row| row.get::<_, i64>(0),
                )?;
                let other = second.with_conn_mut(|conn| {
                    lease_jobs(conn, "second", 2, 10_000, 0, Some("avatar_cleanup"))
                })?;
                assert_eq!(other.len(), 1);
                assert_eq!(other[0].job_id, job_ids[1]);
                let owned = lease_jobs(&claim, "first", 2, 10_000, 0, Some("avatar_cleanup"))?;
                assert_eq!(owned.len(), 1);
                assert_eq!(owned[0].job_id, job_ids[0]);
                claim.commit()?;
                let mut statement =
                    conn.prepare("SELECT job_id, lease_owner, attempts FROM jobs ORDER BY job_id")?;
                let owners = statement
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    })?
                    .collect::<crate::pg::Result<Vec<_>>>()?;
                assert_eq!(
                    owners,
                    vec![
                        (job_ids[0], "first".into(), 1),
                        (job_ids[1], "second".into(), 1)
                    ]
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn conditional_enqueue_keeps_one_active_job_per_lane() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        db.with_conn_mut(|conn| {
            let first = EnqueueJob {
                source: "steam".into(),
                task_type: "enrich_catalog".into(),
                entity_key: "scheduled".into(),
                priority: 50,
                due_at_ms: 0,
                idempotency_key: "enrichment-first".into(),
                payload_json: None,
                max_attempts: 3,
            };
            assert!(enqueue_job_if_no_active(conn, &first, 0)?.is_some());

            let duplicate = EnqueueJob {
                idempotency_key: "enrichment-duplicate".into(),
                ..first.clone()
            };
            assert!(enqueue_job_if_no_active(conn, &duplicate, 0)?.is_none());

            let leased = lease_jobs(conn, "worker", 1, 60_000, 0, Some("steam"))?;
            complete_job(conn, leased[0].job_id, "worker", "completed", 1)?;
            assert!(enqueue_job_if_no_active(conn, &duplicate, 1)?.is_some());
            assert_eq!(count_jobs_by_status(conn, "pending")?, 1);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn an_expired_final_attempt_is_recovered_as_dead() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        db.with_conn_mut(|conn| {
            let job_id = enqueue_job(
                conn,
                &EnqueueJob {
                    source: "test".into(),
                    task_type: "refresh".into(),
                    entity_key: "42".into(),
                    priority: 1,
                    due_at_ms: 0,
                    idempotency_key: "final-attempt".into(),
                    payload_json: None,
                    max_attempts: 1,
                },
                0,
            )?;
            let first = lease_jobs(conn, "worker-a", 1, 1_000, 0, None)?;
            assert_eq!(first.len(), 1);
            assert_eq!(first[0].attempts, 1);

            let retried = lease_jobs(conn, "worker-b", 1, 1_000, 1_000, None)?;
            assert!(retried.is_empty());
            let recovered = get_job(conn, job_id)?.expect("job");
            assert_eq!(recovered.status, "dead");
            assert_eq!(recovered.attempts, 1);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn expired_worker_leases_are_recovered_by_source_without_releasing_live_owners() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        db.with_conn_mut(|conn| {
            let steam_job = enqueue_job(
                conn,
                &EnqueueJob {
                    source: "steam".into(),
                    task_type: "collect_candidates".into(),
                    entity_key: "global".into(),
                    priority: 1,
                    due_at_ms: 0,
                    idempotency_key: "recover-steam".into(),
                    payload_json: None,
                    max_attempts: 3,
                },
                0,
            )?;
            let other_job = enqueue_job(
                conn,
                &EnqueueJob {
                    source: "other".into(),
                    task_type: "refresh".into(),
                    entity_key: "global".into(),
                    priority: 1,
                    due_at_ms: 0,
                    idempotency_key: "recover-other".into(),
                    payload_json: None,
                    max_attempts: 3,
                },
                0,
            )?;
            let final_job = enqueue_job(
                conn,
                &EnqueueJob {
                    source: "steam".into(),
                    task_type: "sync_catalog".into(),
                    entity_key: "global".into(),
                    priority: 1,
                    due_at_ms: 0,
                    idempotency_key: "recover-final".into(),
                    payload_json: None,
                    max_attempts: 1,
                },
                0,
            )?;
            assert_eq!(lease_jobs(conn, "worker-a", 3, 10_000, 0, None)?.len(), 3);

            assert_eq!(recover_leased_jobs(conn, 1, Some("steam"))?, 0);
            assert_eq!(recover_leased_jobs(conn, 9_999, None)?, 0);
            let active = get_job(conn, steam_job)?.expect("active steam job");
            assert_eq!(active.status, "leased");
            assert_eq!(active.lease_owner.as_deref(), Some("worker-a"));
            assert_eq!(active.lease_expires_at_ms, Some(10_000));
            assert_eq!(active.attempts, 1);

            assert_eq!(recover_leased_jobs(conn, 10_000, Some("steam"))?, 2);
            let steam = get_job(conn, steam_job)?.expect("steam job");
            let other = get_job(conn, other_job)?.expect("other job");
            let final_attempt = get_job(conn, final_job)?.expect("final job");
            assert_eq!(steam.status, "pending");
            assert_eq!(steam.attempts, 0);
            assert_eq!(other.status, "leased");
            assert_eq!(final_attempt.status, "pending");
            assert_eq!(final_attempt.attempts, 0);
            assert_eq!(recover_leased_jobs(conn, 10_000, None)?, 1);

            let recovered = lease_jobs(conn, "worker-b", 1, 10_000, 10_000, Some("steam"))?;
            assert_eq!(recovered.len(), 1);
            assert_eq!(recovered[0].job_id, steam_job);
            assert_eq!(recovered[0].attempts, 1);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn storage_failures_keep_their_category_and_existing_attempt_limit() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        db.with_conn_mut(|conn| {
            let job_id = enqueue_job(
                conn,
                &EnqueueJob {
                    source: "maintenance".into(),
                    task_type: "pipeline_snapshot".into(),
                    entity_key: "scheduled".into(),
                    priority: 50,
                    due_at_ms: 0,
                    idempotency_key: "storage-failure-limit".into(),
                    payload_json: None,
                    max_attempts: 2,
                },
                0,
            )?;
            assert_eq!(lease_jobs(conn, "first", 1, 10_000, 0, None)?.len(), 1);
            let retry = fail_job(conn, job_id, "first", "storage", 1_000, 1)?;
            assert_eq!(retry.status, "pending");
            assert_eq!(retry.attempts, 1);
            assert_eq!(retry.last_error_category.as_deref(), Some("storage"));
            assert_eq!(lease_jobs(conn, "second", 1, 10_000, 1_001, None)?.len(), 1);
            let dead = fail_job(conn, job_id, "second", "storage", 1_000, 1_002)?;
            assert_eq!(dead.status, "dead");
            assert_eq!(dead.attempts, 2);
            assert_eq!(dead.last_error_category.as_deref(), Some("storage"));
            assert!(lease_jobs(conn, "third", 1, 10_000, 2_002, None)?.is_empty());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn concurrent_complete_and_fail_have_one_winner() {
        let completing_db = Database::open_in_memory().unwrap();
        completing_db.migrate().unwrap();
        let job_id = completing_db
            .with_conn_mut(|conn| {
                let job_id = enqueue_job(
                    conn,
                    &EnqueueJob {
                        source: "test".into(),
                        task_type: "refresh".into(),
                        entity_key: "42".into(),
                        priority: 1,
                        due_at_ms: 0,
                        idempotency_key: "complete-fail-race".into(),
                        payload_json: None,
                        max_attempts: 3,
                    },
                    0,
                )?;
                assert_eq!(lease_jobs(conn, "worker", 1, 10_000, 0, None)?.len(), 1);
                Ok(job_id)
            })
            .unwrap();
        let failing_db = completing_db.reconnect().unwrap();
        let final_db = completing_db.reconnect().unwrap();
        let barrier = Arc::new(Barrier::new(3));

        let complete_barrier = barrier.clone();
        let complete = std::thread::spawn(move || {
            complete_barrier.wait();
            completing_db
                .with_conn_mut(|conn| complete_job(conn, job_id, "worker", "completion", 1))
        });
        let fail_barrier = barrier.clone();
        let fail = std::thread::spawn(move || {
            fail_barrier.wait();
            failing_db.with_conn_mut(|conn| fail_job(conn, job_id, "worker", "network", 1_000, 1))
        });
        barrier.wait();
        let complete_result = complete.join().unwrap();
        let fail_result = fail.join().unwrap();
        assert_eq!(
            usize::from(complete_result.is_ok()) + usize::from(fail_result.is_ok()),
            1
        );

        let final_job = final_db
            .with_conn(|conn| get_job(conn, job_id))
            .unwrap()
            .expect("job");
        assert!(matches!(final_job.status.as_str(), "completed" | "pending"));
    }

    #[test]
    fn bounded_yield_preserves_job_and_refunds_only_an_owned_live_lease() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        db.with_conn_mut(|conn| {
            let job_id = enqueue_job(
                conn,
                &EnqueueJob {
                    source: "steam".into(),
                    task_type: "sync_catalog".into(),
                    entity_key: "global".into(),
                    priority: 1,
                    due_at_ms: 0,
                    idempotency_key: "bounded-yield".into(),
                    payload_json: Some(r#"{"cursor":"page-2"}"#.into()),
                    max_attempts: 1,
                },
                0,
            )?;
            assert_eq!(lease_jobs(conn, "worker-a", 1, 1_000, 0, None)?.len(), 1);
            assert!(yield_job(conn, job_id, "worker-b", 100, 1).is_err());
            let yielded = yield_job(conn, job_id, "worker-a", 100, 1)?;
            assert_eq!(yielded.status, "pending");
            assert_eq!(yielded.attempts, 0);
            assert_eq!(yielded.due_at_ms, 101);
            assert_eq!(
                yielded.payload_json.as_deref(),
                Some(r#"{"cursor":"page-2"}"#)
            );
            assert!(lease_jobs(conn, "worker-b", 1, 1_000, 100, None)?.is_empty());
            let resumed = lease_jobs(conn, "worker-b", 1, 1_000, 101, None)?;
            assert_eq!(resumed.len(), 1);
            assert_eq!(resumed[0].attempts, 1);
            assert!(yield_job(conn, job_id, "worker-b", 0, 1_101).is_err());
            assert_eq!(get_job(conn, job_id)?.unwrap().attempts, 1);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn worker_lease_arbitrates_across_connections_and_ignores_stale_release() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let workers: Vec<_> = ["worker-a", "worker-b"]
            .into_iter()
            .map(|owner| {
                let worker_db = db.reconnect().unwrap();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    worker_db.with_conn(|conn| try_acquire_worker_lease(conn, owner, 1_000, 0))
                })
            })
            .collect();
        barrier.wait();
        let winners = workers
            .into_iter()
            .map(|worker| usize::from(worker.join().unwrap().unwrap()))
            .sum::<usize>();
        assert_eq!(winners, 1);
        db.with_conn(|conn| {
            assert!(!try_acquire_worker_lease(conn, "worker-c", 1_000, 999)?);
            assert!(try_acquire_worker_lease(conn, "worker-c", 1_000, 1_000)?);
            release_worker_lease(conn, "worker-a", 1_001)?;
            release_worker_lease(conn, "worker-b", 1_001)?;
            assert!(!try_acquire_worker_lease(conn, "worker-d", 1_000, 1_001)?);
            release_worker_lease(conn, "worker-c", 1_002)?;
            assert!(try_acquire_worker_lease(conn, "worker-d", 1_000, 1_002)?);
            Ok(())
        })
        .unwrap();
    }
}
