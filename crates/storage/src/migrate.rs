use crate::error::{StorageError, StorageResult};
use crate::pg::{Connection, params};

/// The fresh PostgreSQL baseline shared with the Supabase migration runner.
/// SQLite migration histories are intentionally not accepted by this deployment.
pub const MIGRATIONS: &[(i64, &str, &str)] = &[(
    1,
    "20261019000000_initial",
    include_str!("../../../supabase/migrations/20261018174837_lobbytally_initial.sql"),
)];

// Index-only additions do not change the application schema contract. Reuse
// the forward Supabase SQL for local bootstrap without rewriting the deployed
// baseline or making older binaries reject a new application schema version.
const RELEASE_TRANSITION_INDEX_SQL: &str = include_str!(
    "../../../supabase/migrations/20261018203956_lobbytally_release_transition_index.sql"
);

pub fn current_version(conn: &Connection) -> StorageResult<i64> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS (
            SELECT 1 FROM information_schema.tables
            WHERE table_schema = coalesce(nullif(current_setting('mpgs.schema', true), ''), 'mpgs')
              AND table_name = 'schema_migrations'
        )",
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
    MIGRATIONS
        .last()
        .map(|(version, _, _)| *version)
        .unwrap_or(0)
}

pub fn migrate_to(conn: &mut Connection, target: i64, now_ms: i64) -> StorageResult<i64> {
    if target < 0 || target > latest_version() {
        return Err(StorageError::migration(format!(
            "target migration version {target} is out of range"
        )));
    }

    let tx = conn.transaction()?;
    // Lock before reading the migration history so concurrent cold starts cannot
    // both apply the baseline. The schema setting also isolates parallel tests.
    tx.execute_batch(
        "SELECT pg_advisory_xact_lock(hashtextextended(
            coalesce(nullif(current_setting('mpgs.schema', true), ''), 'mpgs'), 0
        ));",
    )?;
    let mut current = current_version(&tx)?;
    if current > target {
        return Err(StorageError::migration(format!(
            "database is at version {current}, cannot migrate down to {target}"
        )));
    }

    while current < target {
        let next = current + 1;
        let Some((_, name, sql)) = MIGRATIONS.iter().find(|(version, _, _)| *version == next)
        else {
            return Err(StorageError::migration(format!(
                "missing migration script for version {next}"
            )));
        };
        tx.query_row(
            "SELECT set_config('mpgs.migration_now_ms', $1, true)",
            params![now_ms.to_string()],
            |row| row.get::<_, String>(0),
        )?;
        tx.execute_batch(sql)?;
        // The baseline records itself so the same SQL can be applied through
        // Supabase. Preserve the caller's clock when this runner applies it.
        let updated = tx.execute(
            "UPDATE schema_migrations SET applied_at_ms = $1 WHERE version = $2 AND name = $3",
            params![now_ms, next, name],
        )?;
        if updated != 1 {
            return Err(StorageError::migration(format!(
                "migration {next} did not record its expected history entry"
            )));
        }
        current = next;
    }
    if current > 0 {
        tx.execute_batch(RELEASE_TRANSITION_INDEX_SQL)?;
    }
    tx.commit()?;
    Ok(current)
}

/// Preserve schema history while ensuring additive indexes are installed.
pub fn migrate_idempotent(conn: &mut Connection, now_ms: i64) -> StorageResult<i64> {
    migrate_to_latest(conn, now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Database;

    #[test]
    fn baseline_history_is_idempotent_and_rejects_unknown_versions() {
        let db = Database::open_in_memory().unwrap();
        db.with_conn_mut(|conn| {
            assert_eq!(current_version(conn)?, 0);
            assert_eq!(migrate_to_latest(conn, 7)?, 1);
            assert_eq!(migrate_idempotent(conn, 8)?, 1);
            let applied_at_ms: i64 = conn.query_row(
                "SELECT applied_at_ms FROM schema_migrations WHERE version = 1",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(applied_at_ms, 7);
            let seeded_at: (i64, i64, i64) = conn.query_row(
                "SELECT
                    (SELECT created_at_ms FROM algorithm_configs WHERE status = 'active'),
                    (SELECT generated_at_ms FROM pipeline_status_snapshot WHERE snapshot_id = 1),
                    (SELECT MIN(updated_at_ms) FROM data_refresh_state)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            assert_eq!(seeded_at, (7, 7, 7));
            conn.execute(
                "INSERT INTO schema_migrations (version, name, applied_at_ms)
                 VALUES (2, 'unknown', 9)",
                [],
            )?;
            assert!(matches!(
                current_version(conn),
                Err(StorageError::Migration { .. })
            ));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn baseline_history_rejects_gaps_and_renamed_scripts() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        db.with_conn(|conn| {
            conn.execute("UPDATE schema_migrations SET version = 2", [])?;
            assert!(matches!(
                current_version(conn),
                Err(StorageError::Migration { .. })
            ));
            conn.execute(
                "UPDATE schema_migrations SET version = 1, name = 'tampered'",
                [],
            )?;
            assert!(matches!(
                current_version(conn),
                Err(StorageError::Migration { .. })
            ));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn direct_supabase_baseline_is_recognized_and_all_tables_have_rls() {
        let db = Database::open_in_memory().unwrap();
        db.with_conn_mut(|conn| {
            conn.execute_batch(MIGRATIONS[0].2)?;
            assert_eq!(current_version(conn)?, 1);
            assert_eq!(migrate_to_latest(conn, 1)?, 1);
            let unprotected: i64 = conn.query_row(
                "SELECT COUNT(*) FROM pg_class relation
                 JOIN pg_namespace namespace ON namespace.oid = relation.relnamespace
                 WHERE namespace.nspname = current_schema()
                   AND relation.relkind IN ('r', 'p') AND NOT relation.relrowsecurity",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(unprotected, 0);
            for index in [
                "idx_app_relations_target_type",
                "idx_review_snapshots_app_latest",
                "idx_apps_feed_release_scope",
                "idx_feature_evidence_enrichment_candidates",
                "idx_game_documents_search",
                "idx_apps_name_trigram",
                "idx_app_localizations_name_trigram",
            ] {
                let exists: bool = conn.query_row(
                    "SELECT EXISTS (
                        SELECT 1 FROM pg_indexes
                        WHERE schemaname = current_schema() AND indexname = $1
                    )",
                    [index],
                    |row| row.get(0),
                )?;
                assert!(exists, "missing index {index}");
            }
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn fresh_database_is_ready_before_its_first_catalog_ingestion() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        db.with_conn(|conn| {
            let config = crate::users::active_algorithm_config(conn)?;
            assert_eq!(config.version, "rules-0.3.0");
            assert_eq!(config.config, mpgs_domain::RecommendationConfig::default());
            let app_count: i64 =
                conn.query_row("SELECT COUNT(*) FROM apps", [], |row| row.get(0))?;
            assert_eq!(app_count, 0);

            let mut statement =
                conn.prepare("SELECT task_name FROM data_refresh_state ORDER BY task_name")?;
            let tasks = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            assert_eq!(
                tasks,
                [
                    "candidate_continuation",
                    "candidate_top_refresh",
                    "catalog_sync",
                    "enrichment",
                    "pipeline_snapshot",
                    "quality_check",
                    "retrieval_sync",
                ]
                .map(str::to_owned)
            );

            let (generated_at_ms, snapshot_json): (i64, String) = conn.query_row(
                "SELECT generated_at_ms, snapshot_json
                 FROM pipeline_status_snapshot WHERE snapshot_id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let snapshot: crate::models::PipelineStatusSnapshot =
                serde_json::from_str(&snapshot_json)?;
            assert_eq!(
                snapshot,
                crate::models::PipelineStatusSnapshot {
                    generated_at_ms,
                    ..Default::default()
                }
            );
            let feed_watermark: (i64, i64) = conn.query_row(
                "SELECT epoch_slot, snapshot_ms FROM feed_snapshot_state WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            assert_eq!(feed_watermark, (-1, 0));
            Ok(())
        })
        .unwrap();
        crate::Repository::new(db).readiness_check().unwrap();
    }

    #[test]
    fn feed_evidence_projection_tracks_active_rows() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO apps (
                     app_id, app_type, canonical_name, release_state,
                     created_at_ms, updated_at_ms
                 ) VALUES (42, 'game', 'Taxonomy Projection', 'released', 1, 1)",
                [],
            )?;
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
            )?;
            let projection_rows: i64 =
                conn.query_row("SELECT COUNT(*) FROM feed_feature_evidence", [], |row| {
                    row.get(0)
                })?;
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
            )?;
            assert_eq!(latest_projected_taxonomy(conn)?, r#"{"genres":["new"]}"#);
            conn.execute(
                "UPDATE feature_evidence SET is_active = 0 WHERE evidence_id = 4",
                [],
            )?;
            assert_eq!(latest_projected_taxonomy(conn)?, r#"{"genres":["old"]}"#);
            conn.execute(
                "UPDATE feature_evidence
                 SET feature_name = 'catalog_taxonomy' WHERE evidence_id = 3",
                [],
            )?;
            assert_eq!(
                latest_projected_taxonomy(conn)?,
                r#"{"category":"multiplayer"}"#
            );
            conn.execute("DELETE FROM feature_evidence WHERE evidence_id = 3", [])?;
            assert_eq!(latest_projected_taxonomy(conn)?, r#"{"genres":["old"]}"#);
            Ok(())
        })
        .unwrap();
    }

    fn latest_projected_taxonomy(conn: &Connection) -> StorageResult<String> {
        Ok(conn.query_row(
            "SELECT value_json FROM feed_feature_evidence
             WHERE app_id = 42 AND feature_name = 'catalog_taxonomy'
             ORDER BY observed_at_ms DESC, evidence_id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )?)
    }

    #[test]
    fn released_transition_lookup_uses_partial_index() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        db.with_conn(|conn| {
            let tx = conn.read_transaction()?;
            // A tiny fixture may favor a sequential scan. Verify that the
            // PostgreSQL planner can use this partial index for the real query.
            tx.execute_batch("SET LOCAL enable_seqscan = off")?;
            let plan = tx
                .prepare(
                    "EXPLAIN (COSTS OFF)
                 SELECT app_id, MAX(observed_at_ms)
                 FROM release_events
                 WHERE new_release_state = 'released'
                   AND old_release_state <> 'released'
                 GROUP BY app_id",
                )?
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<crate::pg::Result<Vec<_>>>()?;
            assert!(
                plan.iter()
                    .any(|step| step.contains("idx_release_events_to_released")),
                "released transition lookup did not use partial index: {plan:?}"
            );
            tx.commit()?;
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn additive_release_index_preserves_existing_baseline_and_zero_target() {
        let db = Database::open_in_memory().unwrap();
        db.with_conn_mut(|conn| {
            assert_eq!(migrate_to(conn, 0, 1)?, 0);
            assert_eq!(current_version(conn)?, 0);
            let has_release_table: bool = conn.query_row(
                "SELECT to_regclass(quote_ident(current_schema()) || '.release_events') IS NOT NULL",
                [],
                |row| row.get(0),
            )?;
            assert!(!has_release_table);

            // Simulate the already-deployed Supabase baseline, without the new index.
            conn.execute_batch(MIGRATIONS[0].2)?;
            conn.execute("UPDATE schema_migrations SET applied_at_ms = 7", [])?;
            assert_eq!(current_version(conn)?, 1);
            assert_eq!(migrate_to_latest(conn, 8)?, 1);
            assert_eq!(migrate_idempotent(conn, 9)?, 1);
            let history: (i64, i64) = conn.query_row(
                "SELECT COUNT(*), MIN(applied_at_ms) FROM schema_migrations",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            assert_eq!(history, (1, 7));
            let index_count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM pg_indexes
                 WHERE schemaname = current_schema() AND indexname = 'idx_release_events_to_released'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(index_count, 1);
            Ok(())
        }).unwrap();
    }

    #[test]
    fn latest_review_projection_tracks_insert_update_and_delete() {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO apps (
                     app_id, app_type, canonical_name, release_state,
                     created_at_ms, updated_at_ms
                 ) VALUES (42, 'game', 'Projection Test', 'released', 1, 1)",
                [],
            )?;
            conn.execute_batch(
                "INSERT INTO review_snapshots (
                     app_id, region_scope, language_scope, captured_at_ms,
                     total_positive, total_negative, total_reviews, wilson_lower,
                     filter_offtopic_activity, parameter_hash, content_hash, source
                 ) VALUES
                 (42, 'all', 'english', 20, 90, 10, 100, 0.80, 1, 'p1', 'c1', 'test'),
                 (42, 'all', 'schinese', 10, 40, 10, 50, 0.70, 1, 'p2', 'c2', 'test'),
                 (42, 'all', 'schinese', 20, 95, 5, 100, 0.85, 1, 'p3', 'c3', 'test');",
            )?;
            let projected: (String, i64, f64) = conn.query_row(
                "SELECT language_scope, total_positive, wilson_lower
                 FROM latest_review_snapshots WHERE app_id = 42",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            assert_eq!(projected, ("english".to_owned(), 90, 0.80));
            conn.execute(
                "UPDATE review_snapshots
                 SET total_positive = 91, wilson_lower = 0.81
                 WHERE app_id = 42 AND language_scope = 'english' AND captured_at_ms = 20",
                [],
            )?;
            let updated: (i64, f64) = conn.query_row(
                "SELECT total_positive, wilson_lower
                 FROM latest_review_snapshots WHERE app_id = 42",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            assert_eq!(updated, (91, 0.81));
            conn.execute(
                "DELETE FROM review_snapshots
                 WHERE app_id = 42 AND language_scope = 'english' AND captured_at_ms = 20",
                [],
            )?;
            let fallback: (String, i64, f64) = conn.query_row(
                "SELECT language_scope, total_positive, wilson_lower
                 FROM latest_review_snapshots WHERE app_id = 42",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            assert_eq!(fallback, ("schinese".to_owned(), 95, 0.85));
            Ok(())
        })
        .unwrap();
    }
}
