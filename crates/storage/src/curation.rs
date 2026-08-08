use rusqlite::{Connection, OptionalExtension, params};

use crate::catalog::{
    self, set_availability_integer_field, set_availability_string_list_field,
    set_profile_bool_field, set_profile_text_field,
};
use crate::error::{StorageError, StorageResult};
use crate::models::{
    CreateOverrideRequest, CurationOverride, EffectiveFeatureValue, FeatureOrigin,
};

pub fn create_override(
    conn: &mut Connection,
    app_id: u32,
    request: &CreateOverrideRequest,
    now_ms: i64,
) -> StorageResult<CurationOverride> {
    validate_override_request(request)?;
    if catalog::get_app(conn, app_id)?.is_none() {
        return Err(StorageError::not_found(format!("game {app_id}")));
    }
    let tx = conn.transaction()?;
    let value_json = serde_json::to_string(&request.value_json)?;

    // Revoke any existing active override for the same feature before creating a new one.
    tx.execute(
        "UPDATE curation_overrides
         SET revoked_at_ms = ?1, revoked_by = ?2, revoke_reason = 'superseded'
         WHERE app_id = ?3 AND feature_name = ?4 AND revoked_at_ms IS NULL",
        params![now_ms, request.operator, app_id, request.feature_name],
    )?;

    tx.execute(
        "INSERT INTO curation_overrides (
            app_id, feature_name, value_json, reason, external_evidence,
            operator, created_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            app_id,
            request.feature_name,
            value_json,
            request.reason,
            request.external_evidence,
            request.operator,
            now_ms
        ],
    )?;
    let override_id = tx.last_insert_rowid();

    apply_override_to_profile(
        &tx,
        app_id,
        &request.feature_name,
        &request.value_json,
        now_ms,
    )?;

    tx.execute(
        "INSERT INTO audit_events (
            actor, action, entity_type, entity_key, before_json, after_json, reason, request_id, created_at_ms
         ) VALUES (?1, 'create_override', 'app_feature', ?2, NULL, ?3, ?4, ?5, ?6)",
        params![
            request.operator,
            format!("{app_id}:{}", request.feature_name),
            value_json,
            request.reason,
            request.request_id,
            now_ms
        ],
    )?;

    let created = get_override(&tx, override_id)?
        .ok_or_else(|| StorageError::not_found(format!("override {override_id}")))?;
    tx.commit()?;
    Ok(created)
}

pub fn revoke_override(
    conn: &mut Connection,
    override_id: i64,
    operator: &str,
    reason: &str,
    request_id: Option<&str>,
    now_ms: i64,
) -> StorageResult<CurationOverride> {
    validate_text("operator", operator, 128)?;
    validate_text("reason", reason, 2_000)?;
    let tx = conn.transaction()?;
    let existing = get_override(&tx, override_id)?
        .ok_or_else(|| StorageError::not_found(format!("override {override_id}")))?;
    if existing.revoked_at_ms.is_some() {
        return Err(StorageError::conflict(format!(
            "override {override_id} already revoked"
        )));
    }

    tx.execute(
        "UPDATE curation_overrides
         SET revoked_at_ms = ?1, revoked_by = ?2, revoke_reason = ?3
         WHERE override_id = ?4",
        params![now_ms, operator, reason, override_id],
    )?;

    // After revoke, recompute profile field from remaining active override or source evidence.
    recompute_feature_after_revoke(&tx, existing.app_id, &existing.feature_name, now_ms)?;

    tx.execute(
        "INSERT INTO audit_events (
            actor, action, entity_type, entity_key, before_json, after_json, reason, request_id, created_at_ms
         ) VALUES (?1, 'revoke_override', 'override', ?2, ?3, NULL, ?4, ?5, ?6)",
        params![
            operator,
            override_id.to_string(),
            existing.value_json,
            reason,
            request_id,
            now_ms
        ],
    )?;

    let revoked = get_override(&tx, override_id)?
        .ok_or_else(|| StorageError::not_found(format!("override {override_id}")))?;
    tx.commit()?;
    Ok(revoked)
}

pub fn get_override(
    conn: &Connection,
    override_id: i64,
) -> StorageResult<Option<CurationOverride>> {
    conn.query_row(
        "SELECT override_id, app_id, feature_name, value_json, reason, external_evidence,
                operator, created_at_ms, revoked_at_ms
         FROM curation_overrides WHERE override_id = ?1",
        params![override_id],
        |row| {
            Ok(CurationOverride {
                override_id: row.get(0)?,
                app_id: row.get::<_, i64>(1)? as u32,
                feature_name: row.get(2)?,
                value_json: row.get(3)?,
                reason: row.get(4)?,
                external_evidence: row.get(5)?,
                operator: row.get(6)?,
                created_at_ms: row.get(7)?,
                revoked_at_ms: row.get(8)?,
            })
        },
    )
    .optional()
    .map_err(StorageError::from)
}

pub fn active_override(
    conn: &Connection,
    app_id: u32,
    feature_name: &str,
) -> StorageResult<Option<CurationOverride>> {
    conn.query_row(
        "SELECT override_id, app_id, feature_name, value_json, reason, external_evidence,
                operator, created_at_ms, revoked_at_ms
         FROM curation_overrides
         WHERE app_id = ?1 AND feature_name = ?2 AND revoked_at_ms IS NULL
         ORDER BY created_at_ms DESC, override_id DESC
         LIMIT 1",
        params![app_id, feature_name],
        |row| {
            Ok(CurationOverride {
                override_id: row.get(0)?,
                app_id: row.get::<_, i64>(1)? as u32,
                feature_name: row.get(2)?,
                value_json: row.get(3)?,
                reason: row.get(4)?,
                external_evidence: row.get(5)?,
                operator: row.get(6)?,
                created_at_ms: row.get(7)?,
                revoked_at_ms: row.get(8)?,
            })
        },
    )
    .optional()
    .map_err(StorageError::from)
}

pub fn has_active_override(
    conn: &Connection,
    app_id: u32,
    feature_name: &str,
) -> StorageResult<bool> {
    Ok(active_override(conn, app_id, feature_name)?.is_some())
}

pub fn resolve_effective_feature(
    conn: &Connection,
    app_id: u32,
    feature_name: &str,
) -> StorageResult<EffectiveFeatureValue> {
    if let Some(over) = active_override(conn, app_id, feature_name)? {
        let value_json = serde_json::from_str(&over.value_json)?;
        return Ok(EffectiveFeatureValue {
            app_id,
            feature_name: feature_name.to_owned(),
            value_json,
            origin: FeatureOrigin::HumanOverride,
            override_id: Some(over.override_id),
        });
    }

    if let Some((value_json, _)) = latest_active_evidence(conn, app_id, feature_name)? {
        return Ok(EffectiveFeatureValue {
            app_id,
            feature_name: feature_name.to_owned(),
            value_json: serde_json::from_str(&value_json)?,
            origin: FeatureOrigin::SourceEvidence,
            override_id: None,
        });
    }

    Ok(EffectiveFeatureValue {
        app_id,
        feature_name: feature_name.to_owned(),
        value_json: serde_json::Value::Null,
        origin: FeatureOrigin::Missing,
        override_id: None,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn insert_feature_evidence(
    conn: &Connection,
    app_id: u32,
    feature_name: &str,
    value_json: &serde_json::Value,
    source_type: &str,
    source_ref: &str,
    confidence: f64,
    now_ms: i64,
) -> StorageResult<i64> {
    insert_feature_evidence_with_document(
        conn,
        app_id,
        feature_name,
        value_json,
        source_type,
        source_ref,
        None,
        confidence,
        now_ms,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn insert_feature_evidence_with_document(
    conn: &Connection,
    app_id: u32,
    feature_name: &str,
    value_json: &serde_json::Value,
    source_type: &str,
    source_ref: &str,
    source_document_id: Option<i64>,
    confidence: f64,
    now_ms: i64,
) -> StorageResult<i64> {
    validate_text("feature_name", feature_name, 64)?;
    validate_text("source_type", source_type, 64)?;
    validate_text("source_ref", source_ref, 2_000)?;
    if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
        return Err(StorageError::validation(
            "confidence must be between 0 and 1",
        ));
    }
    catalog::ensure_app_stub(conn, app_id, &format!("app-{app_id}"), now_ms)?;
    let value_json = serde_json::to_string(value_json)?;
    let existing = conn
        .query_row(
            "SELECT evidence_id, value_json FROM feature_evidence
             WHERE app_id = ?1 AND feature_name = ?2 AND source_type = ?3 AND is_active = 1
             ORDER BY observed_at_ms DESC, evidence_id DESC
             LIMIT 1",
            params![app_id, feature_name, source_type],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    if let Some((evidence_id, active_value_json)) = existing
        && active_value_json == value_json
    {
        return Ok(evidence_id);
    }

    // A changed fact creates one historical row and one new active row.
    conn.execute(
        "UPDATE feature_evidence SET is_active = 0
         WHERE app_id = ?1 AND feature_name = ?2 AND source_type = ?3 AND is_active = 1",
        params![app_id, feature_name, source_type],
    )?;
    conn.execute(
        "INSERT INTO feature_evidence (
            app_id, feature_name, value_json, source_type, source_ref,
            source_document_id, confidence, observed_at_ms, is_active
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1)",
        params![
            app_id,
            feature_name,
            value_json,
            source_type,
            source_ref,
            source_document_id,
            confidence,
            now_ms
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

fn latest_active_evidence(
    conn: &Connection,
    app_id: u32,
    feature_name: &str,
) -> StorageResult<Option<(String, i64)>> {
    conn.query_row(
        "SELECT value_json, evidence_id FROM feature_evidence
         WHERE app_id = ?1 AND feature_name = ?2 AND is_active = 1
         ORDER BY observed_at_ms DESC, evidence_id DESC
         LIMIT 1",
        params![app_id, feature_name],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(StorageError::from)
}

fn apply_override_to_profile(
    conn: &Connection,
    app_id: u32,
    feature_name: &str,
    value: &serde_json::Value,
    now_ms: i64,
) -> StorageResult<()> {
    match feature_name {
        "private_session" | "online_coop" | "self_hosted_server" | "drop_in_out" | "crossplay" => {
            let b = value.as_bool();
            set_profile_bool_field(conn, app_id, feature_name, b, now_ms)
        }
        "dominant_mode" => {
            let s = value.as_str();
            set_profile_text_field(conn, app_id, feature_name, s, now_ms)
        }
        "typical_session_minutes_min" | "typical_session_minutes_max" => {
            let minutes = value.as_u64().map(|minutes| minutes as u32);
            set_availability_integer_field(conn, app_id, feature_name, minutes, now_ms)
        }
        "platforms" | "languages" => {
            let values = if value.is_null() {
                Vec::new()
            } else {
                serde_json::from_value::<Vec<String>>(value.clone())?
            };
            set_availability_string_list_field(conn, app_id, feature_name, &values, now_ms)
        }
        _ => Ok(()),
    }
}

fn recompute_feature_after_revoke(
    conn: &Connection,
    app_id: u32,
    feature_name: &str,
    now_ms: i64,
) -> StorageResult<()> {
    // Prefer another active human override, else latest source evidence, else NULL.
    if let Some(over) = active_override(conn, app_id, feature_name)? {
        let value: serde_json::Value = serde_json::from_str(&over.value_json)?;
        return apply_override_to_profile(conn, app_id, feature_name, &value, now_ms);
    }
    if let Some((value_json, _)) = latest_active_evidence(conn, app_id, feature_name)? {
        let value: serde_json::Value = serde_json::from_str(&value_json)?;
        return apply_override_to_profile(conn, app_id, feature_name, &value, now_ms);
    }
    apply_override_to_profile(conn, app_id, feature_name, &serde_json::Value::Null, now_ms)
}

fn validate_override_request(request: &CreateOverrideRequest) -> StorageResult<()> {
    validate_text("feature_name", &request.feature_name, 64)?;
    validate_text("reason", &request.reason, 2_000)?;
    validate_text("operator", &request.operator, 128)?;
    if request
        .external_evidence
        .as_ref()
        .is_some_and(|value| value.len() > 2_000)
    {
        return Err(StorageError::validation(
            "external_evidence must be at most 2000 bytes",
        ));
    }
    match request.feature_name.as_str() {
        "private_session" | "online_coop" | "self_hosted_server" | "drop_in_out" | "crossplay"
            if request.value_json.is_boolean() =>
        {
            Ok(())
        }
        "dominant_mode"
            if request
                .value_json
                .as_str()
                .is_some_and(|value| !value.trim().is_empty() && value.len() <= 64) =>
        {
            Ok(())
        }
        "private_session" | "online_coop" | "self_hosted_server" | "drop_in_out" | "crossplay" => {
            Err(StorageError::validation(
                "boolean override feature requires a boolean value",
            ))
        }
        "dominant_mode" => Err(StorageError::validation(
            "dominant_mode requires a non-empty string of at most 64 bytes",
        )),
        "typical_session_minutes_min" | "typical_session_minutes_max"
            if request
                .value_json
                .as_u64()
                .is_some_and(|value| value <= 1_440) =>
        {
            Ok(())
        }
        "typical_session_minutes_min" | "typical_session_minutes_max" => Err(
            StorageError::validation("typical session minutes must be an integer from 0 to 1440"),
        ),
        "platforms" | "languages" if valid_string_list(&request.value_json) => Ok(()),
        "platforms" | "languages" => Err(StorageError::validation(
            "platforms and languages require an array of 1 to 32 non-empty strings",
        )),
        other => Err(StorageError::validation(format!(
            "unsupported override feature: {other}"
        ))),
    }
}

fn valid_string_list(value: &serde_json::Value) -> bool {
    value.as_array().is_some_and(|items| {
        !items.is_empty()
            && items.len() <= 32
            && items.iter().all(|item| {
                item.as_str()
                    .is_some_and(|text| !text.trim().is_empty() && text.len() <= 64)
            })
    })
}

fn validate_text(name: &str, value: &str, max_bytes: usize) -> StorageResult<()> {
    if value.trim().is_empty() || value.len() > max_bytes {
        return Err(StorageError::validation(format!(
            "{name} must contain between 1 and {max_bytes} bytes"
        )));
    }
    Ok(())
}
