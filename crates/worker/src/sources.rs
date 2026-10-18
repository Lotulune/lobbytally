use std::io::Read;
use std::thread;
use std::time::{Duration, Instant};

use mpgs_steam_source::{
    APP_LIST_MAX_RESULTS, AppListCursor, AppListPage, AppListRequest, CcuRequest,
    DEFAULT_USER_AGENT, RawResponse, ReviewSummaryRequest, STEAM_STORE_HOST, STEAM_WEB_API_HOST,
    SourceError, StoreDetailsRequest, StoreSearchPage, StoreSearchRequest, StoreSearchSort,
    http_not_found_proposal, parse_app_list_page, parse_ccu, parse_popular_reviews,
    parse_review_summary, parse_store_details, parse_store_search_page,
};
use mpgs_storage::{MediaBackfillOutcome, MediaBackfillPolicy, Repository};

use crate::WorkError;

const REQUEST_INTERVAL: Duration = Duration::from_millis(1_100);
const MIN_REQUEST_TIME: Duration = Duration::from_millis(750);
const MAX_REQUEST_TIME: Duration = Duration::from_secs(8);
const STORE_SEARCH_MAX_BYTES: usize = 4 * 1024 * 1024;
const ENRICH_MAX_BYTES: usize = 2 * 1024 * 1024;
const RATE_LIMIT_COOLDOWN_MS: i64 = 5 * 60 * 1_000;

pub(crate) struct Sources {
    client: reqwest::blocking::Client,
    deadline: Instant,
    last_request: Option<Instant>,
    pub(crate) requests: usize,
    pub(crate) successes: usize,
}

enum StoreOutcome {
    Ingested { media_unusable: bool },
    NotFound,
}

impl Sources {
    pub(crate) fn new(deadline: Instant) -> Result<Self, WorkError> {
        request_plan(Instant::now(), deadline, None)?;
        let client = reqwest::blocking::Client::builder()
            .user_agent(DEFAULT_USER_AGENT)
            .connect_timeout(Duration::from_secs(3))
            .timeout(MAX_REQUEST_TIME)
            .redirect(reqwest::redirect::Policy::limited(3))
            .build()
            .map_err(|_| WorkError {
                category: "config",
                message: "Could not initialize the Steam HTTP client".into(),
                retry_after_ms: 60_000,
            })?;
        Ok(Self {
            client,
            deadline,
            last_request: None,
            requests: 0,
            successes: 0,
        })
    }

    pub(crate) fn catalog_page(
        &mut self,
        repo: &Repository,
        api_key: &str,
        cursor: &AppListCursor,
        page_size: u32,
    ) -> Result<AppListPage, WorkError> {
        if api_key.trim().is_empty() {
            return Err(source_error(SourceError::Config {
                message: "Steam Web API key is missing".into(),
            }));
        }
        if !(1..=APP_LIST_MAX_RESULTS).contains(&page_size) {
            return Err(source_error(SourceError::Config {
                message: format!("catalog page size must be between 1 and {APP_LIST_MAX_RESULTS}"),
            }));
        }
        let request = AppListRequest::from_cursor(cursor, page_size);
        let mut url = source_url(STEAM_WEB_API_HOST, &request.path_and_query_without_key())?;
        url.query_pairs_mut().append_pair("key", api_key);
        let raw = self.fetch_raw(&url, RawResponse::DEFAULT_MAX_BYTES)?;
        let page = parse_app_list_page(&raw).map_err(source_error)?;
        page.validate_for_request(&request).map_err(source_error)?;
        repo.ingest_app_list_page(&request, &page)
            .map_err(WorkError::storage)?;
        self.successes = self.successes.saturating_add(1);
        Ok(page)
    }

    pub(crate) fn candidate_page(
        &mut self,
        repo: &Repository,
        start: u32,
        sort: StoreSearchSort,
        page_size: u32,
    ) -> Result<StoreSearchPage, WorkError> {
        let request =
            StoreSearchRequest::with_sort(start, page_size, sort).map_err(source_error)?;
        let url = source_url(STEAM_STORE_HOST, &request.path_and_query())?;
        let raw = self.fetch_raw(&url, STORE_SEARCH_MAX_BYTES)?;
        let page = parse_store_search_page(&request, &raw).map_err(source_error)?;
        repo.ingest_store_search_page_and_queue_new_games(&page)
            .map_err(WorkError::storage)?;
        let app_ids = page
            .candidates
            .iter()
            .map(|candidate| candidate.app_id)
            .collect::<Vec<_>>();
        repo.materialize_store_category_profiles_for_apps(&app_ids)
            .map_err(WorkError::storage)?;
        self.successes = self.successes.saturating_add(1);
        Ok(page)
    }

    pub(crate) fn enrichment_stage(
        &mut self,
        repo: &Repository,
        app_id: u32,
        stage: &str,
        country: &str,
        language: &str,
        media_backfill: bool,
    ) -> Result<(), WorkError> {
        if app_id == 0 {
            return Err(source_error(SourceError::Config {
                message: "Steam app ID must be greater than zero".into(),
            }));
        }
        let fetched = match stage {
            "store_details" => {
                let request = StoreDetailsRequest::with_locale(app_id, country, language)
                    .map_err(source_error)?;
                // Do not claim a media attempt when pacing already exhausts this batch.
                if media_backfill {
                    self.wait_for_request()?;
                }
                let media_claimed = media_backfill
                    && repo
                        .begin_media_backfill_attempt(app_id)
                        .map_err(WorkError::storage)?;
                let result = self.store_details(repo, &request);
                if media_claimed {
                    let outcome = match &result {
                        Ok(StoreOutcome::Ingested { media_unusable }) => {
                            let has_media = repo
                                .app_has_media_assets(app_id)
                                .map_err(WorkError::storage)?;
                            Some(if has_media {
                                MediaBackfillOutcome::Complete
                            } else if *media_unusable {
                                MediaBackfillOutcome::Failed
                            } else {
                                MediaBackfillOutcome::NoneAvailable
                            })
                        }
                        Ok(StoreOutcome::NotFound) => Some(MediaBackfillOutcome::NoneAvailable),
                        Err(error) if error.category == "budget" => None,
                        Err(_) => Some(MediaBackfillOutcome::Failed),
                    };
                    if let Some(outcome) = outcome {
                        repo.finish_media_backfill_attempt(
                            app_id,
                            outcome,
                            media_backfill_max_attempts(),
                        )
                        .map_err(WorkError::storage)?;
                    }
                }
                result?;
                true
            }
            "english_name" => self.english_name(repo, app_id)?,
            "review_summary" => {
                let request = ReviewSummaryRequest::summary_only(app_id);
                let url = source_url(STEAM_STORE_HOST, &request.path_and_query())?;
                let raw = self.fetch_raw(&url, ENRICH_MAX_BYTES)?;
                let proposal = parse_review_summary(&request, &raw).map_err(source_error)?;
                repo.ingest_review(&proposal).map_err(WorkError::storage)?;
                true
            }
            "popular_reviews" => {
                let request = ReviewSummaryRequest::popular_schinese(app_id);
                let url = source_url(STEAM_STORE_HOST, &request.path_and_query())?;
                let raw = self.fetch_raw(&url, ENRICH_MAX_BYTES)?;
                let proposal = parse_popular_reviews(&request, &raw).map_err(source_error)?;
                repo.ingest_popular_reviews(&proposal)
                    .map_err(WorkError::storage)?;
                true
            }
            "ccu" => {
                let request = CcuRequest::new(app_id);
                let url = source_url(STEAM_WEB_API_HOST, &request.path_and_query())?;
                let proposal = match self.fetch_raw(&url, ENRICH_MAX_BYTES) {
                    Ok(raw) => parse_ccu(&request, &raw).map_err(source_error)?,
                    Err(error) if error.category == "not_found" => http_not_found_proposal(app_id),
                    Err(error) => return Err(error),
                };
                repo.ingest_ccu(&proposal).map_err(WorkError::storage)?;
                true
            }
            _ => {
                return Err(source_error(SourceError::Config {
                    message: format!("unsupported enrichment stage: {stage}"),
                }));
            }
        };
        if fetched {
            self.successes = self.successes.saturating_add(1);
        }
        Ok(())
    }

    fn store_details(
        &mut self,
        repo: &Repository,
        request: &StoreDetailsRequest,
    ) -> Result<StoreOutcome, WorkError> {
        let url = source_url(STEAM_STORE_HOST, &request.path_and_query())?;
        let raw = self.fetch_raw(&url, ENRICH_MAX_BYTES)?;
        let parsed = match parse_store_details(request, &raw) {
            Ok(parsed) => parsed,
            Err(SourceError::NotFound { .. }) => {
                repo.record_store_details_not_found(
                    request.app_id,
                    &request.country_code,
                    &request.language,
                )
                .map_err(WorkError::storage)?;
                return Ok(StoreOutcome::NotFound);
            }
            Err(error) => return Err(source_error(error)),
        };
        let media_unusable = (parsed.details.screenshots.is_none()
            && parsed.media_stats.screenshots_rejected > 0)
            || (parsed.details.movies.is_none() && parsed.media_stats.movies_rejected > 0);
        repo.ingest_store_details(&parsed.details, &parsed.relations)
            .map_err(WorkError::storage)?;
        Ok(StoreOutcome::Ingested { media_unusable })
    }

    fn english_name(&mut self, repo: &Repository, app_id: u32) -> Result<bool, WorkError> {
        if repo
            .app_has_localization_name(app_id, "english")
            .map_err(WorkError::storage)?
            || repo
                .has_store_detail_refresh(app_id, "US", "english")
                .map_err(WorkError::storage)?
        {
            return Ok(false);
        }
        let request =
            StoreDetailsRequest::with_locale(app_id, "US", "english").map_err(source_error)?;
        let url = source_url(STEAM_STORE_HOST, &request.path_and_query())?;
        let raw = self.fetch_raw(&url, ENRICH_MAX_BYTES)?;
        let parsed = match parse_store_details(&request, &raw) {
            Ok(parsed) => parsed,
            Err(SourceError::NotFound { .. }) => {
                repo.record_store_details_not_found(app_id, "US", "english")
                    .map_err(WorkError::storage)?;
                return Ok(true);
            }
            Err(error) => return Err(source_error(error)),
        };
        let name = require_english_name(parsed.details.name.as_deref())?;
        repo.upsert_app_localization(
            app_id,
            "english",
            Some(name),
            parsed.details.short_description.as_deref(),
            "steam_store_appdetails_en",
        )
        .map_err(WorkError::storage)?;
        repo.record_store_details_succeeded_locale(
            app_id,
            "US",
            "english",
            "steam_store_appdetails_en",
        )
        .map_err(WorkError::storage)?;
        Ok(true)
    }

    fn wait_for_request(&self) -> Result<Duration, WorkError> {
        let (wait, _) = request_plan(Instant::now(), self.deadline, self.last_request)?;
        if !wait.is_zero() {
            thread::sleep(wait);
        }
        // Sleeping may overshoot; always recompute the actual remaining timeout.
        let (_, timeout) = request_plan(Instant::now(), self.deadline, self.last_request)?;
        Ok(timeout)
    }

    fn fetch_raw(
        &mut self,
        url: &reqwest::Url,
        max_bytes: usize,
    ) -> Result<RawResponse, WorkError> {
        for attempt in 0..2 {
            let timeout = self.wait_for_request()?;
            self.last_request = Some(Instant::now());
            self.requests = self.requests.saturating_add(1);
            match self.fetch_once(url, max_bytes, timeout) {
                Ok(raw) => return Ok(raw),
                Err(error) => {
                    // A rate limit persists its cooldown; it never sleeps inside a function.
                    if attempt == 1
                        || matches!(error, SourceError::RateLimited { .. })
                        || !error.is_retryable()
                    {
                        return Err(source_error(error));
                    }
                }
            }
        }
        unreachable!("the final request attempt always returns")
    }

    fn fetch_once(
        &self,
        url: &reqwest::Url,
        max_bytes: usize,
        timeout: Duration,
    ) -> Result<RawResponse, SourceError> {
        let response = self
            .client
            .get(url.clone())
            .header(reqwest::header::ACCEPT, "application/json")
            .timeout(timeout)
            .send()
            // reqwest errors can include the catalog API key in the request URL.
            .map_err(|error| SourceError::Temporary {
                message: if error.is_timeout() {
                    "Steam request timed out"
                } else if error.is_connect() {
                    "Steam connection failed"
                } else {
                    "Steam request failed"
                }
                .into(),
            })?;
        let status = response.status().as_u16();
        if status == 429 {
            let retry_after_ms = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u64>().ok())
                .map(|seconds| seconds.saturating_mul(1_000));
            return Err(SourceError::RateLimited { retry_after_ms });
        }
        if !(200..300).contains(&status) {
            return Err(SourceError::HttpStatus { status });
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let mut body = Vec::new();
        response
            .take((max_bytes + 1) as u64)
            .read_to_end(&mut body)
            .map_err(|_| SourceError::Temporary {
                message: "Steam response read failed".into(),
            })?;
        RawResponse::validate(status, body, content_type, max_bytes)
    }
}

fn request_plan(
    now: Instant,
    deadline: Instant,
    last_request: Option<Instant>,
) -> Result<(Duration, Duration), WorkError> {
    let next_request = last_request.map_or(now, |last| (last + REQUEST_INTERVAL).max(now));
    let remaining = deadline
        .checked_duration_since(next_request)
        .filter(|remaining| *remaining >= MIN_REQUEST_TIME)
        .ok_or_else(WorkError::budget)?;
    Ok((
        next_request.duration_since(now),
        remaining.min(MAX_REQUEST_TIME),
    ))
}

fn source_url(host: &str, path: &str) -> Result<reqwest::Url, WorkError> {
    reqwest::Url::parse(&format!("{host}{path}")).map_err(|_| {
        source_error(SourceError::Config {
            message: "Steam endpoint configuration is invalid".into(),
        })
    })
}

fn source_error(error: SourceError) -> WorkError {
    let category = match &error {
        SourceError::RateLimited { .. } => "rate_limited",
        SourceError::HttpStatus { status: 401 | 403 } => "auth",
        SourceError::NotFound { .. } | SourceError::HttpStatus { status: 404 } => "not_found",
        SourceError::JsonParse { .. }
        | SourceError::InvalidStructure { .. }
        | SourceError::InvalidUtf8 => "parse_changed",
        SourceError::Config { .. } => "config",
        SourceError::Permanent { .. } | SourceError::ResponseTooLarge { .. } => "invalid_payload",
        SourceError::HttpStatus { .. } | SourceError::Temporary { .. } => "network",
    };
    let retry_after_ms = match &error {
        SourceError::RateLimited { retry_after_ms } => {
            retry_after_ms.unwrap_or(0).min(i64::MAX as u64) as i64
        }
        _ => 60_000,
    };
    WorkError {
        category,
        message: error.to_string(),
        retry_after_ms: if category == "rate_limited" {
            retry_after_ms.max(RATE_LIMIT_COOLDOWN_MS)
        } else {
            retry_after_ms
        },
    }
}

fn require_english_name(name: Option<&str>) -> Result<&str, WorkError> {
    name.map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            source_error(SourceError::InvalidStructure {
                message: "English store response is missing a non-empty name".into(),
            })
        })
}

fn media_backfill_max_attempts() -> u32 {
    std::env::var("MPGS_MEDIA_BACKFILL_MAX_ATTEMPTS")
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|value| (1..=10).contains(value))
        .unwrap_or(MediaBackfillPolicy::DEFAULT.max_attempts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_requests_without_minimum_time_before_deadline() {
        let now = Instant::now();
        for deadline in [now, now + MIN_REQUEST_TIME - Duration::from_millis(1)] {
            assert_eq!(
                request_plan(now, deadline, None).unwrap_err().category,
                "budget"
            );
        }
    }

    #[test]
    fn pacing_reserves_time_for_the_request_before_sleeping() {
        let now = Instant::now();
        let too_soon = now + REQUEST_INTERVAL + MIN_REQUEST_TIME - Duration::from_millis(1);
        assert_eq!(
            request_plan(now, too_soon, Some(now)).unwrap_err().category,
            "budget"
        );
        let (wait, timeout) =
            request_plan(now, now + REQUEST_INTERVAL + MIN_REQUEST_TIME, Some(now)).unwrap();
        assert_eq!(wait, REQUEST_INTERVAL);
        assert_eq!(timeout, MIN_REQUEST_TIME);
    }

    #[test]
    fn timeout_is_bounded_by_eight_seconds_and_remaining_time() {
        let now = Instant::now();
        assert_eq!(
            request_plan(now, now + Duration::from_secs(30), None).unwrap(),
            (Duration::ZERO, MAX_REQUEST_TIME)
        );
        assert_eq!(
            request_plan(now, now + Duration::from_secs(2), None).unwrap(),
            (Duration::ZERO, Duration::from_secs(2))
        );
        assert_eq!(
            request_plan(
                now,
                now + Duration::from_secs(2),
                Some(now - REQUEST_INTERVAL)
            )
            .unwrap()
            .0,
            Duration::ZERO
        );
    }

    #[test]
    fn rate_limit_always_preserves_at_least_five_minutes_of_cooldown() {
        for retry_after_ms in [None, Some(1_000), Some(300_000)] {
            let error = source_error(SourceError::RateLimited { retry_after_ms });
            assert_eq!(error.category, "rate_limited");
            assert_eq!(error.retry_after_ms, RATE_LIMIT_COOLDOWN_MS);
        }
        assert_eq!(
            source_error(SourceError::RateLimited {
                retry_after_ms: Some(900_000),
            })
            .retry_after_ms,
            900_000
        );
        assert_eq!(
            source_error(SourceError::RateLimited {
                retry_after_ms: Some(u64::MAX),
            })
            .retry_after_ms,
            i64::MAX
        );
    }

    #[test]
    fn missing_english_name_cannot_be_recorded_as_a_success() {
        for name in [None, Some(""), Some(" \t\n ")] {
            assert_eq!(
                require_english_name(name).unwrap_err().category,
                "parse_changed"
            );
        }
        assert_eq!(
            require_english_name(Some("  Portal 2  ")).unwrap(),
            "Portal 2"
        );
    }
}
