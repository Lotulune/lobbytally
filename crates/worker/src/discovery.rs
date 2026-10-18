use mpgs_steam_source::{
    APP_LIST_ADAPTER_VERSION, APP_LIST_SOURCE_NAME, AppListCursor, STORE_SEARCH_SOURCE_NAME,
    StoreSearchSort, apply_page_to_cursor,
};
use mpgs_storage::Repository;
use serde::{Deserialize, Serialize};

use crate::{BatchOptions, CANDIDATE_CURSOR, CATALOG_CURSOR, Outcome, WorkError, sources::Sources};

const RELEASED_CURSOR: &str = "steam_store_search:multiplayer:released_desc";
const UPCOMING_CURSOR: &str = "steam_store_search:multiplayer:comingsoon_released_asc";
const CANDIDATE_TARGET: i64 = 2_000;

#[derive(Default, Serialize, Deserialize)]
struct SearchCursor {
    next_start: u32,
    total_count: Option<u32>,
    target: u32,
    complete: bool,
}

#[derive(Serialize, Deserialize)]
struct DiscoveryProgress {
    job_id: i64,
    channel: usize,
    pages: u32,
    refresh_mode: bool,
    exhausted: [bool; 2],
}

pub(crate) fn catalog(
    repo: &Repository,
    options: &BatchOptions,
    sources: &mut Sources,
) -> Result<Outcome, WorkError> {
    let api_key = options.steam_api_key.as_deref().ok_or_else(|| WorkError {
        category: "auth",
        message: "MPGS_STEAM_WEB_API_KEY is not configured".into(),
        retry_after_ms: 300_000,
    })?;
    let mut cursor = load::<AppListCursor>(repo, CATALOG_CURSOR)?
        .unwrap_or_else(|| AppListCursor::new_pass(0, APP_LIST_ADAPTER_VERSION));
    if !cursor.is_in_progress() || cursor.adapter_version != APP_LIST_ADAPTER_VERSION {
        cursor = AppListCursor::new_pass(cursor.if_modified_since, APP_LIST_ADAPTER_VERSION);
    }
    for _ in 0..options.page_limit {
        let page = sources.catalog_page(repo, api_key, &cursor, options.catalog_page_size)?;
        apply_page_to_cursor(&mut cursor, &page);
        // Persist the cursor only after all page rows have been ingested. An
        // interrupted page is replayed idempotently rather than skipped.
        save(repo, CATALOG_CURSOR, APP_LIST_SOURCE_NAME, &cursor)?;
        if !cursor.is_in_progress() {
            return Ok(Outcome::Complete);
        }
    }
    Ok(Outcome::Continue)
}

pub(crate) fn candidates(
    repo: &Repository,
    options: &BatchOptions,
    sources: &mut Sources,
    job_id: i64,
    top: bool,
) -> Result<Outcome, WorkError> {
    let progress_key = if top {
        "steam_serverless:top:v1"
    } else {
        "steam_serverless:continuation:v1"
    };
    let mut progress = load::<DiscoveryProgress>(repo, progress_key)?
        .filter(|progress| progress.job_id == job_id)
        .unwrap_or(DiscoveryProgress {
            job_id,
            channel: 0,
            pages: 0,
            refresh_mode: repo
                .multiplayer_candidate_count()
                .map_err(WorkError::storage)?
                >= CANDIDATE_TARGET,
            exhausted: [false; 2],
        });
    while progress.channel < 2 {
        let channel = progress.channel;
        if !top && progress.exhausted[channel] {
            progress.channel += 1;
            progress.pages = 0;
            save(repo, progress_key, STORE_SEARCH_SOURCE_NAME, &progress)?;
            continue;
        }
        let (cursor_key, sort) = if channel == 0 {
            (RELEASED_CURSOR, StoreSearchSort::ReleasedDesc)
        } else {
            (UPCOMING_CURSOR, StoreSearchSort::ReleasedAsc)
        };
        let previous = load::<SearchCursor>(repo, cursor_key)?.unwrap_or_default();
        let start = if top || previous.complete {
            0
        } else {
            previous.next_start
        };
        let page = sources.candidate_page(repo, start, sort, options.candidate_page_size)?;
        if !top {
            save(
                repo,
                cursor_key,
                STORE_SEARCH_SOURCE_NAME,
                &SearchCursor {
                    next_start: page.next_start(),
                    total_count: Some(page.total_count),
                    target: CANDIDATE_TARGET as u32,
                    complete: page.is_complete(),
                },
            )?;
            progress.exhausted[channel] = page.is_complete();
        }
        progress.pages += 1;
        if top || page.is_complete() || progress.pages >= options.page_limit {
            progress.channel += 1;
            progress.pages = 0;
        }
        save(repo, progress_key, STORE_SEARCH_SOURCE_NAME, &progress)?;
        save_combined_cursor(repo)?;
    }
    if top
        || progress.refresh_mode
        || progress.exhausted.iter().all(|complete| *complete)
        || repo
            .multiplayer_candidate_count()
            .map_err(WorkError::storage)?
            >= CANDIDATE_TARGET
    {
        return Ok(Outcome::Complete);
    }
    // Empty-database catch-up uses one page per channel per invocation. Keep
    // the same job pending without using a failure attempt between batches.
    progress.channel = 0;
    save(repo, progress_key, STORE_SEARCH_SOURCE_NAME, &progress)?;
    Ok(Outcome::Continue)
}

fn save_combined_cursor(repo: &Repository) -> Result<(), WorkError> {
    let value = serde_json::json!({
        "released": load::<serde_json::Value>(repo, RELEASED_CURSOR)?,
        "upcoming": load::<serde_json::Value>(repo, UPCOMING_CURSOR)?,
    });
    save(repo, CANDIDATE_CURSOR, STORE_SEARCH_SOURCE_NAME, &value)
}

pub(crate) fn load<T: for<'de> Deserialize<'de>>(
    repo: &Repository,
    key: &str,
) -> Result<Option<T>, WorkError> {
    repo.source_cursor(key)
        .map_err(WorkError::storage)?
        .map(|value| {
            serde_json::from_str(&value).map_err(|error| {
                WorkError::invalid(format!("invalid persisted cursor {key}: {error}"))
            })
        })
        .transpose()
}

pub(crate) fn save(
    repo: &Repository,
    key: &str,
    source: &str,
    value: &impl Serialize,
) -> Result<(), WorkError> {
    let value = serde_json::to_value(value).map_err(WorkError::storage)?;
    repo.save_source_cursor(key, source, &value)
        .map_err(WorkError::storage)
}
