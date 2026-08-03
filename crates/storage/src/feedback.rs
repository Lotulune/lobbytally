use mpgs_domain::FeedbackType;
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::catalog;
use crate::error::{StorageError, StorageResult};

#[derive(Debug, Clone, PartialEq)]
pub struct FeedbackRecord {
    pub feedback_id: i64,
    pub user_id: String,
    pub app_id: u32,
    pub feedback_type: String,
    pub recommendation_run_id: Option<String>,
    pub idempotency_key: String,
    pub client_created_at_ms: Option<i64>,
    pub created_at_ms: i64,
    pub undone_by: Option<i64>,
    pub request_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveFeedback {
    pub app_id: u32,
    pub feedback_type: String,
}

pub fn request_fingerprint(
    app_id: u32,
    feedback_type: FeedbackType,
    recommendation_run_id: Option<&str>,
    client_created_at_ms: Option<i64>,
) -> StorageResult<String> {
    let canonical = serde_json::to_vec(&(
        app_id,
        feedback_type.as_str(),
        recommendation_run_id,
        client_created_at_ms,
    ))?;
    Ok(hex_sha256(&canonical))
}

#[allow(clippy::too_many_arguments)]
pub fn create_feedback(
    conn: &Connection,
    user_id: &str,
    app_id: u32,
    feedback_type: FeedbackType,
    recommendation_run_id: Option<&str>,
    idempotency_key: &str,
    client_created_at_ms: Option<i64>,
    payload_fingerprint: &str,
    now_ms: i64,
) -> StorageResult<FeedbackRecord> {
    if idempotency_key.trim().is_empty() {
        return Err(StorageError::validation("Idempotency-Key is required"));
    }
    if idempotency_key.len() > 128 {
        return Err(StorageError::validation(
            "Idempotency-Key must be at most 128 bytes",
        ));
    }
    if recommendation_run_id.is_some_and(|value| value.len() > 128) {
        return Err(StorageError::validation(
            "recommendation_run_id must be at most 128 bytes",
        ));
    }
    if catalog::get_app(conn, app_id)?.is_none() {
        return Err(StorageError::not_found(format!("game {app_id}")));
    }

    if let Some(mut existing) = get_by_idempotency(conn, user_id, idempotency_key)? {
        let legacy_payload_matches = existing.app_id == app_id
            && existing.feedback_type == feedback_type.as_str()
            && existing.recommendation_run_id.as_deref() == recommendation_run_id
            && existing.client_created_at_ms == client_created_at_ms;
        if (!existing.request_fingerprint.is_empty()
            && existing.request_fingerprint != payload_fingerprint)
            || (existing.request_fingerprint.is_empty() && !legacy_payload_matches)
        {
            return Err(StorageError::conflict(
                "idempotency key reused with different payload",
            ));
        }
        if existing.request_fingerprint.is_empty() {
            conn.execute(
                "UPDATE feedback_events SET request_fingerprint = ?1 WHERE feedback_id = ?2",
                params![payload_fingerprint, existing.feedback_id],
            )?;
            existing.request_fingerprint = payload_fingerprint.to_owned();
        }
        return Ok(existing);
    }

    conn.execute(
        "INSERT INTO feedback_events (
            user_id, app_id, feedback_type, recommendation_run_id, idempotency_key,
            client_created_at_ms, created_at_ms, request_fingerprint
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            user_id,
            app_id,
            feedback_type.as_str(),
            recommendation_run_id,
            idempotency_key,
            client_created_at_ms,
            now_ms,
            payload_fingerprint
        ],
    )?;
    let id = conn.last_insert_rowid();
    get_feedback(conn, id)?.ok_or_else(|| StorageError::not_found(format!("feedback {id}")))
}

pub fn undo_feedback(
    conn: &mut Connection,
    user_id: &str,
    feedback_id: i64,
    now_ms: i64,
) -> StorageResult<FeedbackRecord> {
    let tx = conn.transaction()?;
    let original = get_feedback(&tx, feedback_id)?
        .ok_or_else(|| StorageError::not_found(format!("feedback {feedback_id}")))?;
    if original.user_id != user_id {
        return Err(StorageError::not_found(format!("feedback {feedback_id}")));
    }
    if original.feedback_type == "undo" {
        return Err(StorageError::validation("an undo event cannot be undone"));
    }
    if let Some(undo_id) = original.undone_by {
        return get_feedback(&tx, undo_id)?
            .ok_or_else(|| StorageError::not_found(format!("feedback {undo_id}")));
    }

    let undo_key = format!("undo:{feedback_id}");
    let undo_fingerprint = hex_sha256(undo_key.as_bytes());
    tx.execute(
        "INSERT INTO feedback_events (
            user_id, app_id, feedback_type, recommendation_run_id, idempotency_key,
            client_created_at_ms, created_at_ms, undone_by, request_fingerprint
         ) VALUES (?1, ?2, 'undo', ?3, ?4, NULL, ?5, NULL, ?6)",
        params![
            user_id,
            original.app_id,
            original.recommendation_run_id,
            undo_key,
            now_ms,
            undo_fingerprint
        ],
    )?;
    let undo_id = tx.last_insert_rowid();
    tx.execute(
        "UPDATE feedback_events SET undone_by = ?1 WHERE feedback_id = ?2",
        params![undo_id, feedback_id],
    )?;
    let undo = get_feedback(&tx, undo_id)?
        .ok_or_else(|| StorageError::not_found(format!("feedback {undo_id}")))?;
    tx.commit()?;
    Ok(undo)
}

pub fn list_active_feedback(
    conn: &Connection,
    user_id: &str,
) -> StorageResult<Vec<ActiveFeedback>> {
    let mut stmt = conn.prepare(
        "WITH ranked AS (
             SELECT app_id, feedback_type,
                    ROW_NUMBER() OVER (
                        PARTITION BY app_id,
                            CASE
                                WHEN feedback_type = 'played' THEN 'ownership'
                                WHEN feedback_type IN ('like', 'not_interested') THEN 'sentiment'
                                ELSE 'reason:' || feedback_type
                            END
                        ORDER BY created_at_ms DESC, feedback_id DESC
                    ) AS row_num
             FROM feedback_events
             WHERE user_id = ?1 AND feedback_type <> 'undo' AND undone_by IS NULL
         )
         SELECT app_id, feedback_type FROM ranked
         WHERE row_num = 1
         ORDER BY app_id, feedback_type",
    )?;
    let rows = stmt.query_map(params![user_id], |row| {
        Ok(ActiveFeedback {
            app_id: row.get::<_, i64>(0)? as u32,
            feedback_type: row.get(1)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn get_feedback(conn: &Connection, feedback_id: i64) -> StorageResult<Option<FeedbackRecord>> {
    conn.query_row(
        "SELECT feedback_id, user_id, app_id, feedback_type, recommendation_run_id,
                idempotency_key, client_created_at_ms, created_at_ms, undone_by,
                request_fingerprint
         FROM feedback_events WHERE feedback_id = ?1",
        params![feedback_id],
        map_feedback,
    )
    .optional()
    .map_err(StorageError::from)
}

fn get_by_idempotency(
    conn: &Connection,
    user_id: &str,
    key: &str,
) -> StorageResult<Option<FeedbackRecord>> {
    conn.query_row(
        "SELECT feedback_id, user_id, app_id, feedback_type, recommendation_run_id,
                idempotency_key, client_created_at_ms, created_at_ms, undone_by,
                request_fingerprint
         FROM feedback_events WHERE user_id = ?1 AND idempotency_key = ?2",
        params![user_id, key],
        map_feedback,
    )
    .optional()
    .map_err(StorageError::from)
}

fn map_feedback(row: &rusqlite::Row<'_>) -> rusqlite::Result<FeedbackRecord> {
    Ok(FeedbackRecord {
        feedback_id: row.get(0)?,
        user_id: row.get(1)?,
        app_id: row.get::<_, i64>(2)? as u32,
        feedback_type: row.get(3)?,
        recommendation_run_id: row.get(4)?,
        idempotency_key: row.get(5)?,
        client_created_at_ms: row.get(6)?,
        created_at_ms: row.get(7)?,
        undone_by: row.get(8)?,
        request_fingerprint: row.get(9)?,
    })
}

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
