use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

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
    // Idempotent enqueue: return existing job id when key already present.
    if let Some(existing) = get_job_by_idempotency(conn, &job.idempotency_key)? {
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
        return Ok(existing.job_id);
    }

    conn.execute(
        "INSERT INTO jobs (
            source, task_type, entity_key, priority, attempts, max_attempts,
            due_at_ms, status, idempotency_key, payload_json, created_at_ms, updated_at_ms
         ) VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, 'pending', ?7, ?8, ?9, ?9)",
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
    )?;
    Ok(conn.last_insert_rowid())
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

    // BEGIN IMMEDIATE makes recovery, selection, and lease acquisition one
    // writer-serialized operation even when multiple Database handles point at
    // the same SQLite file.
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;

    // A lease consumes an attempt when it is acquired. An expired final
    // attempt must therefore become dead instead of being recycled forever.
    tx.execute(
        "UPDATE jobs
         SET status = CASE WHEN attempts >= max_attempts THEN 'dead' ELSE 'pending' END,
             lease_owner = NULL, lease_expires_at_ms = NULL, updated_at_ms = ?1
         WHERE status = 'leased'
           AND (lease_expires_at_ms IS NULL OR lease_expires_at_ms <= ?1)",
        params![now_ms],
    )?;
    // Repair pending rows left over by older recovery logic.
    tx.execute(
        "UPDATE jobs
         SET status = 'dead', lease_owner = NULL, lease_expires_at_ms = NULL, updated_at_ms = ?1
         WHERE status = 'pending' AND attempts >= max_attempts",
        params![now_ms],
    )?;

    let sql = if source_filter.is_some() {
        "SELECT job_id FROM jobs
         WHERE status = 'pending' AND attempts < max_attempts
           AND due_at_ms <= ?1 AND source = ?2
         ORDER BY priority ASC, due_at_ms ASC, job_id ASC
         LIMIT ?3"
    } else {
        "SELECT job_id FROM jobs
         WHERE status = 'pending' AND attempts < max_attempts AND due_at_ms <= ?1
         ORDER BY priority ASC, due_at_ms ASC, job_id ASC
         LIMIT ?2"
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
             SET status = 'leased', lease_owner = ?1, lease_expires_at_ms = ?2,
                 attempts = attempts + 1, updated_at_ms = ?3
             WHERE job_id = ?4 AND status = 'pending' AND attempts < max_attempts",
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

/// Return leases to the queue after the owning worker has been stopped.
///
/// This is intentionally separate from normal expiry recovery: callers must
/// first guarantee that no worker for the selected source is still running.
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
             SET status = CASE WHEN attempts >= max_attempts THEN 'dead' ELSE 'pending' END,
                 lease_owner = NULL, lease_expires_at_ms = NULL, updated_at_ms = ?1
             WHERE status = 'leased' AND source = ?2",
            params![now_ms, source],
        )?
    } else {
        tx.execute(
            "UPDATE jobs
             SET status = CASE WHEN attempts >= max_attempts THEN 'dead' ELSE 'pending' END,
                 lease_owner = NULL, lease_expires_at_ms = NULL, updated_at_ms = ?1
             WHERE status = 'leased'",
            params![now_ms],
        )?
    };
    tx.commit()?;
    Ok(changed)
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
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let changed = tx.execute(
        "UPDATE jobs
         SET status = 'completed', lease_owner = NULL, lease_expires_at_ms = NULL,
             completion_idempotency_key = ?1, updated_at_ms = ?2
         WHERE job_id = ?3 AND status = 'leased' AND lease_owner = ?4
           AND lease_expires_at_ms IS NOT NULL AND lease_expires_at_ms > ?2",
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
        "network" | "rate_limited" | "auth" | "not_found" | "parse_changed" | "invalid_payload"
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
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let changed = tx.execute(
        "UPDATE jobs
         SET status = CASE WHEN ?1 = 1 OR attempts >= max_attempts
                           THEN 'dead' ELSE 'pending' END,
             last_error_category = ?2,
             due_at_ms = CASE WHEN ?1 = 1 OR attempts >= max_attempts
                              THEN due_at_ms ELSE ?3 END,
             lease_owner = NULL, lease_expires_at_ms = NULL, updated_at_ms = ?4
         WHERE job_id = ?5 AND status = 'leased' AND lease_owner = ?6
           AND lease_expires_at_ms IS NOT NULL AND lease_expires_at_ms > ?4",
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
         FROM jobs WHERE job_id = ?1",
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
         FROM jobs WHERE idempotency_key = ?1",
        params![key],
        map_job,
    )
    .optional()
    .map_err(StorageError::from)
}

pub fn count_jobs_by_status(conn: &Connection, status: &str) -> StorageResult<i64> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM jobs WHERE status = ?1",
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
             WHERE source = ?1
               AND task_type = ?2
               AND entity_key = ?3
               AND status IN ('pending', 'leased')
         )",
        params![source, task_type, entity_key],
        |row| row.get::<_, i64>(0),
    )
    .map(|value| value != 0)
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

fn map_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobRecord> {
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
    fn stopped_worker_leases_are_recovered_by_source() {
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

            assert_eq!(recover_leased_jobs(conn, 1, Some("steam"))?, 2);
            let steam = get_job(conn, steam_job)?.expect("steam job");
            let other = get_job(conn, other_job)?.expect("other job");
            let final_attempt = get_job(conn, final_job)?.expect("final job");
            assert_eq!(steam.status, "pending");
            assert_eq!(steam.attempts, 1);
            assert_eq!(other.status, "leased");
            assert_eq!(final_attempt.status, "dead");

            let recovered = lease_jobs(conn, "worker-b", 1, 10_000, 1, Some("steam"))?;
            assert_eq!(recovered.len(), 1);
            assert_eq!(recovered[0].job_id, steam_job);
            assert_eq!(recovered[0].attempts, 2);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn concurrent_complete_and_fail_have_one_winner() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("jobs.db");
        let completing_db = Database::open(&path).unwrap();
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
        let failing_db = Database::open(&path).unwrap();
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

        let final_db = Database::open(&path).unwrap();
        let final_job = final_db
            .with_conn(|conn| get_job(conn, job_id))
            .unwrap()
            .expect("job");
        assert!(matches!(final_job.status.as_str(), "completed" | "pending"));
    }
}
