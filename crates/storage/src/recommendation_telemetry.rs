//! Bounded, privacy-preserving recommendation-run attribution.
//!
//! This module deliberately stores hashes of normalized request context and
//! candidate sets rather than raw natural-language queries. Callers should use
//! a deployment-keyed pseudonym for `subject_hash`; a plain hash of a user id is
//! vulnerable to enumeration and is not considered a safe pseudonym.

use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::{StorageError, StorageResult};
use crate::repo::Repository;

pub const RECOMMENDATION_TELEMETRY_RETENTION_MS: i64 = 90 * 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InsertRecommendationRun {
    /// A deployment-keyed, 32-byte digest encoded as lowercase hexadecimal.
    /// Raw user or session identifiers must not be supplied here.
    pub subject_hash: Option<String>,
    pub request_kind: String,
    pub feed_section: String,
    pub algorithm_version: String,
    pub config_version: String,
    pub score_semantics: String,
    pub context_schema_version: u32,
    /// SHA-256 of normalized structured constraints; never a raw query.
    pub context_hash: String,
    /// SHA-256 of the normalized recalled candidate-id set.
    pub candidate_set_hash: String,
    pub candidate_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecommendationRunRecord {
    pub recommendation_run_id: String,
    pub subject_hash: Option<String>,
    pub request_kind: String,
    pub feed_section: String,
    pub algorithm_version: String,
    pub config_version: String,
    pub score_semantics: String,
    pub context_schema_version: u32,
    pub context_hash: String,
    pub candidate_set_hash: String,
    pub candidate_count: u32,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InsertRecommendationItem {
    pub app_id: u32,
    pub rank: u32,
    pub relevance_score: f64,
    pub recommendation_index: Option<u8>,
    pub data_confidence: f64,
    pub slot_reason: String,
    /// An object containing numeric/null score components. It must not contain
    /// prompts, generated prose, or other free-form user text.
    pub score_components: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecommendationItemAttribution {
    pub recommendation_run_id: String,
    pub app_id: u32,
    pub rank: u32,
    pub relevance_score: f64,
    pub recommendation_index: Option<u8>,
    pub data_confidence: f64,
    pub slot_reason: String,
    pub score_components: Value,
    pub subject_hash: Option<String>,
    pub request_kind: String,
    pub feed_section: String,
    pub algorithm_version: String,
    pub config_version: String,
    pub score_semantics: String,
    pub context_schema_version: u32,
    pub context_hash: String,
    pub candidate_set_hash: String,
    pub candidate_count: u32,
    pub run_created_at_ms: i64,
    pub item_recorded_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InsertRecommendationEvent {
    pub recommendation_run_id: String,
    pub app_id: u32,
    pub event_type: String,
    pub idempotency_key: String,
    pub client_created_at_ms: Option<i64>,
    /// Structured event details such as reason tags. Do not place raw queries,
    /// prompts, generated prose, tokens, or raw subject identifiers here.
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecommendationEventRecord {
    pub recommendation_event_id: i64,
    pub recommendation_run_id: String,
    pub app_id: u32,
    pub event_type: String,
    pub idempotency_key: String,
    pub client_created_at_ms: Option<i64>,
    pub metadata: Value,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PurgedRecommendationTelemetry {
    pub runs: usize,
    pub items: usize,
    pub events: usize,
}

/// Hash a serializable, normalized structured context without persisting it.
pub fn hash_structured_context<T: Serialize>(context: &T) -> StorageResult<String> {
    Ok(hex_sha256(&serde_json::to_vec(context)?))
}

/// Produce an order-independent hash for the distinct recalled app ids.
pub fn hash_candidate_set(app_ids: &[u32]) -> StorageResult<String> {
    let mut normalized = app_ids.to_vec();
    normalized.sort_unstable();
    normalized.dedup();
    hash_structured_context(&normalized)
}

impl Repository {
    pub fn insert_recommendation_run(
        &self,
        input: &InsertRecommendationRun,
    ) -> StorageResult<RecommendationRunRecord> {
        let now_ms = self.db.now_ms();
        self.db
            .with_conn_mut(|conn| insert_run(conn, input, now_ms))
    }

    /// Persist a run and every displayed item in one transaction.
    ///
    /// Feed responses must never expose a run id whose item attribution was
    /// only partially written. Keep the two-step methods for maintenance and
    /// compatibility callers, but use this method for serving recommendations.
    pub fn insert_recommendation_run_with_items(
        &self,
        input: &InsertRecommendationRun,
        items: &[InsertRecommendationItem],
    ) -> StorageResult<RecommendationRunRecord> {
        let now_ms = self.db.now_ms();
        self.db
            .with_conn_mut(|conn| insert_run_with_items(conn, input, items, now_ms))
    }

    pub fn insert_recommendation_items(
        &self,
        recommendation_run_id: &str,
        items: &[InsertRecommendationItem],
    ) -> StorageResult<usize> {
        let now_ms = self.db.now_ms();
        self.db
            .with_conn_mut(|conn| insert_items(conn, recommendation_run_id, items, now_ms))
    }

    pub fn insert_recommendation_event(
        &self,
        input: &InsertRecommendationEvent,
    ) -> StorageResult<RecommendationEventRecord> {
        let now_ms = self.db.now_ms();
        self.db
            .with_conn_mut(|conn| insert_event(conn, input, now_ms))
    }

    pub fn recommendation_item_attribution(
        &self,
        recommendation_run_id: &str,
        app_id: u32,
    ) -> StorageResult<Option<RecommendationItemAttribution>> {
        self.db
            .with_conn(|conn| get_item_attribution(conn, recommendation_run_id, app_id))
    }

    pub fn purge_expired_recommendation_telemetry(
        &self,
    ) -> StorageResult<PurgedRecommendationTelemetry> {
        let now_ms = self.db.now_ms();
        self.db.with_conn_mut(|conn| purge_expired(conn, now_ms))
    }
}

fn insert_run(
    conn: &mut Connection,
    input: &InsertRecommendationRun,
    now_ms: i64,
) -> StorageResult<RecommendationRunRecord> {
    validate_run(input)?;
    let recommendation_run_id = random_id("rr_")?;
    let expires_at_ms = retention_expiry(now_ms)?;
    insert_run_row(conn, input, &recommendation_run_id, now_ms, expires_at_ms)?;
    get_run(conn, &recommendation_run_id)?.ok_or_else(|| {
        StorageError::not_found(format!("recommendation run {recommendation_run_id}"))
    })
}

fn insert_run_row(
    conn: &Connection,
    input: &InsertRecommendationRun,
    recommendation_run_id: &str,
    now_ms: i64,
    expires_at_ms: i64,
) -> StorageResult<()> {
    conn.execute(
        "INSERT INTO recommendation_runs (
             recommendation_run_id, subject_hash, request_kind, feed_section,
             algorithm_version, config_version, score_semantics,
             context_schema_version, context_hash, candidate_set_hash,
             candidate_count, created_at_ms, expires_at_ms
         ) VALUES (
             ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13
         )",
        params![
            recommendation_run_id,
            input.subject_hash,
            input.request_kind,
            input.feed_section,
            input.algorithm_version,
            input.config_version,
            input.score_semantics,
            i64::from(input.context_schema_version),
            input.context_hash,
            input.candidate_set_hash,
            i64::from(input.candidate_count),
            now_ms,
            expires_at_ms,
        ],
    )?;
    Ok(())
}

fn insert_run_with_items(
    conn: &mut Connection,
    input: &InsertRecommendationRun,
    items: &[InsertRecommendationItem],
    now_ms: i64,
) -> StorageResult<RecommendationRunRecord> {
    validate_run(input)?;
    validate_items_for_candidate_count(
        items,
        usize::try_from(input.candidate_count).unwrap_or(usize::MAX),
    )?;

    let recommendation_run_id = random_id("rr_")?;
    let expires_at_ms = retention_expiry(now_ms)?;
    let tx = conn.transaction()?;
    insert_run_row(&tx, input, &recommendation_run_id, now_ms, expires_at_ms)?;
    insert_item_rows(&tx, &recommendation_run_id, items, now_ms, expires_at_ms)?;
    let record = get_run(&tx, &recommendation_run_id)?.ok_or_else(|| {
        StorageError::not_found(format!("recommendation run {recommendation_run_id}"))
    })?;
    tx.commit()?;
    Ok(record)
}

fn insert_items(
    conn: &mut Connection,
    recommendation_run_id: &str,
    items: &[InsertRecommendationItem],
    now_ms: i64,
) -> StorageResult<usize> {
    validate_identifier("recommendation_run_id", recommendation_run_id, 128)?;

    let tx = conn.transaction()?;
    let candidate_count: Option<i64> = tx
        .query_row(
            "SELECT candidate_count FROM recommendation_runs
             WHERE recommendation_run_id = ?1",
            params![recommendation_run_id],
            |row| row.get(0),
        )
        .optional()?;
    let candidate_count = candidate_count.ok_or_else(|| {
        StorageError::not_found(format!("recommendation run {recommendation_run_id}"))
    })?;
    validate_items_for_candidate_count(
        items,
        usize::try_from(candidate_count).unwrap_or(usize::MAX),
    )?;

    let expires_at_ms = retention_expiry(now_ms)?;
    insert_item_rows(&tx, recommendation_run_id, items, now_ms, expires_at_ms)?;
    tx.execute(
        "UPDATE recommendation_runs
         SET expires_at_ms = MAX(expires_at_ms, ?2)
         WHERE recommendation_run_id = ?1",
        params![recommendation_run_id, expires_at_ms],
    )?;
    tx.commit()?;
    Ok(items.len())
}

fn validate_items_for_candidate_count(
    items: &[InsertRecommendationItem],
    candidate_count: usize,
) -> StorageResult<()> {
    let mut app_ids = HashSet::with_capacity(items.len());
    let mut ranks = HashSet::with_capacity(items.len());
    for item in items {
        validate_item(item)?;
        if !app_ids.insert(item.app_id) {
            return Err(StorageError::validation(format!(
                "duplicate recommendation item app_id {}",
                item.app_id
            )));
        }
        if !ranks.insert(item.rank) {
            return Err(StorageError::validation(format!(
                "duplicate recommendation item rank {}",
                item.rank
            )));
        }
    }
    if items.len() > candidate_count
        || items
            .iter()
            .any(|item| usize::try_from(item.rank).unwrap_or(usize::MAX) > candidate_count)
    {
        return Err(StorageError::validation(
            "recommendation items cannot exceed the run candidate count",
        ));
    }
    Ok(())
}

fn insert_item_rows(
    conn: &Connection,
    recommendation_run_id: &str,
    items: &[InsertRecommendationItem],
    now_ms: i64,
    expires_at_ms: i64,
) -> StorageResult<()> {
    for item in items {
        let components_json = serde_json::to_string(&item.score_components)?;
        conn.execute(
            "INSERT INTO recommendation_items (
                 recommendation_run_id, app_id, rank, relevance_score,
                 recommendation_index, data_confidence, slot_reason,
                 score_components_json, recorded_at_ms, expires_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                recommendation_run_id,
                i64::from(item.app_id),
                i64::from(item.rank),
                item.relevance_score,
                item.recommendation_index.map(i64::from),
                item.data_confidence,
                item.slot_reason,
                components_json,
                now_ms,
                expires_at_ms,
            ],
        )?;
    }
    Ok(())
}

fn insert_event(
    conn: &mut Connection,
    input: &InsertRecommendationEvent,
    now_ms: i64,
) -> StorageResult<RecommendationEventRecord> {
    validate_identifier("recommendation_run_id", &input.recommendation_run_id, 128)?;
    validate_identifier("event_type", &input.event_type, 64)?;
    validate_identifier("idempotency_key", &input.idempotency_key, 128)?;
    if !input.metadata.is_object() {
        return Err(StorageError::validation(
            "recommendation event metadata must be a JSON object",
        ));
    }
    let metadata_json = serde_json::to_string(&input.metadata)?;

    let tx = conn.transaction()?;
    if let Some(existing) =
        get_event_by_idempotency(&tx, &input.recommendation_run_id, &input.idempotency_key)?
    {
        let same_payload = existing.app_id == input.app_id
            && existing.event_type == input.event_type
            && existing.client_created_at_ms == input.client_created_at_ms
            && existing.metadata == input.metadata;
        if same_payload {
            tx.commit()?;
            return Ok(existing);
        }
        return Err(StorageError::conflict(
            "recommendation event idempotency key reused with different payload",
        ));
    }

    let item_exists: bool = tx.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM recommendation_items
             WHERE recommendation_run_id = ?1 AND app_id = ?2
         )",
        params![input.recommendation_run_id, i64::from(input.app_id)],
        |row| row.get(0),
    )?;
    if !item_exists {
        return Err(StorageError::not_found(format!(
            "recommendation item {}/{}",
            input.recommendation_run_id, input.app_id
        )));
    }

    let expires_at_ms = retention_expiry(now_ms)?;
    tx.execute(
        "INSERT INTO recommendation_events (
             recommendation_run_id, app_id, event_type, idempotency_key,
             client_created_at_ms, metadata_json, created_at_ms, expires_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            input.recommendation_run_id,
            i64::from(input.app_id),
            input.event_type,
            input.idempotency_key,
            input.client_created_at_ms,
            metadata_json,
            now_ms,
            expires_at_ms,
        ],
    )?;
    let event_id = tx.last_insert_rowid();
    tx.execute(
        "UPDATE recommendation_items
         SET expires_at_ms = MAX(expires_at_ms, ?3)
         WHERE recommendation_run_id = ?1 AND app_id = ?2",
        params![
            input.recommendation_run_id,
            i64::from(input.app_id),
            expires_at_ms
        ],
    )?;
    tx.execute(
        "UPDATE recommendation_runs
         SET expires_at_ms = MAX(expires_at_ms, ?2)
         WHERE recommendation_run_id = ?1",
        params![input.recommendation_run_id, expires_at_ms],
    )?;
    let record = get_event(&tx, event_id)?
        .ok_or_else(|| StorageError::not_found(format!("recommendation event {event_id}")))?;
    tx.commit()?;
    Ok(record)
}

fn get_run(
    conn: &Connection,
    recommendation_run_id: &str,
) -> StorageResult<Option<RecommendationRunRecord>> {
    conn.query_row(
        "SELECT recommendation_run_id, subject_hash, request_kind, feed_section,
                algorithm_version, config_version, score_semantics,
                context_schema_version, context_hash, candidate_set_hash,
                candidate_count, created_at_ms, expires_at_ms
         FROM recommendation_runs WHERE recommendation_run_id = ?1",
        params![recommendation_run_id],
        |row| {
            Ok(RecommendationRunRecord {
                recommendation_run_id: row.get(0)?,
                subject_hash: row.get(1)?,
                request_kind: row.get(2)?,
                feed_section: row.get(3)?,
                algorithm_version: row.get(4)?,
                config_version: row.get(5)?,
                score_semantics: row.get(6)?,
                context_schema_version: row.get::<_, i64>(7)? as u32,
                context_hash: row.get(8)?,
                candidate_set_hash: row.get(9)?,
                candidate_count: row.get::<_, i64>(10)? as u32,
                created_at_ms: row.get(11)?,
                expires_at_ms: row.get(12)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

fn get_item_attribution(
    conn: &Connection,
    recommendation_run_id: &str,
    app_id: u32,
) -> StorageResult<Option<RecommendationItemAttribution>> {
    conn.query_row(
        "SELECT item.recommendation_run_id, item.app_id, item.rank,
                item.relevance_score, item.recommendation_index,
                item.data_confidence, item.slot_reason, item.score_components_json,
                run.subject_hash, run.request_kind, run.feed_section,
                run.algorithm_version, run.config_version, run.score_semantics,
                run.context_schema_version, run.context_hash, run.candidate_set_hash,
                run.candidate_count, run.created_at_ms, item.recorded_at_ms
         FROM recommendation_items AS item
         JOIN recommendation_runs AS run
           ON run.recommendation_run_id = item.recommendation_run_id
         WHERE item.recommendation_run_id = ?1 AND item.app_id = ?2",
        params![recommendation_run_id, i64::from(app_id)],
        |row| {
            let components_json: String = row.get(7)?;
            let score_components = serde_json::from_str(&components_json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    7,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
            Ok(RecommendationItemAttribution {
                recommendation_run_id: row.get(0)?,
                app_id: row.get::<_, i64>(1)? as u32,
                rank: row.get::<_, i64>(2)? as u32,
                relevance_score: row.get(3)?,
                recommendation_index: row.get::<_, Option<i64>>(4)?.map(|v| v as u8),
                data_confidence: row.get(5)?,
                slot_reason: row.get(6)?,
                score_components,
                subject_hash: row.get(8)?,
                request_kind: row.get(9)?,
                feed_section: row.get(10)?,
                algorithm_version: row.get(11)?,
                config_version: row.get(12)?,
                score_semantics: row.get(13)?,
                context_schema_version: row.get::<_, i64>(14)? as u32,
                context_hash: row.get(15)?,
                candidate_set_hash: row.get(16)?,
                candidate_count: row.get::<_, i64>(17)? as u32,
                run_created_at_ms: row.get(18)?,
                item_recorded_at_ms: row.get(19)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

fn get_event(conn: &Connection, event_id: i64) -> StorageResult<Option<RecommendationEventRecord>> {
    conn.query_row(
        "SELECT recommendation_event_id, recommendation_run_id, app_id,
                event_type, idempotency_key, client_created_at_ms,
                metadata_json, created_at_ms, expires_at_ms
         FROM recommendation_events WHERE recommendation_event_id = ?1",
        params![event_id],
        map_event,
    )
    .optional()
    .map_err(Into::into)
}

fn get_event_by_idempotency(
    conn: &Connection,
    recommendation_run_id: &str,
    idempotency_key: &str,
) -> StorageResult<Option<RecommendationEventRecord>> {
    conn.query_row(
        "SELECT recommendation_event_id, recommendation_run_id, app_id,
                event_type, idempotency_key, client_created_at_ms,
                metadata_json, created_at_ms, expires_at_ms
         FROM recommendation_events
         WHERE recommendation_run_id = ?1 AND idempotency_key = ?2",
        params![recommendation_run_id, idempotency_key],
        map_event,
    )
    .optional()
    .map_err(Into::into)
}

fn map_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<RecommendationEventRecord> {
    let metadata_json: String = row.get(6)?;
    let metadata = serde_json::from_str(&metadata_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(RecommendationEventRecord {
        recommendation_event_id: row.get(0)?,
        recommendation_run_id: row.get(1)?,
        app_id: row.get::<_, i64>(2)? as u32,
        event_type: row.get(3)?,
        idempotency_key: row.get(4)?,
        client_created_at_ms: row.get(5)?,
        metadata,
        created_at_ms: row.get(7)?,
        expires_at_ms: row.get(8)?,
    })
}

fn purge_expired(
    conn: &mut Connection,
    now_ms: i64,
) -> StorageResult<PurgedRecommendationTelemetry> {
    let tx = conn.transaction()?;
    let events = tx.execute(
        "DELETE FROM recommendation_events WHERE expires_at_ms <= ?1",
        params![now_ms],
    )?;
    let items = tx.execute(
        "DELETE FROM recommendation_items WHERE expires_at_ms <= ?1",
        params![now_ms],
    )?;
    let runs = tx.execute(
        "DELETE FROM recommendation_runs WHERE expires_at_ms <= ?1",
        params![now_ms],
    )?;
    tx.commit()?;
    Ok(PurgedRecommendationTelemetry {
        runs,
        items,
        events,
    })
}

fn validate_run(input: &InsertRecommendationRun) -> StorageResult<()> {
    validate_optional_hash("subject_hash", input.subject_hash.as_deref())?;
    validate_identifier("request_kind", &input.request_kind, 32)?;
    validate_identifier("feed_section", &input.feed_section, 64)?;
    validate_identifier("algorithm_version", &input.algorithm_version, 128)?;
    validate_identifier("config_version", &input.config_version, 128)?;
    validate_identifier("score_semantics", &input.score_semantics, 64)?;
    if input.context_schema_version == 0 {
        return Err(StorageError::validation(
            "context_schema_version must be greater than zero",
        ));
    }
    validate_hash("context_hash", &input.context_hash)?;
    validate_hash("candidate_set_hash", &input.candidate_set_hash)?;
    Ok(())
}

fn validate_item(item: &InsertRecommendationItem) -> StorageResult<()> {
    if item.rank == 0 {
        return Err(StorageError::validation(
            "recommendation item rank must be greater than zero",
        ));
    }
    if !item.relevance_score.is_finite() {
        return Err(StorageError::validation(
            "recommendation item relevance_score must be finite",
        ));
    }
    if !item.data_confidence.is_finite() || !(0.0..=1.0).contains(&item.data_confidence) {
        return Err(StorageError::validation(
            "recommendation item data_confidence must be between zero and one",
        ));
    }
    validate_identifier("slot_reason", &item.slot_reason, 32)?;
    if !item.score_components.is_object() {
        return Err(StorageError::validation(
            "recommendation item score_components must be a JSON object",
        ));
    }
    Ok(())
}

fn validate_identifier(name: &str, value: &str, max_bytes: usize) -> StorageResult<()> {
    if value.trim().is_empty() || value.len() > max_bytes {
        return Err(StorageError::validation(format!(
            "{name} must contain between 1 and {max_bytes} bytes"
        )));
    }
    Ok(())
}

fn validate_optional_hash(name: &str, value: Option<&str>) -> StorageResult<()> {
    if let Some(value) = value {
        validate_hash(name, value)?;
    }
    Ok(())
}

fn validate_hash(name: &str, value: &str) -> StorageResult<()> {
    let valid = value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !valid {
        return Err(StorageError::validation(format!(
            "{name} must be a lowercase hexadecimal SHA-256 digest"
        )));
    }
    Ok(())
}

fn retention_expiry(now_ms: i64) -> StorageResult<i64> {
    now_ms
        .checked_add(RECOMMENDATION_TELEMETRY_RETENTION_MS)
        .ok_or_else(|| StorageError::validation("recommendation telemetry timestamp overflow"))
}

fn random_id(prefix: &str) -> StorageResult<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| {
        std::io::Error::other(format!("secure random generation failed: {error}"))
    })?;
    let suffix: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("{prefix}{suffix}"))
}

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::{Database, FakeClock};

    fn setup(now_ms: i64) -> (Repository, Arc<FakeClock>) {
        let clock = Arc::new(FakeClock::new(now_ms));
        let db = Database::open_in_memory_with_clock(clock.clone()).unwrap();
        db.migrate().unwrap();
        db.with_conn_mut(|conn| {
            conn.execute(
                "INSERT INTO apps (
                     app_id, app_type, canonical_name, release_state,
                     created_at_ms, updated_at_ms
                 ) VALUES
                 (10, 'game', 'Ten', 'released', 0, 0),
                 (20, 'game', 'Twenty', 'released', 0, 0)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        (Repository::new(db), clock)
    }

    fn run_input() -> InsertRecommendationRun {
        let context_hash = hash_structured_context(&json!({
            "party_size": 4,
            "section": "popular_legacy"
        }))
        .unwrap();
        InsertRecommendationRun {
            subject_hash: Some("a".repeat(64)),
            request_kind: "feed".into(),
            feed_section: "popular_legacy".into(),
            algorithm_version: "rules-0.3.0".into(),
            config_version: "rules-0.3.0-shadow".into(),
            score_semantics: "context_percentile_v1".into(),
            context_schema_version: 1,
            context_hash,
            candidate_set_hash: hash_candidate_set(&[20, 10, 20]).unwrap(),
            candidate_count: 2,
        }
    }

    fn items() -> Vec<InsertRecommendationItem> {
        vec![
            InsertRecommendationItem {
                app_id: 10,
                rank: 1,
                relevance_score: 0.731,
                recommendation_index: Some(75),
                data_confidence: 0.8,
                slot_reason: "base".into(),
                score_components: json!({"group_fit": 0.9, "quality": 0.7}),
            },
            InsertRecommendationItem {
                app_id: 20,
                rank: 2,
                relevance_score: 0.612,
                recommendation_index: Some(25),
                data_confidence: 0.6,
                slot_reason: "diversity".into(),
                score_components: json!({"group_fit": 0.6, "quality": 0.8}),
            },
        ]
    }

    #[test]
    fn run_items_and_event_round_trip_with_attribution() {
        let (repo, _) = setup(1_000);
        let run = repo.insert_recommendation_run(&run_input()).unwrap();
        assert!(run.recommendation_run_id.starts_with("rr_"));
        assert_eq!(
            run.expires_at_ms,
            1_000 + RECOMMENDATION_TELEMETRY_RETENTION_MS
        );
        assert_eq!(
            repo.insert_recommendation_items(&run.recommendation_run_id, &items())
                .unwrap(),
            2
        );

        let attribution = repo
            .recommendation_item_attribution(&run.recommendation_run_id, 20)
            .unwrap()
            .unwrap();
        assert_eq!(attribution.rank, 2);
        assert_eq!(attribution.algorithm_version, "rules-0.3.0");
        assert_eq!(attribution.slot_reason, "diversity");
        assert_eq!(attribution.score_components["quality"], 0.8);

        let input = InsertRecommendationEvent {
            recommendation_run_id: run.recommendation_run_id.clone(),
            app_id: 20,
            event_type: "detail_open".into(),
            idempotency_key: "detail-open-20".into(),
            client_created_at_ms: Some(900),
            metadata: json!({"source": "feed_card"}),
        };
        let event = repo.insert_recommendation_event(&input).unwrap();
        let retry = repo.insert_recommendation_event(&input).unwrap();
        assert_eq!(event, retry);

        let mut changed = input;
        changed.metadata = json!({"source": "other"});
        assert!(matches!(
            repo.insert_recommendation_event(&changed),
            Err(StorageError::Conflict { .. })
        ));
    }

    #[test]
    fn event_requires_an_attributed_item_and_items_are_atomic() {
        let (repo, _) = setup(1_000);
        let run = repo.insert_recommendation_run(&run_input()).unwrap();
        let mut duplicate_rank = items();
        duplicate_rank[1].rank = 1;
        assert!(matches!(
            repo.insert_recommendation_items(&run.recommendation_run_id, &duplicate_rank),
            Err(StorageError::Validation { .. })
        ));
        assert!(
            repo.recommendation_item_attribution(&run.recommendation_run_id, 10)
                .unwrap()
                .is_none()
        );

        let event = InsertRecommendationEvent {
            recommendation_run_id: run.recommendation_run_id,
            app_id: 10,
            event_type: "exposure".into(),
            idempotency_key: "exposure-10".into(),
            client_created_at_ms: None,
            metadata: json!({}),
        };
        assert!(matches!(
            repo.insert_recommendation_event(&event),
            Err(StorageError::NotFound { .. })
        ));
    }

    #[test]
    fn serving_write_is_atomic_across_run_and_items() {
        let (repo, _) = setup(1_000);
        let mut duplicate_rank = items();
        duplicate_rank[1].rank = 1;
        assert!(matches!(
            repo.insert_recommendation_run_with_items(&run_input(), &duplicate_rank),
            Err(StorageError::Validation { .. })
        ));
        let run_count: i64 = repo
            .database()
            .with_conn(|conn| {
                Ok(
                    conn.query_row("SELECT COUNT(*) FROM recommendation_runs", [], |row| {
                        row.get(0)
                    })?,
                )
            })
            .unwrap();
        assert_eq!(run_count, 0, "invalid items must not leave an orphan run");

        let run = repo
            .insert_recommendation_run_with_items(&run_input(), &items())
            .unwrap();
        assert!(
            repo.recommendation_item_attribution(&run.recommendation_run_id, 20)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn retention_is_extended_by_late_events_then_purged() {
        let (repo, clock) = setup(1_000);
        let run = repo.insert_recommendation_run(&run_input()).unwrap();
        repo.insert_recommendation_items(&run.recommendation_run_id, &items())
            .unwrap();

        clock.advance_ms(RECOMMENDATION_TELEMETRY_RETENTION_MS - 10);
        repo.insert_recommendation_event(&InsertRecommendationEvent {
            recommendation_run_id: run.recommendation_run_id.clone(),
            app_id: 10,
            event_type: "like".into(),
            idempotency_key: "late-like".into(),
            client_created_at_ms: None,
            metadata: json!({}),
        })
        .unwrap();

        clock.advance_ms(20);
        let first = repo.purge_expired_recommendation_telemetry().unwrap();
        assert_eq!(first.items, 1, "the item without a late event expires");
        assert_eq!(first.events, 0);
        assert_eq!(first.runs, 0, "the late event extends its parent run");
        assert!(
            repo.recommendation_item_attribution(&run.recommendation_run_id, 10)
                .unwrap()
                .is_some()
        );

        clock.advance_ms(RECOMMENDATION_TELEMETRY_RETENTION_MS);
        let second = repo.purge_expired_recommendation_telemetry().unwrap();
        assert_eq!(second.events, 1);
        assert_eq!(second.items, 1);
        assert_eq!(second.runs, 1);
        assert!(
            repo.recommendation_item_attribution(&run.recommendation_run_id, 10)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn hashes_are_canonical_and_raw_context_is_not_persisted() {
        let (repo, _) = setup(1_000);
        assert_eq!(
            hash_candidate_set(&[20, 10, 20]).unwrap(),
            hash_candidate_set(&[10, 20]).unwrap()
        );
        let run = repo.insert_recommendation_run(&run_input()).unwrap();
        repo.database()
            .with_conn(|conn| {
                let sql = "SELECT context_hash, candidate_set_hash
                           FROM recommendation_runs WHERE recommendation_run_id = ?1";
                let stored: (String, String) =
                    conn.query_row(sql, params![run.recommendation_run_id], |row| {
                        Ok((row.get(0)?, row.get(1)?))
                    })?;
                assert_eq!(stored.0.len(), 64);
                assert_eq!(stored.1.len(), 64);
                assert!(!stored.0.contains("party_size"));
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn migration_from_v17_is_additive_and_ready() {
        let db = Database::open_in_memory().unwrap();
        db.with_conn_mut(|conn| {
            crate::migrate::migrate_to(conn, 17, 1_000)?;
            conn.execute(
                "INSERT INTO apps (
                     app_id, app_type, canonical_name, release_state,
                     created_at_ms, updated_at_ms
                 ) VALUES (10, 'game', 'Preserved', 'released', 0, 0)",
                [],
            )?;
            crate::migrate::migrate_to_latest(conn, 2_000)?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            db.schema_version().unwrap(),
            crate::migrate::MIGRATIONS.last().unwrap().0
        );
        assert_eq!(
            db.with_conn(|conn| {
                conn.query_row(
                    "SELECT canonical_name FROM apps WHERE app_id = 10",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .map_err(Into::into)
            })
            .unwrap(),
            "Preserved"
        );
        db.assert_ready().unwrap();
    }
}
