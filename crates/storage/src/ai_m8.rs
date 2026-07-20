//! M8 progressive AI analyses, web evidence, field proposals, and bootstrap state.

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{StorageError, StorageResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressiveAnalysis {
    pub analysis_id: String,
    pub task_type: String,
    pub status: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub protocol: Option<String>,
    pub route_version: Option<String>,
    pub prompt_version: String,
    pub input_hash: String,
    pub preference_hash: String,
    pub data_snapshot_hash: String,
    pub request_json: String,
    pub base_result_json: Option<String>,
    pub result_json: Option<String>,
    pub error_category: Option<String>,
    pub fallback_reason: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub completed_at_ms: Option<i64>,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InsertProgressiveAnalysis {
    pub analysis_id: String,
    pub task_type: String,
    pub status: String,
    pub prompt_version: String,
    pub input_hash: String,
    pub preference_hash: String,
    pub data_snapshot_hash: String,
    pub request_json: String,
    pub base_result_json: Option<String>,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteProgressiveAnalysis {
    pub analysis_id: String,
    pub status: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub protocol: Option<String>,
    pub route_version: Option<String>,
    pub result_json: Option<String>,
    pub error_category: Option<String>,
    pub fallback_reason: Option<String>,
    pub completed_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebDiscoveryEvidence {
    pub evidence_id: String,
    pub app_id: Option<u32>,
    pub query_text: String,
    pub source_url: String,
    pub source_host: String,
    pub source_tier: String,
    pub title: String,
    pub snippet: String,
    pub content_hash: String,
    pub fetched_at_ms: i64,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InsertWebDiscoveryEvidence {
    pub evidence_id: String,
    pub app_id: Option<u32>,
    pub query_text: String,
    pub source_url: String,
    pub source_host: String,
    pub source_tier: String,
    pub title: String,
    pub snippet: String,
    pub content_hash: String,
    pub fetched_at_ms: i64,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldProposal {
    pub proposal_id: String,
    pub app_id: u32,
    pub field_name: String,
    pub proposed_value_json: String,
    pub confidence: f64,
    pub evidence_ids_json: String,
    pub source_channel: String,
    pub review_status: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InsertFieldProposal {
    pub proposal_id: String,
    pub app_id: u32,
    pub field_name: String,
    pub proposed_value_json: String,
    pub confidence: f64,
    pub evidence_ids_json: String,
    pub source_channel: String,
    pub created_at_ms: i64,
}

/// Authority fields that proposals must never claim to overwrite (AI-014).
pub const AUTHORITY_FIELDS: &[&str] = &[
    "price",
    "platforms",
    "party_size_min",
    "party_size_max",
    "service_status",
    "release_state",
];

pub fn insert_progressive_analysis(
    conn: &Connection,
    row: &InsertProgressiveAnalysis,
) -> StorageResult<()> {
    validate_analysis_status(&row.status)?;
    if row.analysis_id.trim().is_empty() || row.analysis_id.len() > 64 {
        return Err(StorageError::validation("analysis_id must be 1..=64 bytes"));
    }
    serde_json::from_str::<serde_json::Value>(&row.request_json)
        .map_err(|_| StorageError::validation("request_json must be valid JSON"))?;
    if let Some(base) = &row.base_result_json {
        serde_json::from_str::<serde_json::Value>(base)
            .map_err(|_| StorageError::validation("base_result_json must be valid JSON"))?;
    }

    conn.execute(
        "INSERT INTO ai_progressive_analyses (
            analysis_id, task_type, status, prompt_version, input_hash,
            preference_hash, data_snapshot_hash, request_json, base_result_json,
            created_at_ms, updated_at_ms, expires_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, ?11)",
        params![
            row.analysis_id,
            row.task_type,
            row.status,
            row.prompt_version,
            row.input_hash,
            row.preference_hash,
            row.data_snapshot_hash,
            row.request_json,
            row.base_result_json,
            row.created_at_ms,
            row.expires_at_ms,
        ],
    )?;
    Ok(())
}

pub fn get_progressive_analysis(
    conn: &Connection,
    analysis_id: &str,
    now_ms: i64,
) -> StorageResult<Option<ProgressiveAnalysis>> {
    let row = conn
        .query_row(
            "SELECT analysis_id, task_type, status, provider, model, protocol, route_version,
                    prompt_version, input_hash, preference_hash, data_snapshot_hash,
                    request_json, base_result_json, result_json, error_category, fallback_reason,
                    created_at_ms, updated_at_ms, completed_at_ms, expires_at_ms
             FROM ai_progressive_analyses
             WHERE analysis_id = ?1 AND expires_at_ms > ?2",
            params![analysis_id, now_ms],
            |row| {
                Ok(ProgressiveAnalysis {
                    analysis_id: row.get(0)?,
                    task_type: row.get(1)?,
                    status: row.get(2)?,
                    provider: row.get(3)?,
                    model: row.get(4)?,
                    protocol: row.get(5)?,
                    route_version: row.get(6)?,
                    prompt_version: row.get(7)?,
                    input_hash: row.get(8)?,
                    preference_hash: row.get(9)?,
                    data_snapshot_hash: row.get(10)?,
                    request_json: row.get(11)?,
                    base_result_json: row.get(12)?,
                    result_json: row.get(13)?,
                    error_category: row.get(14)?,
                    fallback_reason: row.get(15)?,
                    created_at_ms: row.get(16)?,
                    updated_at_ms: row.get(17)?,
                    completed_at_ms: row.get(18)?,
                    expires_at_ms: row.get(19)?,
                })
            },
        )
        .optional()?;
    Ok(row)
}

pub fn complete_progressive_analysis(
    conn: &Connection,
    update: &CompleteProgressiveAnalysis,
) -> StorageResult<()> {
    validate_analysis_status(&update.status)?;
    if let Some(result) = &update.result_json {
        serde_json::from_str::<serde_json::Value>(result)
            .map_err(|_| StorageError::validation("result_json must be valid JSON"))?;
    }
    let changed = conn.execute(
        "UPDATE ai_progressive_analyses
         SET status = ?2,
             provider = ?3,
             model = ?4,
             protocol = ?5,
             route_version = ?6,
             result_json = ?7,
             error_category = ?8,
             fallback_reason = ?9,
             completed_at_ms = ?10,
             updated_at_ms = ?10
         WHERE analysis_id = ?1",
        params![
            update.analysis_id,
            update.status,
            update.provider,
            update.model,
            update.protocol,
            update.route_version,
            update.result_json,
            update.error_category,
            update.fallback_reason,
            update.completed_at_ms,
        ],
    )?;
    if changed == 0 {
        return Err(StorageError::not_found("progressive analysis not found"));
    }
    Ok(())
}

pub fn insert_web_discovery_evidence(
    conn: &Connection,
    row: &InsertWebDiscoveryEvidence,
) -> StorageResult<bool> {
    validate_source_tier(&row.source_tier)?;
    if row.source_url.trim().is_empty() || row.content_hash.trim().is_empty() {
        return Err(StorageError::validation(
            "source_url and content_hash are required",
        ));
    }
    // Content-hash dedupe: identical URL+hash is a no-op success (no re-bill).
    let changed = conn.execute(
        "INSERT OR IGNORE INTO web_discovery_evidence (
            evidence_id, app_id, query_text, source_url, source_host, source_tier,
            title, snippet, content_hash, fetched_at_ms, created_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            row.evidence_id,
            row.app_id.map(i64::from),
            row.query_text,
            row.source_url,
            row.source_host,
            row.source_tier,
            row.title,
            row.snippet,
            row.content_hash,
            row.fetched_at_ms,
            row.created_at_ms,
        ],
    )?;
    Ok(changed == 1)
}

pub fn list_web_discovery_for_app(
    conn: &Connection,
    app_id: u32,
    limit: i64,
) -> StorageResult<Vec<WebDiscoveryEvidence>> {
    let limit = limit.clamp(1, 100);
    let mut stmt = conn.prepare(
        "SELECT evidence_id, app_id, query_text, source_url, source_host, source_tier,
                title, snippet, content_hash, fetched_at_ms, created_at_ms
         FROM web_discovery_evidence
         WHERE app_id = ?1
         ORDER BY fetched_at_ms DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![i64::from(app_id), limit], |row| {
        Ok(WebDiscoveryEvidence {
            evidence_id: row.get(0)?,
            app_id: row
                .get::<_, Option<i64>>(1)?
                .map(|v| u32::try_from(v).unwrap_or(0)),
            query_text: row.get(2)?,
            source_url: row.get(3)?,
            source_host: row.get(4)?,
            source_tier: row.get(5)?,
            title: row.get(6)?,
            snippet: row.get(7)?,
            content_hash: row.get(8)?,
            fetched_at_ms: row.get(9)?,
            created_at_ms: row.get(10)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(StorageError::from)
}

pub fn insert_field_proposal(conn: &Connection, row: &InsertFieldProposal) -> StorageResult<()> {
    if AUTHORITY_FIELDS.contains(&row.field_name.as_str()) {
        return Err(StorageError::validation(format!(
            "field '{}' is an authority field and cannot be written as a proposal",
            row.field_name
        )));
    }
    if !(0.0..=1.0).contains(&row.confidence) {
        return Err(StorageError::validation(
            "confidence must be between 0 and 1",
        ));
    }
    match row.source_channel.as_str() {
        "web_discovery" | "ai_extract" | "manual" => {}
        _ => {
            return Err(StorageError::validation(
                "source_channel must be web_discovery|ai_extract|manual",
            ));
        }
    }
    serde_json::from_str::<serde_json::Value>(&row.proposed_value_json)
        .map_err(|_| StorageError::validation("proposed_value_json must be valid JSON"))?;

    conn.execute(
        "INSERT INTO field_proposals (
            proposal_id, app_id, field_name, proposed_value_json, confidence,
            evidence_ids_json, source_channel, review_status, created_at_ms, updated_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending_review', ?8, ?8)",
        params![
            row.proposal_id,
            i64::from(row.app_id),
            row.field_name,
            row.proposed_value_json,
            row.confidence,
            row.evidence_ids_json,
            row.source_channel,
            row.created_at_ms,
        ],
    )?;
    Ok(())
}

pub fn put_bootstrap_state(
    conn: &Connection,
    key: &str,
    value_json: &str,
    now_ms: i64,
) -> StorageResult<()> {
    if key.trim().is_empty() || key.len() > 64 {
        return Err(StorageError::validation(
            "bootstrap key must be 1..=64 bytes",
        ));
    }
    serde_json::from_str::<serde_json::Value>(value_json)
        .map_err(|_| StorageError::validation("value_json must be valid JSON"))?;
    conn.execute(
        "INSERT INTO bootstrap_state(key, value_json, updated_at_ms)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET
            value_json = excluded.value_json,
            updated_at_ms = excluded.updated_at_ms",
        params![key, value_json, now_ms],
    )?;
    Ok(())
}

pub fn get_bootstrap_state(conn: &Connection, key: &str) -> StorageResult<Option<String>> {
    let value = conn
        .query_row(
            "SELECT value_json FROM bootstrap_state WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?;
    Ok(value)
}

fn validate_analysis_status(status: &str) -> StorageResult<()> {
    match status {
        "pending" | "used" | "cached" | "fallback" | "disabled" => Ok(()),
        _ => Err(StorageError::validation(
            "status must be pending|used|cached|fallback|disabled",
        )),
    }
}

fn validate_source_tier(tier: &str) -> StorageResult<()> {
    match tier {
        "official" | "developer" | "community" | "unknown" => Ok(()),
        _ => Err(StorageError::validation(
            "source_tier must be official|developer|community|unknown",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::repo::Repository;

    fn repo() -> Repository {
        let db = Database::open_in_memory().unwrap();
        let repo = Repository::new(db);
        repo.migrate().unwrap();
        repo
    }

    #[test]
    fn progressive_analysis_round_trip() {
        let repo = repo();
        let now = repo.database().now_ms();
        repo.insert_progressive_analysis(&InsertProgressiveAnalysis {
            analysis_id: "an_1".into(),
            task_type: "rank_explain".into(),
            status: "pending".into(),
            prompt_version: "rank-v5".into(),
            input_hash: "ih".into(),
            preference_hash: "ph".into(),
            data_snapshot_hash: "ds".into(),
            request_json: r#"{"query":"3 player coop"}"#.into(),
            base_result_json: Some(r#"{"items":[]}"#.into()),
            created_at_ms: now,
            expires_at_ms: now + 3_600_000,
        })
        .unwrap();

        let loaded = repo.get_progressive_analysis("an_1", now).unwrap().unwrap();
        assert_eq!(loaded.status, "pending");
        assert_eq!(loaded.task_type, "rank_explain");

        repo.complete_progressive_analysis(&CompleteProgressiveAnalysis {
            analysis_id: "an_1".into(),
            status: "used".into(),
            provider: Some("openai_compat".into()),
            model: Some("grok-4.3".into()),
            protocol: Some("responses".into()),
            route_version: Some("m8-route-v1".into()),
            result_json: Some(r#"{"summary":"ok"}"#.into()),
            error_category: None,
            fallback_reason: None,
            completed_at_ms: now + 1,
        })
        .unwrap();

        let done = repo
            .get_progressive_analysis("an_1", now + 1)
            .unwrap()
            .unwrap();
        assert_eq!(done.status, "used");
        assert_eq!(done.model.as_deref(), Some("grok-4.3"));
    }

    #[test]
    fn web_evidence_dedupes_by_url_and_content_hash() {
        let repo = repo();
        repo.seed_demo_if_empty().unwrap();
        let insert = InsertWebDiscoveryEvidence {
            evidence_id: "ev1".into(),
            app_id: Some(548430),
            query_text: "Deep Rock private lobby".into(),
            source_url: "https://example.com/doc".into(),
            source_host: "example.com".into(),
            source_tier: "official".into(),
            title: "Docs".into(),
            snippet: "private lobbies".into(),
            content_hash: "sha256:abc".into(),
            fetched_at_ms: 1,
            created_at_ms: 1,
        };

        assert!(repo.insert_web_discovery_evidence(&insert).unwrap());
        let mut again = insert.clone();
        again.evidence_id = "ev2".into();
        assert!(!repo.insert_web_discovery_evidence(&again).unwrap());
    }

    #[test]
    fn field_proposal_rejects_authority_fields() {
        let repo = repo();
        repo.seed_demo_if_empty().unwrap();
        let err = repo
            .insert_field_proposal(&InsertFieldProposal {
                proposal_id: "p1".into(),
                app_id: 548430,
                field_name: "price".into(),
                proposed_value_json: "100".into(),
                confidence: 0.9,
                evidence_ids_json: "[]".into(),
                source_channel: "ai_extract".into(),
                created_at_ms: 1,
            })
            .unwrap_err();
        assert!(err.to_string().contains("authority"));
    }

    #[test]
    fn bootstrap_state_round_trip() {
        let repo = repo();
        repo.put_bootstrap_state(
            "first_start",
            r#"{"mode":"store_only","priority_remaining":300}"#,
        )
        .unwrap();
        let value = repo.get_bootstrap_state("first_start").unwrap().unwrap();
        assert!(value.contains("store_only"));
    }
}
