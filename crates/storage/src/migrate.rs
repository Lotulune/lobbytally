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
    (
        24,
        "0024_pipeline_observability",
        include_str!("../../../migrations/0024_pipeline_observability.sql"),
    ),
    (
        25,
        "0025_integrated_game_ingestion",
        include_str!("../../../migrations/0025_integrated_game_ingestion.sql"),
    ),
    (
        26,
        "0026_pipeline_reliability",
        include_str!("../../../migrations/0026_pipeline_reliability.sql"),
    ),
    (
        27,
        "0027_ingestion_lane_circuit_breaker",
        include_str!("../../../migrations/0027_ingestion_lane_circuit_breaker.sql"),
    ),
    (
        28,
        "0028_feed_scope_covering_index",
        include_str!("../../../migrations/0028_feed_scope_covering_index.sql"),
    ),
    (
        29,
        "0029_feed_evidence_projection",
        include_str!("../../../migrations/0029_feed_evidence_projection.sql"),
    ),
    (
        30,
        "0030_worker_candidate_scope_index",
        include_str!("../../../migrations/0030_worker_candidate_scope_index.sql"),
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
            "idx_apps_feed_release_scope",
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

        for (scope, predicate) in [
            (
                "recent",
                "release_state = 'released'
                 AND release_date >= '2026-01-01'
                 AND release_date <= '2026-08-10'",
            ),
            (
                "legacy",
                "release_state = 'released' AND release_date < '2026-01-01'",
            ),
            (
                "upcoming",
                "((release_state IN ('upcoming', 'coming_soon')
                    AND release_date >= '2026-08-10'
                    AND release_date <= '2026-09-09')
                   OR app_type IN ('demo', 'playtest'))",
            ),
        ] {
            let sql = format!(
                "EXPLAIN QUERY PLAN
                 SELECT app_id FROM apps
                 WHERE app_type IN ('game', 'demo', 'playtest') AND ({predicate})"
            );
            let mut statement = conn.prepare(&sql).unwrap();
            let plan: Vec<String> = statement
                .query_map([], |row| row.get(3))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            assert!(
                plan.iter()
                    .any(|step| step.contains("COVERING INDEX idx_apps_feed_release_scope")),
                "{scope} feed scope did not use covering index: {plan:?}"
            );
        }
    }

    #[test]
    fn feed_evidence_projection_tracks_active_rows() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate_to(&mut conn, 28, 1).unwrap();
        conn.execute(
            "INSERT INTO apps (
                 app_id, app_type, canonical_name, release_state,
                 created_at_ms, updated_at_ms
             ) VALUES (42, 'game', 'Taxonomy Projection', 'released', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO feature_evidence (
                 evidence_id, app_id, feature_name, value_json, source_type,
                 source_ref, confidence, observed_at_ms, expires_at_ms, is_active
             ) VALUES
             (1, 42, 'catalog_taxonomy', '{\"genres\":[\"old\"]}',
                'test', 'old', 0.8, 10, NULL, 1),
             (2, 42, 'catalog_taxonomy', '{\"genres\":[\"inactive\"]}',
                'test', 'inactive', 0.8, 20, NULL, 0),
             (3, 42, 'category_hint', '{\"category\":\"multiplayer\"}',
                'test', 'category', 0.3, 15, NULL, 1);",
        )
        .unwrap();

        migrate_to_latest(&mut conn, 2).unwrap();
        let projection_rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM feed_feature_evidence", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(projection_rows, 1);

        conn.execute(
            "INSERT INTO feature_evidence (
                 evidence_id, app_id, feature_name, value_json, source_type,
                 source_ref, confidence, observed_at_ms, expires_at_ms, is_active
             ) VALUES (
                 4, 42, 'catalog_taxonomy', '{\"genres\":[\"new\"]}',
                 'test', 'new', 0.9, 30, NULL, 1
             )",
            [],
        )
        .unwrap();
        assert_eq!(latest_projected_taxonomy(&conn), r#"{"genres":["new"]}"#);

        conn.execute(
            "UPDATE feature_evidence SET is_active = 0 WHERE evidence_id = 4",
            [],
        )
        .unwrap();
        assert_eq!(latest_projected_taxonomy(&conn), r#"{"genres":["old"]}"#);

        conn.execute(
            "UPDATE feature_evidence
             SET feature_name = 'catalog_taxonomy' WHERE evidence_id = 3",
            [],
        )
        .unwrap();
        assert_eq!(
            latest_projected_taxonomy(&conn),
            r#"{"category":"multiplayer"}"#
        );

        conn.execute("DELETE FROM feature_evidence WHERE evidence_id = 3", [])
            .unwrap();
        assert_eq!(latest_projected_taxonomy(&conn), r#"{"genres":["old"]}"#);

        let plan: Vec<String> = conn
            .prepare(
                "EXPLAIN QUERY PLAN
                 SELECT value_json FROM feed_feature_evidence
                 WHERE app_id = 42
                   AND feature_name = 'catalog_taxonomy'
                   AND observed_at_ms >= 1
                   AND (expires_at_ms IS NULL OR expires_at_ms >= 1)
                 ORDER BY observed_at_ms DESC, evidence_id DESC LIMIT 1",
            )
            .unwrap()
            .query_map([], |row| row.get(3))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(
            plan.iter()
                .any(|step| step.contains("COVERING INDEX idx_feed_feature_evidence_latest")),
            "Feed evidence projection did not use its covering index: {plan:?}"
        );
    }

    fn latest_projected_taxonomy(conn: &Connection) -> String {
        conn.query_row(
            "SELECT value_json FROM feed_feature_evidence
             WHERE app_id = 42 AND feature_name = 'catalog_taxonomy'
             ORDER BY observed_at_ms DESC, evidence_id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn worker_candidate_scope_uses_partial_category_index() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate_to_latest(&mut conn, 1).unwrap();
        let plan: Vec<String> = conn
            .prepare(
                "EXPLAIN QUERY PLAN
                 SELECT evidence.app_id
                 FROM feature_evidence evidence
                      INDEXED BY idx_feature_evidence_enrichment_candidates
                 CROSS JOIN apps candidate_app ON candidate_app.app_id = evidence.app_id
                 WHERE evidence.feature_name = 'category_hint'
                   AND evidence.is_active = 1
                   AND evidence.confidence >= 0.3
                   AND candidate_app.app_type IN ('game', 'demo', 'playtest')",
            )
            .unwrap()
            .query_map([], |row| row.get(3))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(
            plan.iter().any(|step| {
                step.contains("USING INDEX idx_feature_evidence_enrichment_candidates")
                    || step
                        .contains("USING COVERING INDEX idx_feature_evidence_enrichment_candidates")
            }),
            "worker candidate scope did not use partial index: {plan:?}"
        );
        let evidence_scan = plan
            .iter()
            .position(|step| step.contains("idx_feature_evidence_enrichment_candidates"))
            .expect("partial evidence index must appear in the plan");
        let app_lookup = plan
            .iter()
            .position(|step| step.contains("candidate_app"))
            .expect("candidate app lookup must appear in the plan");
        assert!(
            evidence_scan < app_lookup,
            "partial evidence index must drive the candidate join: {plan:?}"
        );
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
