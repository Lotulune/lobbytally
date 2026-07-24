#![forbid(unsafe_code)]

use std::env;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use mpgs_ai::{EmbeddingInput, embedding_provider_from_env, encode_f32_le};
use mpgs_steam_source::{
    APP_LIST_ADAPTER_VERSION, APP_LIST_MAX_RESULTS, APP_LIST_SOURCE_NAME, AppListCursor,
    AppListRequest, CcuRequest, DEFAULT_STORE_COUNTRY, DEFAULT_STORE_LANGUAGE, DEFAULT_USER_AGENT,
    GoldenSet, RawResponse, ReviewSummaryRequest, STEAM_STORE_HOST, STEAM_WEB_API_HOST,
    STORE_ADAPTER_VERSION, STORE_SEARCH_ADAPTER_VERSION, STORE_SEARCH_SOURCE_NAME,
    STORE_SOURCE_NAME, SourceError, StoreDetailsRequest, StoreSearchPage, StoreSearchRequest,
    apply_page_to_cursor, http_not_found_proposal, parse_app_list_page, parse_ccu,
    parse_popular_reviews, parse_review_summary, parse_store_details, parse_store_search_page,
};
use mpgs_storage::{
    Clock, Database, EnrichmentNeedFilter, HASH_EMBED_MODEL, PutEmbedding, Repository,
    StorageResult, SystemClock,
};
use serde::{Deserialize, Serialize};

const STORE_SEARCH_CURSOR_KEY: &str = "steam_store_search:multiplayer:reviews_desc";
const ENRICH_CURSOR_KEY: &str = "steam_store_enrichment:dynamic:v2";
const APP_LIST_CURSOR_KEY: &str = "steam_istore_getapplist:games:v1";
const APP_LIST_PAGES_DEFAULT: u32 = 1;
const APP_LIST_PAGES_MAX: u32 = 100;
const APP_LIST_PAGE_SIZE_DEFAULT: u32 = 1_000;
const APP_LIST_INTER_REQUEST_MS: u64 = 1_100;
const WORKER_JOB_LIMIT_DEFAULT: i64 = 1;
const WORKER_JOB_LIMIT_MAX: i64 = 10;
const WORKER_LEASE_MS: i64 = 30 * 60 * 1_000;
const WORKER_RETRY_BASE_MS: i64 = 60 * 1_000;
const STORE_SEARCH_TARGET_DEFAULT: u32 = 2_000;
const STORE_SEARCH_TARGET_MAX: u32 = 10_000;
const STORE_SEARCH_RESPONSE_MAX_BYTES: usize = 4 * 1024 * 1024;
const ENRICH_LIMIT_DEFAULT: u32 = 100;
const ENRICH_LIMIT_MAX: u32 = 5_000;
const ENRICH_RESPONSE_MAX_BYTES: usize = 2 * 1024 * 1024;
const ENRICH_INTER_REQUEST_MS: u64 = 2_500;
const ENRICH_RATE_LIMIT_COOLDOWN_MS: u64 = 5 * 60 * 1_000;
const STORAGE_WRITE_RETRIES: u64 = 3;

fn env_flag(name: &str) -> bool {
    env::var(name).ok().is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// Explicit disable via `0/false/off`; default is enabled when unset.
fn env_flag_default_true(name: &str) -> bool {
    match env::var(name) {
        Ok(value) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
        Err(_) => true,
    }
}

fn configured_media_backfill_policy() -> mpgs_storage::MediaBackfillPolicy {
    let defaults = mpgs_storage::MediaBackfillPolicy::DEFAULT;
    if !env_flag_default_true("MPGS_MEDIA_BACKFILL_ENABLED") {
        return mpgs_storage::MediaBackfillPolicy::DISABLED;
    }
    let coverage_threshold = env::var("MPGS_MEDIA_BACKFILL_COVERAGE_THRESHOLD")
        .ok()
        .and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|value| (0.05..=1.0).contains(value))
        .unwrap_or(defaults.coverage_threshold);
    let max_attempts = env::var("MPGS_MEDIA_BACKFILL_MAX_ATTEMPTS")
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|value| (1..=10).contains(value))
        .unwrap_or(defaults.max_attempts);
    let cooldown_ms = env::var("MPGS_MEDIA_BACKFILL_COOLDOWN_MS")
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|value| (60_000..=7 * 24 * 60 * 60 * 1_000).contains(value))
        .unwrap_or(defaults.cooldown_ms);
    mpgs_storage::MediaBackfillPolicy {
        enabled: true,
        coverage_threshold,
        max_attempts,
        cooldown_ms,
    }
}

fn enrich_inter_request_ms() -> u64 {
    env::var("MPGS_ENRICH_INTER_REQUEST_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| (1_000..=60_000).contains(value))
        .unwrap_or(ENRICH_INTER_REQUEST_MS)
}
const M7_MIN_CANDIDATES: i64 = 2_000;
const M7_MIN_TRUSTED_FRIEND_PROFILES: i64 = 300;
const M7_MIN_SECTION_CANDIDATES: i64 = 20;
const M7_MIN_DATE_COVERAGE_PERCENT: i64 = 95;
const M7_MIN_COVER_COVERAGE_PERCENT: i64 = 95;
const M7_MIN_SEVEN_DAY_FOCUS: i64 = 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoreSearchCursor {
    next_start: u32,
    total_count: Option<u32>,
    target: u32,
    complete: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
struct EnrichmentCursor {
    after_app_id: u32,
}

#[derive(Debug, Clone, Copy, Default)]
struct CollectionStats {
    request_count: i64,
    success_count: i64,
}

#[derive(Debug)]
struct CollectionError {
    category: &'static str,
    message: String,
    stats: CollectionStats,
}

#[derive(Debug, Default)]
struct SteamWorkerStats {
    leased: usize,
    completed: usize,
    retried: usize,
    dead: usize,
}

#[derive(Debug)]
struct WorkerTaskError {
    category: &'static str,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let cmd = args.next().ok_or_else(|| usage().to_owned())?;

    match cmd.as_str() {
        "migrate" => {
            let db_path = required_path(args.next(), "--db path")?;
            let db = Database::open(&db_path).map_err(err)?;
            let version = db.migrate().map_err(err)?;
            println!("migrated {} to schema version {version}", db_path.display());
            db.assert_ready().map_err(err)?;
            Ok(())
        }
        "integrity" => {
            let db_path = required_path(args.next(), "--db path")?;
            let db = Database::open(&db_path).map_err(err)?;
            let check = db.integrity_check().map_err(err)?;
            let version = db.schema_version().map_err(err)?;
            println!("path={}", db_path.display());
            println!("schema_version={version}");
            println!("integrity_check={check:?}");
            db.assert_ready().map_err(err)?;
            println!("ready=ok");
            Ok(())
        }
        "m3-audit" => {
            let db_path = required_path(args.next(), "--db path")?;
            let db = Database::open(&db_path).map_err(err)?;
            db.assert_ready().map_err(err)?;
            let repo = Repository::new(db);
            repo.readiness_check().map_err(err)?;
            audit_m3(&repo, &db_path)
        }
        "m7-data-audit" => {
            let db_path = required_path(args.next(), "--db path")?;
            let upcoming_shortfall_reason = parse_m7_audit_options(&mut args)?;
            let db = Database::open(&db_path).map_err(err)?;
            db.assert_ready().map_err(err)?;
            let repo = Repository::new(db);
            repo.readiness_check().map_err(err)?;
            audit_m7(&repo, &db_path, upcoming_shortfall_reason.as_deref())
        }
        "sync-retrieval" => {
            let db_path = required_path(args.next(), "--db path")?;
            let limit = args
                .next()
                .as_deref()
                .unwrap_or("5000")
                .parse::<u32>()
                .map_err(|_| "sync-retrieval limit must be an integer".to_owned())?
                .clamp(1, 50_000);
            let after = args
                .next()
                .as_deref()
                .unwrap_or("0")
                .parse::<u32>()
                .map_err(|_| "sync-retrieval after_app_id must be an integer".to_owned())?;
            let db = Database::open(&db_path).map_err(err)?;
            db.assert_ready().map_err(err)?;
            let repo = Repository::new(db);
            let stats = repo
                .sync_retrieval_from_catalog(limit, after, true)
                .map_err(err)?;
            println!("path={}", db_path.display());
            println!("apps_scanned={}", stats.apps_scanned);
            println!("documents_written={}", stats.documents_written);
            println!("documents_unchanged={}", stats.documents_unchanged);
            println!("embeddings_written={}", stats.embeddings_written);
            println!("embeddings_unchanged={}", stats.embeddings_unchanged);
            println!("document_count={}", repo.document_count().map_err(err)?);
            println!("retrieval_sync=ok");
            Ok(())
        }
        "extract-offline-features" => {
            let db_path = required_path(args.next(), "--db path")?;
            let limit = args
                .next()
                .as_deref()
                .unwrap_or("5000")
                .parse::<u32>()
                .map_err(|_| "extract-offline-features limit must be an integer".to_owned())?
                .clamp(1, 50_000);
            let after = args
                .next()
                .as_deref()
                .unwrap_or("0")
                .parse::<u32>()
                .map_err(|_| {
                    "extract-offline-features after_app_id must be an integer".to_owned()
                })?;
            let db = Database::open(&db_path).map_err(err)?;
            db.assert_ready().map_err(err)?;
            let repo = Repository::new(db);
            let stats = repo.extract_offline_features(limit, after).map_err(err)?;
            println!("path={}", db_path.display());
            println!("apps_scanned={}", stats.apps_scanned);
            println!("analyses_written={}", stats.analyses_written);
            println!("analyses_unchanged={}", stats.analyses_unchanged);
            println!(
                "ai_analysis_count={}",
                repo.ai_analysis_count().map_err(err)?
            );
            println!("offline_feature_extract=ok");
            Ok(())
        }
        "materialize-store-profiles" => {
            let db_path = required_path(args.next(), "--db path")?;
            if args.next().is_some() {
                return Err("materialize-store-profiles accepts only db-path".into());
            }
            let db = Database::open(&db_path).map_err(err)?;
            db.migrate().map_err(err)?;
            let repo = Repository::new(db);
            repo.ensure_runtime_defaults().map_err(err)?;
            let applied = repo.materialize_store_category_profiles().map_err(err)?;
            println!("path={}", db_path.display());
            println!("store_profiles_applied={applied}");
            println!("materialize_store_profiles=ok");
            Ok(())
        }
        "repair-empty-availability" => {
            let db_path = required_path(args.next(), "--db path")?;
            if args.next().is_some() {
                return Err("repair-empty-availability accepts only db-path".into());
            }
            let db = Database::open(&db_path).map_err(err)?;
            db.migrate().map_err(err)?;
            let repo = Repository::new(db);
            repo.ensure_runtime_defaults().map_err(err)?;
            let restored = repo
                .restore_empty_availability_from_evidence()
                .map_err(err)?;
            println!("path={}", db_path.display());
            println!("availability_fields_restored={restored}");
            println!("repair_empty_availability=ok");
            Ok(())
        }
        "embed-documents" => {
            let db_path = required_path(args.next(), "--db path")?;
            let limit = args
                .next()
                .as_deref()
                .unwrap_or("200")
                .parse::<u32>()
                .map_err(|_| "embed-documents limit must be an integer".to_owned())?
                .clamp(1, 10_000);
            let batch = args
                .next()
                .as_deref()
                .unwrap_or("16")
                .parse::<usize>()
                .map_err(|_| "embed-documents batch must be an integer".to_owned())?
                .clamp(1, 64);
            let db = Database::open(&db_path).map_err(err)?;
            db.assert_ready().map_err(err)?;
            let repo = Repository::new(db);
            // Ensure documents exist before embedding.
            if repo.document_count().map_err(err)? == 0 {
                let _ = repo
                    .sync_retrieval_from_catalog(limit.max(100), 0, false)
                    .map_err(err)?;
            }
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?;
            let stats = rt
                .block_on(embed_documents_batch(&repo, limit, batch))
                .map_err(err_ai)?;
            println!("path={}", db_path.display());
            println!("provider={}", stats.provider);
            println!("model={}", stats.model);
            println!("targets={}", stats.targets);
            println!("written={}", stats.written);
            println!("unchanged={}", stats.unchanged);
            println!("batches={}", stats.batches);
            println!("embedding_count={}", repo.embedding_count().map_err(err)?);
            println!("embed_documents=ok");
            Ok(())
        }
        "collect-steam-catalog" => {
            let db_path = required_path(args.next(), "--db path")?;
            let max_pages = optional_catalog_page_count(args.next())?;
            let page_size = optional_catalog_page_size(args.next())?;
            if args.next().is_some() {
                return Err(
                    "collect-steam-catalog accepts db-path, max-pages, and page-size only".into(),
                );
            }
            let api_key = steam_web_api_key()?;
            let db = Database::open(&db_path).map_err(err)?;
            db.migrate().map_err(err)?;
            let repo = Repository::new(db);
            repo.ensure_runtime_defaults().map_err(err)?;
            let run_id = repo
                .start_source_run(
                    APP_LIST_SOURCE_NAME,
                    "catalog_sync",
                    APP_LIST_ADAPTER_VERSION,
                    Some(&format!(
                        "max_pages={max_pages};page_size={page_size};key=present"
                    )),
                )
                .map_err(err)?;
            match collect_steam_catalog(&repo, &api_key, max_pages, page_size) {
                Ok(stats) => {
                    repo.finish_source_run(
                        run_id,
                        "succeeded",
                        stats.request_count,
                        stats.success_count,
                        None,
                        Some("cursor persisted"),
                    )
                    .map_err(err)?;
                    println!("path={}", db_path.display());
                    println!("requests={}", stats.request_count);
                    println!("apps_ingested={}", stats.success_count);
                    println!("catalog_apps={}", repo.count_apps().map_err(err)?);
                    println!("steam_catalog_sync=ok");
                    Ok(())
                }
                Err(failure) => {
                    let status = if failure.stats.success_count > 0 {
                        "partial"
                    } else {
                        "failed"
                    };
                    repo.finish_source_run(
                        run_id,
                        status,
                        failure.stats.request_count,
                        failure.stats.success_count,
                        Some(failure.category),
                        Some(&failure.message),
                    )
                    .map_err(err)?;
                    Err(format!(
                        "Steam catalog sync {} after {} requests and {} ingested apps: {}",
                        failure.category,
                        failure.stats.request_count,
                        failure.stats.success_count,
                        failure.message
                    ))
                }
            }
        }
        "collect-steam-candidates" => {
            let db_path = required_path(args.next(), "--db path")?;
            let target = optional_target(args.next())?;
            if args.next().is_some() {
                return Err("collect-steam-candidates accepts at most db-path and target".into());
            }
            let db = Database::open(&db_path).map_err(err)?;
            db.migrate().map_err(err)?;
            let repo = Repository::new(db);
            repo.ensure_runtime_defaults().map_err(err)?;
            let before = repo.m3_catalog_coverage().map_err(err)?;
            if before.normalized_multiplayer_candidates >= i64::from(target) {
                println!("path={}", db_path.display());
                println!(
                    "normalized_multiplayer_candidates={}",
                    before.normalized_multiplayer_candidates
                );
                println!("steam_candidate_collection=already_satisfied");
                return Ok(());
            }

            let run_id = repo
                .start_source_run(
                    STORE_SEARCH_SOURCE_NAME,
                    "candidate_discovery",
                    STORE_SEARCH_ADAPTER_VERSION,
                    Some("category2=1;sort=Reviews_DESC;cc=US;l=english"),
                )
                .map_err(err)?;
            match collect_steam_candidates(&repo, target) {
                Ok(stats) => {
                    repo.finish_source_run(
                        run_id,
                        "succeeded",
                        stats.request_count,
                        stats.success_count,
                        None,
                        Some("target reached"),
                    )
                    .map_err(err)?;
                    let after = repo.m3_catalog_coverage().map_err(err)?;
                    println!("path={}", db_path.display());
                    println!("requests={}", stats.request_count);
                    println!("rows_ingested={}", stats.success_count);
                    println!(
                        "normalized_multiplayer_candidates={}",
                        after.normalized_multiplayer_candidates
                    );
                    println!("steam_candidate_collection=ok");
                    Ok(())
                }
                Err(failure) => {
                    let status = if failure.stats.success_count > 0 {
                        "partial"
                    } else {
                        "failed"
                    };
                    repo.finish_source_run(
                        run_id,
                        status,
                        failure.stats.request_count,
                        failure.stats.success_count,
                        Some(failure.category),
                        Some(&failure.message),
                    )
                    .map_err(err)?;
                    Err(format!(
                        "Steam candidate collection {} after {} requests and {} ingested rows: {}",
                        failure.category,
                        failure.stats.request_count,
                        failure.stats.success_count,
                        failure.message
                    ))
                }
            }
        }
        "enrich-steam-candidates" => {
            let db_path = required_path(args.next(), "--db path")?;
            let limit = optional_enrich_limit(args.next())?;
            if args.next().is_some() {
                return Err("enrich-steam-candidates accepts at most db-path and limit".into());
            }
            let db = Database::open(&db_path).map_err(err)?;
            db.migrate().map_err(err)?;
            let repo = Repository::new(db);
            repo.ensure_runtime_defaults().map_err(err)?;
            let before = repo.m3_catalog_coverage().map_err(err)?;
            let (country_code, language) = configured_store_locale()?;
            let cursor = repo
                .source_cursor(ENRICH_CURSOR_KEY)
                .map_err(err)?
                .map(|value| serde_json::from_str::<EnrichmentCursor>(&value))
                .transpose()
                .map_err(|error| format!("invalid enrichment cursor: {error}"))?
                .unwrap_or_default();
            let store_only = env_flag("MPGS_ENRICH_STORE_ONLY");
            let skip_reviews = store_only || env_flag("MPGS_ENRICH_SKIP_REVIEWS");
            let skip_ccu = store_only || env_flag("MPGS_ENRICH_SKIP_CCU");
            let skip_store = env_flag("MPGS_ENRICH_SKIP_STORE");
            let media_policy = configured_media_backfill_policy();
            let need_filter = EnrichmentNeedFilter {
                store: !skip_store,
                reviews: !skip_reviews,
                review_excerpts: !store_only,
                ccu: !skip_ccu,
                price: !skip_store,
                media_backfill: !skip_store && media_policy.enabled,
            };
            let mut targets = repo
                .list_enrichment_targets_after_filtered_with_media(
                    limit,
                    Some(cursor.after_app_id),
                    &country_code,
                    &language,
                    need_filter,
                    media_policy,
                )
                .map_err(err)?;
            targets.retain(|target| target.matches_filter(need_filter));
            if targets.is_empty() {
                println!("path={}", db_path.display());
                print_coverage(&before);
                println!("steam_candidate_enrichment=already_satisfied");
                return Ok(());
            }

            let run_id = repo
                .start_source_run(
                    STORE_SOURCE_NAME,
                    "candidate_enrichment",
                    STORE_ADAPTER_VERSION,
                    Some(&format!(
                        "limit={limit};sources=appdetails,reviews,ccu;targets={};country={country_code};language={language};after_app_id={}",
                        targets.len(), cursor.after_app_id
                    )),
                )
                .map_err(err)?;
            match enrich_steam_candidates(&repo, &targets, &country_code, &language) {
                Ok(stats) => {
                    let next_cursor = EnrichmentCursor {
                        after_app_id: targets
                            .last()
                            .map_or(cursor.after_app_id, |target| target.app_id),
                    };
                    repo.save_source_cursor(
                        ENRICH_CURSOR_KEY,
                        STORE_SOURCE_NAME,
                        &serde_json::to_value(next_cursor).map_err(err)?,
                    )
                    .map_err(err)?;
                    let status = if stats.error_count > 0 && stats.success_count > 0 {
                        "partial"
                    } else if stats.error_count > 0 {
                        "failed"
                    } else {
                        "succeeded"
                    };
                    repo.finish_source_run(
                        run_id,
                        status,
                        stats.request_count,
                        stats.success_count,
                        if stats.error_count > 0 {
                            Some("partial_errors")
                        } else {
                            None
                        },
                        Some(&format!(
                            "apps_attempted={} store={} store_not_found={} reviews={} popular_reviews={} ccu={} errors={}",
                            stats.apps_attempted,
                            stats.store_ok,
                            stats.store_not_found,
                            stats.reviews_ok,
                            stats.popular_reviews_ok,
                            stats.ccu_ok,
                            stats.error_count
                        )),
                    )
                    .map_err(err)?;
                    let after = repo.m3_catalog_coverage().map_err(err)?;
                    println!("path={}", db_path.display());
                    println!("requests={}", stats.request_count);
                    println!("apps_attempted={}", stats.apps_attempted);
                    println!("store_details_ok={}", stats.store_ok);
                    println!("store_details_not_found={}", stats.store_not_found);
                    println!("reviews_ok={}", stats.reviews_ok);
                    println!("popular_reviews_ok={}", stats.popular_reviews_ok);
                    println!("ccu_ok={}", stats.ccu_ok);
                    println!("errors={}", stats.error_count);
                    print_coverage(&after);
                    if stats.error_count > 0 {
                        Err(format!(
                            "Steam candidate enrichment completed with {} errors after {} requests",
                            stats.error_count, stats.request_count
                        ))
                    } else {
                        println!("steam_candidate_enrichment=ok");
                        Ok(())
                    }
                }
                Err(failure) => {
                    let status = if failure.stats.success_count > 0 {
                        "partial"
                    } else {
                        "failed"
                    };
                    repo.finish_source_run(
                        run_id,
                        status,
                        failure.stats.request_count,
                        failure.stats.success_count,
                        Some(failure.category),
                        Some(&failure.message),
                    )
                    .map_err(err)?;
                    Err(format!(
                        "Steam candidate enrichment {} after {} requests: {}",
                        failure.category, failure.stats.request_count, failure.message
                    ))
                }
            }
        }
        "run-steam-worker-once" => {
            let db_path = required_path(args.next(), "--db path")?;
            let job_limit = optional_worker_job_limit(args.next())?;
            let enrich_limit = optional_enrich_limit(args.next())?;
            if args.next().is_some() {
                return Err(
                    "run-steam-worker-once accepts db-path, job-limit, and enrich-limit only"
                        .into(),
                );
            }
            let api_key = optional_steam_web_api_key()?;
            let owner = configured_worker_owner()?;
            let db = Database::open(&db_path).map_err(err)?;
            db.migrate().map_err(err)?;
            let repo = Repository::new(db);
            repo.ensure_runtime_defaults().map_err(err)?;
            let stats =
                run_steam_worker_once(&repo, &owner, job_limit, enrich_limit, api_key.as_deref())?;
            println!("path={}", db_path.display());
            println!("worker={owner}");
            println!("jobs_leased={}", stats.leased);
            println!("jobs_completed={}", stats.completed);
            println!("jobs_retried={}", stats.retried);
            println!("jobs_dead={}", stats.dead);
            if stats.dead > 0 {
                Err(format!("Steam worker marked {} job(s) dead", stats.dead))
            } else {
                println!("steam_worker=ok");
                Ok(())
            }
        }
        "import-golden-profiles" => {
            let db_path = required_path(args.next(), "--db path")?;
            if args.next().is_some() {
                return Err("import-golden-profiles accepts only db-path".into());
            }
            let db = Database::open(&db_path).map_err(err)?;
            db.migrate().map_err(err)?;
            let repo = Repository::new(db);
            repo.ensure_runtime_defaults().map_err(err)?;
            let set = GoldenSet::load_embedded().map_err(err)?;
            let before = repo.m3_catalog_coverage().map_err(err)?;
            let mut applied = 0_i64;
            for game in &set.games {
                if repo.import_golden_multiplayer_profile(game).map_err(err)? {
                    applied += 1;
                }
            }
            let after = repo.m3_catalog_coverage().map_err(err)?;
            println!("path={}", db_path.display());
            println!("golden_set_version={}", set.version);
            println!("golden_games={}", set.games.len());
            println!("profiles_applied={}", applied);
            println!(
                "recommendation_ready_profiles_before={}",
                before.recommendation_ready_profiles
            );
            println!(
                "recommendation_ready_profiles_after={}",
                after.recommendation_ready_profiles
            );
            println!(
                "trusted_familiar_profiles_after={}",
                after.trusted_familiar_profiles
            );
            println!("golden_profile_import=ok");
            Ok(())
        }
        "backup" => {
            let db_path = required_path(args.next(), "--db path")?;
            let out_path = required_path(args.next(), "--out path")?;
            let db = Database::open(&db_path).map_err(err)?;
            db.assert_ready().map_err(err)?;
            let repo = Repository::new(db);
            repo.backup_to(&out_path).map_err(err)?;
            println!("backed up {} -> {}", db_path.display(), out_path.display());
            Ok(())
        }
        "restore" => {
            let backup_path = required_path(args.next(), "--from path")?;
            let dest_path = required_path(args.next(), "--to path")?;
            let now = SystemClock.now_ms();
            let repo = Repository::restore_backup(&backup_path, &dest_path, now).map_err(err)?;
            repo.assert_ready().map_err(err)?;
            println!(
                "restored {} -> {} (schema ok)",
                backup_path.display(),
                dest_path.display()
            );
            Ok(())
        }
        "help" | "-h" | "--help" => {
            println!("{}", usage());
            Ok(())
        }
        other => Err(format!("unknown command '{other}'\n{}", usage())),
    }
}

fn required_path(arg: Option<String>, label: &str) -> Result<PathBuf, String> {
    let value = arg.ok_or_else(|| format!("missing {label}"))?;
    // allow either bare path or --db path form
    if value.starts_with("--") {
        return Err(format!("expected path for {label}, got flag {value}"));
    }
    Ok(PathBuf::from(value))
}

fn optional_target(arg: Option<String>) -> Result<u32, String> {
    let target = match arg {
        Some(value) => value
            .parse::<u32>()
            .map_err(|_| format!("invalid candidate target: {value}"))?,
        None => STORE_SEARCH_TARGET_DEFAULT,
    };
    if !(1..=STORE_SEARCH_TARGET_MAX).contains(&target) {
        return Err(format!(
            "candidate target must be between 1 and {STORE_SEARCH_TARGET_MAX}"
        ));
    }
    Ok(target)
}

fn optional_enrich_limit(arg: Option<String>) -> Result<u32, String> {
    let limit = match arg {
        Some(value) => value
            .parse::<u32>()
            .map_err(|_| format!("invalid enrich limit: {value}"))?,
        None => ENRICH_LIMIT_DEFAULT,
    };
    if !(1..=ENRICH_LIMIT_MAX).contains(&limit) {
        return Err(format!(
            "enrich limit must be between 1 and {ENRICH_LIMIT_MAX}"
        ));
    }
    Ok(limit)
}

fn optional_catalog_page_count(arg: Option<String>) -> Result<u32, String> {
    let pages = match arg {
        Some(value) => value
            .parse::<u32>()
            .map_err(|_| format!("invalid catalog page count: {value}"))?,
        None => APP_LIST_PAGES_DEFAULT,
    };
    if !(1..=APP_LIST_PAGES_MAX).contains(&pages) {
        return Err(format!(
            "catalog page count must be between 1 and {APP_LIST_PAGES_MAX}"
        ));
    }
    Ok(pages)
}

fn optional_catalog_page_size(arg: Option<String>) -> Result<u32, String> {
    let page_size = match arg {
        Some(value) => value
            .parse::<u32>()
            .map_err(|_| format!("invalid catalog page size: {value}"))?,
        None => APP_LIST_PAGE_SIZE_DEFAULT,
    };
    if !(1..=APP_LIST_MAX_RESULTS).contains(&page_size) {
        return Err(format!(
            "catalog page size must be between 1 and {APP_LIST_MAX_RESULTS}"
        ));
    }
    Ok(page_size)
}

fn optional_worker_job_limit(arg: Option<String>) -> Result<i64, String> {
    let limit = match arg {
        Some(value) => value
            .parse::<i64>()
            .map_err(|_| format!("invalid worker job limit: {value}"))?,
        None => WORKER_JOB_LIMIT_DEFAULT,
    };
    if !(1..=WORKER_JOB_LIMIT_MAX).contains(&limit) {
        return Err(format!(
            "worker job limit must be between 1 and {WORKER_JOB_LIMIT_MAX}"
        ));
    }
    Ok(limit)
}

fn steam_web_api_key() -> Result<String, String> {
    optional_steam_web_api_key()?
        .ok_or_else(|| "MPGS_STEAM_WEB_API_KEY is required for collect-steam-catalog".to_owned())
}

fn optional_steam_web_api_key() -> Result<Option<String>, String> {
    let value = match env::var("MPGS_STEAM_WEB_API_KEY") {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => return Ok(None),
        Err(_) => return Err("MPGS_STEAM_WEB_API_KEY could not be read".into()),
    };
    let key = value.trim().to_owned();
    if key.len() != 32 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("MPGS_STEAM_WEB_API_KEY must be a 32-character hexadecimal key".into());
    }
    Ok(Some(key))
}

fn configured_worker_owner() -> Result<String, String> {
    let owner = env::var("MPGS_STEAM_WORKER_ID")
        .unwrap_or_else(|_| format!("mpgs-dbtool-{}", std::process::id()));
    let owner = owner.trim().to_owned();
    if owner.is_empty() || owner.len() > 96 {
        return Err("MPGS_STEAM_WORKER_ID must be between 1 and 96 bytes".into());
    }
    Ok(owner)
}

fn worker_retry_delay_ms(attempts: i64) -> i64 {
    let exponent = attempts.clamp(1, 6) as u32 - 1;
    WORKER_RETRY_BASE_MS.saturating_mul(1_i64 << exponent)
}

fn current_coverage_ratio(repo: &Repository) -> Result<Option<f64>, String> {
    let coverage = repo.m3_catalog_coverage().map_err(err)?;
    Ok(Some(if coverage.normalized_multiplayer_candidates > 0 {
        (coverage.recommendation_ready_profiles as f64
            / coverage.normalized_multiplayer_candidates as f64)
            .clamp(0.0, 1.0)
    } else {
        0.0
    }))
}

fn worker_task_status(task_type: &str) -> Option<(&'static str, &'static str)> {
    match task_type {
        "sync_catalog" => Some(("catalog_sync", APP_LIST_CURSOR_KEY)),
        "collect_candidates" => Some(("candidate_collection", STORE_SEARCH_CURSOR_KEY)),
        "enrich_catalog" => Some(("enrichment", ENRICH_CURSOR_KEY)),
        _ => None,
    }
}

fn record_worker_refresh(
    repo: &Repository,
    task_name: &str,
    cursor_key: &str,
    error_category: Option<&str>,
) -> Result<(), String> {
    let previous = repo
        .data_refresh_status()
        .map_err(err)?
        .into_iter()
        .find(|status| status.task_name == task_name);
    let cursor = repo.source_cursor(cursor_key).map_err(err)?;
    let now_ms = repo.database().now_ms();
    repo.update_data_refresh_status(
        task_name,
        if error_category.is_none() {
            Some(now_ms)
        } else {
            previous
                .as_ref()
                .and_then(|status| status.last_success_at_ms)
        },
        previous.as_ref().and_then(|status| status.next_run_at_ms),
        error_category,
        cursor.as_deref(),
        current_coverage_ratio(repo)?,
    )
    .map_err(err)
}

fn worker_storage_error(_error: impl std::fmt::Display) -> WorkerTaskError {
    WorkerTaskError {
        category: "network",
    }
}

fn worker_task_error(category: &'static str, _message: impl Into<String>) -> WorkerTaskError {
    WorkerTaskError { category }
}

fn worker_job_error_category(category: &'static str) -> &'static str {
    match category {
        "network" | "rate_limited" | "auth" | "not_found" | "parse_changed" | "invalid_payload" => {
            category
        }
        // The job contract has no distinct oversized-response category; a
        // response exceeding the adapter bound is treated as a source-shape
        // change and requires operator review rather than blind retries.
        "response_too_large" => "parse_changed",
        // SQLite writer contention or a transient local filesystem issue must
        // not make an otherwise valid collection job permanently dead.
        "storage" => "network",
        _ => "invalid_payload",
    }
}

fn worker_completion_key(owner: &str, job_id: i64) -> String {
    format!("worker:{owner}:{job_id}")
}

fn run_steam_worker_once(
    repo: &Repository,
    owner: &str,
    job_limit: i64,
    enrich_limit: u32,
    api_key: Option<&str>,
) -> Result<SteamWorkerStats, String> {
    let jobs = repo
        .lease_jobs(owner, job_limit, WORKER_LEASE_MS, Some("steam"))
        .map_err(err)?;
    let mut stats = SteamWorkerStats {
        leased: jobs.len(),
        ..SteamWorkerStats::default()
    };
    for job in jobs {
        let status = worker_task_status(&job.task_type);
        let result = match job.task_type.as_str() {
            "sync_catalog" => match api_key {
                Some(key) => run_catalog_worker_task(repo, key),
                None => Err(worker_task_error(
                    "auth",
                    "MPGS_STEAM_WEB_API_KEY is not configured for the worker",
                )),
            },
            "collect_candidates" => run_candidate_worker_task(repo),
            "enrich_catalog" => run_enrichment_worker_task(repo, enrich_limit),
            _ => Err(worker_task_error(
                "invalid_payload",
                "unsupported Steam job type",
            )),
        };
        match result {
            Ok(()) => {
                repo.complete_job(job.job_id, owner, &worker_completion_key(owner, job.job_id))
                    .map_err(err)?;
                if let Some((task_name, cursor_key)) = status {
                    record_worker_refresh(repo, task_name, cursor_key, None)?;
                }
                stats.completed = stats.completed.saturating_add(1);
            }
            Err(failure) => {
                let job_error_category = worker_job_error_category(failure.category);
                let failed = repo
                    .fail_job(
                        job.job_id,
                        owner,
                        job_error_category,
                        worker_retry_delay_ms(job.attempts),
                    )
                    .map_err(err)?;
                if let Some((task_name, cursor_key)) = status {
                    record_worker_refresh(repo, task_name, cursor_key, Some(job_error_category))?;
                }
                if failed.status == "dead" {
                    stats.dead = stats.dead.saturating_add(1);
                } else {
                    stats.retried = stats.retried.saturating_add(1);
                }
                eprintln!(
                    "warn job_id={} task_type={} category={}",
                    job.job_id, job.task_type, job_error_category
                );
            }
        }
    }
    Ok(stats)
}

fn run_catalog_worker_task(repo: &Repository, api_key: &str) -> Result<(), WorkerTaskError> {
    let run_id = repo
        .start_source_run(
            APP_LIST_SOURCE_NAME,
            "catalog_sync",
            APP_LIST_ADAPTER_VERSION,
            Some("worker=true;max_pages=1;page_size=1000;key=present"),
        )
        .map_err(worker_storage_error)?;
    match collect_steam_catalog(repo, api_key, 1, APP_LIST_PAGE_SIZE_DEFAULT) {
        Ok(stats) => repo
            .finish_source_run(
                run_id,
                "succeeded",
                stats.request_count,
                stats.success_count,
                None,
                Some("cursor persisted"),
            )
            .map_err(worker_storage_error),
        Err(failure) => {
            let status = if failure.stats.success_count > 0 {
                "partial"
            } else {
                "failed"
            };
            let _ = repo.finish_source_run(
                run_id,
                status,
                failure.stats.request_count,
                failure.stats.success_count,
                Some(failure.category),
                Some(&failure.message),
            );
            Err(worker_task_error(failure.category, failure.message))
        }
    }
}

fn run_candidate_worker_task(repo: &Repository) -> Result<(), WorkerTaskError> {
    let run_id = repo
        .start_source_run(
            STORE_SEARCH_SOURCE_NAME,
            "candidate_discovery",
            STORE_SEARCH_ADAPTER_VERSION,
            Some("worker=true;category2=1;sort=Reviews_DESC;cc=US;l=english"),
        )
        .map_err(worker_storage_error)?;
    match collect_steam_candidates(repo, STORE_SEARCH_TARGET_DEFAULT) {
        Ok(stats) => repo
            .finish_source_run(
                run_id,
                "succeeded",
                stats.request_count,
                stats.success_count,
                None,
                Some("target reached or already satisfied"),
            )
            .map_err(worker_storage_error),
        Err(failure) => {
            let status = if failure.stats.success_count > 0 {
                "partial"
            } else {
                "failed"
            };
            let _ = repo.finish_source_run(
                run_id,
                status,
                failure.stats.request_count,
                failure.stats.success_count,
                Some(failure.category),
                Some(&failure.message),
            );
            Err(worker_task_error(failure.category, failure.message))
        }
    }
}

fn run_enrichment_worker_task(repo: &Repository, limit: u32) -> Result<(), WorkerTaskError> {
    let (country_code, language) = configured_store_locale()
        .map_err(|_| worker_task_error("invalid_payload", "invalid Steam store locale"))?;
    let cursor = repo
        .source_cursor(ENRICH_CURSOR_KEY)
        .map_err(worker_storage_error)?
        .map(|value| serde_json::from_str::<EnrichmentCursor>(&value))
        .transpose()
        .map_err(|_| worker_task_error("invalid_payload", "invalid stored enrichment cursor"))?
        .unwrap_or_default();
    let media_policy = configured_media_backfill_policy();
    let need_filter = EnrichmentNeedFilter {
        store: true,
        reviews: true,
        review_excerpts: true,
        ccu: true,
        price: true,
        media_backfill: media_policy.enabled,
    };
    let targets = repo
        .list_enrichment_targets_after_filtered_with_media(
            limit,
            Some(cursor.after_app_id),
            &country_code,
            &language,
            need_filter,
            media_policy,
        )
        .map_err(worker_storage_error)?;
    let coverage = repo.media_coverage_stats().map_err(worker_storage_error)?;
    let run_id = repo
        .start_source_run(
            STORE_SOURCE_NAME,
            "candidate_enrichment",
            STORE_ADAPTER_VERSION,
            Some(&format!(
                "worker=true;limit={limit};targets={};country={country_code};language={language};after_app_id={};media_coverage={:.3};media_with={}/{}",
                targets.len(),
                cursor.after_app_id,
                coverage.coverage_ratio,
                coverage.apps_with_media,
                coverage.candidate_apps
            )),
        )
        .map_err(worker_storage_error)?;
    if targets.is_empty() {
        return repo
            .finish_source_run(run_id, "succeeded", 0, 0, None, Some("no targets due"))
            .map_err(worker_storage_error);
    }
    match enrich_steam_candidates(repo, &targets, &country_code, &language) {
        Ok(stats) => {
            let next_cursor = EnrichmentCursor {
                after_app_id: targets
                    .last()
                    .map_or(cursor.after_app_id, |target| target.app_id),
            };
            repo.save_source_cursor(
                ENRICH_CURSOR_KEY,
                STORE_SOURCE_NAME,
                &serde_json::to_value(next_cursor).map_err(worker_storage_error)?,
            )
            .map_err(worker_storage_error)?;
            let status = if stats.error_count > 0 && stats.success_count > 0 {
                "partial"
            } else if stats.error_count > 0 {
                "failed"
            } else {
                "succeeded"
            };
            repo.finish_source_run(
                run_id,
                status,
                stats.request_count,
                stats.success_count,
                (stats.error_count > 0).then_some("partial_errors"),
                Some(&format!(
                    "apps_attempted={} store={} store_not_found={} reviews={} popular_reviews={} ccu={} errors={}",
                    stats.apps_attempted,
                    stats.store_ok,
                    stats.store_not_found,
                    stats.reviews_ok,
                    stats.popular_reviews_ok,
                    stats.ccu_ok,
                    stats.error_count
                )),
            )
            .map_err(worker_storage_error)?;
            if stats.error_count > 0 {
                Err(worker_task_error(
                    "network",
                    "one or more enrichment requests failed",
                ))
            } else {
                Ok(())
            }
        }
        Err(failure) => {
            let _ = repo.finish_source_run(
                run_id,
                "failed",
                failure.stats.request_count,
                failure.stats.success_count,
                Some(failure.category),
                Some(&failure.message),
            );
            Err(worker_task_error(failure.category, failure.message))
        }
    }
}

fn print_coverage(coverage: &mpgs_storage::M3CatalogCoverage) {
    println!(
        "normalized_multiplayer_candidates={}",
        coverage.normalized_multiplayer_candidates
    );
    println!(
        "category_evidence_candidates={}",
        coverage.category_evidence_candidates
    );
    println!(
        "recommendation_ready_profiles={}",
        coverage.recommendation_ready_profiles
    );
    println!(
        "trusted_familiar_profiles={}",
        coverage.trusted_familiar_profiles
    );
    println!("with_platforms={}", coverage.with_platforms);
    println!("with_languages={}", coverage.with_languages);
    println!("with_typical_session={}", coverage.with_typical_session);
    println!("with_price={}", coverage.with_price);
    println!("with_reviews={}", coverage.with_reviews);
    println!("with_ccu={}", coverage.with_ccu);
}

fn audit_m3(repo: &Repository, db_path: &std::path::Path) -> Result<(), String> {
    let active = repo.active_algorithm_config().map_err(err)?;
    let coverage = repo.m3_catalog_coverage().map_err(err)?;
    println!("path={}", db_path.display());
    println!("algorithm_version={}", active.version);
    println!(
        "normalized_multiplayer_candidates={}",
        coverage.normalized_multiplayer_candidates
    );
    println!(
        "category_evidence_candidates={}",
        coverage.category_evidence_candidates
    );
    println!(
        "recommendation_ready_profiles={}",
        coverage.recommendation_ready_profiles
    );
    println!(
        "trusted_familiar_profiles={}",
        coverage.trusted_familiar_profiles
    );
    println!("with_platforms={}", coverage.with_platforms);
    println!("with_languages={}", coverage.with_languages);
    println!("with_typical_session={}", coverage.with_typical_session);
    println!("with_price={}", coverage.with_price);
    println!("with_reviews={}", coverage.with_reviews);
    println!("with_ccu={}", coverage.with_ccu);
    if coverage.normalized_multiplayer_candidates < i64::from(STORE_SEARCH_TARGET_DEFAULT) {
        return Err(format!(
            "M3 catalog gate failed: {} normalized multiplayer candidates, need at least {}",
            coverage.normalized_multiplayer_candidates, STORE_SEARCH_TARGET_DEFAULT
        ));
    }
    println!("m3_catalog_gate=ok");
    Ok(())
}

fn parse_m7_audit_options(
    args: &mut impl Iterator<Item = String>,
) -> Result<Option<String>, String> {
    let mut upcoming_shortfall_reason = None;
    for arg in args {
        let Some(value) = arg.strip_prefix("--allow-upcoming-shortfall=") else {
            return Err(
                "m7-data-audit accepts only --allow-upcoming-shortfall=<reason> after db-path"
                    .into(),
            );
        };
        let reason = value.trim();
        if reason.is_empty()
            || reason.len() > 240
            || reason.contains('\r')
            || reason.contains('\n')
            || upcoming_shortfall_reason.is_some()
        {
            return Err(
                "--allow-upcoming-shortfall needs one non-empty single-line reason of at most 240 bytes"
                    .into(),
            );
        }
        upcoming_shortfall_reason = Some(reason.to_owned());
    }
    Ok(upcoming_shortfall_reason)
}

fn percentage(covered: i64, total: i64) -> f64 {
    if total <= 0 {
        0.0
    } else {
        (covered.max(0) as f64 / total as f64) * 100.0
    }
}

fn meets_percentage(covered: i64, total: i64, minimum_percent: i64) -> bool {
    total > 0 && covered.max(0).saturating_mul(100) >= total.saturating_mul(minimum_percent)
}

fn print_m7_coverage(coverage: &mpgs_storage::M7DataCoverage) {
    println!(
        "normalized_multiplayer_candidates={}",
        coverage.normalized_multiplayer_candidates
    );
    println!(
        "trusted_friend_multiplayer_profiles={}",
        coverage.trusted_friend_multiplayer_profiles
    );
    println!("candidates_with_date={}", coverage.candidates_with_date);
    println!("candidates_with_cover={}", coverage.candidates_with_cover);
    println!(
        "date_coverage_percent={:.1}",
        percentage(
            coverage.candidates_with_date,
            coverage.normalized_multiplayer_candidates
        )
    );
    println!(
        "cover_coverage_percent={:.1}",
        percentage(
            coverage.candidates_with_cover,
            coverage.normalized_multiplayer_candidates
        )
    );
    println!("upcoming_candidates={}", coverage.upcoming_candidates);
    println!(
        "recent_release_candidates={}",
        coverage.recent_release_candidates
    );
    println!(
        "popular_legacy_candidates={}",
        coverage.popular_legacy_candidates
    );
    println!(
        "classic_legacy_candidates={}",
        coverage.classic_legacy_candidates
    );
    println!(
        "trusted_profiles_with_seven_day_reviews={}",
        coverage.trusted_profiles_with_seven_day_reviews
    );
    println!(
        "trusted_profiles_with_seven_day_ccu={}",
        coverage.trusted_profiles_with_seven_day_ccu
    );
}

fn audit_m7(
    repo: &Repository,
    db_path: &std::path::Path,
    upcoming_shortfall_reason: Option<&str>,
) -> Result<(), String> {
    let active = repo.active_algorithm_config().map_err(err)?;
    let coverage = repo.m7_data_coverage(&active.config).map_err(err)?;
    println!("path={}", db_path.display());
    println!("algorithm_version={}", active.version);
    println!("m7_policy_min_candidates={M7_MIN_CANDIDATES}");
    println!("m7_policy_min_trusted_profiles={M7_MIN_TRUSTED_FRIEND_PROFILES}");
    println!("m7_policy_min_section_candidates={M7_MIN_SECTION_CANDIDATES}");
    println!("m7_policy_min_date_coverage_percent={M7_MIN_DATE_COVERAGE_PERCENT}");
    println!("m7_policy_min_cover_coverage_percent={M7_MIN_COVER_COVERAGE_PERCENT}");
    println!("m7_policy_min_seven_day_focus={M7_MIN_SEVEN_DAY_FOCUS}");
    print_m7_coverage(&coverage);

    let mut failures = Vec::new();
    if coverage.normalized_multiplayer_candidates < M7_MIN_CANDIDATES {
        failures.push(format!(
            "{} normalized multiplayer candidates, need at least {M7_MIN_CANDIDATES}",
            coverage.normalized_multiplayer_candidates
        ));
    }
    if coverage.trusted_friend_multiplayer_profiles < M7_MIN_TRUSTED_FRIEND_PROFILES {
        failures.push(format!(
            "{} trusted familiar-multiplayer profiles, need at least {M7_MIN_TRUSTED_FRIEND_PROFILES}",
            coverage.trusted_friend_multiplayer_profiles
        ));
    }
    if !meets_percentage(
        coverage.candidates_with_date,
        coverage.normalized_multiplayer_candidates,
        M7_MIN_DATE_COVERAGE_PERCENT,
    ) {
        failures.push(format!(
            "date coverage is {:.1}%, need at least {M7_MIN_DATE_COVERAGE_PERCENT}%",
            percentage(
                coverage.candidates_with_date,
                coverage.normalized_multiplayer_candidates
            )
        ));
    }
    if !meets_percentage(
        coverage.candidates_with_cover,
        coverage.normalized_multiplayer_candidates,
        M7_MIN_COVER_COVERAGE_PERCENT,
    ) {
        failures.push(format!(
            "cover coverage is {:.1}%, need at least {M7_MIN_COVER_COVERAGE_PERCENT}%",
            percentage(
                coverage.candidates_with_cover,
                coverage.normalized_multiplayer_candidates
            )
        ));
    }
    if coverage.upcoming_candidates < M7_MIN_SECTION_CANDIDATES {
        if let Some(reason) = upcoming_shortfall_reason {
            println!("upcoming_shortfall_exception={reason}");
        } else {
            failures.push(format!(
                "{} upcoming candidates, need at least {M7_MIN_SECTION_CANDIDATES} or an explicit documented shortfall exception",
                coverage.upcoming_candidates
            ));
        }
    } else if upcoming_shortfall_reason.is_some() {
        failures.push(
            "--allow-upcoming-shortfall is only valid when fewer than 20 upcoming candidates are available"
                .into(),
        );
    }
    for (name, count) in [
        ("recent_release", coverage.recent_release_candidates),
        ("popular_legacy", coverage.popular_legacy_candidates),
        ("classic_legacy", coverage.classic_legacy_candidates),
    ] {
        if count < M7_MIN_SECTION_CANDIDATES {
            failures.push(format!(
                "{count} {name} candidates, need at least {M7_MIN_SECTION_CANDIDATES}"
            ));
        }
    }
    if coverage.trusted_profiles_with_seven_day_reviews < M7_MIN_SEVEN_DAY_FOCUS {
        failures.push(format!(
            "{} trusted profiles have seven consecutive review days, need at least {M7_MIN_SEVEN_DAY_FOCUS}",
            coverage.trusted_profiles_with_seven_day_reviews
        ));
    }
    if coverage.trusted_profiles_with_seven_day_ccu < M7_MIN_SEVEN_DAY_FOCUS {
        failures.push(format!(
            "{} trusted profiles have seven consecutive CCU days, need at least {M7_MIN_SEVEN_DAY_FOCUS}",
            coverage.trusted_profiles_with_seven_day_ccu
        ));
    }
    if !failures.is_empty() {
        return Err(format!("M7 data gate failed: {}", failures.join("; ")));
    }
    println!("m7_data_gate=ok");
    Ok(())
}

fn collect_steam_catalog(
    repo: &Repository,
    api_key: &str,
    max_pages: u32,
    page_size: u32,
) -> Result<CollectionStats, CollectionError> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(DEFAULT_USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .map_err(|error| CollectionError {
            category: "network",
            message: error.to_string(),
            stats: CollectionStats::default(),
        })?;
    let stored_cursor = repo
        .source_cursor(APP_LIST_CURSOR_KEY)
        .map_err(|error| CollectionError {
            category: "storage",
            message: error.to_string(),
            stats: CollectionStats::default(),
        })?
        .map(|value| serde_json::from_str::<AppListCursor>(&value))
        .transpose()
        .map_err(|error| CollectionError {
            category: "invalid_payload",
            message: format!("invalid stored catalog cursor: {error}"),
            stats: CollectionStats::default(),
        })?;
    let mut cursor =
        stored_cursor.unwrap_or_else(|| AppListCursor::new_pass(0, APP_LIST_ADAPTER_VERSION));
    // A completed pass starts a new incremental window. A parser upgrade keeps
    // the prior watermark but restarts pagination with the new adapter version.
    if !cursor.is_in_progress() || cursor.adapter_version != APP_LIST_ADAPTER_VERSION {
        cursor = AppListCursor::new_pass(cursor.if_modified_since, APP_LIST_ADAPTER_VERSION);
    }

    let mut stats = CollectionStats::default();
    for page_index in 0..max_pages {
        let request = AppListRequest::from_cursor(&cursor, page_size);
        let page = fetch_app_list_page_with_retry(&client, &request, api_key, &mut stats)?;
        let ingested = repo
            .ingest_app_list_page(&request, &page)
            .map_err(|error| CollectionError {
                category: "storage",
                message: error.to_string(),
                stats,
            })?;
        apply_page_to_cursor(&mut cursor, &page);
        repo.save_source_cursor(
            APP_LIST_CURSOR_KEY,
            APP_LIST_SOURCE_NAME,
            &serde_json::to_value(&cursor).map_err(|error| CollectionError {
                category: "invalid_payload",
                message: error.to_string(),
                stats,
            })?,
        )
        .map_err(|error| CollectionError {
            category: "storage",
            message: error.to_string(),
            stats,
        })?;
        stats.success_count = stats.success_count.saturating_add(ingested as i64);
        println!(
            "progress catalog_apps={} page={} rows={} next_last_appid={} pass_complete={}",
            repo.count_apps().map_err(|error| CollectionError {
                category: "storage",
                message: error.to_string(),
                stats,
            })?,
            page_index + 1,
            ingested,
            cursor.last_appid,
            !cursor.is_in_progress(),
        );
        if !cursor.is_in_progress() {
            break;
        }
        if page_index + 1 < max_pages {
            thread::sleep(Duration::from_millis(APP_LIST_INTER_REQUEST_MS));
        }
    }
    Ok(stats)
}

fn fetch_app_list_page_with_retry(
    client: &reqwest::blocking::Client,
    request: &AppListRequest,
    api_key: &str,
    stats: &mut CollectionStats,
) -> Result<mpgs_steam_source::AppListPage, CollectionError> {
    let mut last_error = None;
    for attempt in 0..3_u64 {
        stats.request_count = stats.request_count.saturating_add(1);
        match fetch_app_list_page(client, request, api_key) {
            Ok(page) => return Ok(page),
            Err(error) => {
                let retryable = error.is_retryable();
                let category = source_error_category(&error);
                let message = error.to_string();
                last_error = Some((category, message));
                if !retryable || attempt == 2 {
                    break;
                }
                thread::sleep(Duration::from_secs(1_u64 << attempt));
            }
        }
    }
    let (category, message) = last_error.unwrap_or(("network", "unknown fetch failure".into()));
    Err(CollectionError {
        category,
        message,
        stats: *stats,
    })
}

fn fetch_app_list_page(
    client: &reqwest::blocking::Client,
    request: &AppListRequest,
    api_key: &str,
) -> Result<mpgs_steam_source::AppListPage, SourceError> {
    let url = app_list_url(request, api_key)?;
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        // Do not preserve reqwest's URL text here: it would contain the key.
        .map_err(|_| SourceError::Temporary {
            message: "Steam Web API request failed".into(),
        })?;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let mut body = Vec::new();
    response
        .take((RawResponse::DEFAULT_MAX_BYTES + 1) as u64)
        .read_to_end(&mut body)
        .map_err(|_| SourceError::Temporary {
            message: "Steam Web API response read failed".into(),
        })?;
    let raw = RawResponse::validate(status, body, content_type, RawResponse::DEFAULT_MAX_BYTES)?;
    parse_app_list_page(&raw)
}

fn app_list_url(request: &AppListRequest, api_key: &str) -> Result<reqwest::Url, SourceError> {
    let mut url = reqwest::Url::parse(&format!(
        "{STEAM_WEB_API_HOST}{}",
        request.path_and_query_without_key()
    ))
    .map_err(|_| SourceError::Config {
        message: "Steam Web API endpoint configuration is invalid".into(),
    })?;
    url.query_pairs_mut().append_pair("key", api_key);
    Ok(url)
}

fn collect_steam_candidates(
    repo: &Repository,
    target: u32,
) -> Result<CollectionStats, CollectionError> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(DEFAULT_USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .map_err(|error| CollectionError {
            category: "network",
            message: error.to_string(),
            stats: CollectionStats::default(),
        })?;
    let mut stats = CollectionStats::default();
    let coverage = repo
        .m3_catalog_coverage()
        .map_err(|error| CollectionError {
            category: "storage",
            message: error.to_string(),
            stats: CollectionStats::default(),
        })?;
    if coverage.normalized_multiplayer_candidates >= i64::from(target) {
        return Ok(stats);
    }

    let stored_cursor = repo
        .source_cursor(STORE_SEARCH_CURSOR_KEY)
        .map_err(|error| CollectionError {
            category: "storage",
            message: error.to_string(),
            stats: CollectionStats::default(),
        })?
        .map(|value| serde_json::from_str::<StoreSearchCursor>(&value))
        .transpose()
        .map_err(|error| CollectionError {
            category: "invalid_payload",
            message: format!("invalid stored cursor: {error}"),
            stats: CollectionStats::default(),
        })?;
    let mut start = stored_cursor
        .as_ref()
        .filter(|cursor| !cursor.complete)
        .map_or(0, |cursor| cursor.next_start);

    loop {
        let current = repo
            .m3_catalog_coverage()
            .map_err(|error| CollectionError {
                category: "storage",
                message: error.to_string(),
                stats,
            })?;
        if current.normalized_multiplayer_candidates >= i64::from(target) {
            return Ok(stats);
        }

        let request = StoreSearchRequest::new(start, 100).map_err(|error| CollectionError {
            category: source_error_category(&error),
            message: error.to_string(),
            stats,
        })?;
        let page = fetch_store_search_page_with_retry(&client, &request, &mut stats)?;
        let ingested = repo
            .ingest_store_search_page(&page)
            .map_err(|error| CollectionError {
                category: "storage",
                message: error.to_string(),
                stats,
            })?;
        // Refresh multiplayer dominant_mode from the Multi-player search hint
        // (and any co-op / PvP category_hint evidence already stored).
        let _ = repo
            .materialize_store_category_profiles()
            .map_err(|error| CollectionError {
                category: "storage",
                message: error.to_string(),
                stats,
            })?;
        stats.success_count = stats.success_count.saturating_add(ingested as i64);

        let next_start = page.next_start();
        let cursor = StoreSearchCursor {
            next_start,
            total_count: Some(page.total_count),
            target,
            complete: page.is_complete(),
        };
        repo.save_source_cursor(
            STORE_SEARCH_CURSOR_KEY,
            STORE_SEARCH_SOURCE_NAME,
            &serde_json::to_value(&cursor).map_err(|error| CollectionError {
                category: "invalid_payload",
                message: error.to_string(),
                stats,
            })?,
        )
        .map_err(|error| CollectionError {
            category: "storage",
            message: error.to_string(),
            stats,
        })?;

        let current = repo
            .m3_catalog_coverage()
            .map_err(|error| CollectionError {
                category: "storage",
                message: error.to_string(),
                stats,
            })?;
        println!(
            "progress candidates={} target={} page_start={} rows={}",
            current.normalized_multiplayer_candidates, target, page.start, page.result_count
        );
        if current.normalized_multiplayer_candidates >= i64::from(target) {
            return Ok(stats);
        }
        if page.is_complete() {
            return Err(CollectionError {
                category: "insufficient_results",
                message: format!(
                    "Steam search exhausted at {} candidates before target {target}",
                    current.normalized_multiplayer_candidates
                ),
                stats,
            });
        }
        start = next_start;
        thread::sleep(Duration::from_millis(1_100));
    }
}

fn fetch_store_search_page_with_retry(
    client: &reqwest::blocking::Client,
    request: &StoreSearchRequest,
    stats: &mut CollectionStats,
) -> Result<StoreSearchPage, CollectionError> {
    let mut last_error = None;
    for attempt in 0..3_u64 {
        stats.request_count = stats.request_count.saturating_add(1);
        match fetch_store_search_page(client, request) {
            Ok(page) => return Ok(page),
            Err(error) => {
                let retryable = error.is_retryable();
                let category = source_error_category(&error);
                let message = error.to_string();
                last_error = Some((category, message));
                if !retryable || attempt == 2 {
                    break;
                }
                thread::sleep(Duration::from_secs(1_u64 << attempt));
            }
        }
    }
    let (category, message) = last_error.unwrap_or(("network", "unknown fetch failure".into()));
    Err(CollectionError {
        category,
        message,
        stats: CollectionStats {
            request_count: stats.request_count,
            success_count: stats.success_count,
        },
    })
}

fn fetch_store_search_page(
    client: &reqwest::blocking::Client,
    request: &StoreSearchRequest,
) -> Result<StoreSearchPage, SourceError> {
    let url = format!("{STEAM_STORE_HOST}{}", request.path_and_query());
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .map_err(|error| SourceError::Temporary {
            message: error.to_string(),
        })?;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let mut body = Vec::new();
    response
        .take((STORE_SEARCH_RESPONSE_MAX_BYTES + 1) as u64)
        .read_to_end(&mut body)
        .map_err(|error| SourceError::Temporary {
            message: error.to_string(),
        })?;
    let raw = RawResponse::validate(status, body, content_type, STORE_SEARCH_RESPONSE_MAX_BYTES)?;
    parse_store_search_page(request, &raw)
}

fn source_error_category(error: &SourceError) -> &'static str {
    match error {
        SourceError::RateLimited { .. } => "rate_limited",
        SourceError::HttpStatus { status: 401 | 403 } => "auth",
        SourceError::NotFound { .. } | SourceError::HttpStatus { status: 404 } => "not_found",
        SourceError::JsonParse { .. }
        | SourceError::InvalidStructure { .. }
        | SourceError::InvalidUtf8 => "parse_changed",
        SourceError::Config { .. } | SourceError::Permanent { .. } => "invalid_payload",
        SourceError::ResponseTooLarge { .. } => "response_too_large",
        SourceError::HttpStatus { .. } | SourceError::Temporary { .. } => "network",
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct EnrichStats {
    request_count: i64,
    success_count: i64,
    apps_attempted: i64,
    store_ok: i64,
    store_not_found: i64,
    reviews_ok: i64,
    popular_reviews_ok: i64,
    ccu_ok: i64,
    error_count: i64,
}

fn enrich_steam_candidates(
    repo: &Repository,
    targets: &[mpgs_storage::EnrichmentTarget],
    country_code: &str,
    language: &str,
) -> Result<EnrichStats, CollectionError> {
    let store_only = env_flag("MPGS_ENRICH_STORE_ONLY");
    let skip_reviews = store_only || env_flag("MPGS_ENRICH_SKIP_REVIEWS");
    let skip_ccu = store_only || env_flag("MPGS_ENRICH_SKIP_CCU");
    let skip_store = env_flag("MPGS_ENRICH_SKIP_STORE");
    let inter_request_ms = enrich_inter_request_ms();
    let client = reqwest::blocking::Client::builder()
        .user_agent(DEFAULT_USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .map_err(|error| CollectionError {
            category: "network",
            message: error.to_string(),
            stats: CollectionStats::default(),
        })?;

    let mut stats = EnrichStats::default();
    let total = targets.len();
    for (index, target) in targets.iter().enumerate() {
        stats.apps_attempted = stats.apps_attempted.saturating_add(1);
        let mut app_ok = 0_i64;

        // Price/media come from appdetails; re-fetch store when those dimensions are due.
        if !skip_store && target.needs_store_fetch() {
            if target.needs_media_backfill {
                let _ = repo.begin_media_backfill_attempt(target.app_id);
            }
            match enrich_store_details(
                &client,
                repo,
                target.app_id,
                country_code,
                language,
                &mut stats,
            ) {
                Ok(StoreDetailsOutcome::Ingested) => {
                    stats.store_ok = stats.store_ok.saturating_add(1);
                    app_ok = app_ok.saturating_add(1);
                    if target.needs_media_backfill {
                        let has_media = repo.app_has_media_assets(target.app_id).unwrap_or(false);
                        let outcome = if has_media {
                            mpgs_storage::MediaBackfillOutcome::Complete
                        } else {
                            // Successful parse/ingest with no rows ⇒ Steam has none usable.
                            mpgs_storage::MediaBackfillOutcome::NoneAvailable
                        };
                        let _ = repo.finish_media_backfill_attempt(
                            target.app_id,
                            outcome,
                            configured_media_backfill_policy().max_attempts,
                        );
                    }
                }
                Ok(StoreDetailsOutcome::NotFound) => {
                    stats.store_not_found = stats.store_not_found.saturating_add(1);
                    app_ok = app_ok.saturating_add(1);
                    if target.needs_media_backfill {
                        let _ = repo.finish_media_backfill_attempt(
                            target.app_id,
                            mpgs_storage::MediaBackfillOutcome::NoneAvailable,
                            configured_media_backfill_policy().max_attempts,
                        );
                    }
                }
                Err(error) => {
                    stats.error_count = stats.error_count.saturating_add(1);
                    eprintln!(
                        "warn app_id={} store_details: {} ({})",
                        target.app_id, error.message, error.category
                    );
                    if target.needs_media_backfill {
                        let _ = repo.finish_media_backfill_attempt(
                            target.app_id,
                            mpgs_storage::MediaBackfillOutcome::Failed,
                            configured_media_backfill_policy().max_attempts,
                        );
                    }
                }
            }
            thread::sleep(Duration::from_millis(inter_request_ms));
        }

        if target.needs_reviews && !skip_reviews {
            match enrich_reviews(&client, repo, target.app_id, &mut stats) {
                Ok(()) => {
                    stats.reviews_ok = stats.reviews_ok.saturating_add(1);
                    app_ok = app_ok.saturating_add(1);
                }
                Err(error) => {
                    stats.error_count = stats.error_count.saturating_add(1);
                    eprintln!(
                        "warn app_id={} reviews: {} ({})",
                        target.app_id, error.message, error.category
                    );
                }
            }
            thread::sleep(Duration::from_millis(inter_request_ms));
        }

        if target.needs_review_excerpts && !store_only {
            match enrich_popular_reviews(&client, repo, target.app_id, &mut stats) {
                Ok(()) => {
                    stats.popular_reviews_ok = stats.popular_reviews_ok.saturating_add(1);
                    app_ok = app_ok.saturating_add(1);
                }
                Err(error) => {
                    stats.error_count = stats.error_count.saturating_add(1);
                    eprintln!(
                        "warn app_id={} popular_reviews: {} ({})",
                        target.app_id, error.message, error.category
                    );
                }
            }
            thread::sleep(Duration::from_millis(inter_request_ms));
        }

        if target.needs_ccu && !skip_ccu {
            match enrich_ccu(&client, repo, target.app_id, &mut stats) {
                Ok(()) => {
                    stats.ccu_ok = stats.ccu_ok.saturating_add(1);
                    app_ok = app_ok.saturating_add(1);
                }
                Err(error) => {
                    stats.error_count = stats.error_count.saturating_add(1);
                    eprintln!(
                        "warn app_id={} ccu: {} ({})",
                        target.app_id, error.message, error.category
                    );
                }
            }
            // CCU uses the separate Steam Web API host. When this app also had
            // Store work, the preceding Store delay already spaces the next
            // appdetails request. Preserve pacing for CCU-only refresh passes.
            if !(target.needs_store_fetch()
                || target.needs_reviews
                || target.needs_review_excerpts)
            {
                thread::sleep(Duration::from_millis(inter_request_ms));
            }
        }

        stats.success_count = stats.success_count.saturating_add(app_ok);
        if (index + 1) % 10 == 0 || index + 1 == total {
            // stderr so progress is visible when stdout is piped/fully buffered.
            eprintln!(
                "progress enriched_apps={}/{} store_ok={} store_not_found={} reviews_ok={} popular_reviews_ok={} ccu_ok={} errors={}",
                index + 1,
                total,
                stats.store_ok,
                stats.store_not_found,
                stats.reviews_ok,
                stats.popular_reviews_ok,
                stats.ccu_ok,
                stats.error_count
            );
            let _ = io::stderr().flush();
        }
    }
    Ok(stats)
}

struct SoftEnrichError {
    category: &'static str,
    message: String,
}

enum StoreDetailsOutcome {
    Ingested,
    NotFound,
}

fn enrich_store_details(
    client: &reqwest::blocking::Client,
    repo: &Repository,
    app_id: u32,
    country_code: &str,
    language: &str,
    stats: &mut EnrichStats,
) -> Result<StoreDetailsOutcome, SoftEnrichError> {
    let request =
        StoreDetailsRequest::with_locale(app_id, country_code, language).map_err(|error| {
            SoftEnrichError {
                category: source_error_category(&error),
                message: error.to_string(),
            }
        })?;
    let raw = fetch_raw_with_retry(
        client,
        &format!("{STEAM_STORE_HOST}{}", request.path_and_query()),
        stats,
    )?;
    let parsed = match parse_store_details(&request, &raw) {
        Ok(parsed) => parsed,
        Err(_error @ SourceError::NotFound { .. }) => {
            persist_with_retry(|| {
                repo.record_store_details_not_found(app_id, country_code, language)
            })?;
            return Ok(StoreDetailsOutcome::NotFound);
        }
        Err(error) => {
            return Err(SoftEnrichError {
                category: source_error_category(&error),
                message: error.to_string(),
            });
        }
    };
    persist_with_retry(|| repo.ingest_store_details(&parsed.details, &parsed.relations))?;
    Ok(StoreDetailsOutcome::Ingested)
}

fn enrich_reviews(
    client: &reqwest::blocking::Client,
    repo: &Repository,
    app_id: u32,
    stats: &mut EnrichStats,
) -> Result<(), SoftEnrichError> {
    let request = ReviewSummaryRequest::summary_only(app_id);
    let raw = fetch_raw_with_retry(
        client,
        &format!("{STEAM_STORE_HOST}{}", request.path_and_query()),
        stats,
    )?;
    let proposal = parse_review_summary(&request, &raw).map_err(|error| SoftEnrichError {
        category: source_error_category(&error),
        message: error.to_string(),
    })?;
    persist_with_retry(|| repo.ingest_review(&proposal))?;
    Ok(())
}

fn enrich_popular_reviews(
    client: &reqwest::blocking::Client,
    repo: &Repository,
    app_id: u32,
    stats: &mut EnrichStats,
) -> Result<(), SoftEnrichError> {
    let request = ReviewSummaryRequest::popular_schinese(app_id);
    let raw = fetch_raw_with_retry(
        client,
        &format!("{STEAM_STORE_HOST}{}", request.path_and_query()),
        stats,
    )?;
    let proposal = parse_popular_reviews(&request, &raw).map_err(|error| SoftEnrichError {
        category: source_error_category(&error),
        message: error.to_string(),
    })?;
    persist_with_retry(|| repo.ingest_popular_reviews(&proposal))?;
    Ok(())
}

fn enrich_ccu(
    client: &reqwest::blocking::Client,
    repo: &Repository,
    app_id: u32,
    stats: &mut EnrichStats,
) -> Result<(), SoftEnrichError> {
    let request = CcuRequest::new(app_id);
    let raw = match fetch_raw_with_retry(
        client,
        &format!("{STEAM_WEB_API_HOST}{}", request.path_and_query()),
        stats,
    ) {
        Ok(raw) => raw,
        Err(error) if error.category == "not_found" => {
            return persist_with_retry(|| repo.ingest_ccu(&http_not_found_proposal(app_id)));
        }
        Err(error) => return Err(error),
    };
    let proposal = parse_ccu(&request, &raw).map_err(|error| SoftEnrichError {
        category: source_error_category(&error),
        message: error.to_string(),
    })?;
    persist_with_retry(|| repo.ingest_ccu(&proposal))?;
    Ok(())
}

fn persist_with_retry(mut write: impl FnMut() -> StorageResult<()>) -> Result<(), SoftEnrichError> {
    let mut last_error = None;
    for attempt in 0..STORAGE_WRITE_RETRIES {
        match write() {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error.to_string());
                if attempt + 1 < STORAGE_WRITE_RETRIES {
                    thread::sleep(Duration::from_secs(1_u64 << attempt));
                }
            }
        }
    }
    Err(SoftEnrichError {
        category: "storage",
        message: last_error.unwrap_or_else(|| "unknown storage failure".into()),
    })
}

fn fetch_raw_with_retry(
    client: &reqwest::blocking::Client,
    url: &str,
    stats: &mut EnrichStats,
) -> Result<RawResponse, SoftEnrichError> {
    let mut last_error = None;
    for attempt in 0..3_u64 {
        stats.request_count = stats.request_count.saturating_add(1);
        match fetch_raw(client, url) {
            Ok(raw) => return Ok(raw),
            Err(error) => {
                let retryable = error.is_retryable();
                let category = source_error_category(&error);
                let message = error.to_string();
                last_error = Some((category, message));
                if !retryable || attempt == 2 {
                    break;
                }
                thread::sleep(enrich_retry_delay(&error, attempt));
            }
        }
    }
    let (category, message) = last_error.unwrap_or(("network", "unknown fetch failure".into()));
    Err(SoftEnrichError { category, message })
}

fn fetch_raw(client: &reqwest::blocking::Client, url: &str) -> Result<RawResponse, SourceError> {
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .map_err(|error| SourceError::Temporary {
            message: error.to_string(),
        })?;
    let status = response.status().as_u16();
    let retry_after_ms = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|seconds| seconds.saturating_mul(1_000));
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    if status == 429 {
        return Err(SourceError::RateLimited { retry_after_ms });
    }
    let mut body = Vec::new();
    response
        .take((ENRICH_RESPONSE_MAX_BYTES + 1) as u64)
        .read_to_end(&mut body)
        .map_err(|error| SourceError::Temporary {
            message: error.to_string(),
        })?;
    RawResponse::validate(status, body, content_type, ENRICH_RESPONSE_MAX_BYTES)
}

fn enrich_retry_delay(error: &SourceError, attempt: u64) -> Duration {
    let delay_ms = match error {
        SourceError::RateLimited { retry_after_ms } => retry_after_ms
            .unwrap_or(0)
            .max(ENRICH_RATE_LIMIT_COOLDOWN_MS),
        _ => (1_000_u64).saturating_mul(1_u64 << attempt.min(6)),
    };
    Duration::from_millis(delay_ms)
}

fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn err_ai(error: impl std::fmt::Display) -> String {
    format!("ai: {error}")
}

#[derive(Debug, Default)]
struct EmbedBatchStats {
    provider: String,
    model: String,
    targets: u32,
    written: u32,
    unchanged: u32,
    batches: u32,
}

async fn embed_documents_batch(
    repo: &Repository,
    limit: u32,
    batch: usize,
) -> Result<EmbedBatchStats, String> {
    let provider = embedding_provider_from_env().map_err(|e| e.to_string())?;
    if !provider.is_available() {
        return Err(
            "embedding provider is disabled; set MPGS_AI_EMBED_PROVIDER=hash|openai_compat".into(),
        );
    }
    let provider_name = provider.name().to_owned();
    // Prefer model reported by first successful embed; default label for hash-embed.
    let model_hint = std::env::var("MPGS_AI_EMBED_MODEL").unwrap_or_else(|_| {
        if provider_name.contains("hash") {
            HASH_EMBED_MODEL.into()
        } else {
            "text-embedding-3-small".into()
        }
    });

    let targets = repo
        .list_documents_missing_embedding(&provider_name, &model_hint, provider.dimensions(), limit)
        .map_err(err)?;
    // Hash provider uses the fixed versioned model name, not the external model env var.
    let targets = if targets.is_empty() && provider_name.contains("hash") {
        repo.list_documents_missing_embedding(
            &provider_name,
            HASH_EMBED_MODEL,
            provider.dimensions(),
            limit,
        )
        .map_err(err)?
    } else {
        targets
    };

    let mut stats = EmbedBatchStats {
        provider: provider_name.clone(),
        model: model_hint.clone(),
        targets: targets.len() as u32,
        ..EmbedBatchStats::default()
    };

    for chunk in targets.chunks(batch) {
        stats.batches += 1;
        let inputs: Vec<EmbeddingInput> = chunk
            .iter()
            .map(|doc| EmbeddingInput {
                id: doc.document_id.clone(),
                text: format!("{} {}", doc.title, doc.body),
            })
            .collect();
        let embeddings = provider.embed(&inputs).await.map_err(|e| e.to_string())?;
        if embeddings.len() != chunk.len() {
            return Err(format!(
                "embedding provider returned {} vectors for {} inputs",
                embeddings.len(),
                chunk.len()
            ));
        }
        for (doc, emb) in chunk.iter().zip(embeddings.iter()) {
            if stats.model == model_hint && emb.model != model_hint {
                stats.model = emb.model.clone();
            }
            let written = repo
                .put_embedding(&PutEmbedding {
                    document_id: doc.document_id.clone(),
                    provider: provider_name.clone(),
                    model: emb.model.clone(),
                    dimensions: emb.dimensions,
                    vector_blob: encode_f32_le(&emb.vector),
                    is_l2_normalized: true,
                    content_hash: doc.content_hash.clone(),
                })
                .map_err(err)?;
            if written {
                stats.written += 1;
            } else {
                stats.unchanged += 1;
            }
        }
    }
    Ok(stats)
}

fn configured_store_locale() -> Result<(String, String), String> {
    let country =
        env::var("MPGS_STEAM_COUNTRY").unwrap_or_else(|_| DEFAULT_STORE_COUNTRY.to_owned());
    let language =
        env::var("MPGS_STEAM_LANGUAGE").unwrap_or_else(|_| DEFAULT_STORE_LANGUAGE.to_owned());
    let request = StoreDetailsRequest::with_locale(1, country, language).map_err(err)?;
    Ok((request.country_code, request.language))
}

fn usage() -> &'static str {
    "mpgs-dbtool <command> [args]\n\n\
     Commands:\n\
       migrate <db-path>\n\
       integrity <db-path>\n\
       m3-audit <db-path>\n\
       m7-data-audit <db-path> [--allow-upcoming-shortfall=<reason>]\n\
       sync-retrieval <db-path> [limit=5000] [after_app_id=0]\n\
       extract-offline-features <db-path> [limit=5000] [after_app_id=0]\n\
       materialize-store-profiles <db-path>\n\
       repair-empty-availability <db-path>\n\
       embed-documents <db-path> [limit=200] [batch=16]\n\
       collect-steam-catalog <db-path> [max-pages=1] [page-size=1000]\n\
       collect-steam-candidates <db-path> [target, default 2000]\n\
       enrich-steam-candidates <db-path> [limit, default 100]\n\
       run-steam-worker-once <db-path> [job-limit=1] [enrich-limit=100]\n\
       import-golden-profiles <db-path>\n\
       backup <db-path> <backup-path>\n\
       restore <backup-path> <dest-db-path>\n\n\
     Enrichment environment:\n\
       MPGS_STEAM_WEB_API_KEY (required by collect-steam-catalog; server-side only)\n\
       MPGS_STEAM_WORKER_ID (optional stable lease owner name)\n\
       MPGS_STEAM_COUNTRY (default cn)\n\
       MPGS_STEAM_LANGUAGE (default schinese)\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enrichment_retry_uses_a_long_rate_limit_cooldown() {
        assert_eq!(
            enrich_retry_delay(
                &SourceError::RateLimited {
                    retry_after_ms: None,
                },
                0,
            ),
            Duration::from_secs(300)
        );
        assert_eq!(
            enrich_retry_delay(
                &SourceError::RateLimited {
                    retry_after_ms: Some(600_000),
                },
                0,
            ),
            Duration::from_secs(600)
        );
        assert_eq!(
            enrich_retry_delay(
                &SourceError::Temporary {
                    message: "x".into()
                },
                1
            ),
            Duration::from_secs(2)
        );
    }

    fn app_list_request() -> AppListRequest {
        AppListRequest {
            last_appid: 730,
            if_modified_since: 1_700_000_000,
            max_results: 100,
            include_games: true,
            include_dlc: false,
            include_software: false,
            include_videos: false,
            include_hardware: false,
        }
    }

    #[test]
    fn catalog_arguments_stay_within_safe_bounds() {
        assert_eq!(
            optional_catalog_page_count(None).unwrap(),
            APP_LIST_PAGES_DEFAULT
        );
        assert!(optional_catalog_page_count(Some("0".into())).is_err());
        assert!(optional_catalog_page_count(Some((APP_LIST_PAGES_MAX + 1).to_string())).is_err());
        assert_eq!(
            optional_catalog_page_size(Some(APP_LIST_MAX_RESULTS.to_string())).unwrap(),
            APP_LIST_MAX_RESULTS
        );
        assert!(optional_catalog_page_size(Some("0".into())).is_err());
        assert!(optional_catalog_page_size(Some((APP_LIST_MAX_RESULTS + 1).to_string())).is_err());
        assert_eq!(
            optional_worker_job_limit(None).unwrap(),
            WORKER_JOB_LIMIT_DEFAULT
        );
        assert!(optional_worker_job_limit(Some("0".into())).is_err());
        assert_eq!(worker_job_error_category("storage"), "network");
        assert_eq!(
            worker_job_error_category("response_too_large"),
            "parse_changed"
        );
    }

    #[test]
    fn m7_audit_requires_an_explicit_single_line_upcoming_exception_reason() {
        let mut valid =
            vec!["--allow-upcoming-shortfall=Steam schedule has fewer releases".into()].into_iter();
        assert_eq!(
            parse_m7_audit_options(&mut valid).unwrap().as_deref(),
            Some("Steam schedule has fewer releases")
        );

        let mut missing_reason = vec!["--allow-upcoming-shortfall=".into()].into_iter();
        assert!(parse_m7_audit_options(&mut missing_reason).is_err());
        let mut unknown = vec!["--unexpected".into()].into_iter();
        assert!(parse_m7_audit_options(&mut unknown).is_err());
    }

    #[test]
    fn catalog_url_uses_the_official_endpoint_and_query_key() {
        let key = "a".repeat(32);
        let url = app_list_url(&app_list_request(), &key).unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("api.steampowered.com"));
        let pairs = url.query_pairs().collect::<Vec<_>>();
        assert!(
            pairs
                .iter()
                .any(|(name, value)| name == "key" && value.as_ref() == key)
        );
        assert!(
            pairs
                .iter()
                .any(|(name, value)| name == "last_appid" && value.as_ref() == "730")
        );
        assert!(
            pairs
                .iter()
                .any(|(name, value)| name == "if_modified_since" && value.as_ref() == "1700000000")
        );
    }

    #[test]
    fn worker_marks_unsupported_steam_jobs_dead_without_network_access() {
        let db = Database::open_in_memory().unwrap();
        let repo = Repository::new(db);
        repo.migrate().unwrap();
        repo.ensure_runtime_defaults().unwrap();
        let job_id = repo
            .enqueue_job(&mpgs_storage::EnqueueJob {
                source: "steam".into(),
                task_type: "unsupported".into(),
                entity_key: "test".into(),
                priority: 1,
                due_at_ms: 0,
                idempotency_key: "dbtool-worker-unsupported".into(),
                payload_json: None,
                max_attempts: 1,
            })
            .unwrap();

        let stats = run_steam_worker_once(&repo, "test-worker", 1, 1, None).unwrap();

        assert_eq!(stats.leased, 1);
        assert_eq!(stats.completed, 0);
        assert_eq!(stats.dead, 1);
        let job = repo
            .database()
            .with_conn(|conn| mpgs_storage::jobs::get_job(conn, job_id))
            .unwrap()
            .unwrap();
        assert_eq!(job.status, "dead");
        assert_eq!(job.last_error_category.as_deref(), Some("invalid_payload"));
    }
}
