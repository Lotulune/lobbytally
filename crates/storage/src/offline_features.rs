//! Deterministic offline multiplayer feature extraction into `ai_analyses`.
//!
//! This is not an LLM call. It materializes catalog-backed features with explicit
//! evidence ids so AI enhancement layers can cite real sources, and high-impact
//! unknowns stay marked for review rather than invented.

use rusqlite::params;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::error::{StorageError, StorageResult};
use crate::repo::Repository;

pub const OFFLINE_FEATURE_PROVIDER: &str = "offline-rules";
pub const OFFLINE_FEATURE_MODEL: &str = "catalog-v1";
pub const OFFLINE_FEATURE_PROMPT_VERSION: &str = "offline-rules-v1";
pub const OFFLINE_FEATURE_TASK: &str = "feature_extract";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OfflineFeatureStats {
    pub apps_scanned: u32,
    pub analyses_written: u32,
    pub analyses_unchanged: u32,
}

#[derive(Debug, Clone)]
pub struct PutAiAnalysis {
    pub analysis_id: String,
    pub app_id: u32,
    pub task_type: String,
    pub provider: String,
    pub model: String,
    pub prompt_version: String,
    pub input_hash: String,
    pub raw_output_json: String,
    pub accepted_json: Option<String>,
    pub validation_status: String,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone)]
struct ProfileRow {
    app_id: u32,
    dominant_mode: Option<String>,
    private_session: Option<bool>,
    online_coop: Option<bool>,
    self_hosted_server: Option<bool>,
    drop_in_out: Option<bool>,
    crossplay: Option<bool>,
    recommended_min: Option<i64>,
    recommended_max: Option<i64>,
    profile_confidence: Option<f64>,
    short_description: String,
}

impl Repository {
    /// Extract accepted offline features for multiplayer catalog rows.
    pub fn extract_offline_features(
        &self,
        limit: u32,
        after_app_id: u32,
    ) -> StorageResult<OfflineFeatureStats> {
        let limit = limit.clamp(1, 50_000);
        let rows: Vec<ProfileRow> = self.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT p.app_id, p.dominant_mode, p.private_session, p.online_coop,
                        p.self_hosted_server, p.drop_in_out, p.crossplay,
                        p.recommended_min_players, p.recommended_max_players,
                        p.profile_confidence,
                        COALESCE((
                            SELECT loc.short_description FROM app_localizations loc
                            WHERE loc.app_id = p.app_id
                            ORDER BY CASE loc.language
                                WHEN 'schinese' THEN 0
                                WHEN 'english' THEN 1
                                WHEN 'en' THEN 2
                                ELSE 9 END
                            LIMIT 1
                        ), '')
                 FROM multiplayer_profiles p
                 WHERE p.app_id > ?1
                 ORDER BY p.app_id ASC
                 LIMIT ?2",
            )?;
            let mapped = stmt.query_map(params![after_app_id as i64, limit as i64], |row| {
                Ok(ProfileRow {
                    app_id: row.get::<_, i64>(0)? as u32,
                    dominant_mode: row.get(1)?,
                    private_session: row.get::<_, Option<i64>>(2)?.map(|v| v != 0),
                    online_coop: row.get::<_, Option<i64>>(3)?.map(|v| v != 0),
                    self_hosted_server: row.get::<_, Option<i64>>(4)?.map(|v| v != 0),
                    drop_in_out: row.get::<_, Option<i64>>(5)?.map(|v| v != 0),
                    crossplay: row.get::<_, Option<i64>>(6)?.map(|v| v != 0),
                    recommended_min: row.get(7)?,
                    recommended_max: row.get(8)?,
                    profile_confidence: row.get(9)?,
                    short_description: row.get(10)?,
                })
            })?;
            let mut out = Vec::new();
            for row in mapped {
                out.push(row?);
            }
            Ok(out)
        })?;

        let mut stats = OfflineFeatureStats {
            apps_scanned: rows.len() as u32,
            ..OfflineFeatureStats::default()
        };
        let now = self.db.now_ms();
        for row in &rows {
            let payload = build_offline_feature_payload(row);
            let raw = serde_json::to_string(&payload)?;
            let input_hash = sha256_hex(raw.as_bytes());
            let analysis_id = format!(
                "offline-feature:{}:{}",
                row.app_id,
                &input_hash[..16.min(input_hash.len())]
            );
            let written = self.put_ai_analysis_if_new(&PutAiAnalysis {
                analysis_id,
                app_id: row.app_id,
                task_type: OFFLINE_FEATURE_TASK.into(),
                provider: OFFLINE_FEATURE_PROVIDER.into(),
                model: OFFLINE_FEATURE_MODEL.into(),
                prompt_version: OFFLINE_FEATURE_PROMPT_VERSION.into(),
                input_hash,
                raw_output_json: raw.clone(),
                accepted_json: Some(raw),
                validation_status: "accepted".into(),
                created_at_ms: now,
            })?;
            if written {
                stats.analyses_written += 1;
            } else {
                stats.analyses_unchanged += 1;
            }
        }
        Ok(stats)
    }

    /// Insert analysis only when this input_hash is new for the app/task/provider.
    pub fn put_ai_analysis_if_new(&self, analysis: &PutAiAnalysis) -> StorageResult<bool> {
        self.db.with_conn_mut(|conn| {
            let exists: bool = conn.query_row(
                "SELECT COUNT(*) > 0 FROM ai_analyses
                 WHERE app_id = ?1 AND task_type = ?2 AND provider = ?3 AND input_hash = ?4",
                params![
                    analysis.app_id,
                    analysis.task_type,
                    analysis.provider,
                    analysis.input_hash
                ],
                |row| row.get(0),
            )?;
            if exists {
                return Ok(false);
            }
            conn.execute(
                "INSERT INTO ai_analyses(
                    analysis_id, app_id, task_type, provider, model, prompt_version,
                    input_hash, raw_output_json, accepted_json, validation_status, created_at_ms
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![
                    analysis.analysis_id,
                    analysis.app_id,
                    analysis.task_type,
                    analysis.provider,
                    analysis.model,
                    analysis.prompt_version,
                    analysis.input_hash,
                    analysis.raw_output_json,
                    analysis.accepted_json,
                    analysis.validation_status,
                    analysis.created_at_ms
                ],
            )?;
            Ok(true)
        })
    }

    pub fn latest_accepted_offline_features(
        &self,
        app_id: u32,
    ) -> StorageResult<Option<Value>> {
        self.db.with_conn(|conn| {
            conn.query_row(
                "SELECT accepted_json FROM ai_analyses
                 WHERE app_id = ?1 AND task_type = ?2 AND provider = ?3
                   AND validation_status = 'accepted' AND accepted_json IS NOT NULL
                 ORDER BY created_at_ms DESC LIMIT 1",
                params![app_id, OFFLINE_FEATURE_TASK, OFFLINE_FEATURE_PROVIDER],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(StorageError::from)?
            .map(|raw| serde_json::from_str(&raw).map_err(StorageError::from))
            .transpose()
        })
    }

    pub fn ai_analysis_count(&self) -> StorageResult<i64> {
        self.db.with_conn(|conn| {
            conn.query_row("SELECT COUNT(*) FROM ai_analyses", [], |row| row.get(0))
                .map_err(StorageError::from)
        })
    }
}

use rusqlite::OptionalExtension;

fn build_offline_feature_payload(row: &ProfileRow) -> Value {
    let mut features = Vec::new();
    let mut unknowns = Vec::new();
    let confidence = row.profile_confidence.unwrap_or(0.5).clamp(0.0, 1.0);

    push_bool_feature(
        &mut features,
        row.app_id,
        "private_session",
        row.private_session,
        confidence,
    );
    push_bool_feature(
        &mut features,
        row.app_id,
        "online_coop",
        row.online_coop,
        confidence,
    );
    match row.self_hosted_server {
        Some(value) => {
            features.push(json!({
                "name": "self_hosted_server",
                "value": value,
                "confidence": confidence,
                "evidence_refs": [format!("feature:self_hosted_server:{}", row.app_id)],
                "rationale": "Materialized from multiplayer_profiles.self_hosted_server"
            }));
        }
        None => unknowns.push("self_hosted_server"),
    }
    push_bool_feature(
        &mut features,
        row.app_id,
        "drop_in_out",
        row.drop_in_out,
        confidence,
    );
    push_bool_feature(
        &mut features,
        row.app_id,
        "crossplay",
        row.crossplay,
        confidence,
    );

    if let Some(mode) = row.dominant_mode.as_deref().filter(|m| !m.is_empty()) {
        features.push(json!({
            "name": "dominant_mode",
            "value": mode,
            "confidence": confidence,
            "evidence_refs": [format!("feature:dominant_mode:{}", row.app_id)],
            "rationale": "Materialized from multiplayer_profiles.dominant_mode"
        }));
    } else {
        unknowns.push("dominant_mode");
    }

    if row.recommended_min.is_some() || row.recommended_max.is_some() {
        features.push(json!({
            "name": "party_size_band",
            "value": {
                "min": row.recommended_min,
                "max": row.recommended_max
            },
            "confidence": confidence,
            "evidence_refs": [format!("feature:party_size:{}", row.app_id)],
            "rationale": "Materialized from recommended player bounds"
        }));
    } else {
        unknowns.push("party_size_band");
    }

    // High-impact unknown: official service dependency cannot be inferred offline without evidence.
    unknowns.push("official_service_required");
    unknowns.push("service_shutdown");

    let summary = format!(
        "Offline feature snapshot for app {} (confidence={confidence:.2})",
        row.app_id
    );
    let desc_note = row.short_description.trim();
    json!({
        "app_id": row.app_id,
        "document_hash": sha256_hex(format!(
            "{}|{:?}|{:?}|{:?}|{}",
            row.app_id,
            row.dominant_mode,
            row.private_session,
            row.self_hosted_server,
            desc_note
        ).as_bytes()),
        "features": features,
        "summary": summary,
        "unknowns": unknowns,
        "source": {
            "provider": OFFLINE_FEATURE_PROVIDER,
            "model": OFFLINE_FEATURE_MODEL,
            "prompt_version": OFFLINE_FEATURE_PROMPT_VERSION
        }
    })
}

fn push_bool_feature(
    features: &mut Vec<Value>,
    app_id: u32,
    name: &str,
    value: Option<bool>,
    confidence: f64,
) {
    let Some(value) = value else {
        return;
    };
    features.push(json!({
        "name": name,
        "value": value,
        "confidence": confidence,
        "evidence_refs": [format!("feature:{name}:{app_id}")],
        "rationale": format!("Materialized from multiplayer_profiles.{name}")
    }));
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    #[test]
    fn offline_feature_extract_is_idempotent() {
        let db = Database::open_in_memory().unwrap();
        let repo = Repository::new(db);
        repo.migrate().unwrap();
        repo.ensure_runtime_defaults().unwrap();
        repo.seed_demo_if_empty().unwrap();

        let first = repo.extract_offline_features(100, 0).unwrap();
        assert!(first.apps_scanned > 0);
        assert!(first.analyses_written > 0);
        assert_eq!(first.analyses_unchanged, 0);

        let second = repo.extract_offline_features(100, 0).unwrap();
        assert_eq!(second.apps_scanned, first.apps_scanned);
        assert_eq!(second.analyses_written, 0);
        assert_eq!(second.analyses_unchanged, first.analyses_written);
        assert!(repo.ai_analysis_count().unwrap() > 0);

        // Every accepted analysis includes evidence-bearing features or unknowns.
        let app_id = repo
            .database()
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT app_id FROM multiplayer_profiles LIMIT 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map(|v| v as u32)
                .map_err(StorageError::from)
            })
            .unwrap();
        let payload = repo
            .latest_accepted_offline_features(app_id)
            .unwrap()
            .expect("analysis");
        assert_eq!(payload["app_id"], app_id);
        assert!(payload["features"].as_array().is_some());
        assert!(payload["unknowns"].as_array().is_some());
    }
}
