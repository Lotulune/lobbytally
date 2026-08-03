use mpgs_domain::{RecommendationConfig, UserPreferences};
use rusqlite::{Connection, OptionalExtension, params, types::Type};
use sha2::{Digest, Sha256};

use crate::error::{StorageError, StorageResult};

#[derive(Debug, Clone, PartialEq)]
pub struct SessionTokens {
    pub user_id: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at_ms: i64,
    pub refresh_expires_at_ms: i64,
}

pub const ACCESS_TOKEN_TTL_MS: i64 = 60 * 60 * 1000;
pub const REFRESH_TOKEN_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, PartialEq)]
pub struct ActiveAlgorithmConfig {
    pub version: String,
    pub config: RecommendationConfig,
}

pub fn create_anonymous_session(
    conn: &mut Connection,
    now_ms: i64,
) -> StorageResult<SessionTokens> {
    let user_id = format!("u_{}", random_hex::<16>()?);
    let access_token = format!("mpgs_at_{}", random_hex::<32>()?);
    let refresh_token = format!("mpgs_rt_{}", random_hex::<32>()?);
    let access_hash = token_hash(&access_token);
    let refresh_hash = token_hash(&refresh_token);
    let expires_at_ms = now_ms.saturating_add(ACCESS_TOKEN_TTL_MS);
    let refresh_expires_at_ms = now_ms.saturating_add(REFRESH_TOKEN_TTL_MS);

    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO anonymous_users (
            user_id, created_at_ms, last_active_at_ms, access_token_hash, refresh_token_hash,
            access_expires_at_ms, refresh_expires_at_ms
         ) VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?6)",
        params![
            user_id,
            now_ms,
            access_hash,
            refresh_hash,
            expires_at_ms,
            refresh_expires_at_ms
        ],
    )?;

    let prefs = UserPreferences::default();
    upsert_preferences(&tx, &user_id, &prefs, now_ms)?;
    tx.commit()?;

    Ok(SessionTokens {
        user_id,
        access_token,
        refresh_token,
        expires_at_ms,
        refresh_expires_at_ms,
    })
}

pub fn refresh_anonymous_session(
    conn: &mut Connection,
    refresh_token: &str,
    now_ms: i64,
) -> StorageResult<SessionTokens> {
    if refresh_token.trim().is_empty() {
        return Err(StorageError::validation("refresh_token is required"));
    }

    let old_hash = token_hash(refresh_token);
    let user_id: String = conn
        .query_row(
            "SELECT user_id FROM anonymous_users
             WHERE refresh_token_hash = ?1 AND refresh_expires_at_ms > ?2",
            params![old_hash, now_ms],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| StorageError::not_found("session"))?;

    let access_token = format!("mpgs_at_{}", random_hex::<32>()?);
    let new_refresh_token = format!("mpgs_rt_{}", random_hex::<32>()?);
    let expires_at_ms = now_ms.saturating_add(ACCESS_TOKEN_TTL_MS);
    let refresh_expires_at_ms = now_ms.saturating_add(REFRESH_TOKEN_TTL_MS);
    let tx = conn.transaction()?;
    let changed = tx.execute(
        "UPDATE anonymous_users
         SET access_token_hash = ?1, refresh_token_hash = ?2,
             access_expires_at_ms = ?3, refresh_expires_at_ms = ?4,
             last_active_at_ms = ?5
         WHERE user_id = ?6 AND refresh_token_hash = ?7 AND refresh_expires_at_ms > ?5",
        params![
            token_hash(&access_token),
            token_hash(&new_refresh_token),
            expires_at_ms,
            refresh_expires_at_ms,
            now_ms,
            user_id,
            old_hash
        ],
    )?;
    if changed != 1 {
        return Err(StorageError::not_found("session"));
    }
    tx.commit()?;

    Ok(SessionTokens {
        user_id,
        access_token,
        refresh_token: new_refresh_token,
        expires_at_ms,
        refresh_expires_at_ms,
    })
}

pub fn resolve_user_id(
    conn: &Connection,
    access_token: &str,
    now_ms: i64,
) -> StorageResult<String> {
    let hash = token_hash(access_token);
    let user_id: String = conn
        .query_row(
            "SELECT user_id FROM anonymous_users
             WHERE access_token_hash = ?1 AND access_expires_at_ms > ?2",
            params![hash, now_ms],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| StorageError::not_found("session"))?;
    conn.execute(
        "UPDATE anonymous_users SET last_active_at_ms = ?1
         WHERE user_id = ?2 AND last_active_at_ms <= ?3",
        params![now_ms, user_id, now_ms.saturating_sub(5 * 60 * 1000)],
    )?;
    Ok(user_id)
}

pub fn get_preferences(conn: &Connection, user_id: &str) -> StorageResult<UserPreferences> {
    conn.query_row(
        "SELECT version, preference_confidence, party_size, coop_competitive,
                session_minutes_min, session_minutes_max,
                budget_currency, budget_max_each_minor, platforms_json, self_hosting_willingness,
                languages_json, excluded_modes_json
         FROM user_preferences WHERE user_id = ?1",
        params![user_id],
        |row| {
            let platforms_json: String = row.get(8)?;
            let languages_json: String = row.get(10)?;
            let excluded_json: String = row.get(11)?;
            Ok(UserPreferences {
                version: row.get(0)?,
                preference_confidence: row.get(1)?,
                party_size: row.get::<_, i64>(2)? as u8,
                coop_competitive: row.get(3)?,
                session_minutes_min: row.get::<_, i64>(4)? as u32,
                session_minutes_max: row.get::<_, i64>(5)? as u32,
                budget_currency: row.get(6)?,
                budget_max_each_minor: row.get(7)?,
                platforms: serde_json::from_str(&platforms_json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(8, Type::Text, Box::new(error))
                })?,
                self_hosting_willingness: row.get(9)?,
                languages: serde_json::from_str(&languages_json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(10, Type::Text, Box::new(error))
                })?,
                excluded_modes: serde_json::from_str(&excluded_json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(11, Type::Text, Box::new(error))
                })?,
            })
        },
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => StorageError::not_found("preferences"),
        other => other.into(),
    })
}

pub fn put_preferences(
    conn: &Connection,
    user_id: &str,
    prefs: &UserPreferences,
    now_ms: i64,
) -> StorageResult<UserPreferences> {
    prefs.validate().map_err(StorageError::validation)?;

    let current = get_preferences(conn, user_id)?;
    if prefs.version != current.version {
        return Err(StorageError::conflict(format!(
            "preference version conflict: client {}, server {}",
            prefs.version, current.version
        )));
    }

    let mut next = prefs.clone();
    next.version = current.version + 1;
    upsert_preferences(conn, user_id, &next, now_ms)?;
    Ok(next)
}

fn upsert_preferences(
    conn: &Connection,
    user_id: &str,
    prefs: &UserPreferences,
    now_ms: i64,
) -> StorageResult<()> {
    conn.execute(
        "INSERT INTO user_preferences (
            user_id, version, preference_confidence, party_size, coop_competitive,
            session_minutes_min, session_minutes_max,
            budget_currency, budget_max_each_minor, platforms_json, self_hosting_willingness,
            languages_json, excluded_modes_json, updated_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
         ON CONFLICT(user_id) DO UPDATE SET
            version = excluded.version,
            preference_confidence = excluded.preference_confidence,
            party_size = excluded.party_size,
            coop_competitive = excluded.coop_competitive,
            session_minutes_min = excluded.session_minutes_min,
            session_minutes_max = excluded.session_minutes_max,
            budget_currency = excluded.budget_currency,
            budget_max_each_minor = excluded.budget_max_each_minor,
            platforms_json = excluded.platforms_json,
            self_hosting_willingness = excluded.self_hosting_willingness,
            languages_json = excluded.languages_json,
            excluded_modes_json = excluded.excluded_modes_json,
            updated_at_ms = excluded.updated_at_ms",
        params![
            user_id,
            prefs.version,
            prefs.preference_confidence,
            prefs.party_size,
            prefs.coop_competitive,
            prefs.session_minutes_min,
            prefs.session_minutes_max,
            prefs.budget_currency,
            prefs.budget_max_each_minor,
            serde_json::to_string(&prefs.platforms)?,
            prefs.self_hosting_willingness,
            serde_json::to_string(&prefs.languages)?,
            serde_json::to_string(&prefs.excluded_modes)?,
            now_ms
        ],
    )?;
    Ok(())
}

fn random_hex<const N: usize>() -> StorageResult<String> {
    let mut bytes = [0_u8; N];
    getrandom::fill(&mut bytes).map_err(|error| {
        std::io::Error::other(format!("secure random generation failed: {error}"))
    })?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

pub fn token_hash(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn ensure_algorithm_config(conn: &Connection, now_ms: i64) -> StorageResult<()> {
    let exists: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM algorithm_configs WHERE status = 'active'",
        [],
        |row| row.get(0),
    )?;
    if exists {
        return Ok(());
    }
    let config_json = serde_json::to_string(&RecommendationConfig::default())?;
    conn.execute(
        "INSERT INTO algorithm_configs (version, schema_version, config_json, status, created_by, created_at_ms)
         VALUES ('rules-0.3.0', 1, ?1, 'active', 'system', ?2)",
        params![config_json, now_ms],
    )?;
    Ok(())
}

pub fn active_algorithm_config(conn: &Connection) -> StorageResult<ActiveAlgorithmConfig> {
    let (version, schema_version, config_json): (String, i64, String) = conn
        .query_row(
            "SELECT version, schema_version, config_json
             FROM algorithm_configs WHERE status = 'active'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => {
                StorageError::migration("active algorithm config is missing")
            }
            other => other.into(),
        })?;
    if schema_version != 1 {
        return Err(StorageError::migration(format!(
            "unsupported algorithm config schema version {schema_version}"
        )));
    }
    let config_value: serde_json::Value = serde_json::from_str(&config_json).map_err(|error| {
        StorageError::migration(format!("invalid active algorithm config JSON: {error}"))
    })?;
    let has_play_intent_weight = config_value.get("play_intent_weight").is_some();
    let has_play_intent_saturation = config_value.get("play_intent_saturation").is_some();
    let mut config: RecommendationConfig =
        serde_json::from_value(config_value).map_err(|error| {
            StorageError::migration(format!("invalid active algorithm config JSON: {error}"))
        })?;
    if version == "rules-0.1.0" && (!has_play_intent_weight || !has_play_intent_saturation) {
        config.play_intent_weight = 0.0;
        config.play_intent_saturation = 0;
    }
    config.validate().map_err(|error| {
        StorageError::migration(format!("invalid active algorithm config: {error}"))
    })?;
    Ok(ActiveAlgorithmConfig { version, config })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    #[test]
    fn legacy_algorithm_config_without_vote_fields_stays_inert() {
        let db = Database::open_in_memory().unwrap();
        db.with_conn_mut(|conn| {
            crate::migrate::migrate_to_latest(conn, 0)?;
            let mut value = serde_json::to_value(RecommendationConfig::default())?;
            let object = value.as_object_mut().expect("config object");
            object.remove("play_intent_weight");
            object.remove("play_intent_saturation");
            conn.execute(
                "INSERT INTO algorithm_configs (
                    version, schema_version, config_json, status, created_by, created_at_ms
                 ) VALUES ('rules-0.1.0', 1, ?1, 'active', 'test', 0)",
                params![serde_json::to_string(&value)?],
            )?;

            let active = active_algorithm_config(conn)?;
            assert_eq!(active.version, "rules-0.1.0");
            assert_eq!(active.config.play_intent_weight, 0.0);
            assert_eq!(active.config.play_intent_saturation, 0);
            Ok(())
        })
        .unwrap();
    }
}
