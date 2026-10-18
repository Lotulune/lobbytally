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
}

pub(crate) fn run(
    repo: &Repository,
    options: &BatchOptions,
    sources: &mut Sources,
    deadline: Instant,
    stages_left: &mut usize,
    stats: &mut BatchStats,
) -> Result<Outcome, WorkError> {
    if *stages_left == 0 || Instant::now() >= deadline {
        return Err(WorkError::budget());
    }
    let mut cursor = load::<EnrichmentCursor>(repo, ENRICHMENT_CURSOR)?.unwrap_or_default();
    if cursor.lane >= LANES {
        return Err(WorkError::invalid("enrichment lane is out of range"));
    }
    let allowance = *stages_left;
    let preferred = allowance.div_ceil(2);
    let other = allowance / 2;
    let dynamic_first = cursor.dynamic_first;
    cursor.dynamic_first = !dynamic_first;
    save(repo, ENRICHMENT_CURSOR, "steam_store_appdetails", &cursor)?;

    let mut completed = 0;
    if dynamic_first {
        completed += dynamic(
            repo,
            options,
            sources,
            deadline,
            preferred,
            &mut cursor,
            stages_left,
        )?;
        completed += integrated(repo, options, sources, deadline, other, stages_left, stats)?;
    } else {
        completed += integrated(
            repo,
            options,
            sources,
            deadline,
            preferred,
            stages_left,
            stats,
        )?;
        completed += dynamic(
            repo,
            options,
            sources,
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
    sources: &mut Sources,
    deadline: Instant,
    limit: usize,
    stages_left: &mut usize,
    stats: &mut BatchStats,
) -> Result<usize, WorkError> {
    let mut completed = 0;
    let mut active: Option<mpgs_storage::GameIngestionTask> = None;
    for _ in 0..limit {
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
        *stages_left = stages_left.saturating_sub(1);
        let basic_skip = matches!(
            task.enrichment_profile.as_str(),
            "basic_upcoming" | "basic_demo"
        ) && task.stage != "store_details";
        let observed = basic_skip
            || repo
                .game_ingestion_stage_observed(
                    task.app_id,
                    &task.stage,
                    &options.country,
                    &options.language,
                )
                .map_err(WorkError::storage)?;
        let result = if observed {
            Ok(())
        } else {
            sources.enrichment_stage(
                repo,
                task.app_id,
                &task.stage,
                &options.country,
                &options.language,
                false,
            )
        };
        match result {
            Ok(()) => {
                let next = repo
                    .advance_game_ingestion_stage(task.app_id, &options.owner)
                    .map_err(WorkError::storage)?;
                completed += 1;
                if next.stage == "complete" {
                    stats.ingestion_completed += 1;
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
    sources: &mut Sources,
    deadline: Instant,
    limit: usize,
    cursor: &mut EnrichmentCursor,
    stages_left: &mut usize,
) -> Result<usize, WorkError> {
    if limit == 0 {
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
    while completed < limit && empty_lanes < LANES {
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
        *stages_left = stages_left.saturating_sub(1);
        match sources.enrichment_stage(
            repo,
            target.app_id,
            stage,
            &options.country,
            &options.language,
            target.needs_media_backfill,
        ) {
            Ok(()) => {
                // The app cursor advances only after this individual dimension
                // is committed. Other dimensions retain independent cursors.
                cursor.after_app_ids[lane] = target.app_id;
                advance_lane(repo, cursor)?;
                completed += 1;
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

    #[test]
    fn dimensions_keep_independent_checkpoints_across_invocations() {
        let mut cursor = EnrichmentCursor::default();
        cursor.after_app_ids[0] = 1_000;
        cursor.lane = 1;
        let stored = serde_json::to_string(&cursor).unwrap();
        let restored: EnrichmentCursor = serde_json::from_str(&stored).unwrap();
        assert_eq!(restored.lane, 1);
        assert_eq!(restored.after_app_ids, [1_000, 0, 0, 0, 0, 0]);
    }
}
