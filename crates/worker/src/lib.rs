//! A single resumable maintenance invocation for Vercel and Supabase Cron.
//!
//! All progress and leases live in PostgreSQL. No timer, process, or local file
//! needs to survive an invocation. Call this synchronous API in spawn_blocking.

#![forbid(unsafe_code)]

mod discovery;
mod enrichment;
mod sources;

use std::env;
use std::time::{Duration, Instant};

use mpgs_storage::{DataRefreshStatus, EnqueueJob, JobRecord, Repository};
use serde::Serialize;

use sources::Sources;

const LEASE_MS: i64 = 90_000;
const CONTINUE_DELAY_MS: i64 = 60_000;
const CHECKPOINT_RESERVE: Duration = Duration::from_secs(8);
const CATALOG_CURSOR: &str = "steam_istore_getapplist:games:v1";
const CANDIDATE_CURSOR: &str = "steam_store_search:multiplayer:combined:v1";
const ENRICHMENT_CURSOR: &str = "steam_serverless:enrichment:v1";
const STEAM_COOLDOWN_CURSOR: &str = "steam_serverless:cooldown:v1";

#[derive(Clone)]
pub struct BatchOptions {
    pub owner: String,
    pub budget: Duration,
    pub job_limit: usize,
    pub stage_limit: usize,
    pub page_limit: u32,
    pub catalog_page_size: u32,
    pub candidate_page_size: u32,
    pub steam_api_key: Option<String>,
    pub country: String,
    pub language: String,
}

impl BatchOptions {
    /// The owner must uniquely identify this invocation. Invalid limits are
    /// configuration errors, not silently enlarged budgets.
    pub fn from_env(owner: String) -> Result<Self, String> {
        let steam_api_key = env::var("MPGS_STEAM_WEB_API_KEY")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        let country = env::var("MPGS_STEAM_COUNTRY")
            .unwrap_or_else(|_| mpgs_steam_source::DEFAULT_STORE_COUNTRY.into());
        let language = env::var("MPGS_STEAM_LANGUAGE")
            .unwrap_or_else(|_| mpgs_steam_source::DEFAULT_STORE_LANGUAGE.into());
        let options = Self {
            owner,
            budget: Duration::from_secs(configured_number("MPGS_WORKER_BUDGET_SECS", 45, 10, 50)?),
            job_limit: configured_number("MPGS_WORKER_JOB_LIMIT", 4, 1, 8)? as usize,
            stage_limit: configured_number("MPGS_WORKER_STAGE_LIMIT", 6, 1, 16)? as usize,
            page_limit: configured_number("MPGS_WORKER_PAGE_LIMIT", 1, 1, 2)? as u32,
            catalog_page_size: configured_number("MPGS_WORKER_CATALOG_PAGE_SIZE", 100, 1, 100)?
                as u32,
            candidate_page_size: configured_number("MPGS_WORKER_CANDIDATE_PAGE_SIZE", 25, 1, 100)?
                as u32,
            steam_api_key,
            country,
            language,
        };
        options.validate()?;
        Ok(options)
    }

    fn validate(&self) -> Result<(), String> {
        if self.owner.trim().is_empty() || self.owner.len() > 96 {
            return Err("worker owner must contain between 1 and 96 bytes".into());
        }
        if !(Duration::from_secs(10)..=Duration::from_secs(50)).contains(&self.budget)
            || !(1..=8).contains(&self.job_limit)
            || !(1..=16).contains(&self.stage_limit)
            || !(1..=2).contains(&self.page_limit)
            || !(1..=100).contains(&self.catalog_page_size)
            || !(1..=100).contains(&self.candidate_page_size)
        {
            return Err("worker limits exceed the bounded invocation contract".into());
        }
        if self
            .steam_api_key
            .as_ref()
            .is_some_and(|key| key.len() != 32 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            return Err("MPGS_STEAM_WEB_API_KEY must be a 32-character hexadecimal key".into());
        }
        mpgs_steam_source::StoreDetailsRequest::with_locale(1, &self.country, &self.language)
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}

#[derive(Debug, Default, Serialize)]
pub struct BatchStats {
    pub skipped: bool,
    pub leased: usize,
    pub completed: usize,
    pub yielded: usize,
    pub retried: usize,
    pub dead: usize,
    pub requests: usize,
    pub successes: usize,
    pub ingestion_completed: usize,
    pub elapsed_ms: u128,
}

#[derive(Debug)]
pub(crate) struct WorkError {
    pub(crate) category: &'static str,
    pub(crate) message: String,
    pub(crate) retry_after_ms: i64,
}

impl WorkError {
    pub(crate) fn storage(error: impl std::fmt::Display) -> Self {
        Self {
            category: "storage",
            message: error.to_string(),
            retry_after_ms: CONTINUE_DELAY_MS,
        }
    }

    pub(crate) fn budget() -> Self {
        Self {
            category: "budget",
            message: "invocation budget reached; progress is checkpointed".into(),
            retry_after_ms: CONTINUE_DELAY_MS,
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self {
            category: "invalid_payload",
            message: message.into(),
            retry_after_ms: 0,
        }
    }
}

pub(crate) enum Outcome {
    Complete,
    Continue,
}

/// Run one finite batch. Duplicate deliveries return `skipped` while the live
/// owner's lease remains valid; a terminated invocation becomes recoverable
/// after 90 seconds without resetting any ingestion stage or discovery cursor.
pub fn run_batch(repo: &Repository, options: &BatchOptions) -> Result<BatchStats, String> {
    options.validate()?;
    let locale =
        mpgs_steam_source::StoreDetailsRequest::with_locale(1, &options.country, &options.language)
            .map_err(display)?;
    let normalized = BatchOptions {
        country: locale.country_code,
        language: locale.language,
        ..options.clone()
    };
    let options = &normalized;
    let started = Instant::now();
    if !repo
        .try_acquire_worker_lease(&options.owner, LEASE_MS)
        .map_err(display)?
    {
        return Ok(BatchStats {
            skipped: true,
            elapsed_ms: started.elapsed().as_millis(),
            ..BatchStats::default()
        });
    }
    let result = run_owned_batch(repo, options, started);
    let release = repo.release_worker_lease(&options.owner).map_err(display);
    match (result, release) {
        (Ok(mut stats), Ok(())) => {
            stats.elapsed_ms = started.elapsed().as_millis();
            Ok(stats)
        }
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
}

fn run_owned_batch(
    repo: &Repository,
    options: &BatchOptions,
    started: Instant,
) -> Result<BatchStats, String> {
    let deadline = started + options.budget.saturating_sub(CHECKPOINT_RESERVE);
    schedule_due_jobs(repo, options, deadline).map_err(display)?;
    if deadline.saturating_duration_since(Instant::now()) < Duration::from_millis(750) {
        return Ok(BatchStats::default());
    }
    let mut sources = Sources::new(deadline).map_err(|error| error.message)?;
    let mut stats = BatchStats::default();
    let mut stages_left = options.stage_limit;
    // Alternate queues each invocation so a busy ingestion backlog cannot
    // indefinitely postpone retrieval maintenance or vice versa.
    let maintenance_first = (repo.database().now_ms() / 60_000) % 2 == 0;
    for index in 0..options.job_limit {
        if Instant::now() >= deadline {
            break;
        }
        let source_order = if (index % 2 == 0) == maintenance_first {
            ["maintenance", "steam"]
        } else {
            ["steam", "maintenance"]
        };
        let mut leased = None;
        for source in source_order {
            if source == "steam" && steam_cooldown_active(repo).map_err(|error| error.message)? {
                continue;
            }
            leased = repo
                .lease_jobs(&options.owner, 1, LEASE_MS, Some(source))
                .map_err(display)?
                .into_iter()
                .next();
            if leased.is_some() {
                break;
            }
        }
        let Some(job) = leased else { break };
        stats.leased += 1;
        let successes_before = sources.successes;
        let result = run_job(
            repo,
            options,
            &job,
            &mut sources,
            deadline,
            &mut stages_left,
            &mut stats,
        );
        match result {
            Ok(Outcome::Complete) => {
                repo.complete_job(
                    job.job_id,
                    &options.owner,
                    &format!("cloud-batch:{}", job.job_id),
                )
                .map_err(display)?;
                record_result(repo, &job, None).map_err(display)?;
                stats.completed += 1;
            }
            Ok(Outcome::Continue)
            | Err(WorkError {
                category: "budget", ..
            }) => {
                repo.yield_job(job.job_id, &options.owner, CONTINUE_DELAY_MS)
                    .map_err(display)?;
                if sources.successes > successes_before {
                    record_result(repo, &job, None).map_err(display)?;
                }
                stats.yielded += 1;
            }
            Err(error) => {
                let category = queue_error_category(error.category);
                let delay_ms = retry_delay(category, job.attempts)
                    .max(error.retry_after_ms)
                    .min(604_800_000);
                if category == "rate_limited" {
                    discovery::save(
                        repo,
                        STEAM_COOLDOWN_CURSOR,
                        "steam",
                        &serde_json::json!({
                            "until_ms": repo.database().now_ms().saturating_add(delay_ms),
                        }),
                    )
                    .map_err(|error| error.message)?;
                }
                let failed = repo
                    .fail_job(job.job_id, &options.owner, category, delay_ms)
                    .map_err(display)?;
                record_result(repo, &job, Some(category)).map_err(display)?;
                if failed.status == "dead" {
                    stats.dead += 1;
                } else {
                    stats.retried += 1;
                }
                eprintln!(
                    "maintenance job={} task={} category={} message={}",
                    job.job_id, job.task_type, category, error.message
                );
                if category == "rate_limited" {
                    break;
                }
            }
        }
    }
    stats.requests = sources.requests;
    stats.successes = sources.successes;
    Ok(stats)
}

fn run_job(
    repo: &Repository,
    options: &BatchOptions,
    job: &JobRecord,
    sources: &mut Sources,
    deadline: Instant,
    stages_left: &mut usize,
    stats: &mut BatchStats,
) -> Result<Outcome, WorkError> {
    if Instant::now() >= deadline {
        return Err(WorkError::budget());
    }
    let source_metadata = match job.task_type.as_str() {
        "sync_catalog" => Some((
            mpgs_steam_source::APP_LIST_SOURCE_NAME,
            "catalog_sync",
            mpgs_steam_source::APP_LIST_ADAPTER_VERSION,
        )),
        "collect_candidate_top" => Some((
            mpgs_steam_source::STORE_SEARCH_SOURCE_NAME,
            "candidate_top_refresh",
            mpgs_steam_source::STORE_SEARCH_ADAPTER_VERSION,
        )),
        "collect_candidates" | "collect_candidate_continuation" => Some((
            mpgs_steam_source::STORE_SEARCH_SOURCE_NAME,
            "candidate_discovery",
            mpgs_steam_source::STORE_SEARCH_ADAPTER_VERSION,
        )),
        "enrich_catalog" => Some((
            mpgs_steam_source::STORE_SOURCE_NAME,
            "candidate_enrichment",
            mpgs_steam_source::STORE_ADAPTER_VERSION,
        )),
        _ => None,
    };
    let run_id = source_metadata
        .map(|(source, task, adapter)| {
            repo.start_source_run(
                source,
                task,
                adapter,
                Some(&format!(
                    "serverless=true;job_id={};owner={}",
                    job.job_id, options.owner
                )),
            )
        })
        .transpose()
        .map_err(WorkError::storage)?;
    let requests_before = sources.requests;
    let successes_before = sources.successes;
    let result = match (job.source.as_str(), job.task_type.as_str()) {
        ("steam", "sync_catalog") => discovery::catalog(repo, options, sources),
        ("steam", "collect_candidate_top") => {
            discovery::candidates(repo, options, sources, job.job_id, true)
        }
        ("steam", "collect_candidates" | "collect_candidate_continuation") => {
            discovery::candidates(repo, options, sources, job.job_id, false)
        }
        ("steam", "enrich_catalog") => {
            enrichment::run(repo, options, sources, deadline, stages_left, stats)
        }
        ("maintenance", "retrieval_sync") => sync_retrieval(repo),
        ("maintenance", "quality_check") => repo
            .run_quality_checks()
            .map(|_| Outcome::Complete)
            .map_err(WorkError::storage),
        ("maintenance", "pipeline_snapshot") => repo
            .refresh_pipeline_status_snapshot_full()
            .map(|_| Outcome::Complete)
            .map_err(WorkError::storage),
        ("maintenance", "recommendation_telemetry_retention") => repo
            .purge_expired_recommendation_telemetry()
            .map(|_| Outcome::Complete)
            .map_err(WorkError::storage),
        _ => Err(WorkError::invalid("unsupported maintenance job type")),
    };
    if let Some(run_id) = run_id {
        let (status, category, notes) = match &result {
            Ok(Outcome::Complete) => ("succeeded", None, "bounded batch completed"),
            Ok(Outcome::Continue) => (
                "partial",
                None,
                "progress checkpointed; continuation is due next minute",
            ),
            Err(error) if error.category == "budget" => (
                "partial",
                None,
                "time budget reached; progress checkpointed",
            ),
            Err(error) => (
                if sources.successes > successes_before {
                    "partial"
                } else {
                    "failed"
                },
                Some(error.category),
                error.message.as_str(),
            ),
        };
        repo.finish_source_run(
            run_id,
            status,
            (sources.requests - requests_before) as i64,
            (sources.successes - successes_before) as i64,
            category,
            Some(notes),
        )
        .map_err(WorkError::storage)?;
    }
    result
}

fn sync_retrieval(repo: &Repository) -> Result<Outcome, WorkError> {
    let previous = repo.data_refresh_status().map_err(WorkError::storage)?;
    let cursor = previous
        .iter()
        .find(|status| status.task_name == "retrieval_sync")
        .and_then(|status| status.cursor_value.as_deref())
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    let sync = repo
        .sync_retrieval_from_catalog(10, cursor, true)
        .map_err(WorkError::storage)?;
    let now = repo.database().now_ms();
    repo.update_data_refresh_status(
        "retrieval_sync",
        Some(now),
        Some(now + 60_000),
        None,
        Some(&sync.next_after_app_id.to_string()),
        Some(sync.coverage_ratio()),
    )
    .map_err(WorkError::storage)?;
    Ok(Outcome::Complete)
}

fn schedule_due_jobs(
    repo: &Repository,
    options: &BatchOptions,
    deadline: Instant,
) -> mpgs_storage::StorageResult<()> {
    let now = repo.database().now_ms();
    let statuses = repo.data_refresh_status()?;
    for (source, task_type, task_name, interval, enabled) in [
        (
            "steam",
            "sync_catalog",
            "catalog_sync",
            task_interval_ms("MPGS_CATALOG_SYNC_INTERVAL_SECS", 15 * 60),
            options.steam_api_key.is_some(),
        ),
        (
            "steam",
            "collect_candidate_top",
            "candidate_top_refresh",
            task_interval_ms("MPGS_CANDIDATE_TOP_REFRESH_INTERVAL_SECS", 15 * 60),
            true,
        ),
        (
            "steam",
            "collect_candidate_continuation",
            "candidate_continuation",
            task_interval_ms("MPGS_CANDIDATE_CONTINUATION_INTERVAL_SECS", 6 * 60 * 60),
            true,
        ),
        (
            "steam",
            "enrich_catalog",
            "enrichment",
            task_interval_ms("MPGS_ENRICHMENT_INTERVAL_SECS", 5 * 60),
            true,
        ),
        (
            "maintenance",
            "retrieval_sync",
            "retrieval_sync",
            60_000,
            true,
        ),
        (
            "maintenance",
            "quality_check",
            "quality_check",
            30 * 60_000,
            true,
        ),
        (
            "maintenance",
            "pipeline_snapshot",
            "pipeline_snapshot",
            15 * 60_000,
            true,
        ),
        (
            "maintenance",
            "recommendation_telemetry_retention",
            "recommendation_telemetry_retention",
            24 * 60 * 60_000,
            true,
        ),
    ] {
        if Instant::now() >= deadline {
            break;
        }
        let previous = statuses.iter().find(|status| status.task_name == task_name);
        if !enabled
            || previous
                .and_then(|status| status.next_run_at_ms)
                .is_some_and(|next| next > now)
        {
            continue;
        }
        repo.enqueue_job_if_no_active(&EnqueueJob {
            source: source.into(),
            task_type: task_type.into(),
            entity_key: "scheduled".into(),
            priority: 50,
            due_at_ms: now,
            idempotency_key: format!("cloud-schedule:{task_name}:{}", now / interval),
            payload_json: None,
            max_attempts: 5,
        })?;
        repo.update_data_refresh_status(
            task_name,
            previous.and_then(|status| status.last_success_at_ms),
            Some(now.saturating_add(interval)),
            None,
            previous.and_then(|status| status.cursor_value.as_deref()),
            previous.and_then(|status| status.coverage_ratio),
        )?;
    }
    Ok(())
}

fn steam_cooldown_active(repo: &Repository) -> Result<bool, WorkError> {
    let cursor = discovery::load::<serde_json::Value>(repo, STEAM_COOLDOWN_CURSOR)?;
    Ok(cursor
        .and_then(|value| value.get("until_ms").and_then(serde_json::Value::as_i64))
        .is_some_and(|until| until > repo.database().now_ms()))
}

fn record_result(
    repo: &Repository,
    job: &JobRecord,
    error: Option<&str>,
) -> mpgs_storage::StorageResult<()> {
    let (task_name, cursor_key) = match job.task_type.as_str() {
        "sync_catalog" => ("catalog_sync", Some(CATALOG_CURSOR)),
        "collect_candidate_top" => ("candidate_top_refresh", Some(CANDIDATE_CURSOR)),
        "collect_candidates" | "collect_candidate_continuation" => {
            ("candidate_continuation", Some(CANDIDATE_CURSOR))
        }
        "enrich_catalog" => ("enrichment", Some(ENRICHMENT_CURSOR)),
        value => (value, None),
    };
    let statuses = repo.data_refresh_status()?;
    let previous: Option<&DataRefreshStatus> =
        statuses.iter().find(|status| status.task_name == task_name);
    let cursor = cursor_key
        .map(|key| repo.source_cursor(key))
        .transpose()?
        .flatten();
    repo.update_data_refresh_status(
        task_name,
        if error.is_none() {
            Some(repo.database().now_ms())
        } else {
            previous.and_then(|status| status.last_success_at_ms)
        },
        previous.and_then(|status| status.next_run_at_ms),
        error,
        cursor
            .as_deref()
            .or_else(|| previous.and_then(|status| status.cursor_value.as_deref())),
        previous.and_then(|status| status.coverage_ratio),
    )
}

fn queue_error_category(category: &str) -> &str {
    match category {
        "network" | "storage" | "rate_limited" | "auth" | "not_found" | "parse_changed"
        | "invalid_payload" => category,
        "config" => "auth",
        _ => "parse_changed",
    }
}

fn retry_delay(category: &str, attempts: i64) -> i64 {
    let delay = 60_000_i64.saturating_mul(1_i64 << (attempts.clamp(1, 6) - 1));
    if category == "rate_limited" {
        delay.max(300_000)
    } else {
        delay
    }
}

fn configured_number(name: &str, default: u64, min: u64, max: u64) -> Result<u64, String> {
    match env::var(name) {
        Ok(raw) => raw
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|value| (min..=max).contains(value))
            .ok_or_else(|| format!("{name} must be between {min} and {max}")),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(_) => Err(format!("{name} must contain valid Unicode")),
    }
}

fn task_interval_ms(name: &str, default_seconds: i64) -> i64 {
    env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|value| (60..=86_400).contains(value))
        .unwrap_or(default_seconds)
        .saturating_mul(1_000)
}

fn display(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> BatchOptions {
        BatchOptions {
            owner: "worker-test".into(),
            budget: Duration::from_secs(45),
            job_limit: 4,
            stage_limit: 6,
            page_limit: 1,
            catalog_page_size: 100,
            candidate_page_size: 25,
            steam_api_key: None,
            country: "CN".into(),
            language: "schinese".into(),
        }
    }

    #[test]
    fn invocation_limits_cannot_be_bypassed_by_direct_callers() {
        let mut config = options();
        assert!(config.validate().is_ok());
        config.budget = Duration::from_secs(51);
        assert!(config.validate().is_err());
        config = options();
        config.stage_limit = 17;
        assert!(config.validate().is_err());
        config = options();
        config.steam_api_key = Some("secret".into());
        assert!(config.validate().is_err());
    }

    #[test]
    fn rate_limits_are_rescheduled_instead_of_sleeping() {
        assert_eq!(retry_delay("rate_limited", 1), 300_000);
        assert_eq!(retry_delay("network", 1), 60_000);
        assert_eq!(retry_delay("network", 100), 1_920_000);
        assert_eq!(queue_error_category("storage"), "storage");
    }

    #[test]
    fn overlapping_cron_delivery_does_not_start_any_jobs_or_network_requests() {
        let db = mpgs_storage::Database::open_in_memory().unwrap();
        let repo = Repository::new(db);
        repo.migrate().unwrap();
        assert!(
            repo.try_acquire_worker_lease("first-invocation", LEASE_MS)
                .unwrap()
        );
        let stats = run_batch(&repo, &options()).unwrap();
        assert!(stats.skipped);
        assert_eq!(stats.leased, 0);
        assert_eq!(stats.requests, 0);
    }

    #[test]
    fn empty_database_maintenance_completes_during_steam_cooldown_without_duplicate_jobs() {
        let clock = std::sync::Arc::new(mpgs_storage::FakeClock::new(1_000_000));
        let db = mpgs_storage::Database::open_in_memory_with_clock(clock).unwrap();
        let repo = Repository::new(db);
        repo.migrate().unwrap();
        repo.ensure_runtime_defaults().unwrap();
        discovery::save(
            &repo,
            STEAM_COOLDOWN_CURSOR,
            "steam",
            &serde_json::json!({ "until_ms": 2_000_000 }),
        )
        .unwrap();
        let first = run_batch(&repo, &options()).unwrap();
        assert_eq!(first.completed, 4);
        assert_eq!(first.requests, 0);
        assert_eq!(first.retried, 0);
        assert_eq!(first.dead, 0);
        let second = run_batch(&repo, &options()).unwrap();
        assert_eq!(second.leased, 0);
        assert_eq!(second.requests, 0);
    }
}
