use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{StorageError, StorageResult};
use crate::models::AppRecord;
use crate::util::{opt_bool_to_sql, sql_to_opt_bool};

#[allow(clippy::too_many_arguments)]
pub fn upsert_app(
    conn: &Connection,
    app_id: u32,
    app_type: &str,
    canonical_name: &str,
    release_state: &str,
    release_date: Option<&str>,
    release_date_precision: Option<&str>,
    source_modified_at_ms: Option<i64>,
    now_ms: i64,
) -> StorageResult<()> {
    conn.execute(
        "INSERT INTO apps (
            app_id, app_type, canonical_name, release_state, release_date,
            release_date_precision, source_modified_at_ms, created_at_ms, updated_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
         ON CONFLICT(app_id) DO UPDATE SET
            app_type = excluded.app_type,
            canonical_name = excluded.canonical_name,
            release_state = excluded.release_state,
            release_date = COALESCE(excluded.release_date, apps.release_date),
            release_date_precision = COALESCE(excluded.release_date_precision, apps.release_date_precision),
            source_modified_at_ms = COALESCE(excluded.source_modified_at_ms, apps.source_modified_at_ms),
            updated_at_ms = excluded.updated_at_ms",
        params![
            app_id,
            app_type,
            canonical_name,
            release_state,
            release_date,
            release_date_precision,
            source_modified_at_ms,
            now_ms
        ],
    )?;
    Ok(())
}

/// Merge a low-fidelity catalog row without downgrading store-enriched fields.
///
/// App-list and store-search rows do not carry an authoritative release state,
/// release date, or locale. Once store details have materialized a localization,
/// that localized display name must also remain authoritative.
pub fn upsert_catalog_app(
    conn: &Connection,
    app_id: u32,
    app_type: &str,
    canonical_name: &str,
    source_modified_at_ms: Option<i64>,
    now_ms: i64,
) -> StorageResult<()> {
    conn.execute(
        "INSERT INTO apps (
            app_id, app_type, canonical_name, release_state,
            source_modified_at_ms, created_at_ms, updated_at_ms
         ) VALUES (?1, ?2, ?3, 'unknown', ?4, ?5, ?5)
         ON CONFLICT(app_id) DO UPDATE SET
            app_type = CASE
                WHEN apps.app_type = 'unknown'
                     AND NOT (
                         apps.source_modified_at_ms IS NOT NULL
                         AND (
                             excluded.source_modified_at_ms IS NULL
                             OR excluded.source_modified_at_ms < apps.source_modified_at_ms
                         )
                     )
                    THEN excluded.app_type
                ELSE apps.app_type
            END,
            canonical_name = CASE
                WHEN apps.source_modified_at_ms IS NOT NULL
                     AND (
                         excluded.source_modified_at_ms IS NULL
                         OR excluded.source_modified_at_ms < apps.source_modified_at_ms
                     )
                    THEN apps.canonical_name
                WHEN trim(apps.canonical_name) = ''
                     OR apps.canonical_name = 'app-' || CAST(apps.app_id AS TEXT)
                    THEN excluded.canonical_name
                WHEN EXISTS (
                    SELECT 1 FROM app_localizations localization
                    WHERE localization.app_id = apps.app_id
                      AND localization.name IS NOT NULL
                      AND trim(localization.name) != ''
                )
                    THEN apps.canonical_name
                ELSE excluded.canonical_name
            END,
            source_modified_at_ms = CASE
                WHEN excluded.source_modified_at_ms IS NULL
                    THEN apps.source_modified_at_ms
                WHEN apps.source_modified_at_ms IS NULL
                     OR excluded.source_modified_at_ms >= apps.source_modified_at_ms
                    THEN excluded.source_modified_at_ms
                ELSE apps.source_modified_at_ms
            END,
            updated_at_ms = excluded.updated_at_ms
         WHERE (
                apps.app_type = 'unknown'
                AND excluded.app_type != 'unknown'
                AND NOT (
                    apps.source_modified_at_ms IS NOT NULL
                    AND (
                        excluded.source_modified_at_ms IS NULL
                        OR excluded.source_modified_at_ms < apps.source_modified_at_ms
                    )
                )
            )
            OR (
                apps.canonical_name IS NOT excluded.canonical_name
                AND NOT (
                    apps.source_modified_at_ms IS NOT NULL
                    AND (
                        excluded.source_modified_at_ms IS NULL
                        OR excluded.source_modified_at_ms < apps.source_modified_at_ms
                    )
                )
                AND (
                    trim(apps.canonical_name) = ''
                    OR apps.canonical_name = 'app-' || CAST(apps.app_id AS TEXT)
                    OR NOT EXISTS (
                        SELECT 1 FROM app_localizations localization
                        WHERE localization.app_id = apps.app_id
                          AND localization.name IS NOT NULL
                          AND trim(localization.name) != ''
                    )
                )
            )
            OR (
                excluded.source_modified_at_ms IS NOT NULL
                AND (
                    apps.source_modified_at_ms IS NULL
                    OR excluded.source_modified_at_ms > apps.source_modified_at_ms
                )
            )",
        params![
            app_id,
            app_type,
            canonical_name,
            source_modified_at_ms,
            now_ms
        ],
    )?;
    Ok(())
}

pub fn get_app(conn: &Connection, app_id: u32) -> StorageResult<Option<AppRecord>> {
    conn.query_row(
        "SELECT app_id, app_type, canonical_name, release_state, release_date,
                release_date_raw, release_date_precision, is_early_access,
                current_data_confidence, source_modified_at_ms, created_at_ms, updated_at_ms
         FROM apps WHERE app_id = ?1",
        params![app_id],
        |row| {
            Ok(AppRecord {
                app_id: row.get::<_, i64>(0)? as u32,
                app_type: row.get(1)?,
                canonical_name: row.get(2)?,
                release_state: row.get(3)?,
                release_date: row.get(4)?,
                release_date_raw: row.get(5)?,
                release_date_precision: row.get(6)?,
                is_early_access: sql_to_opt_bool(row.get(7)?),
                current_data_confidence: row.get(8)?,
                source_modified_at_ms: row.get(9)?,
                created_at_ms: row.get(10)?,
                updated_at_ms: row.get(11)?,
            })
        },
    )
    .optional()
    .map_err(StorageError::from)
}

pub fn ensure_app_stub(
    conn: &Connection,
    app_id: u32,
    name: &str,
    now_ms: i64,
) -> StorageResult<()> {
    if get_app(conn, app_id)?.is_some() {
        return Ok(());
    }
    upsert_app(
        conn, app_id, "unknown", name, "unknown", None, None, None, now_ms,
    )
}

/// Preserve the localized store identity returned by a store-details request.
///
/// A response may omit either field transiently, so an incomplete refresh must
/// not erase the last usable localized summary.
pub fn upsert_app_localization(
    conn: &Connection,
    app_id: u32,
    language: &str,
    name: Option<&str>,
    short_description: Option<&str>,
    source: &str,
    now_ms: i64,
) -> StorageResult<()> {
    if name.is_none() && short_description.is_none() {
        return Ok(());
    }
    ensure_app_stub(conn, app_id, &format!("app-{app_id}"), now_ms)?;
    conn.execute(
        "INSERT INTO app_localizations (
             app_id, language, name, short_description, source, updated_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(app_id, language) DO UPDATE SET
             name = COALESCE(excluded.name, app_localizations.name),
             short_description = COALESCE(excluded.short_description, app_localizations.short_description),
             source = excluded.source,
             updated_at_ms = excluded.updated_at_ms",
        params![app_id, language, name, short_description, source, now_ms],
    )?;
    Ok(())
}

pub fn upsert_relation(
    conn: &Connection,
    source_app_id: u32,
    target_app_id: u32,
    relation_type: &str,
    confidence: f64,
    verified_by_human: bool,
    now_ms: i64,
) -> StorageResult<()> {
    ensure_app_stub(conn, source_app_id, &format!("app-{source_app_id}"), now_ms)?;
    ensure_app_stub(conn, target_app_id, &format!("app-{target_app_id}"), now_ms)?;
    conn.execute(
        "INSERT INTO app_relations (
            source_app_id, target_app_id, relation_type, confidence,
            verified_by_human, created_at_ms, updated_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
         ON CONFLICT(source_app_id, target_app_id, relation_type) DO UPDATE SET
            confidence = excluded.confidence,
            verified_by_human = MAX(app_relations.verified_by_human, excluded.verified_by_human),
            updated_at_ms = excluded.updated_at_ms",
        params![
            source_app_id,
            target_app_id,
            relation_type,
            confidence,
            if verified_by_human { 1 } else { 0 },
            now_ms
        ],
    )?;
    Ok(())
}

pub fn count_apps(conn: &Connection) -> StorageResult<i64> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM apps", [], |row| row.get(0))?;
    Ok(n)
}

pub fn set_profile_bool_field(
    conn: &Connection,
    app_id: u32,
    field: &str,
    value: Option<bool>,
    now_ms: i64,
) -> StorageResult<()> {
    let column = match field {
        "private_session" => "private_session",
        "online_coop" => "online_coop",
        "self_hosted_server" => "self_hosted_server",
        "drop_in_out" => "drop_in_out",
        "crossplay" => "crossplay",
        other => {
            return Err(StorageError::validation(format!(
                "unsupported profile bool field: {other}"
            )));
        }
    };
    ensure_app_stub(conn, app_id, &format!("app-{app_id}"), now_ms)?;
    conn.execute(
        "INSERT INTO multiplayer_profiles (app_id, computed_at_ms) VALUES (?1, ?2)
         ON CONFLICT(app_id) DO NOTHING",
        params![app_id, now_ms],
    )?;
    let sql = format!(
        "UPDATE multiplayer_profiles SET {column} = ?1, computed_at_ms = ?2 WHERE app_id = ?3"
    );
    conn.execute(&sql, params![opt_bool_to_sql(value), now_ms, app_id])?;
    Ok(())
}

pub fn set_profile_text_field(
    conn: &Connection,
    app_id: u32,
    field: &str,
    value: Option<&str>,
    now_ms: i64,
) -> StorageResult<()> {
    let column = match field {
        "dominant_mode" => "dominant_mode",
        "service_status" => "service_status",
        other => {
            return Err(StorageError::validation(format!(
                "unsupported profile text field: {other}"
            )));
        }
    };
    ensure_app_stub(conn, app_id, &format!("app-{app_id}"), now_ms)?;
    conn.execute(
        "INSERT INTO multiplayer_profiles (app_id, computed_at_ms) VALUES (?1, ?2)
         ON CONFLICT(app_id) DO NOTHING",
        params![app_id, now_ms],
    )?;
    let sql = format!(
        "UPDATE multiplayer_profiles SET {column} = ?1, computed_at_ms = ?2 WHERE app_id = ?3"
    );
    conn.execute(&sql, params![value, now_ms, app_id])?;
    Ok(())
}

pub fn upsert_app_availability(
    conn: &Connection,
    app_id: u32,
    platforms: Option<&[String]>,
    languages: Option<&[String]>,
    is_free: Option<bool>,
    now_ms: i64,
) -> StorageResult<()> {
    ensure_app_stub(conn, app_id, &format!("app-{app_id}"), now_ms)?;
    let platforms_json = platforms.map(serde_json::to_string).transpose()?;
    let languages_json = languages.map(serde_json::to_string).transpose()?;
    conn.execute(
        "INSERT INTO app_availability (
            app_id, platforms_json, languages_json, is_free, updated_at_ms
         ) VALUES (?1, COALESCE(?2, '[]'), COALESCE(?3, '[]'), ?4, ?5)
         ON CONFLICT(app_id) DO UPDATE SET
            platforms_json = COALESCE(?2, app_availability.platforms_json),
            languages_json = COALESCE(?3, app_availability.languages_json),
            is_free = COALESCE(?4, app_availability.is_free),
            updated_at_ms = ?5",
        params![
            app_id,
            platforms_json,
            languages_json,
            opt_bool_to_sql(is_free),
            now_ms
        ],
    )?;
    Ok(())
}

pub fn restore_empty_availability_from_evidence(
    conn: &Connection,
    now_ms: i64,
) -> StorageResult<usize> {
    let mut restored = 0_usize;
    for (column, feature) in [
        ("platforms_json", "platforms"),
        ("languages_json", "languages"),
    ] {
        let sql = format!(
            "UPDATE app_availability AS availability
             SET {column} = (
                     SELECT evidence.value_json
                     FROM feature_evidence evidence
                     WHERE evidence.app_id = availability.app_id
                       AND evidence.feature_name = ?1
                       AND evidence.value_json <> '[]'
                     ORDER BY evidence.observed_at_ms DESC, evidence.evidence_id DESC
                     LIMIT 1
                 ),
                 updated_at_ms = ?2
             WHERE {column} = '[]'
               AND EXISTS (
                   SELECT 1 FROM feature_evidence evidence
                   WHERE evidence.app_id = availability.app_id
                     AND evidence.feature_name = ?1
                     AND evidence.value_json <> '[]'
               )"
        );
        restored += conn.execute(&sql, params![feature, now_ms])?;
    }
    Ok(restored)
}

pub fn set_availability_integer_field(
    conn: &Connection,
    app_id: u32,
    field: &str,
    value: Option<u32>,
    now_ms: i64,
) -> StorageResult<()> {
    let column = match field {
        "typical_session_minutes_min" => "typical_session_minutes_min",
        "typical_session_minutes_max" => "typical_session_minutes_max",
        other => {
            return Err(StorageError::validation(format!(
                "unsupported availability integer field: {other}"
            )));
        }
    };
    upsert_app_availability(conn, app_id, None, None, None, now_ms)?;
    let (current_min, current_max): (Option<u32>, Option<u32>) = conn.query_row(
        "SELECT typical_session_minutes_min, typical_session_minutes_max
         FROM app_availability WHERE app_id = ?1",
        params![app_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let (next_min, next_max) = match field {
        "typical_session_minutes_min" => (value, current_max),
        _ => (current_min, value),
    };
    if matches!((next_min, next_max), (Some(min), Some(max)) if min > max) {
        return Err(StorageError::validation(
            "typical session minimum must not exceed maximum",
        ));
    }
    let sql =
        format!("UPDATE app_availability SET {column} = ?1, updated_at_ms = ?2 WHERE app_id = ?3");
    conn.execute(&sql, params![value, now_ms, app_id])?;
    Ok(())
}

pub fn set_availability_string_list_field(
    conn: &Connection,
    app_id: u32,
    field: &str,
    values: &[String],
    now_ms: i64,
) -> StorageResult<()> {
    let column = match field {
        "platforms" => "platforms_json",
        "languages" => "languages_json",
        other => {
            return Err(StorageError::validation(format!(
                "unsupported availability list field: {other}"
            )));
        }
    };
    upsert_app_availability(conn, app_id, None, None, None, now_ms)?;
    let sql =
        format!("UPDATE app_availability SET {column} = ?1, updated_at_ms = ?2 WHERE app_id = ?3");
    conn.execute(
        &sql,
        params![serde_json::to_string(values)?, now_ms, app_id],
    )?;
    Ok(())
}

pub fn get_multiplayer_profile(
    conn: &Connection,
    app_id: u32,
) -> StorageResult<Option<crate::models::MultiplayerProfile>> {
    conn.query_row(
        "SELECT app_id, dominant_mode, private_session, online_coop, self_hosted_server,
                drop_in_out, crossplay, recommended_min_players, recommended_max_players,
                profile_confidence, computed_at_ms
         FROM multiplayer_profiles WHERE app_id = ?1",
        params![app_id],
        |row| {
            Ok(crate::models::MultiplayerProfile {
                app_id: row.get::<_, i64>(0)? as u32,
                dominant_mode: row.get(1)?,
                private_session: sql_to_opt_bool(row.get(2)?),
                online_coop: sql_to_opt_bool(row.get(3)?),
                self_hosted_server: sql_to_opt_bool(row.get(4)?),
                drop_in_out: sql_to_opt_bool(row.get(5)?),
                crossplay: sql_to_opt_bool(row.get(6)?),
                recommended_min_players: row.get(7)?,
                recommended_max_players: row.get(8)?,
                profile_confidence: row.get(9)?,
                computed_at_ms: row.get(10)?,
            })
        },
    )
    .optional()
    .map_err(StorageError::from)
}
