//! Controlled M7 background maintenance. Network collection remains a leased
//! job so no server process opens a shared SQLite file from a worker. Locally
//! executable derived work (quality and retrieval sync) runs in-process and
//! updates `data_refresh_state` only after it actually succeeds.

use std::{env, time::Duration};

use mpgs_storage::{DataRefreshStatus, EnqueueJob, Repository, RetrievalSyncStats};
use tokio::time::{MissedTickBehavior, interval};
use tracing::{info, warn};

const DEFAULT_INTERVAL_SECS: u64 = 300;
const DEFAULT_CATALOG_SYNC_INTERVAL_SECS: u64 = 15 * 60;
const DEFAULT_CANDIDATE_COLLECTION_INTERVAL_SECS: u64 = 6 * 60 * 60;
const DEFAULT_ENRICHMENT_INTERVAL_SECS: u64 = 5 * 60;
const RECOMMENDATION_TELEMETRY_RETENTION_INTERVAL_SECS: u64 = 24 * 60 * 60;
const RETRIEVAL_SYNC_BATCH_SIZE: u32 = 2_000;
const TASK_INTERVAL_MIN_SECS: u64 = 60;
const TASK_INTERVAL_MAX_SECS: u64 = 86_400;
const TELEMETRY_RETENTION_TASK: &str = "recommendation_telemetry_retention";

#[derive(Clone, Copy)]
struct TaskIntervals {
    catalog_sync_secs: u64,
    candidate_collection_secs: u64,
    enrichment_secs: u64,
}

impl TaskIntervals {
    fn from_env() -> Self {
        Self {
            catalog_sync_secs: configured_interval(
                "MPGS_CATALOG_SYNC_INTERVAL_SECS",
                DEFAULT_CATALOG_SYNC_INTERVAL_SECS,
                TASK_INTERVAL_MIN_SECS,
            ),
            candidate_collection_secs: configured_interval(
                "MPGS_CANDIDATE_COLLECTION_INTERVAL_SECS",
                DEFAULT_CANDIDATE_COLLECTION_INTERVAL_SECS,
                TASK_INTERVAL_MIN_SECS,
            ),
            enrichment_secs: configured_interval(
                "MPGS_ENRICHMENT_INTERVAL_SECS",
                DEFAULT_ENRICHMENT_INTERVAL_SECS,
                TASK_INTERVAL_MIN_SECS,
            ),
        }
    }
}

impl Default for TaskIntervals {
    fn default() -> Self {
        Self {
            catalog_sync_secs: DEFAULT_CATALOG_SYNC_INTERVAL_SECS,
            candidate_collection_secs: DEFAULT_CANDIDATE_COLLECTION_INTERVAL_SECS,
            enrichment_secs: DEFAULT_ENRICHMENT_INTERVAL_SECS,
        }
    }
}

pub fn spawn(repo: Option<Repository>) {
    let Some(repo) = repo else {
        return;
    };
    let interval_secs =
        configured_interval("MPGS_SCHEDULER_INTERVAL_SECS", DEFAULT_INTERVAL_SECS, 30);
    let task_intervals = TaskIntervals::from_env();
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(interval_secs));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let run_repo = repo.clone();
            match tokio::task::spawn_blocking(move || {
                run_once(&run_repo, interval_secs, task_intervals)
            })
            .await
            {
                Ok(Ok(())) => info!("background data maintenance completed"),
                Ok(Err(error)) => warn!(error = %error, "background data maintenance failed"),
                Err(error) => warn!(error = %error, "background data maintenance task panicked"),
            }
        }
    });
}

fn configured_interval(name: &str, default: u64, minimum: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| (minimum..=TASK_INTERVAL_MAX_SECS).contains(value))
        .unwrap_or(default)
}

fn run_once(
    repo: &Repository,
    interval_secs: u64,
    task_intervals: TaskIntervals,
) -> mpgs_storage::StorageResult<()> {
    run_once_with_schedule(
        repo,
        interval_secs,
        steam_web_api_key_configured(),
        task_intervals,
    )
}

#[cfg(test)]
fn run_once_with_catalog_sync(
    repo: &Repository,
    interval_secs: u64,
    catalog_sync_enabled: bool,
) -> mpgs_storage::StorageResult<()> {
    run_once_with_schedule(
        repo,
        interval_secs,
        catalog_sync_enabled,
        TaskIntervals::default(),
    )
}

fn run_once_with_schedule(
    repo: &Repository,
    interval_secs: u64,
    catalog_sync_enabled: bool,
    task_intervals: TaskIntervals,
) -> mpgs_storage::StorageResult<()> {
    let now_ms = repo.database().now_ms();
    let interval_ms = (interval_secs as i64).saturating_mul(1_000);
    let next_run_at_ms = now_ms.saturating_add(interval_ms);
    let coverage = repo.m3_catalog_coverage()?;
    let coverage_ratio = if coverage.normalized_multiplayer_candidates > 0 {
        Some(
            (coverage.recommendation_ready_profiles as f64
                / coverage.normalized_multiplayer_candidates as f64)
                .clamp(0.0, 1.0),
        )
    } else {
        Some(0.0)
    };
    let previous_status = repo.data_refresh_status()?;

    // Lease-backed collection work is deliberately queued rather than run in a
    // database transaction. A co-located worker leases these tasks and writes
    // source snapshots independently, so Steam failure cannot clear a good
    // catalog snapshot held by the API process. Each task has an independent
    // cadence and only one active scheduled job, which prevents a slow catalog
    // sync from accumulating ahead of candidate discovery or enrichment.
    for (task_name, task_type, task_interval_secs, enabled) in [
        (
            "catalog_sync",
            "sync_catalog",
            task_intervals.catalog_sync_secs,
            catalog_sync_enabled,
        ),
        (
            "candidate_collection",
            "collect_candidates",
            task_intervals.candidate_collection_secs,
            true,
        ),
        (
            "enrichment",
            "enrich_catalog",
            task_intervals.enrichment_secs,
            true,
        ),
    ] {
        let previous = previous_status
            .iter()
            .find(|status| status.task_name == task_name);
        let task_interval_ms = (task_interval_secs as i64).saturating_mul(1_000);
        let task_next_run_at_ms = now_ms.saturating_add(task_interval_ms);
        if !enabled {
            update_scheduled_status(
                repo,
                task_name,
                previous,
                task_next_run_at_ms,
                Some("auth"),
                coverage_ratio,
            )?;
            continue;
        }
        let due = previous
            .and_then(|status| status.next_run_at_ms)
            .is_none_or(|next_run_at_ms| next_run_at_ms <= now_ms);
        if !due || repo.has_active_job("steam", task_type, "scheduled")? {
            continue;
        }
        let slot = now_ms / task_interval_ms.max(1);
        let _ = repo.enqueue_job(&EnqueueJob {
            source: "steam".to_owned(),
            task_type: task_type.to_owned(),
            entity_key: "scheduled".to_owned(),
            priority: 50,
            due_at_ms: now_ms,
            idempotency_key: format!("m7-scheduler:{task_name}:{slot}"),
            payload_json: None,
            max_attempts: 3,
        })?;
        update_scheduled_status(
            repo,
            task_name,
            previous,
            task_next_run_at_ms,
            None,
            coverage_ratio,
        )?;
    }

    let quality_previous = previous_status
        .iter()
        .find(|status| status.task_name == "quality_check");
    match repo.run_quality_checks() {
        Ok(_) => repo.update_data_refresh_status(
            "quality_check",
            Some(now_ms),
            Some(next_run_at_ms),
            None,
            None,
            coverage_ratio,
        )?,
        Err(error) => {
            repo.update_data_refresh_status(
                "quality_check",
                quality_previous.and_then(|status| status.last_success_at_ms),
                Some(next_run_at_ms),
                Some("quality_check_failed"),
                quality_previous.and_then(|status| status.cursor_value.as_deref()),
                coverage_ratio,
            )?;
            return Err(error);
        }
    }

    let retrieval_previous = previous_status
        .iter()
        .find(|status| status.task_name == "retrieval_sync");
    sync_retrieval_batch(
        repo,
        retrieval_previous,
        now_ms,
        next_run_at_ms,
        RETRIEVAL_SYNC_BATCH_SIZE,
        true,
    )?;
    let telemetry_previous = previous_status
        .iter()
        .find(|status| status.task_name == TELEMETRY_RETENTION_TASK);
    run_recommendation_telemetry_retention(repo, telemetry_previous, now_ms, next_run_at_ms)?;
    Ok(())
}

fn run_recommendation_telemetry_retention(
    repo: &Repository,
    previous: Option<&DataRefreshStatus>,
    now_ms: i64,
    retry_at_ms: i64,
) -> mpgs_storage::StorageResult<()> {
    let due = previous
        .and_then(|status| status.next_run_at_ms)
        .is_none_or(|next_run_at_ms| next_run_at_ms <= now_ms);
    if !due {
        return Ok(());
    }

    match repo.purge_expired_recommendation_telemetry() {
        Ok(purged) => {
            let next_run_at_ms = now_ms.saturating_add(
                (RECOMMENDATION_TELEMETRY_RETENTION_INTERVAL_SECS as i64).saturating_mul(1_000),
            );
            repo.update_data_refresh_status(
                TELEMETRY_RETENTION_TASK,
                Some(now_ms),
                Some(next_run_at_ms),
                None,
                None,
                None,
            )?;
            info!(
                runs = purged.runs,
                items = purged.items,
                events = purged.events,
                "expired recommendation telemetry purged"
            );
            Ok(())
        }
        Err(error) => {
            // A failed retention pass must not erase the last known success.
            // Retry on the scheduler cadence rather than waiting another day.
            repo.update_data_refresh_status(
                TELEMETRY_RETENTION_TASK,
                previous.and_then(|status| status.last_success_at_ms),
                Some(retry_at_ms),
                Some("telemetry_retention_failed"),
                previous.and_then(|status| status.cursor_value.as_deref()),
                previous.and_then(|status| status.coverage_ratio),
            )?;
            Err(error)
        }
    }
}

fn sync_retrieval_batch(
    repo: &Repository,
    previous: Option<&DataRefreshStatus>,
    now_ms: i64,
    next_run_at_ms: i64,
    batch_size: u32,
    write_embeddings: bool,
) -> mpgs_storage::StorageResult<RetrievalSyncStats> {
    // Older releases stored the number scanned here. Treating that numeric
    // value as an app-id cursor is safe: it may repeat work once, but cannot
    // skip the high app ids that were previously invisible after the first
    // fixed 2,000 rows. Invalid values restart a full pass.
    let after_app_id = previous
        .and_then(|status| status.cursor_value.as_deref())
        .and_then(|cursor| cursor.parse::<u32>().ok())
        .unwrap_or(0);
    match repo.sync_retrieval_from_catalog(batch_size, after_app_id, write_embeddings) {
        Ok(stats) => {
            let next_cursor = stats.next_after_app_id.to_string();
            repo.update_data_refresh_status(
                "retrieval_sync",
                Some(now_ms),
                Some(next_run_at_ms),
                None,
                Some(&next_cursor),
                Some(stats.coverage_ratio()),
            )?;
            Ok(stats)
        }
        Err(error) => {
            repo.update_data_refresh_status(
                "retrieval_sync",
                previous.and_then(|status| status.last_success_at_ms),
                Some(next_run_at_ms),
                Some("retrieval_sync_failed"),
                previous.and_then(|status| status.cursor_value.as_deref()),
                previous.and_then(|status| status.coverage_ratio),
            )?;
            Err(error)
        }
    }
}

fn steam_web_api_key_configured() -> bool {
    env::var("MPGS_STEAM_WEB_API_KEY")
        .ok()
        .map(|value| value.trim().to_owned())
        .is_some_and(|value| {
            value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}

fn update_scheduled_status(
    repo: &Repository,
    task_name: &str,
    previous: Option<&DataRefreshStatus>,
    next_run_at_ms: i64,
    error_category: Option<&str>,
    coverage_ratio: Option<f64>,
) -> mpgs_storage::StorageResult<()> {
    repo.update_data_refresh_status(
        task_name,
        previous.and_then(|status| status.last_success_at_ms),
        Some(next_run_at_ms),
        error_category,
        previous.and_then(|status| status.cursor_value.as_deref()),
        coverage_ratio,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use mpgs_storage::{
        Database, FakeClock, InsertRecommendationItem, InsertRecommendationRun,
        RECOMMENDATION_TELEMETRY_RETENTION_MS, Repository, hash_candidate_set,
        hash_structured_context,
    };
    use serde_json::json;

    fn insert_telemetry(repo: &Repository) -> (String, u32) {
        let app_id = repo
            .database()
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT app_id FROM apps ORDER BY app_id LIMIT 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map(|app_id| app_id as u32)
                .map_err(Into::into)
            })
            .unwrap();
        let run = repo
            .insert_recommendation_run(&InsertRecommendationRun {
                subject_hash: None,
                request_kind: "feed".into(),
                feed_section: "recent".into(),
                algorithm_version: "rules-0.3.0".into(),
                config_version: "rules-0.3.0".into(),
                score_semantics: "context_percentile_v1".into(),
                context_schema_version: 1,
                context_hash: hash_structured_context(&json!({"section": "recent"})).unwrap(),
                candidate_set_hash: hash_candidate_set(&[app_id]).unwrap(),
                candidate_count: 1,
            })
            .unwrap();
        repo.insert_recommendation_items(
            &run.recommendation_run_id,
            &[InsertRecommendationItem {
                app_id,
                rank: 1,
                relevance_score: 0.75,
                recommendation_index: Some(50),
                data_confidence: 0.8,
                slot_reason: "base".into(),
                score_components: json!({"quality": 0.7}),
            }],
        )
        .unwrap();
        (run.recommendation_run_id, app_id)
    }

    #[test]
    fn records_only_completed_derived_work_as_success() {
        let db = Database::open_in_memory().unwrap();
        let repo = Repository::new(db);
        repo.migrate().unwrap();
        repo.ensure_runtime_defaults().unwrap();
        repo.seed_demo_if_empty().unwrap();
        run_once_with_catalog_sync(&repo, DEFAULT_INTERVAL_SECS, false).unwrap();
        let status = repo.data_refresh_status().unwrap();
        let quality = status
            .iter()
            .find(|item| item.task_name == "quality_check")
            .unwrap();
        assert!(quality.last_success_at_ms.is_some());
        let collection = status
            .iter()
            .find(|item| item.task_name == "candidate_collection")
            .unwrap();
        assert!(collection.last_success_at_ms.is_none());
        assert!(collection.last_error_category.is_none());
        let catalog = status
            .iter()
            .find(|item| item.task_name == "catalog_sync")
            .unwrap();
        assert_eq!(catalog.last_error_category.as_deref(), Some("auth"));
    }

    #[test]
    fn keeps_collection_success_when_scheduling_the_next_job() {
        let db = Database::open_in_memory().unwrap();
        let repo = Repository::new(db);
        repo.migrate().unwrap();
        repo.ensure_runtime_defaults().unwrap();
        repo.seed_demo_if_empty().unwrap();
        repo.update_data_refresh_status(
            "candidate_collection",
            Some(123),
            None,
            None,
            Some("cursor"),
            Some(0.5),
        )
        .unwrap();

        run_once_with_catalog_sync(&repo, DEFAULT_INTERVAL_SECS, false).unwrap();

        let status = repo.data_refresh_status().unwrap();
        let collection = status
            .iter()
            .find(|item| item.task_name == "candidate_collection")
            .unwrap();
        assert_eq!(collection.last_success_at_ms, Some(123));
        assert_eq!(collection.cursor_value.as_deref(), Some("cursor"));
        assert!(collection.next_run_at_ms.is_some());
        assert!(collection.last_error_category.is_none());
    }

    #[test]
    fn keeps_one_active_job_per_collection_task_and_respects_task_cadence() {
        let clock = Arc::new(FakeClock::new(0));
        let db = Database::open_in_memory_with_clock(clock.clone()).unwrap();
        let repo = Repository::new(db);
        repo.migrate().unwrap();
        repo.ensure_runtime_defaults().unwrap();
        repo.seed_demo_if_empty().unwrap();
        let task_intervals = TaskIntervals {
            catalog_sync_secs: 60,
            candidate_collection_secs: 600,
            enrichment_secs: 60,
        };

        run_once_with_schedule(&repo, 30, false, task_intervals).unwrap();
        assert!(
            repo.has_active_job("steam", "collect_candidates", "scheduled")
                .unwrap()
        );
        assert!(
            repo.has_active_job("steam", "enrich_catalog", "scheduled")
                .unwrap()
        );

        clock.advance_ms(60_000);
        run_once_with_schedule(&repo, 30, false, task_intervals).unwrap();
        let scheduled_jobs: i64 = repo
            .database()
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM jobs
                     WHERE source = 'steam' AND entity_key = 'scheduled'",
                    [],
                    |row| row.get(0),
                )
                .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(
            scheduled_jobs, 2,
            "due tasks must not accumulate while active"
        );

        let jobs = repo
            .lease_jobs("test-worker", 10, 60_000, Some("steam"))
            .unwrap();
        assert_eq!(jobs.len(), 2);
        for job in jobs {
            repo.complete_job(job.job_id, "test-worker", &format!("done-{}", job.job_id))
                .unwrap();
        }

        run_once_with_schedule(&repo, 30, false, task_intervals).unwrap();
        assert!(
            repo.has_active_job("steam", "enrich_catalog", "scheduled")
                .unwrap()
        );
        assert!(
            !repo
                .has_active_job("steam", "collect_candidates", "scheduled")
                .unwrap(),
            "candidate collection waits for its longer interval"
        );
    }

    #[test]
    fn retrieval_sync_persists_cursor_until_full_catalog_coverage() {
        let db = Database::open_in_memory().unwrap();
        let repo = Repository::new(db);
        repo.migrate().unwrap();
        repo.ensure_runtime_defaults().unwrap();
        repo.seed_demo_if_empty().unwrap();
        let catalog_apps = repo
            .database()
            .with_conn(|conn| {
                conn.query_row("SELECT COUNT(*) FROM apps", [], |row| row.get::<_, i64>(0))
                    .map_err(Into::into)
            })
            .unwrap() as u32;
        assert!(catalog_apps > 2);

        let mut previous_coverage = 0.0;
        let mut batches = 0;
        loop {
            let statuses = repo.data_refresh_status().unwrap();
            let previous = statuses
                .iter()
                .find(|status| status.task_name == "retrieval_sync");
            let stats = sync_retrieval_batch(&repo, previous, 100, 200, 2, false).unwrap();
            batches += 1;

            let updated = repo
                .data_refresh_status()
                .unwrap()
                .into_iter()
                .find(|status| status.task_name == "retrieval_sync")
                .unwrap();
            assert_eq!(
                updated.cursor_value.as_deref(),
                Some(stats.next_after_app_id.to_string().as_str())
            );
            let coverage = updated.coverage_ratio.unwrap();
            assert!(coverage >= previous_coverage);
            assert_eq!(coverage, stats.coverage_ratio());

            if !stats.has_more {
                assert_eq!(updated.cursor_value.as_deref(), Some("0"));
                assert_eq!(coverage, 1.0);
                break;
            }
            assert_ne!(updated.cursor_value.as_deref(), Some("0"));
            assert!(coverage < 1.0);
            previous_coverage = coverage;
            assert!(
                batches <= catalog_apps,
                "persisted cursor must make progress"
            );
        }

        assert_eq!(batches, catalog_apps.div_ceil(2));
        let indexed_apps = repo
            .database()
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(DISTINCT app_id) FROM game_documents",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(Into::into)
            })
            .unwrap() as u32;
        assert_eq!(indexed_apps, catalog_apps);
    }

    #[test]
    fn scheduler_purges_expired_telemetry_once_per_daily_cadence() {
        let clock = Arc::new(FakeClock::new(1_000));
        let db = Database::open_in_memory_with_clock(clock.clone()).unwrap();
        let repo = Repository::new(db);
        repo.migrate().unwrap();
        repo.ensure_runtime_defaults().unwrap();
        repo.seed_demo_if_empty().unwrap();

        let (expired_run_id, app_id) = insert_telemetry(&repo);
        clock.advance_ms(RECOMMENDATION_TELEMETRY_RETENTION_MS + 1);
        run_once_with_catalog_sync(&repo, DEFAULT_INTERVAL_SECS, false).unwrap();
        assert!(
            repo.recommendation_item_attribution(&expired_run_id, app_id)
                .unwrap()
                .is_none()
        );

        let first_status = repo
            .data_refresh_status()
            .unwrap()
            .into_iter()
            .find(|status| status.task_name == TELEMETRY_RETENTION_TASK)
            .unwrap();
        let first_success_at_ms = first_status.last_success_at_ms.unwrap();
        assert_eq!(first_success_at_ms, repo.database().now_ms());
        assert_eq!(
            first_status.next_run_at_ms,
            Some(
                first_success_at_ms
                    + (RECOMMENDATION_TELEMETRY_RETENTION_INTERVAL_SECS as i64) * 1_000
            )
        );
        assert!(first_status.last_error_category.is_none());

        let (next_run_id, next_app_id) = insert_telemetry(&repo);
        repo.database()
            .with_conn_mut(|conn| {
                conn.execute(
                    "UPDATE recommendation_items SET expires_at_ms = ?2
                     WHERE recommendation_run_id = ?1",
                    (&next_run_id, first_success_at_ms + 1),
                )?;
                conn.execute(
                    "UPDATE recommendation_runs SET expires_at_ms = ?2
                     WHERE recommendation_run_id = ?1",
                    (&next_run_id, first_success_at_ms + 1),
                )?;
                Ok(())
            })
            .unwrap();

        clock.advance_ms((DEFAULT_INTERVAL_SECS as i64) * 1_000);
        run_once_with_catalog_sync(&repo, DEFAULT_INTERVAL_SECS, false).unwrap();
        assert!(
            repo.recommendation_item_attribution(&next_run_id, next_app_id)
                .unwrap()
                .is_some(),
            "a scheduler tick before the daily deadline must not run retention again"
        );

        clock.advance_ms(
            (RECOMMENDATION_TELEMETRY_RETENTION_INTERVAL_SECS as i64
                - DEFAULT_INTERVAL_SECS as i64)
                * 1_000,
        );
        run_once_with_catalog_sync(&repo, DEFAULT_INTERVAL_SECS, false).unwrap();
        assert!(
            repo.recommendation_item_attribution(&next_run_id, next_app_id)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn telemetry_retention_failure_is_observable_and_retries_on_scheduler_cadence() {
        let clock = Arc::new(FakeClock::new(10_000));
        let db = Database::open_in_memory_with_clock(clock).unwrap();
        let repo = Repository::new(db);
        repo.migrate().unwrap();
        repo.update_data_refresh_status(
            TELEMETRY_RETENTION_TASK,
            Some(7_000),
            Some(9_000),
            None,
            None,
            None,
        )
        .unwrap();
        repo.database()
            .with_conn_mut(|conn| {
                conn.execute("DROP TABLE recommendation_events", [])?;
                Ok(())
            })
            .unwrap();

        let previous = repo
            .data_refresh_status()
            .unwrap()
            .into_iter()
            .find(|status| status.task_name == TELEMETRY_RETENTION_TASK)
            .unwrap();
        let retry_at_ms = 10_000 + (DEFAULT_INTERVAL_SECS as i64) * 1_000;
        assert!(
            run_recommendation_telemetry_retention(&repo, Some(&previous), 10_000, retry_at_ms,)
                .is_err()
        );

        let failed = repo
            .data_refresh_status()
            .unwrap()
            .into_iter()
            .find(|status| status.task_name == TELEMETRY_RETENTION_TASK)
            .unwrap();
        assert_eq!(failed.last_success_at_ms, Some(7_000));
        assert_eq!(failed.next_run_at_ms, Some(retry_at_ms));
        assert_eq!(
            failed.last_error_category.as_deref(),
            Some("telemetry_retention_failed")
        );
    }
}
