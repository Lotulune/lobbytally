use rusqlite::Connection;

use crate::error::{StorageError, StorageResult};

/// Ordered migration scripts shipped with the workspace.
pub const MIGRATIONS: &[(i64, &str, &str)] = &[
    (
        1,
        "0001_initial",
        include_str!("../../../migrations/0001_initial.sql"),
    ),
    (
        2,
        "0002_data_quality_findings",
        include_str!("../../../migrations/0002_data_quality_findings.sql"),
    ),
    (
        3,
        "0003_users_feedback_algorithm",
        include_str!("../../../migrations/0003_users_feedback_algorithm.sql"),
    ),
    (
        4,
        "0004_m3_integrity_fixes",
        include_str!("../../../migrations/0004_m3_integrity_fixes.sql"),
    ),
    (
        5,
        "0005_m3_recommendation_inputs",
        include_str!("../../../migrations/0005_m3_recommendation_inputs.sql"),
    ),
    (
        6,
        "0006_play_intent_votes",
        include_str!("../../../migrations/0006_play_intent_votes.sql"),
    ),
    (
        7,
        "0007_m5_ai_retrieval",
        include_str!("../../../migrations/0007_m5_ai_retrieval.sql"),
    ),
    (
        8,
        "0008_m7_accounts_community_ai",
        include_str!("../../../migrations/0008_m7_accounts_community_ai.sql"),
    ),
    (
        9,
        "0009_m7_avatar_moderation",
        include_str!("../../../migrations/0009_m7_avatar_moderation.sql"),
    ),
    (
        10,
        "0010_popular_reviews",
        include_str!("../../../migrations/0010_popular_reviews.sql"),
    ),
    (
        11,
        "0011_local_ai_credentials",
        include_str!("../../../migrations/0011_local_ai_credentials.sql"),
    ),
    (
        12,
        "0012_enrichment_refresh_state",
        include_str!("../../../migrations/0012_enrichment_refresh_state.sql"),
    ),
    (
        13,
        "0013_store_detail_success_state",
        include_str!("../../../migrations/0013_store_detail_success_state.sql"),
    ),
    (
        14,
        "0014_device_local_ai_mode",
        include_str!("../../../migrations/0014_device_local_ai_mode.sql"),
    ),
    (
        15,
        "0015_m8_ai_routing",
        include_str!("../../../migrations/0015_m8_ai_routing.sql"),
    ),
    (
        16,
        "0016_steam_media_gallery",
        include_str!("../../../migrations/0016_steam_media_gallery.sql"),
    ),
    (
        17,
        "0017_media_backfill_state",
        include_str!("../../../migrations/0017_media_backfill_state.sql"),
    ),
    (
        18,
        "0018_recommendation_telemetry",
        include_str!("../../../migrations/0018_recommendation_telemetry.sql"),
    ),
    (
        19,
        "0019_preference_confidence",
        include_str!("../../../migrations/0019_preference_confidence.sql"),
    ),
    (
        20,
        "0020_feed_query_indexes",
        include_str!("../../../migrations/0020_feed_query_indexes.sql"),
    ),
    (
        21,
        "0021_read_path_indexes",
        include_str!("../../../migrations/0021_read_path_indexes.sql"),
    ),
    (
        22,
        "0022_latest_review_snapshots",
        include_str!("../../../migrations/0022_latest_review_snapshots.sql"),
    ),
    (
        23,
        "0023_search_name_trigram",
        include_str!("../../../migrations/0023_search_name_trigram.sql"),
    ),
];

pub fn current_version(conn: &Connection) -> StorageResult<i64> {
    let exists: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='schema_migrations'",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(0);
    }
    let mut statement =
        conn.prepare("SELECT version, name FROM schema_migrations ORDER BY version ASC")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut current = 0;
    for (index, row) in rows.enumerate() {
        let (version, name) = row?;
        let expected_version = i64::try_from(index)
            .map_err(|_| StorageError::migration("migration history is too large"))?
            .saturating_add(1);
        if version != expected_version {
            return Err(StorageError::migration(format!(
                "migration history is not contiguous: expected version {expected_version}, found {version}"
            )));
        }
        let Some((_, expected_name, _)) = MIGRATIONS.get(index) else {
            return Err(StorageError::migration(format!(
                "database contains unknown migration version {version} ({name})"
            )));
        };
        if name != *expected_name {
            return Err(StorageError::migration(format!(
                "migration {version} name mismatch: expected {expected_name}, found {name}"
            )));
        }
        current = version;
    }
    Ok(current)
}

pub fn migrate_to_latest(conn: &mut Connection, now_ms: i64) -> StorageResult<i64> {
    migrate_to(conn, latest_version(), now_ms)
}

pub fn latest_version() -> i64 {
    MIGRATIONS.last().map(|(v, _, _)| *v).unwrap_or(0)
}

pub fn migrate_to(conn: &mut Connection, target: i64, now_ms: i64) -> StorageResult<i64> {
    if target < 0 || target > latest_version() {
        return Err(StorageError::migration(format!(
            "target migration version {target} is out of range"
        )));
    }

    let mut current = current_version(conn)?;
    if current > target {
        return Err(StorageError::migration(format!(
            "database is at version {current}, cannot migrate down to {target}"
        )));
    }

    while current < target {
        let next = current + 1;
        let Some((_, name, sql)) = MIGRATIONS.iter().find(|(v, _, _)| *v == next) else {
            return Err(StorageError::migration(format!(
                "missing migration script for version {next}"
            )));
        };

        let tx = conn.transaction()?;
        // schema_migrations is created by 0001; for version 1 the table is created by SQL itself.
        if next > 1 {
            ensure_migrations_table(&tx)?;
        }
        tx.execute_batch(sql)?;
        if next == 1 {
            // 0001 creates schema_migrations but does not insert its own row.
            tx.execute(
                "INSERT INTO schema_migrations (version, name, applied_at_ms) VALUES (?1, ?2, ?3)",
                rusqlite::params![next, name, now_ms],
            )?;
        } else {
            tx.execute(
                "INSERT INTO schema_migrations (version, name, applied_at_ms) VALUES (?1, ?2, ?3)",
                rusqlite::params![next, name, now_ms],
            )?;
        }
        tx.commit()?;
        current = next;
    }

    Ok(current)
}

fn ensure_migrations_table(conn: &Connection) -> StorageResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            applied_at_ms INTEGER NOT NULL
        );",
    )?;
    Ok(())
}

/// Re-running migrate on an already-current database is a no-op.
pub fn migrate_idempotent(conn: &mut Connection, now_ms: i64) -> StorageResult<i64> {
    migrate_to_latest(conn, now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_history_must_be_contiguous_and_named_exactly() {
        let mut gap = Connection::open_in_memory().unwrap();
        migrate_to_latest(&mut gap, 1).unwrap();
        gap.execute("DELETE FROM schema_migrations WHERE version = 2", [])
            .unwrap();
        assert!(matches!(
            current_version(&gap),
            Err(StorageError::Migration { .. })
        ));

        let mut renamed = Connection::open_in_memory().unwrap();
        migrate_to_latest(&mut renamed, 1).unwrap();
        renamed
            .execute(
                "UPDATE schema_migrations SET name = 'tampered' WHERE version = 2",
                [],
            )
            .unwrap();
        assert!(matches!(
            current_version(&renamed),
            Err(StorageError::Migration { .. })
        ));
    }

    #[test]
    fn latest_migration_adds_feed_and_calendar_read_indexes() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate_to_latest(&mut conn, 1).unwrap();

        for index in [
            "idx_app_relations_target_type",
            "idx_review_snapshots_app_latest",
        ] {
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?1)",
                    [index],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(exists, "missing index {index}");
        }
    }

    #[test]
    fn latest_review_projection_tracks_insert_update_and_delete() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate_to_latest(&mut conn, 1).unwrap();
        conn.execute(
            "INSERT INTO apps (
                 app_id, app_type, canonical_name, release_state,
                 created_at_ms, updated_at_ms
             ) VALUES (42, 'game', 'Projection Test', 'released', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO review_snapshots (
                 app_id, region_scope, language_scope, captured_at_ms,
                 total_positive, total_negative, total_reviews, wilson_lower,
                 filter_offtopic_activity, parameter_hash, content_hash, source
             ) VALUES
             (42, 'all', 'english', 20, 90, 10, 100, 0.80, 1, 'p1', 'c1', 'test'),
             (42, 'all', 'schinese', 10, 40, 10, 50, 0.70, 1, 'p2', 'c2', 'test'),
             (42, 'all', 'schinese', 20, 95, 5, 100, 0.85, 1, 'p3', 'c3', 'test');",
        )
        .unwrap();

        let projected: (String, i64, f64) = conn
            .query_row(
                "SELECT language_scope, total_positive, wilson_lower
                 FROM latest_review_snapshots WHERE app_id = 42",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(projected, ("english".to_owned(), 90, 0.80));

        conn.execute(
            "UPDATE review_snapshots
             SET total_positive = 91, wilson_lower = 0.81
             WHERE app_id = 42 AND language_scope = 'english' AND captured_at_ms = 20",
            [],
        )
        .unwrap();
        let updated: (i64, f64) = conn
            .query_row(
                "SELECT total_positive, wilson_lower
                 FROM latest_review_snapshots WHERE app_id = 42",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(updated, (91, 0.81));

        conn.execute(
            "DELETE FROM review_snapshots
             WHERE app_id = 42 AND language_scope = 'english' AND captured_at_ms = 20",
            [],
        )
        .unwrap();
        let fallback: (String, i64, f64) = conn
            .query_row(
                "SELECT language_scope, total_positive, wilson_lower
                 FROM latest_review_snapshots WHERE app_id = 42",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(fallback, ("schinese".to_owned(), 95, 0.85));
    }
}
