use std::env;
use std::time::Instant;

use mpgs_storage::{EnrichmentNeedFilter, MediaBackfillPolicy, Repository};
use serde::{Deserialize, Serialize};

use crate::{
    BatchOptions, BatchStats, ENRICHMENT_CURSOR, LEASE_MS, Outcome, WorkError,
    discovery::{load, save},
    queue_error_category, retry_delay,
    sources::Sources,
};

const LANES: usize = 6;
const LAUNCH_STAGES: [&str; 3] = ["review_summary", "popular_reviews", "ccu"];
const NO_NEEDS: EnrichmentNeedFilter = EnrichmentNeedFilter {
    store: false,
    reviews: false,
    review_excerpts: false,
    ccu: false,
    price: false,
    media_backfill: false,
    english_name: false,
};

#[derive(Default, Serialize, Deserialize)]
struct EnrichmentCursor {
    lane: usize,
    after_app_ids: [u32; LANES],
    dynamic_first: bool,
    #[serde(default)]
    pending_launch: Option<LaunchCursor>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
struct LaunchCursor {
    app_id: u32,
    next_stage: usize,
}

pub(crate) fn run(
    repo: &Repository,
    options: &BatchOptions,
    sources: &mut Sources,
    deadline: Instant,
    stages_left: &mut usize,
    stats: &mut BatchStats,
) -> Result<Outcome, WorkError> {
    run_with(
        repo,
        options,
        deadline,
        stages_left,
        stats,
        &mut |app_id, stage, media| {
            sources.enrichment_stage(
                repo,
                app_id,
                stage,
                &options.country,
                &options.language,
                media,
            )
        },
    )
}

fn run_with(
    repo: &Repository,
    options: &BatchOptions,
    deadline: Instant,
    stages_left: &mut usize,
    stats: &mut BatchStats,
    run_stage: &mut impl FnMut(u32, &str, bool) -> Result<(), WorkError>,
) -> Result<Outcome, WorkError> {
    if *stages_left == 0 || Instant::now() >= deadline {
        return Err(WorkError::budget());
    }
    let mut cursor = load::<EnrichmentCursor>(repo, ENRICHMENT_CURSOR)?.unwrap_or_default();
    if cursor.lane >= LANES
        || cursor
            .pending_launch
            .is_some_and(|pending| pending.app_id == 0 || pending.next_stage >= LAUNCH_STAGES.len())
    {
        return Err(WorkError::invalid("enrichment lane is out of range"));
    }
    // A release discovered in the previous invocation stays ahead of the
    // ordinary backlog, including after a deadline or upstream failure.
    let mut completed = resume_launch(repo, deadline, &mut cursor, stages_left, run_stage)?;
    if *stages_left == 0 {
        return Ok(Outcome::Continue);
    }
    let allowance = *stages_left;
    let preferred = allowance.div_ceil(2);
    let other = allowance / 2;
    let dynamic_first = cursor.dynamic_first;
    cursor.dynamic_first = !dynamic_first;
    save(repo, ENRICHMENT_CURSOR, "steam_store_appdetails", &cursor)?;

    if dynamic_first {
        completed += dynamic(
            repo,
            options,
            run_stage,
            deadline,
            preferred,
            &mut cursor,
            stages_left,
        )?;
        completed += integrated(
            repo,
            options,
            run_stage,
            deadline,
            other,
            stages_left,
            stats,
        )?;
    } else {
        completed += integrated(
            repo,
            options,
            run_stage,
            deadline,
            preferred,
            stages_left,
            stats,
        )?;
        completed += dynamic(
            repo,
            options,
            run_stage,
            deadline,
            other,
            &mut cursor,
            stages_left,
        )?;
    }
    // A productive batch continues on the next Cron tick. If all enabled lanes
    // are empty, the normal five-minute schedule resumes. Upstream failures
    // use the queue's persisted cooldown instead of an immediate continuation.
    Ok(if completed > 0 {
        Outcome::Continue
    } else {
        Outcome::Complete
    })
}

fn integrated(
    repo: &Repository,
    options: &BatchOptions,
    run_stage: &mut impl FnMut(u32, &str, bool) -> Result<(), WorkError>,
    deadline: Instant,
    limit: usize,
    stages_left: &mut usize,
    stats: &mut BatchStats,
) -> Result<usize, WorkError> {
    let mut completed = 0;
    let mut processed = 0;
    let mut launch_priority = false;
    let mut active: Option<mpgs_storage::GameIngestionTask> = None;
    while (processed < limit || launch_priority) && *stages_left > 0 {
        if Instant::now() >= deadline {
            if let Some(task) = active.as_ref() {
                repo.defer_ingestion_job(task.app_id, &options.owner, 0)
                    .map_err(WorkError::storage)?;
            }
            return Err(WorkError::budget());
        }
        if active.is_none() {
            active = repo
                .claim_game_ingestion_tasks(&options.owner, 1, LEASE_MS)
                .map_err(WorkError::storage)?
                .into_iter()
                .next();
        }
        let Some(task) = active.take() else { break };
        processed += 1;
        *stages_left = stages_left.saturating_sub(1);
        let basic_skip = matches!(
            task.enrichment_profile.as_str(),
            "basic_upcoming" | "basic_demo"
        ) && task.stage != "store_details";
        let pre_release_skip = LAUNCH_STAGES.contains(&task.stage.as_str())
            && explicitly_pre_release(repo, task.app_id)?;
        let was_released = task.stage == "store_details" && is_released(repo, task.app_id)?;
        let observed = basic_skip
            || pre_release_skip
            || (!launch_priority
                && repo
                    .game_ingestion_stage_observed(
                        task.app_id,
                        &task.stage,
                        &options.country,
                        &options.language,
                    )
                    .map_err(WorkError::storage)?);
        let result = if observed {
            Ok(())
        } else {
            run_stage(task.app_id, &task.stage, false)
        };
        match result {
            Ok(()) => {
                if task.stage == "store_details" && !was_released && is_released(repo, task.app_id)?
                {
                    launch_priority = true;
                }
                let next = repo
                    .advance_game_ingestion_stage(task.app_id, &options.owner)
                    .map_err(WorkError::storage)?;
                completed += 1;
                if next.stage == "complete" {
                    stats.ingestion_completed += 1;
                    launch_priority = false;
                } else {
                    active = Some(next);
                }
            }
            Err(error) if error.category == "budget" => {
                repo.defer_ingestion_job(task.app_id, &options.owner, 0)
                    .map_err(WorkError::storage)?;
                return Err(error);
            }
            Err(error) => {
                let category = queue_error_category(error.category);
                let delay = retry_delay(category, task.stage_failure_attempts + 1)
                    .max(error.retry_after_ms)
                    .min(604_800_000);
                if matches!(error.category, "auth" | "config") {
                    repo.pause_game_ingestion_lane(
                        &options.owner,
                        error.category,
                        &error.message,
                        delay,
                    )
                    .map_err(WorkError::storage)?;
                } else {
                    repo.retry_game_ingestion_stage(
                        task.app_id,
                        &options.owner,
                        category,
                        &error.message,
                        delay,
                    )
                    .map_err(WorkError::storage)?;
                }
                return Err(error);
            }
        }
    }
    if let Some(task) = active {
        repo.defer_ingestion_job(task.app_id, &options.owner, 0)
            .map_err(WorkError::storage)?;
    }
    Ok(completed)
}

fn dynamic(
    repo: &Repository,
    options: &BatchOptions,
    run_stage: &mut impl FnMut(u32, &str, bool) -> Result<(), WorkError>,
    deadline: Instant,
    limit: usize,
    cursor: &mut EnrichmentCursor,
    stages_left: &mut usize,
) -> Result<usize, WorkError> {
    if limit == 0 || *stages_left == 0 {
        return Ok(0);
    }
    let media_policy = media_policy();
    let media_enabled = if media_policy.enabled && !flag("MPGS_ENRICH_SKIP_STORE") {
        let coverage = repo.media_coverage_stats().map_err(WorkError::storage)?;
        coverage.candidate_apps > 0
            && coverage.coverage_ratio + f64::EPSILON < media_policy.coverage_threshold
    } else {
        false
    };
    let mut completed = 0;
    let mut empty_lanes = 0;
    while completed < limit && empty_lanes < LANES && *stages_left > 0 {
        if Instant::now() >= deadline {
            return Err(WorkError::budget());
        }
        let lane = cursor.lane;
        let (stage, filter) = lane_filter(lane, media_enabled);
        if !filter.any() {
            advance_lane(repo, cursor)?;
            empty_lanes += 1;
            continue;
        }
        let target = repo
            .list_enrichment_targets_after_filtered_with_media(
                1,
                Some(cursor.after_app_ids[lane]),
                &options.country,
                &options.language,
                filter,
                media_policy,
            )
            .map_err(WorkError::storage)?
            .into_iter()
            .next();
        let Some(target) = target else {
            advance_lane(repo, cursor)?;
            empty_lanes += 1;
            continue;
        };
        let was_released = stage == "store_details" && is_released(repo, target.app_id)?;
        *stages_left = stages_left.saturating_sub(1);
        match run_stage(target.app_id, stage, target.needs_media_backfill) {
            Ok(()) => {
                // The app cursor advances only after this individual dimension
                // is committed. Other dimensions retain independent cursors.
                cursor.after_app_ids[lane] = target.app_id;
                if stage == "store_details" && !was_released && is_released(repo, target.app_id)? {
                    cursor.pending_launch = Some(LaunchCursor {
                        app_id: target.app_id,
                        next_stage: 0,
                    });
                }
                // Save the launch checkpoint together with the store cursor,
                // before issuing any of the newly due dynamic requests.
                advance_lane(repo, cursor)?;
                completed += 1;
                completed += resume_launch(repo, deadline, cursor, stages_left, run_stage)?;
                empty_lanes = 0;
            }
            Err(error) => {
                // Revisit the failed app in this lane after the persisted queue
                // cooldown, while giving other dimensions their next turn.
                if error.category != "budget" {
                    advance_lane(repo, cursor)?;
                }
                return Err(error);
            }
        }
    }
    Ok(completed)
}

fn resume_launch(
    repo: &Repository,
    deadline: Instant,
    cursor: &mut EnrichmentCursor,
    stages_left: &mut usize,
    run_stage: &mut impl FnMut(u32, &str, bool) -> Result<(), WorkError>,
) -> Result<usize, WorkError> {
    let mut completed = 0;
    while let Some(pending) = cursor.pending_launch {
        if Instant::now() >= deadline {
            return Err(WorkError::budget());
        }
        if explicitly_pre_release(repo, pending.app_id)? {
            cursor.pending_launch = None;
            save(repo, ENRICHMENT_CURSOR, "steam_store_appdetails", cursor)?;
            break;
        }
        let stage = LAUNCH_STAGES[pending.next_stage];
        let enabled = match stage {
            "review_summary" | "popular_reviews" => {
                !flag("MPGS_ENRICH_STORE_ONLY") && !flag("MPGS_ENRICH_SKIP_REVIEWS")
            }
            "ccu" => !flag("MPGS_ENRICH_STORE_ONLY") && !flag("MPGS_ENRICH_SKIP_CCU"),
            _ => unreachable!("launch stages are fixed"),
        };
        if enabled {
            if *stages_left == 0 {
                break;
            }
            *stages_left -= 1;
            run_stage(pending.app_id, stage, false)?;
            completed += 1;
        }
        cursor.pending_launch =
            (pending.next_stage + 1 < LAUNCH_STAGES.len()).then_some(LaunchCursor {
                next_stage: pending.next_stage + 1,
                ..pending
            });
        save(repo, ENRICHMENT_CURSOR, "steam_store_appdetails", cursor)?;
    }
    Ok(completed)
}

fn explicitly_pre_release(repo: &Repository, app_id: u32) -> Result<bool, WorkError> {
    Ok(repo
        .get_app(app_id)
        .map_err(WorkError::storage)?
        .is_some_and(|app| matches!(app.release_state.as_str(), "upcoming" | "coming_soon")))
}

fn is_released(repo: &Repository, app_id: u32) -> Result<bool, WorkError> {
    Ok(repo
        .get_app(app_id)
        .map_err(WorkError::storage)?
        .is_some_and(|app| app.release_state == "released"))
}

fn advance_lane(repo: &Repository, cursor: &mut EnrichmentCursor) -> Result<(), WorkError> {
    cursor.lane = (cursor.lane + 1) % LANES;
    save(repo, ENRICHMENT_CURSOR, "steam_store_appdetails", cursor)
}

fn lane_filter(lane: usize, media_enabled: bool) -> (&'static str, EnrichmentNeedFilter) {
    let store = !flag("MPGS_ENRICH_SKIP_STORE");
    let reviews = !flag("MPGS_ENRICH_STORE_ONLY") && !flag("MPGS_ENRICH_SKIP_REVIEWS");
    let ccu = !flag("MPGS_ENRICH_STORE_ONLY") && !flag("MPGS_ENRICH_SKIP_CCU");
    match lane {
        0 => (
            "store_details",
            EnrichmentNeedFilter {
                store,
                price: store,
                ..NO_NEEDS
            },
        ),
        1 => (
            "review_summary",
            EnrichmentNeedFilter {
                reviews,
                ..NO_NEEDS
            },
        ),
        2 => (
            "popular_reviews",
            EnrichmentNeedFilter {
                review_excerpts: reviews,
                ..NO_NEEDS
            },
        ),
        3 => ("ccu", EnrichmentNeedFilter { ccu, ..NO_NEEDS }),
        4 => (
            "english_name",
            EnrichmentNeedFilter {
                english_name: store,
                ..NO_NEEDS
            },
        ),
        _ => (
            "store_details",
            EnrichmentNeedFilter {
                media_backfill: store && media_enabled,
                ..NO_NEEDS
            },
        ),
    }
}

fn flag(name: &str) -> bool {
    env::var(name).ok().is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn media_policy() -> MediaBackfillPolicy {
    let mut policy = MediaBackfillPolicy::DEFAULT;
    if env::var("MPGS_MEDIA_BACKFILL_ENABLED")
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        })
    {
        return MediaBackfillPolicy::DISABLED;
    }
    if let Some(value) = env::var("MPGS_MEDIA_BACKFILL_COVERAGE_THRESHOLD")
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| (0.05..=1.0).contains(value))
    {
        policy.coverage_threshold = value;
    }
    if let Some(value) = env::var("MPGS_MEDIA_BACKFILL_MAX_ATTEMPTS")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| (1..=10).contains(value))
    {
        policy.max_attempts = value;
    }
    if let Some(value) = env::var("MPGS_MEDIA_BACKFILL_COOLDOWN_MS")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| (60_000..=604_800_000).contains(value))
    {
        policy.cooldown_ms = value;
    }
    policy
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const APP_ID: u32 = 4_001_890;

    fn options() -> BatchOptions {
        BatchOptions {
            owner: "release-transition-test".into(),
            budget: Duration::from_secs(45),
            job_limit: 4,
            stage_limit: 6,
            page_limit: 1,
            catalog_page_size: 100,
            candidate_page_size: 25,
            steam_api_key: None,
            country: "cn".into(),
            language: "schinese".into(),
        }
    }

    fn pre_release_fixture(integrated_queue: bool) -> Repository {
        let db = mpgs_storage::Database::open_in_memory().unwrap();
        let repo = Repository::new(db);
        repo.migrate().unwrap();
        let page = mpgs_steam_source::StoreSearchPage {
            candidates: vec![mpgs_steam_source::StoreSearchCandidate {
                app_id: APP_ID,
                name: "Release transition fixture".into(),
            }],
            start: 0,
            result_count: 1,
            total_count: 1,
            content_hash: "cloud-release-transition".into(),
            sort: mpgs_steam_source::StoreSearchSort::ReleasedAsc,
        };
        repo.ingest_store_search_page_and_queue_new_games(&page)
            .unwrap();
        repo.database()
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE apps SET release_state = 'coming_soon' WHERE app_id = $1",
                    [APP_ID],
                )?;
                if !integrated_queue {
                    // Both repository search-ingest entrypoints queue new apps.
                    // A steady-state launch is a game whose initial pre-release
                    // ingestion already completed, so only dynamic lanes run.
                    conn.execute(
                        "UPDATE game_ingestion_queue SET stage = 'complete', status = 'complete' WHERE app_id = $1",
                        [APP_ID],
                    )?;
                }
                Ok(())
            })
            .unwrap();
        if !integrated_queue {
            assert!(repo.pending_game_ingestion_app_ids().unwrap().is_empty());
        }
        repo
    }

    fn store_reports_release(repo: &Repository, app_id: u32, stage: &str) -> Result<(), WorkError> {
        if stage == "store_details" {
            repo.database()
                .with_conn(|conn| {
                    conn.execute(
                        "UPDATE apps SET release_state = 'released' WHERE app_id = $1",
                        [app_id],
                    )?;
                    Ok(())
                })
                .map_err(WorkError::storage)?;
        }
        Ok(())
    }

    #[test]
    fn existing_dimension_checkpoints_load_without_a_launch_field() {
        let restored: EnrichmentCursor = serde_json::from_str(
            r#"{"lane":1,"after_app_ids":[1000,0,0,0,0,0],"dynamic_first":true}"#,
        )
        .unwrap();
        assert_eq!(restored.lane, 1);
        assert_eq!(restored.after_app_ids, [1_000, 0, 0, 0, 0, 0]);
        assert!(restored.pending_launch.is_none());
    }

    #[test]
    fn dynamic_store_launch_runs_reviews_and_ccu_in_the_same_bounded_pass() {
        let repo = pre_release_fixture(false);
        let mut stages_left = 4;
        let mut calls = Vec::new();
        run_with(
            &repo,
            &options(),
            Instant::now() + Duration::from_secs(30),
            &mut stages_left,
            &mut BatchStats::default(),
            &mut |app_id, stage, _| {
                calls.push(stage.to_owned());
                store_reports_release(&repo, app_id, stage)
            },
        )
        .unwrap();
        assert_eq!(
            calls,
            ["store_details", "review_summary", "popular_reviews", "ccu"]
        );
        assert_eq!(stages_left, 0);
        assert!(
            load::<EnrichmentCursor>(&repo, ENRICHMENT_CURSOR)
                .unwrap()
                .unwrap()
                .pending_launch
                .is_none()
        );
    }

    #[test]
    fn launch_checkpoint_resumes_after_budget_exhaustion_and_an_expired_invocation() {
        let repo = pre_release_fixture(false);
        let mut calls = Vec::new();
        let mut first_budget = 2;
        run_with(
            &repo,
            &options(),
            Instant::now() + Duration::from_secs(30),
            &mut first_budget,
            &mut BatchStats::default(),
            &mut |app_id, stage, _| {
                calls.push(stage.to_owned());
                store_reports_release(&repo, app_id, stage)
            },
        )
        .unwrap();
        assert_eq!(calls, ["store_details", "review_summary"]);
        let pending = load::<EnrichmentCursor>(&repo, ENRICHMENT_CURSOR)
            .unwrap()
            .unwrap()
            .pending_launch
            .unwrap();
        assert_eq!(pending.app_id, APP_ID);
        assert_eq!(pending.next_stage, 1);

        let restarted = Repository::new(repo.database().reconnect().unwrap());
        let expired = run_with(
            &restarted,
            &options(),
            Instant::now(),
            &mut 2,
            &mut BatchStats::default(),
            &mut |_, _, _| panic!("expired invocation must not fetch"),
        );
        assert!(matches!(
            expired,
            Err(WorkError {
                category: "budget",
                ..
            })
        ));
        run_with(
            &restarted,
            &options(),
            Instant::now() + Duration::from_secs(30),
            &mut 2,
            &mut BatchStats::default(),
            &mut |_, stage, _| {
                calls.push(stage.to_owned());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            calls,
            ["store_details", "review_summary", "popular_reviews", "ccu"]
        );
        assert!(
            load::<EnrichmentCursor>(&restarted, ENRICHMENT_CURSOR)
                .unwrap()
                .unwrap()
                .pending_launch
                .is_none()
        );
    }

    #[test]
    fn launch_failure_retries_only_the_unfinished_dynamic_stages() {
        let repo = pre_release_fixture(false);
        let mut calls = Vec::new();
        let first = run_with(
            &repo,
            &options(),
            Instant::now() + Duration::from_secs(30),
            &mut 4,
            &mut BatchStats::default(),
            &mut |app_id, stage, _| {
                calls.push(stage.to_owned());
                store_reports_release(&repo, app_id, stage)?;
                if stage == "review_summary" {
                    return Err(WorkError {
                        category: "network",
                        message: "fixture timeout".into(),
                        retry_after_ms: 60_000,
                    });
                }
                Ok(())
            },
        );
        assert!(matches!(
            first,
            Err(WorkError {
                category: "network",
                ..
            })
        ));
        run_with(
            &repo,
            &options(),
            Instant::now() + Duration::from_secs(30),
            &mut 3,
            &mut BatchStats::default(),
            &mut |_, stage, _| {
                calls.push(stage.to_owned());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            calls,
            [
                "store_details",
                "review_summary",
                "review_summary",
                "popular_reviews",
                "ccu"
            ]
        );
    }

    #[test]
    fn integrated_pre_release_skips_dynamic_requests_even_with_a_full_profile() {
        let repo = pre_release_fixture(true);
        repo.database().with_conn(|conn| {
            conn.execute("UPDATE game_ingestion_queue SET enrichment_profile = 'full_released' WHERE app_id = $1", [APP_ID])?;
            Ok(())
        }).unwrap();
        let mut calls = Vec::new();
        let mut stats = BatchStats::default();
        integrated(
            &repo,
            &options(),
            &mut |_, stage, _| {
                calls.push(stage.to_owned());
                Ok(())
            },
            Instant::now() + Duration::from_secs(30),
            4,
            &mut 4,
            &mut stats,
        )
        .unwrap();
        assert_eq!(calls, ["store_details"]);
        assert_eq!(stats.ingestion_completed, 1);
        assert!(repo.pending_game_ingestion_app_ids().unwrap().is_empty());
    }

    #[test]
    fn integrated_launch_can_use_the_remaining_shared_budget() {
        let repo = pre_release_fixture(true);
        repo.database().with_conn(|conn| {
            conn.execute("INSERT INTO popular_review_refresh_state (app_id, captured_at_ms, result_count, source) VALUES ($1, 1, 0, 'pre-release-fixture')", [APP_ID])?;
            Ok(())
        }).unwrap();
        let mut calls = Vec::new();
        let mut stages_left = 4;
        let mut stats = BatchStats::default();
        integrated(
            &repo,
            &options(),
            &mut |app_id, stage, _| {
                calls.push(stage.to_owned());
                store_reports_release(&repo, app_id, stage)
            },
            Instant::now() + Duration::from_secs(30),
            1,
            &mut stages_left,
            &mut stats,
        )
        .unwrap();
        assert_eq!(
            calls,
            ["store_details", "review_summary", "popular_reviews", "ccu"]
        );
        assert_eq!(stages_left, 0);
        assert_eq!(stats.ingestion_completed, 1);
    }
}
