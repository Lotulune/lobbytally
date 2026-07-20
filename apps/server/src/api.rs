//! HTTP routes for health, public catalog/recommendation, admin, and internal jobs.

use axum::body::{Body, Bytes};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use image::{DynamicImage, ImageFormat, imageops::FilterType};
use mpgs_ai::{
    AiError, AiGateway, AiPolicy, AiRankResult, AiStatus, AiTaskType, CandidateEvidence,
    DEFAULT_ROUTE_VERSION, EmbeddingInput, EmbeddingProvider, OpenAiCompatProvider,
    RANK_PROMPT_VERSION, StructuredRequest, TaskRouter, rank_analysis_schema,
    rank_analysis_system_prompt, validate_rank_result, wrap_untrusted_data_block,
};
use mpgs_domain::{FeedSection, FeedbackType, RecommendationConfig, UserPreferences};
use mpgs_recommender::{
    ALGORITHM_VERSION, AiAdjustment, RankingInput, blend_ai, rank_feed_configured,
};
use mpgs_storage::{
    CreateOverrideRequest, EnqueueJob, Repository, StorageError,
    accounts::{AiMode, LoginAccount, PreferenceChoice, PutAiSettings, RegisterAccount},
    community::{CommunityFilters, CommunitySort},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::{OpenApi, ToSchema};

use crate::ai_limits::AccountAiLimiter;
use crate::cors::CorsConfig;
use crate::rate_limit::{RateLimitConfig, RateLimiter};

tokio::task_local! {
    static CURRENT_REQUEST_ID: String;
}
use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Cursor;
use std::path::{Path as FilePath, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct AppState {
    pub repo: Option<Repository>,
    pub admin_token: Option<String>,
    pub rate_limits: RateLimitConfig,
    pub cors: CorsConfig,
    pub account_ai_limits: AccountAiLimiter,
    pub ai: AiGateway,
    /// Multi-model task router for the built-in provider (M8). User custom keys
    /// continue to use a single-model [`AiGateway`] and never share this router.
    pub task_router: Arc<TaskRouter>,
    pub embedding: Arc<dyn EmbeddingProvider>,
}

const LOGIN_WINDOW: Duration = Duration::from_secs(60);
const LOGIN_ATTEMPTS_PER_ACCOUNT: u32 = 10;
const MAX_LOGIN_IDENTITIES: usize = 100_000;

#[derive(Debug, Clone, Copy)]
struct LoginAttemptCounter {
    started: Instant,
    attempts: u32,
}

/// Account-name throttle layered on top of the request middleware's IP/device
/// throttle. It deliberately returns no account-existence signal.
#[derive(Default)]
pub struct LoginAttemptLimiter {
    counters: Mutex<HashMap<String, LoginAttemptCounter>>,
}

impl LoginAttemptLimiter {
    fn allow(&self, username: &str) -> bool {
        let now = Instant::now();
        // Never retain attacker-controlled username bytes in the process-wide
        // limiter. Invalid login bodies are rejected later by storage.
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        username.trim().to_ascii_lowercase().hash(&mut hasher);
        let key = format!("{:016x}", hasher.finish());
        let mut counters = self
            .counters
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if counters.len() >= MAX_LOGIN_IDENTITIES {
            counters.retain(|_, counter| now.duration_since(counter.started) < LOGIN_WINDOW);
        }
        if counters.len() >= MAX_LOGIN_IDENTITIES {
            return false;
        }
        let counter = counters.entry(key).or_insert(LoginAttemptCounter {
            started: now,
            attempts: 0,
        });
        if now.duration_since(counter.started) >= LOGIN_WINDOW {
            *counter = LoginAttemptCounter {
                started: now,
                attempts: 0,
            };
        }
        if counter.attempts >= LOGIN_ATTEMPTS_PER_ACCOUNT {
            return false;
        }
        counter.attempts = counter.attempts.saturating_add(1);
        true
    }
}

fn login_attempt_limiter() -> &'static LoginAttemptLimiter {
    static LIMITER: OnceLock<LoginAttemptLimiter> = OnceLock::new();
    LIMITER.get_or_init(LoginAttemptLimiter::default)
}

#[derive(Debug, Serialize, ToSchema)]
struct HealthResponse {
    status: &'static str,
    service: &'static str,
    version: &'static str,
}

#[derive(Debug, Serialize, ToSchema)]
struct ReadyResponse {
    status: &'static str,
    database: &'static str,
    schema_version: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
struct MetaResponse {
    api_version: &'static str,
    service_version: &'static str,
    algorithm_version: String,
    /// SQLite schema migration version when storage is enabled.
    schema_version: Option<i64>,
    /// Compile-time git SHA from `MPGS_BUILD_GIT_SHA`, or `unknown` for local builds.
    build_git_sha: &'static str,
    /// Latest catalog / snapshot freshness marker from storage, when available.
    data_updated_at_ms: Option<i64>,
    supported_sections: Vec<&'static str>,
    ai_available: bool,
    storage_enabled: bool,
    demo_mode: bool,
}

/// Git commit stamped into release binaries via `MPGS_BUILD_GIT_SHA` (see `build.rs`).
pub fn build_git_sha() -> &'static str {
    env!("MPGS_BUILD_GIT_SHA")
}

#[derive(Debug, Serialize, ToSchema)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Debug, Serialize, ToSchema)]
struct ErrorDetail {
    code: String,
    message: String,
    request_id: Option<String>,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct SessionResponseSchema {
    access_token: String,
    refresh_token: String,
    user_id: String,
    expires_at_ms: i64,
    refresh_expires_at_ms: i64,
    account: bool,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct PartySchema {
    recommended_min: Option<u8>,
    recommended_max: Option<u8>,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct MultiplayerSummarySchema {
    dominant_mode: Option<String>,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct PlayIntentSummarySchema {
    count: u32,
    voted: bool,
    voters_preview: Option<Vec<PublicVoterSchema>>,
    omitted_count: Option<u32>,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct ScoreComponentsSchema {
    friend_fit: f64,
    section_score: f64,
    personalized_score: f64,
    final_score: f64,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct FeedItemSchema {
    app_id: u32,
    name: String,
    section: FeedSection,
    release_date: Option<String>,
    release_date_raw: Option<String>,
    release_date_precision: Option<String>,
    cover_url: Option<String>,
    cover_updated_at_ms: Option<i64>,
    total_reviews: Option<u32>,
    total_positive: Option<u32>,
    latest_ccu: Option<u32>,
    typical_ccu_7d: Option<u32>,
    score: f64,
    confidence: f64,
    party: PartySchema,
    multiplayer: MultiplayerSummarySchema,
    play_intent: PlayIntentSummarySchema,
    reasons: Vec<String>,
    cautions: Vec<String>,
    evidence_ids: Vec<String>,
    components: ScoreComponentsSchema,
    algorithm_version: String,
    hybrid_score: Option<f64>,
    ai_fit: Option<f64>,
    ai_confidence: Option<f64>,
    ai_reasons: Option<Vec<String>>,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct FeedResponseSchema {
    items: Vec<FeedItemSchema>,
    next_cursor: Option<String>,
    total: usize,
    limit: i64,
    offset: usize,
    page: usize,
    total_pages: usize,
    snapshot_at_ms: i64,
    algorithm_version: String,
    data_updated_at_ms: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
struct NaturalLanguageRequest {
    query: String,
    limit: Option<i64>,
    custom_ai: Option<TransientCustomAiRequest>,
}

#[derive(Debug, Deserialize, ToSchema)]
struct TransientCustomAiRequest {
    provider: String,
    base_url: String,
    model: String,
    api_key: String,
}

#[allow(dead_code)]
#[derive(Debug, Serialize, ToSchema)]
struct NaturalLanguageInterpretationSchema {
    party_size: Option<u8>,
    session_minutes_max: Option<u32>,
    coop_competitive: Option<f64>,
    self_hosting_willingness: Option<f64>,
    platforms: Vec<String>,
    demo_only: bool,
    selected_section: FeedSection,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct NaturalLanguageResponseSchema {
    query: String,
    interpreted: NaturalLanguageInterpretationSchema,
    items: Vec<FeedItemSchema>,
    ai_status: String,
    ai_provider: String,
    ai_latency_ms: u64,
    fallback_reason: Option<String>,
    ai_summary: Option<String>,
    ai_summary_evidence_ids: Vec<String>,
    algorithm_version: String,
    data_updated_at_ms: i64,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct CalendarItemSchema {
    app_id: u32,
    app_type: String,
    canonical_name: String,
    release_state: String,
    release_date: Option<String>,
    release_date_raw: Option<String>,
    release_date_precision: Option<String>,
    is_early_access: Option<bool>,
    current_data_confidence: Option<f64>,
    review_total: Option<u32>,
    early_data: bool,
    source_modified_at_ms: Option<i64>,
    created_at_ms: i64,
    updated_at_ms: i64,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct CalendarResponseSchema {
    dated_items: Vec<CalendarItemSchema>,
    undated_items: Vec<CalendarItemSchema>,
    data_updated_at_ms: i64,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct SearchItemSchema {
    app_id: u32,
    name: String,
    release_state: String,
    release_date: Option<String>,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct SearchResponseSchema {
    items: Vec<SearchItemSchema>,
    algorithm_version: String,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct MultiplayerDetailSchema {
    dominant_mode: Option<String>,
    private_session: Option<bool>,
    online_coop: Option<bool>,
    self_hosted_server: Option<bool>,
    recommended_min: Option<u8>,
    recommended_max: Option<u8>,
    profile_confidence: Option<f64>,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct ReviewSummarySchema {
    total: Option<u32>,
    positive: Option<u32>,
    featured: Vec<PopularReviewSchema>,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct PopularReviewSchema {
    recommendation_id: String,
    rank: u8,
    author_name: Option<String>,
    author_profile_url: Option<String>,
    text: String,
    voted_up: bool,
    votes_up: u32,
    votes_funny: u32,
    comment_count: u32,
    playtime_forever_minutes: Option<u32>,
    playtime_at_review_minutes: Option<u32>,
    created_at_ms: i64,
    written_during_early_access: bool,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct GameAvailabilitySchema {
    platforms: Vec<String>,
    languages: Vec<String>,
    typical_session_minutes_min: Option<u32>,
    typical_session_minutes_max: Option<u32>,
    is_free: Option<bool>,
    final_price_minor: Option<i64>,
    price_currency: Option<String>,
    has_demo: bool,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct GameResponseSchema {
    app_id: u32,
    name: String,
    app_type: String,
    release_state: String,
    release_date: Option<String>,
    release_date_raw: Option<String>,
    release_date_precision: Option<String>,
    cover_url: Option<String>,
    cover_updated_at_ms: Option<i64>,
    short_description: Option<String>,
    steam_url: String,
    multiplayer: MultiplayerDetailSchema,
    play_intent: PlayIntentSummarySchema,
    reviews: ReviewSummarySchema,
    latest_ccu: Option<u32>,
    availability: GameAvailabilitySchema,
    algorithm_version: String,
    data_updated_at_ms: i64,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct EvidenceItemSchema {
    evidence_id: String,
    feature: String,
    #[schema(value_type = Object)]
    value: serde_json::Value,
    source_type: String,
    source_label: String,
    confidence: f64,
    observed_at_ms: i64,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct EvidenceResponseSchema {
    items: Vec<EvidenceItemSchema>,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct FeedbackResponseSchema {
    feedback_id: i64,
    app_id: u32,
    #[schema(rename = "type")]
    feedback_type: String,
    recommendation_run_id: Option<String>,
    created_at_ms: i64,
}

struct SecurityAddon;

impl utoipa::Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::security::{Http, HttpAuthScheme, SecurityScheme};

        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "bearer_auth",
                SecurityScheme::Http(Http::new(HttpAuthScheme::Bearer)),
            );
        }
    }
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "MPGS API",
        version = "0.2.0",
        description = "Deterministic friend-group multiplayer game recommendation API"
    ),
    paths(
        health_live,
        health_ready,
        meta,
        create_session,
        refresh_session,
        register_account,
        login_account,
        refresh_account,
        logout_account,
        logout_all_accounts,
        change_account_password,
        get_me,
        patch_me,
        delete_me,
        put_avatar,
        delete_avatar,
        get_ai_settings,
        put_ai_settings,
        test_ai_settings,
        delete_custom_ai_key,
        get_community_play_intents,
        get_preferences,
        put_preferences,
        get_feed,
        natural_language_recommendations,
        get_calendar,
        search_games,
        get_game,
        get_evidence,
        post_feedback,
        undo_feedback,
        set_play_intent
    ),
    components(schemas(
        HealthResponse,
        ReadyResponse,
        MetaResponse,
        ErrorBody,
        ErrorDetail,
        SessionResponseSchema,
        RefreshSessionBody,
        RegisterAccountBody,
        LoginAccountBody,
        RefreshAccountBody,
        ChangePasswordBody,
        PatchMeBody,
        AccountProfileSchema,
        AiSettingsBody,
        AiSettingsSchema,
        CommunityResponseSchema,
        CommunityItemSchema,
        CommunityPlayIntentSchema,
        PublicVoterSchema,
        UserPreferences,
        FeedSection,
        FeedItemSchema,
        FeedResponseSchema,
        NaturalLanguageRequest,
        NaturalLanguageInterpretationSchema,
        NaturalLanguageResponseSchema,
        PartySchema,
        MultiplayerSummarySchema,
        PlayIntentSummarySchema,
        ScoreComponentsSchema,
        CalendarItemSchema,
        CalendarResponseSchema,
        SearchItemSchema,
        SearchResponseSchema,
        GameResponseSchema,
        MultiplayerDetailSchema,
        ReviewSummarySchema,
        PopularReviewSchema,
        GameAvailabilitySchema,
        EvidenceItemSchema,
        EvidenceResponseSchema,
        FeedbackBody,
        FeedbackResponseSchema,
        PlayIntentBody,
        PlayIntentResponseSchema
    )),
    modifiers(&SecurityAddon),
    tags(
        (name = "health", description = "Liveness and readiness"),
        (name = "session", description = "Anonymous sessions"),
        (name = "preferences", description = "User recommendation preferences"),
        (name = "recommendations", description = "Deterministic recommendation feeds"),
        (name = "catalog", description = "Catalog, calendar, and evidence"),
        (name = "feedback", description = "Recommendation feedback"),
        (name = "public", description = "Public service metadata")
    )
)]
struct ApiDoc;

async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

pub fn build_router(state: AppState) -> Router {
    let rate_limiter = Arc::new(RateLimiter::new(state.rate_limits.clone()));
    let cors_config = Arc::new(state.cors.clone());
    let state = Arc::new(state);
    Router::new()
        .route("/health/live", get(health_live))
        .route("/health/ready", get(health_ready))
        .route("/openapi.json", get(openapi_json))
        .route("/v1/meta", get(meta))
        .route("/v1/session/anonymous", post(create_session))
        .route("/v1/session/refresh", post(refresh_session))
        .route("/v1/auth/register", post(register_account))
        .route("/v1/auth/login", post(login_account))
        .route("/v1/auth/refresh", post(refresh_account))
        .route("/v1/auth/logout", post(logout_account))
        .route("/v1/auth/logout-all", post(logout_all_accounts))
        .route("/v1/auth/password", put(change_account_password))
        .route("/v1/me", get(get_me).patch(patch_me).delete(delete_me))
        .route("/v1/me/avatar", put(put_avatar).delete(delete_avatar))
        .route(
            "/v1/me/ai-settings",
            get(get_ai_settings).put(put_ai_settings),
        )
        .route("/v1/me/ai-settings/test", post(test_ai_settings))
        .route(
            "/v1/me/ai-settings/custom-key",
            delete(delete_custom_ai_key),
        )
        .route("/v1/avatars/{avatar_id}", get(get_avatar))
        .route(
            "/v1/community/play-intents",
            get(get_community_play_intents),
        )
        .route("/v1/preferences", get(get_preferences).put(put_preferences))
        .route("/v1/feeds/{section}", get(get_feed))
        .route(
            "/v1/recommendations/natural-language",
            post(natural_language_recommendations),
        )
        .route("/v1/ai/search", post(ai_search))
        .route("/v1/ai/analyses/{analysis_id}", get(get_ai_analysis))
        .route("/v1/ai/compare", post(ai_compare))
        .route("/v1/games/{app_id}/ai-summary", get(get_game_ai_summary))
        .route("/v1/calendar", get(get_calendar))
        .route("/v1/search", get(search_games))
        .route("/v1/games/{app_id}", get(get_game))
        .route("/v1/games/{app_id}/evidence", get(get_evidence))
        .route("/v1/games/{app_id}/play-intent", post(set_play_intent))
        .route("/v1/feedback", post(post_feedback))
        .route("/v1/feedback/{feedback_id}/undo", post(undo_feedback))
        .route("/admin/v1/games/{app_id}/overrides", post(create_override))
        .route("/admin/v1/overrides/{id}/revoke", post(revoke_override))
        .route(
            "/admin/v1/accounts/{user_id}/avatar/block",
            post(block_account_avatar).delete(unblock_account_avatar),
        )
        .route("/admin/v1/games/{app_id}/debug", get(game_debug))
        .route("/admin/v1/data-status", get(data_status))
        .route("/internal/v1/jobs/enqueue", post(enqueue_job))
        .route("/internal/v1/jobs/lease", post(lease_jobs))
        .route("/internal/v1/jobs/{job_id}/complete", post(complete_job))
        .route("/internal/v1/jobs/{job_id}/fail", post(fail_job))
        // Avatar uploads are capped and decoded separately; every smaller JSON
        // endpoint still validates its own bounded DTO.
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .layer(middleware::from_fn_with_state(
            rate_limiter,
            crate::rate_limit::middleware,
        ))
        .layer(middleware::from_fn(request_id_middleware))
        .layer(middleware::from_fn_with_state(
            cors_config,
            crate::cors::middleware,
        ))
        .with_state(state)
}

async fn request_id_middleware(mut req: axum::http::Request<Body>, next: Next) -> Response {
    let request_id = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .filter(|value| valid_request_id(value))
        .map(str::to_owned)
        .unwrap_or_else(new_request_id);
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        req.headers_mut().insert("x-request-id", value);
    }
    let mut response = CURRENT_REQUEST_ID
        .scope(request_id.clone(), next.run(req))
        .await;
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert("x-request-id", value);
    }
    response
}

fn new_request_id() -> String {
    static FALLBACK_COUNTER: AtomicU64 = AtomicU64::new(1);
    let mut bytes = [0_u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        let counter = FALLBACK_COUNTER.fetch_add(1, Ordering::Relaxed);
        return format!("req-fallback-{}-{counter}", std::process::id());
    }
    let suffix: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("req-{suffix}")
}

fn valid_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

#[utoipa::path(
    get,
    path = "/health/live",
    responses((status = 200, description = "Process is alive", body = HealthResponse)),
    tag = "health"
)]
async fn health_live() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        service: "mpgs-server",
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[utoipa::path(
    get,
    path = "/health/ready",
    responses(
        (status = 200, description = "Service is ready", body = ReadyResponse),
        (status = 503, description = "Storage or catalog is not ready", body = ReadyResponse)
    ),
    tag = "health"
)]
async fn health_ready(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match &state.repo {
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ReadyResponse {
                status: "not_ready",
                database: "disabled",
                schema_version: None,
            }),
        )
            .into_response(),
        Some(repo) => match storage_result(repo, |repo| {
            repo.readiness_check()?;
            repo.database().schema_version()
        })
        .await
        {
            Ok(schema_version) => (
                StatusCode::OK,
                Json(ReadyResponse {
                    status: "ready",
                    database: "ok",
                    schema_version: Some(schema_version),
                }),
            )
                .into_response(),
            Err(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ReadyResponse {
                    status: "not_ready",
                    database: "unavailable",
                    schema_version: None,
                }),
            )
                .into_response(),
        },
    }
}

#[utoipa::path(
    get,
    path = "/v1/meta",
    responses(
        (status = 200, body = MetaResponse),
        (status = 304, description = "Not modified")
    ),
    tag = "public"
)]
async fn meta(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    let (algorithm_version, schema_version, data_updated_at_ms) = match state.repo.as_ref() {
        Some(repo) => {
            match storage_result(repo, |repo| {
                let algorithm = repo.active_algorithm_config()?.version;
                let schema = repo.database().schema_version()?;
                let data_updated = repo.data_updated_at_ms().ok();
                Ok((algorithm, Some(schema), data_updated))
            })
            .await
            {
                Ok(values) => values,
                Err(error) => return map_storage_error(error, None),
            }
        }
        None => (ALGORITHM_VERSION.to_owned(), None, None),
    };
    let body = MetaResponse {
        api_version: "v1",
        service_version: env!("CARGO_PKG_VERSION"),
        algorithm_version,
        schema_version,
        build_git_sha: build_git_sha(),
        data_updated_at_ms,
        supported_sections: FeedSection::ALL
            .into_iter()
            .map(FeedSection::as_str)
            .collect(),
        ai_available: state.ai.is_available(),
        storage_enabled: state.repo.is_some(),
        demo_mode: std::env::var("MPGS_SEED_DEMO").ok().is_some_and(|value| {
            matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes")
        }),
    };
    let etag = weak_etag(&format!(
        "{}:{}:{}:{}:{}:{}:{}",
        body.service_version,
        body.algorithm_version,
        body.schema_version.unwrap_or(-1),
        body.build_git_sha,
        body.data_updated_at_ms.unwrap_or(0),
        body.storage_enabled,
        body.demo_mode
    ));
    if_none_match_ok(&headers, &etag)
        .unwrap_or_else(|| ([(header::ETAG, etag)], Json(body)).into_response())
}

#[utoipa::path(
    post,
    path = "/v1/session/anonymous",
    responses(
        (status = 201, body = SessionResponseSchema),
        (status = 503, body = ErrorBody)
    ),
    tag = "session"
)]
async fn create_session(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    match storage_result(repo, |repo| repo.create_anonymous_session()).await {
        Ok(session) => (StatusCode::CREATED, Json(session_json(&session))).into_response(),
        Err(error) => map_storage_error(error, None),
    }
}

#[derive(Debug, Deserialize, ToSchema)]
struct RefreshSessionBody {
    refresh_token: String,
}

#[utoipa::path(
    post,
    path = "/v1/session/refresh",
    request_body = RefreshSessionBody,
    responses(
        (status = 200, body = SessionResponseSchema),
        (status = 401, body = ErrorBody)
    ),
    tag = "session"
)]
async fn refresh_session(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RefreshSessionBody>,
) -> impl IntoResponse {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let refresh_token = body.refresh_token;
    match storage_result(repo, move |repo| {
        repo.refresh_anonymous_session(&refresh_token)
    })
    .await
    {
        Ok(session) => (StatusCode::OK, Json(session_json(&session))).into_response(),
        Err(StorageError::NotFound { .. }) => error_response(
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
            "invalid or expired refresh token",
            None,
        ),
        Err(error) => map_storage_error(error, None),
    }
}

fn session_json(session: &mpgs_storage::users::SessionTokens) -> serde_json::Value {
    json!({
        "access_token": session.access_token,
        "refresh_token": session.refresh_token,
        "user_id": session.user_id,
        "expires_at_ms": session.expires_at_ms,
        "refresh_expires_at_ms": session.refresh_expires_at_ms,
        "account": false,
    })
}

fn account_session_json(
    session: &mpgs_storage::accounts::AccountSessionTokens,
) -> serde_json::Value {
    json!({
        "access_token": session.access_token,
        "refresh_token": session.refresh_token,
        "user_id": session.user_id,
        "expires_at_ms": session.expires_at_ms,
        "refresh_expires_at_ms": session.refresh_expires_at_ms,
        "account": true,
    })
}

#[derive(Debug, Deserialize, ToSchema)]
struct RegisterAccountBody {
    username: String,
    display_name: String,
    password: String,
    device_label: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
struct LoginAccountBody {
    username: String,
    password: String,
    device_label: Option<String>,
    /// Required only when both the anonymous and cloud profiles contain
    /// non-default preferences. The UI makes this a deliberate choice.
    merge_preference: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
struct RefreshAccountBody {
    refresh_token: String,
}

#[derive(Debug, Deserialize, ToSchema)]
struct ChangePasswordBody {
    old_password: String,
    new_password: String,
}

#[derive(Debug, Deserialize, ToSchema)]
struct PatchMeBody {
    display_name: String,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct AccountProfileSchema {
    username: String,
    display_name: String,
    avatar_url: String,
    avatar_version: u32,
}

#[utoipa::path(
    post,
    path = "/v1/auth/register",
    request_body = RegisterAccountBody,
    responses((status = 201, body = SessionResponseSchema), (status = 409, body = ErrorBody)),
    tag = "account"
)]
async fn register_account(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<RegisterAccountBody>,
) -> Response {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let anonymous_user_id = optional_anonymous_user_id(repo, &headers).await;
    let input = RegisterAccount {
        username: body.username,
        display_name: body.display_name,
        password: body.password,
        device_label: body
            .device_label
            .unwrap_or_else(|| "MPGS client".to_owned()),
    };
    match storage_result(repo, move |repo| {
        repo.register_account(&input, anonymous_user_id.as_deref())
    })
    .await
    {
        Ok(session) => (StatusCode::CREATED, Json(account_session_json(&session))).into_response(),
        Err(StorageError::Conflict { .. }) => error_response(
            StatusCode::CONFLICT,
            "account_conflict",
            "unable to create account with those details",
            None,
        ),
        Err(error) => map_storage_error(error, None),
    }
}

#[utoipa::path(
    post,
    path = "/v1/auth/login",
    request_body = LoginAccountBody,
    responses((status = 200, body = SessionResponseSchema), (status = 401, body = ErrorBody)),
    tag = "account"
)]
async fn login_account(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<LoginAccountBody>,
) -> Response {
    if !login_attempt_limiter().allow(&body.username) {
        return error_response(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "too many login attempts; try again later",
            None,
        );
    }
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let preference_choice = match body.merge_preference.as_deref() {
        None => None,
        Some("anonymous") => Some(PreferenceChoice::Anonymous),
        Some("account") => Some(PreferenceChoice::Account),
        Some(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "merge_preference must be anonymous or account",
                None,
            );
        }
    };
    let anonymous_user_id = optional_anonymous_user_id(repo, &headers).await;
    let input = LoginAccount {
        username: body.username,
        password: body.password,
        device_label: body
            .device_label
            .unwrap_or_else(|| "MPGS client".to_owned()),
        preference_choice,
    };
    match storage_result(repo, move |repo| {
        repo.login_account(&input, anonymous_user_id.as_deref())
    })
    .await
    {
        Ok(session) => (StatusCode::OK, Json(account_session_json(&session))).into_response(),
        Err(StorageError::NotFound { .. }) => error_response(
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
            "invalid username or password",
            None,
        ),
        Err(StorageError::Conflict { message })
            if message == "merge_preference_choice_required" =>
        {
            error_response(
                StatusCode::CONFLICT,
                "merge_choice_required",
                "choose which preferences to keep before signing in",
                None,
            )
        }
        Err(error) => map_storage_error(error, None),
    }
}

#[utoipa::path(
    post,
    path = "/v1/auth/refresh",
    request_body = RefreshAccountBody,
    responses((status = 200, body = SessionResponseSchema), (status = 401, body = ErrorBody)),
    tag = "account"
)]
async fn refresh_account(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RefreshAccountBody>,
) -> Response {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    match storage_result(repo, move |repo| {
        repo.refresh_account_session(&body.refresh_token)
    })
    .await
    {
        Ok(session) => (StatusCode::OK, Json(account_session_json(&session))).into_response(),
        Err(StorageError::NotFound { .. }) => error_response(
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
            "invalid or expired refresh token",
            None,
        ),
        Err(error) => map_storage_error(error, None),
    }
}

#[utoipa::path(
    post,
    path = "/v1/auth/logout",
    responses((status = 204), (status = 401, body = ErrorBody)),
    security(("bearer_auth" = [])),
    tag = "account"
)]
async fn logout_account(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let account = match require_account(repo, &headers).await {
        Ok(account) => account,
        Err(response) => return *response,
    };
    match storage_result(repo, move |repo| {
        repo.revoke_current_account_session(&account.access_token)
    })
    .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => map_account_error(error),
    }
}

#[utoipa::path(
    post,
    path = "/v1/auth/logout-all",
    responses((status = 204), (status = 401, body = ErrorBody)),
    security(("bearer_auth" = [])),
    tag = "account"
)]
async fn logout_all_accounts(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let account = match require_account(repo, &headers).await {
        Ok(account) => account,
        Err(response) => return *response,
    };
    match storage_result(repo, move |repo| {
        repo.revoke_all_account_sessions(&account.user_id)
    })
    .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => map_account_error(error),
    }
}

#[utoipa::path(
    put,
    path = "/v1/auth/password",
    request_body = ChangePasswordBody,
    responses((status = 204), (status = 401, body = ErrorBody)),
    security(("bearer_auth" = [])),
    tag = "account"
)]
async fn change_account_password(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ChangePasswordBody>,
) -> Response {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let account = match require_account(repo, &headers).await {
        Ok(account) => account,
        Err(response) => return *response,
    };
    match storage_result(repo, move |repo| {
        repo.change_account_password(
            &account.user_id,
            &account.access_token,
            &body.old_password,
            &body.new_password,
        )
    })
    .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => map_account_error(error),
    }
}

#[utoipa::path(
    get,
    path = "/v1/me",
    responses((status = 200, body = AccountProfileSchema), (status = 401, body = ErrorBody)),
    security(("bearer_auth" = [])),
    tag = "account"
)]
async fn get_me(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let account = match require_account(repo, &headers).await {
        Ok(account) => account,
        Err(response) => return *response,
    };
    match storage_result(repo, move |repo| repo.account_profile(&account.user_id)).await {
        Ok(profile) => (StatusCode::OK, Json(account_profile_json(&profile))).into_response(),
        Err(error) => map_account_error(error),
    }
}

#[utoipa::path(
    patch,
    path = "/v1/me",
    request_body = PatchMeBody,
    responses((status = 200, body = AccountProfileSchema), (status = 401, body = ErrorBody)),
    security(("bearer_auth" = [])),
    tag = "account"
)]
async fn patch_me(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PatchMeBody>,
) -> Response {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let account = match require_account(repo, &headers).await {
        Ok(account) => account,
        Err(response) => return *response,
    };
    match storage_result(repo, move |repo| {
        repo.update_account_display_name(&account.user_id, &body.display_name)
    })
    .await
    {
        Ok(profile) => (StatusCode::OK, Json(account_profile_json(&profile))).into_response(),
        Err(error) => map_account_error(error),
    }
}

#[utoipa::path(
    delete,
    path = "/v1/me",
    responses((status = 204), (status = 401, body = ErrorBody)),
    security(("bearer_auth" = [])),
    tag = "account"
)]
async fn delete_me(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let account = match require_account(repo, &headers).await {
        Ok(account) => account,
        Err(response) => return *response,
    };
    let avatar_dir = avatar_directory(repo);
    match storage_result(repo, move |repo| repo.delete_account(&account.user_id)).await {
        Ok(avatar) => {
            if let Some(avatar) = avatar {
                let _ = tokio::task::spawn_blocking(move || {
                    remove_avatar_file(&avatar_dir, &avatar.storage_key)
                })
                .await;
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => map_account_error(error),
    }
}

#[utoipa::path(
    put,
    path = "/v1/me/avatar",
    request_body = Vec<u8>,
    responses((status = 200, body = AccountProfileSchema), (status = 400, body = ErrorBody)),
    security(("bearer_auth" = [])),
    tag = "account"
)]
async fn put_avatar(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let account = match require_account(repo, &headers).await {
        Ok(account) => account,
        Err(response) => return *response,
    };
    let profile_user_id = account.user_id.clone();
    let previous_storage_key = match storage_result(repo, move |repo| {
        repo.account_profile(&profile_user_id)
            .map(|profile| profile.avatar_storage_key)
    })
    .await
    {
        Ok(storage_key) => storage_key,
        Err(error) => return map_account_error(error),
    };
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let raw = body.to_vec();
    let encoded = match tokio::task::spawn_blocking(move || {
        process_avatar_image(&raw, content_type.as_deref())
    })
    .await
    {
        Ok(Ok(image)) => image,
        Ok(Err(message)) => {
            return error_response(StatusCode::BAD_REQUEST, "invalid_avatar", &message, None);
        }
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "avatar processing failed",
                None,
            );
        }
    };
    let content_hash = hex_sha256(&encoded);
    // storage_key is UNIQUE globally. Content-hash alone collides when two
    // accounts upload the same bytes; include a sanitized user id.
    let storage_key = avatar_storage_key(&account.user_id, &content_hash);
    let avatar_dir = avatar_directory(repo);
    let write_dir = avatar_dir.clone();
    let write_key = storage_key.clone();
    if !matches!(
        tokio::task::spawn_blocking(move || write_avatar_file(&write_dir, &write_key, &encoded))
            .await,
        Ok(Ok(()))
    ) {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            "avatar storage is unavailable",
            None,
        );
    }
    let user_id = account.user_id.clone();
    let hash_for_db = content_hash.clone();
    let key_for_db = storage_key.clone();
    match storage_result(repo, move |repo| {
        repo.set_account_avatar_metadata(&user_id, &hash_for_db, &key_for_db)
            .and_then(|_| repo.account_profile(&user_id))
    })
    .await
    {
        Ok(profile) => {
            if let Some(previous_storage_key) = previous_storage_key
                && previous_storage_key != storage_key
            {
                let cleanup_dir = avatar_dir.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    remove_avatar_file(&cleanup_dir, &previous_storage_key)
                })
                .await;
            }
            (StatusCode::OK, Json(account_profile_json(&profile))).into_response()
        }
        Err(error) => {
            if previous_storage_key.as_deref() != Some(storage_key.as_str()) {
                let cleanup_dir = avatar_dir.clone();
                let cleanup_key = storage_key.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    remove_avatar_file(&cleanup_dir, &cleanup_key)
                })
                .await;
            }
            map_account_error(error)
        }
    }
}

#[utoipa::path(
    delete,
    path = "/v1/me/avatar",
    responses((status = 204), (status = 401, body = ErrorBody)),
    security(("bearer_auth" = [])),
    tag = "account"
)]
async fn delete_avatar(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let account = match require_account(repo, &headers).await {
        Ok(account) => account,
        Err(response) => return *response,
    };
    let avatar_dir = avatar_directory(repo);
    match storage_result(repo, move |repo| {
        repo.delete_account_avatar_metadata(&account.user_id)
    })
    .await
    {
        Ok(avatar) => {
            if let Some(avatar) = avatar {
                let _ = tokio::task::spawn_blocking(move || {
                    remove_avatar_file(&avatar_dir, &avatar.storage_key)
                })
                .await;
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => map_account_error(error),
    }
}

#[derive(Debug, Deserialize)]
struct AvatarModerationBody {
    operator: String,
    reason: String,
}

async fn block_account_avatar(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
    Json(body): Json<AvatarModerationBody>,
) -> Response {
    set_account_avatar_moderation(state, headers, user_id, body, true).await
}

async fn unblock_account_avatar(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
    Json(body): Json<AvatarModerationBody>,
) -> Response {
    set_account_avatar_moderation(state, headers, user_id, body, false).await
}

async fn set_account_avatar_moderation(
    state: Arc<AppState>,
    headers: HeaderMap,
    user_id: String,
    body: AvatarModerationBody,
    blocked: bool,
) -> Response {
    if let Err(response) = require_admin(&state, &headers) {
        return *response;
    }
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    match storage_result(repo, move |repo| {
        repo.set_account_avatar_moderation(&user_id, &body.operator, &body.reason, blocked)
    })
    .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => map_storage_error(error, None),
    }
}

async fn get_avatar(State(state): State<Arc<AppState>>, Path(avatar_id): Path<String>) -> Response {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let lookup_id = avatar_id.clone();
    let lookup = match storage_result(repo, move |repo| {
        repo.account_avatar_by_public_id(&lookup_id)
    })
    .await
    {
        Ok(lookup) => lookup,
        Err(StorageError::NotFound { .. }) => return StatusCode::NOT_FOUND.into_response(),
        Err(error) => return map_storage_error(error, None),
    };
    if let Some(storage_key) = lookup.storage_key {
        let avatar_dir = avatar_directory(repo);
        let media_type = lookup.media_type.unwrap_or_else(|| "image/webp".to_owned());
        match tokio::task::spawn_blocking(move || read_avatar_file(&avatar_dir, &storage_key)).await
        {
            Ok(Ok(bytes)) => {
                return ([(header::CONTENT_TYPE, media_type)], bytes).into_response();
            }
            Ok(Err(_)) | Err(_) => {
                // A missing or corrupt file degrades to the deterministic default
                // avatar without exposing storage details to callers.
            }
        }
    }
    (
        [(header::CONTENT_TYPE, "image/svg+xml; charset=utf-8")],
        default_avatar_svg(&lookup.display_name, &avatar_id),
    )
        .into_response()
}

fn account_profile_json(profile: &mpgs_storage::accounts::AccountProfile) -> serde_json::Value {
    json!({
        "username": profile.username,
        "display_name": profile.display_name,
        "avatar_url": avatar_url(&profile.avatar_public_id, profile.avatar_version),
        "avatar_version": profile.avatar_version,
    })
}

fn avatar_url(public_id: &str, version: u32) -> String {
    format!("/v1/avatars/{public_id}?v={version}")
}

fn avatar_directory(repo: &Repository) -> PathBuf {
    if let Ok(configured) = std::env::var("MPGS_AVATAR_DIR")
        && !configured.trim().is_empty()
    {
        return PathBuf::from(configured);
    }
    let db_path = repo.database().path();
    if db_path == FilePath::new(":memory:") {
        return std::env::temp_dir().join("mpgs-avatars");
    }
    db_path
        .parent()
        .unwrap_or_else(|| FilePath::new("."))
        .join("avatars")
}

fn avatar_storage_key(user_id: &str, content_hash: &str) -> String {
    let safe_user: String = user_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    format!("{safe_user}_{content_hash}.webp")
}

fn avatar_file_path(root: &FilePath, storage_key: &str) -> Result<PathBuf, String> {
    if storage_key.is_empty()
        || storage_key.len() > 160
        || !storage_key.ends_with(".webp")
        || !storage_key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err("invalid avatar storage key".to_owned());
    }
    Ok(root.join(storage_key))
}

fn write_avatar_file(root: &FilePath, storage_key: &str, bytes: &[u8]) -> Result<(), String> {
    let path = avatar_file_path(root, storage_key)?;
    fs::create_dir_all(root).map_err(|error| error.to_string())?;
    fs::write(path, bytes).map_err(|error| error.to_string())
}

fn read_avatar_file(root: &FilePath, storage_key: &str) -> Result<Vec<u8>, String> {
    let path = avatar_file_path(root, storage_key)?;
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    if bytes.len() > 256 * 1024 {
        return Err("avatar file exceeds expected processed size".to_owned());
    }
    Ok(bytes)
}

fn remove_avatar_file(root: &FilePath, storage_key: &str) -> Result<(), String> {
    let path = avatar_file_path(root, storage_key)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn sniff_avatar_image_format(raw: &[u8]) -> Option<ImageFormat> {
    if raw.len() >= 3 && raw[0] == 0xFF && raw[1] == 0xD8 && raw[2] == 0xFF {
        return Some(ImageFormat::Jpeg);
    }
    if raw.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some(ImageFormat::Png);
    }
    if raw.len() >= 12 && &raw[0..4] == b"RIFF" && &raw[8..12] == b"WEBP" {
        return Some(ImageFormat::WebP);
    }
    None
}

fn resolve_avatar_image_format(
    raw: &[u8],
    content_type: Option<&str>,
) -> Result<ImageFormat, String> {
    let declared = content_type
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .unwrap_or("");
    let from_header = match declared.to_ascii_lowercase().as_str() {
        "image/jpeg" | "image/jpg" => Some(ImageFormat::Jpeg),
        "image/png" => Some(ImageFormat::Png),
        "image/webp" => Some(ImageFormat::WebP),
        // Browsers sometimes send an empty type or generic binary for file picks.
        "" | "application/octet-stream" | "binary/octet-stream" => None,
        _ => None,
    };
    if let Some(format) = from_header {
        return Ok(format);
    }
    sniff_avatar_image_format(raw).ok_or_else(|| {
        "avatar content type must be image/jpeg, image/png, or image/webp".to_owned()
    })
}

fn process_avatar_image(raw: &[u8], content_type: Option<&str>) -> Result<Vec<u8>, String> {
    const MAX_AVATAR_BYTES: usize = 2 * 1024 * 1024;
    const MAX_AVATAR_PIXELS: u64 = 16_000_000;
    if raw.is_empty() || raw.len() > MAX_AVATAR_BYTES {
        return Err("avatar must be a non-empty JPEG, PNG, or WebP smaller than 2 MiB".to_owned());
    }
    let format = resolve_avatar_image_format(raw, content_type)?;
    let image = match image::load_from_memory_with_format(raw, format) {
        Ok(image) => image,
        Err(_) => {
            // Declared type can disagree with bytes (renamed extensions). Fall back
            // to magic-byte sniffing once before rejecting.
            let sniffed = sniff_avatar_image_format(raw)
                .ok_or_else(|| "avatar data does not match its declared image type".to_owned())?;
            if sniffed == format {
                return Err("avatar data does not match its declared image type".to_owned());
            }
            image::load_from_memory_with_format(raw, sniffed)
                .map_err(|_| "avatar data does not match its declared image type".to_owned())?
        }
    };
    if u64::from(image.width()) * u64::from(image.height()) > MAX_AVATAR_PIXELS {
        return Err("avatar dimensions are too large".to_owned());
    }
    let side = image.width().min(image.height());
    if side == 0 {
        return Err("avatar image has no pixels".to_owned());
    }
    let left = (image.width() - side) / 2;
    let top = (image.height() - side) / 2;
    let resized: DynamicImage =
        image
            .crop_imm(left, top, side, side)
            .resize_exact(128, 128, FilterType::Lanczos3);
    let mut out = Cursor::new(Vec::new());
    resized
        .write_to(&mut out, ImageFormat::WebP)
        .map_err(|_| "avatar image could not be encoded".to_owned())?;
    Ok(out.into_inner())
}

fn default_avatar_svg(display_name: &str, avatar_id: &str) -> Vec<u8> {
    let palette = [
        "#147D92", "#A23E48", "#44633F", "#875A26", "#405A8C", "#7D3D78",
    ];
    let color = palette[(hash64(avatar_id) as usize) % palette.len()];
    let initial = display_name
        .trim()
        .chars()
        .next()
        .unwrap_or('?')
        .to_string();
    let escaped = xml_escape(&initial);
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 128 128\" role=\"img\" aria-label=\"{escaped}\"><rect width=\"128\" height=\"128\" fill=\"{color}\"/><text x=\"64\" y=\"82\" text-anchor=\"middle\" font-family=\"sans-serif\" font-size=\"64\" fill=\"white\">{escaped}</text></svg>"
    )
    .into_bytes()
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\"', "&quot;")
        .replace('\'', "&apos;")
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct CommunityQuery {
    sort: Option<String>,
    limit: Option<i64>,
    cursor: Option<String>,
    release_state: Option<String>,
    demo_only: Option<bool>,
    platform: Option<String>,
    party_size: Option<u8>,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct PublicVoterSchema {
    display_name: String,
    avatar_url: String,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct CommunityPlayIntentSchema {
    count: u32,
    voted: bool,
    voters_preview: Vec<PublicVoterSchema>,
    omitted_count: u32,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct CommunityItemSchema {
    app_id: u32,
    name: String,
    release_date: Option<String>,
    release_date_raw: Option<String>,
    release_date_precision: Option<String>,
    cover_url: Option<String>,
    play_intent: CommunityPlayIntentSchema,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct CommunityResponseSchema {
    items: Vec<CommunityItemSchema>,
    next_cursor: Option<String>,
    snapshot_revision: u64,
    data_updated_at_ms: i64,
}

#[utoipa::path(
    get,
    path = "/v1/community/play-intents",
    params(CommunityQuery),
    responses((status = 200, body = CommunityResponseSchema), (status = 409, body = ErrorBody)),
    tag = "community"
)]
async fn get_community_play_intents(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<CommunityQuery>,
) -> Response {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let sort = match query.sort.as_deref().unwrap_or("trending") {
        "trending" => CommunitySort::Trending,
        "most_voted" => CommunitySort::MostVoted,
        _ => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "sort must be trending or most_voted",
                None,
            );
        }
    };
    let requested_limit = query.limit.unwrap_or(20);
    if !(1..=100).contains(&requested_limit) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "limit must be between 1 and 100",
            None,
        );
    }
    let filters = match community_filters_from_query(&query) {
        Ok(filters) => filters,
        Err(response) => return *response,
    };
    let filter_key = format!("{:x}", hash64(&filters.signature()));
    let epoch = match storage_result(repo, |repo| repo.play_intent_epoch()).await {
        Ok(epoch) => epoch,
        Err(error) => return map_storage_error(error, None),
    };
    let offset =
        match decode_community_cursor(query.cursor.as_deref(), sort, &filter_key, epoch.revision) {
            Ok(offset) => offset,
            Err(CursorError::Invalid) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    "invalid community cursor",
                    None,
                );
            }
            Err(CursorError::Stale) => {
                return error_response(
                    StatusCode::CONFLICT,
                    "cursor_stale",
                    "community cursor snapshot is stale; restart pagination",
                    None,
                );
            }
        };
    let user_id = optional_account_user_id(repo, &headers).await;
    let page_user_id = user_id.clone();
    let page_filters = filters.clone();
    let page = match storage_result(repo, move |repo| {
        repo.community_play_intents(
            page_user_id.as_deref(),
            sort,
            &page_filters,
            requested_limit as usize,
            offset,
        )
    })
    .await
    {
        Ok(page) => page,
        Err(error) => return map_storage_error(error, None),
    };
    if page.epoch.revision != epoch.revision {
        return error_response(
            StatusCode::CONFLICT,
            "cursor_stale",
            "community snapshot changed; restart pagination",
            None,
        );
    }
    let data_updated_at_ms = match storage_result(repo, |repo| repo.data_updated_at_ms()).await {
        Ok(value) => value,
        Err(error) => return map_storage_error(error, None),
    };
    let next_cursor = page.has_more.then(|| {
        encode_community_cursor(
            sort,
            &filter_key,
            page.epoch.revision,
            offset.saturating_add(requested_limit as usize),
        )
    });
    let items: Vec<_> = page
        .items
        .into_iter()
        .map(|item| {
            let preview: Vec<_> = item
                .voters_preview
                .into_iter()
                .map(|voter| {
                    json!({
                        "display_name": voter.display_name,
                        "avatar_url": avatar_url(&voter.avatar_public_id, voter.avatar_version),
                    })
                })
                .collect();
            json!({
                "app_id": item.app_id,
                "name": item.name,
                "app_type": item.app_type,
                "release_state": item.release_state,
                "release_date": item.release_date,
                "release_date_raw": item.release_date_raw,
                "release_date_precision": item.release_date_precision,
                "cover_url": item.cover_url,
                "cover_updated_at_ms": item.cover_updated_at_ms,
                "trending_count": item.trending_count,
                "play_intent": {
                    "count": item.count,
                    "voted": item.voted,
                    "voters_preview": preview,
                    "omitted_count": item.omitted_count,
                }
            })
        })
        .collect();
    let etag = weak_etag(&format!(
        "community:v1:{}:{}:{}:{}:{}:{}",
        sort.as_str(),
        filter_key,
        page.epoch.revision,
        offset,
        requested_limit,
        user_id.as_deref().unwrap_or("public")
    ));
    if let Some(response) = if_none_match_ok(&headers, &etag) {
        return response;
    }
    (
        StatusCode::OK,
        [(header::ETAG, etag)],
        Json(json!({
            "items": items,
            "next_cursor": next_cursor,
            "snapshot_revision": page.epoch.revision,
            "data_updated_at_ms": data_updated_at_ms,
        })),
    )
        .into_response()
}

fn encode_community_cursor(
    sort: CommunitySort,
    filter_key: &str,
    revision: u64,
    offset: usize,
) -> String {
    format!(
        "community:v1:{}:{filter_key}:{revision}:{offset}",
        sort.as_str()
    )
}

fn decode_community_cursor(
    cursor: Option<&str>,
    expected_sort: CommunitySort,
    expected_filter_key: &str,
    expected_revision: u64,
) -> Result<usize, CursorError> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let mut parts = cursor.split(':');
    let namespace = parts.next();
    let version = parts.next();
    let sort = parts.next();
    let filter_key = parts.next();
    let revision = parts.next().and_then(|part| part.parse::<u64>().ok());
    let offset = parts.next().and_then(|part| part.parse::<usize>().ok());
    if namespace != Some("community")
        || version != Some("v1")
        || sort != Some(expected_sort.as_str())
        || filter_key != Some(expected_filter_key)
        || revision.is_none()
        || offset.is_none()
        || parts.next().is_some()
    {
        return Err(CursorError::Invalid);
    }
    if revision != Some(expected_revision) {
        return Err(CursorError::Stale);
    }
    Ok(offset.expect("validated above"))
}

fn community_filters_from_query(query: &CommunityQuery) -> Result<CommunityFilters, Box<Response>> {
    let release_state = match query.release_state.as_deref() {
        None | Some("") => None,
        Some(value) => {
            let normalized = value.trim().to_ascii_lowercase();
            if !matches!(
                normalized.as_str(),
                "released" | "upcoming" | "coming_soon" | "retired" | "unknown"
            ) {
                return Err(Box::new(error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    "release_state is not supported",
                    None,
                )));
            }
            Some(normalized)
        }
    };
    let platform = match query.platform.as_deref() {
        None | Some("") => None,
        Some(value) => {
            let normalized = value.trim().to_ascii_lowercase();
            if !matches!(normalized.as_str(), "windows" | "macos" | "linux") {
                return Err(Box::new(error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    "platform must be windows, macos, or linux",
                    None,
                )));
            }
            Some(normalized)
        }
    };
    if let Some(party_size) = query.party_size
        && !(1..=64).contains(&party_size)
    {
        return Err(Box::new(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "party_size must be between 1 and 64",
            None,
        )));
    }
    Ok(CommunityFilters {
        release_state,
        demo_only: query.demo_only.unwrap_or(false),
        platform,
        party_size: query.party_size,
    })
}

#[derive(Debug, Deserialize, ToSchema)]
struct AiSettingsBody {
    mode: String,
    provider: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct AiSettingsSchema {
    mode: String,
    provider: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
    configured: bool,
    key_mask: Option<String>,
}

#[utoipa::path(
    get,
    path = "/v1/me/ai-settings",
    responses((status = 200, body = AiSettingsSchema), (status = 401, body = ErrorBody)),
    security(("bearer_auth" = [])),
    tag = "account"
)]
async fn get_ai_settings(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let account = match require_account(repo, &headers).await {
        Ok(account) => account,
        Err(response) => return *response,
    };
    let user_id = account.user_id.clone();
    match storage_result(repo, move |repo| repo.account_ai_settings(&user_id)).await {
        Ok(settings) => {
            let body = ai_settings_json(&settings, &state, repo, &account.user_id).await;
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(error) => map_account_error(error),
    }
}

#[utoipa::path(
    put,
    path = "/v1/me/ai-settings",
    request_body = AiSettingsBody,
    responses((status = 200, body = AiSettingsSchema), (status = 400, body = ErrorBody)),
    security(("bearer_auth" = [])),
    tag = "account"
)]
async fn put_ai_settings(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<AiSettingsBody>,
) -> Response {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let account = match require_account(repo, &headers).await {
        Ok(account) => account,
        Err(response) => return *response,
    };
    let (input, _) = match prepare_ai_settings(body, false).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let user_id = account.user_id.clone();
    match storage_result(repo, move |repo| {
        repo.put_account_ai_settings(&user_id, &input, None)
    })
    .await
    {
        Ok(settings) => {
            let body = ai_settings_json(&settings, &state, repo, &account.user_id).await;
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(error) => map_account_error(error),
    }
}

#[utoipa::path(
    post,
    path = "/v1/me/ai-settings/test",
    request_body = AiSettingsBody,
    responses((status = 204), (status = 400, body = ErrorBody)),
    security(("bearer_auth" = [])),
    tag = "account"
)]
async fn test_ai_settings(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<AiSettingsBody>,
) -> Response {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let account = match require_account(repo, &headers).await {
        Ok(account) => account,
        Err(response) => return *response,
    };
    let _ = account;
    match prepare_ai_settings(body, true).await {
        Ok((_input, _cipher)) => StatusCode::NO_CONTENT.into_response(),
        Err(response) => response,
    }
}

#[utoipa::path(
    delete,
    path = "/v1/me/ai-settings/custom-key",
    responses((status = 200, body = AiSettingsSchema), (status = 401, body = ErrorBody)),
    security(("bearer_auth" = [])),
    tag = "account"
)]
async fn delete_custom_ai_key(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let account = match require_account(repo, &headers).await {
        Ok(account) => account,
        Err(response) => return *response,
    };
    let user_id = account.user_id.clone();
    match storage_result(repo, move |repo| repo.delete_custom_ai_key(&user_id)).await {
        Ok(settings) => {
            let body = ai_settings_json(&settings, &state, repo, &account.user_id).await;
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(error) => map_account_error(error),
    }
}

async fn prepare_ai_settings(
    body: AiSettingsBody,
    test_connection: bool,
) -> Result<(PutAiSettings, Option<()>), Response> {
    let mode = AiMode::parse(&body.mode).ok_or_else(|| {
        error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "mode must be builtin, custom, or off",
            None,
        )
    })?;
    if mode != AiMode::Custom {
        return Ok((
            PutAiSettings {
                mode,
                provider: None,
                base_url: None,
                model: None,
                api_key: None,
            },
            None,
        ));
    }
    if body.provider.as_deref() != Some("openai_compat") {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "custom provider must be openai_compat",
            None,
        ));
    }
    let base_url = body.base_url.as_deref().ok_or_else(|| {
        error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "base_url is required for custom AI",
            None,
        )
    })?;
    let model = body
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            error_response(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "model is required for custom AI",
                None,
            )
        })?;
    if model.len() > 256 {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "model is too long",
            None,
        ));
    }
    let api_key = body
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if !test_connection && api_key.is_some() {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "custom AI API keys must remain in device-local storage",
            None,
        ));
    }
    let base_url = mpgs_ai::validate_custom_base_url(base_url)
        .await
        .map_err(|_| {
            error_response(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "custom AI endpoint is not allowed",
                None,
            )
        })?;
    if test_connection && api_key.is_none() {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "api_key is required for a connection test",
            None,
        ));
    }
    if test_connection && let Some(api_key) = api_key {
        mpgs_ai::test_custom_openai_connection(&base_url, api_key)
            .await
            .map_err(|_| {
                error_response(
                    StatusCode::BAD_GATEWAY,
                    "ai_connection_failed",
                    "custom AI connection test failed",
                    None,
                )
            })?;
    }
    Ok((
        PutAiSettings {
            mode,
            provider: Some("openai_compat".to_owned()),
            base_url: Some(base_url),
            model: Some(model.to_owned()),
            api_key: api_key.map(str::to_owned),
        },
        None,
    ))
}

async fn ai_settings_json(
    settings: &mpgs_storage::accounts::AiSettings,
    state: &AppState,
    repo: &Repository,
    user_id: &str,
) -> serde_json::Value {
    let usage_user_id = user_id.to_owned();
    let day_utc = chrono_like_now_ms() / 86_400_000;
    let daily_remaining = storage_result(repo, move |repo| {
        repo.account_ai_daily_usage(&usage_user_id, day_utc)
    })
    .await
    .ok()
    .map(|used| state.account_ai_limits.daily_budget().saturating_sub(used));
    json!({
        "mode": settings.mode.as_str(),
        "provider": settings.provider,
        "base_url": settings.base_url,
        "model": settings.model,
        "configured": settings.configured,
        "key_mask": settings.key_mask,
        "updated_at_ms": settings.updated_at_ms,
        "builtin": {
            "available": state.ai.is_available(),
            "model": state.ai.provider_name(),
            "daily_remaining": daily_remaining,
        }
    })
}

#[utoipa::path(
    get,
    path = "/v1/preferences",
    responses(
        (status = 200, body = UserPreferences),
        (status = 401, body = ErrorBody)
    ),
    security(("bearer_auth" = [])),
    tag = "preferences"
)]
async fn get_preferences(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let user_id = match require_user(repo, &headers).await {
        Ok(id) => id,
        Err(resp) => return *resp,
    };
    match storage_result(repo, move |repo| repo.get_preferences(&user_id)).await {
        Ok(prefs) => (StatusCode::OK, Json(prefs)).into_response(),
        Err(error) => map_storage_error(error, None),
    }
}

#[utoipa::path(
    put,
    path = "/v1/preferences",
    request_body = UserPreferences,
    responses(
        (status = 200, body = UserPreferences),
        (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody),
        (status = 409, body = ErrorBody)
    ),
    security(("bearer_auth" = [])),
    tag = "preferences"
)]
async fn put_preferences(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<UserPreferences>,
) -> impl IntoResponse {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let user_id = match require_user(repo, &headers).await {
        Ok(id) => id,
        Err(resp) => return *resp,
    };
    match storage_result(repo, move |repo| repo.put_preferences(&user_id, &body)).await {
        Ok(prefs) => (StatusCode::OK, Json(prefs)).into_response(),
        Err(error) => map_storage_error(error, None),
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct FeedQuery {
    limit: Option<i64>,
    /// 1-based page index. When set, wins over cursor for offset calculation.
    page: Option<i64>,
    cursor: Option<String>,
    party_size: Option<u8>,
    coop_competitive: Option<f64>,
    self_hosting_willingness: Option<f64>,
    platforms: Option<String>,
    languages: Option<String>,
    session_minutes_min: Option<u32>,
    session_minutes_max: Option<u32>,
    max_price_minor: Option<i64>,
    currency: Option<String>,
    demo_only: Option<bool>,
    /// Ranking override after the recommendation score: `recommended` (default),
    /// `ccu`, `reviews`, or `release_date`.
    sort: Option<String>,
    /// `desc` (default for ccu/reviews) or `asc` (default for release_date).
    order: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FeedSort {
    Recommended,
    Ccu,
    Reviews,
    ReleaseDate,
}

impl FeedSort {
    fn parse(raw: Option<&str>) -> Result<Self, ()> {
        match raw.map(str::trim).filter(|value| !value.is_empty()) {
            None | Some("recommended") | Some("score") => Ok(Self::Recommended),
            Some("ccu") | Some("players") | Some("player_count") => Ok(Self::Ccu),
            Some("reviews") | Some("review_count") => Ok(Self::Reviews),
            Some("release_date") | Some("release") | Some("date") => Ok(Self::ReleaseDate),
            _ => Err(()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Recommended => "recommended",
            Self::Ccu => "ccu",
            Self::Reviews => "reviews",
            Self::ReleaseDate => "release_date",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FeedSortOrder {
    Asc,
    Desc,
}

impl FeedSortOrder {
    fn parse(raw: Option<&str>, sort: FeedSort) -> Result<Self, ()> {
        match raw.map(str::trim).filter(|value| !value.is_empty()) {
            None => Ok(match sort {
                FeedSort::ReleaseDate => Self::Asc,
                _ => Self::Desc,
            }),
            Some("asc") | Some("ascending") => Ok(Self::Asc),
            Some("desc") | Some("descending") => Ok(Self::Desc),
            _ => Err(()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Asc => "asc",
            Self::Desc => "desc",
        }
    }
}

#[derive(Debug, Clone)]
struct FeedPresentation {
    release_date: Option<String>,
    release_date_raw: Option<String>,
    release_date_precision: Option<String>,
    cover_url: Option<String>,
    cover_updated_at_ms: Option<i64>,
    total_reviews: Option<u32>,
    total_positive: Option<u32>,
    latest_ccu: Option<u32>,
    typical_ccu_7d: Option<u32>,
}

/// Public Steam header CDN fallback when local media is missing.
fn steam_header_cover_url(app_id: u32) -> String {
    format!("https://cdn.akamai.steamstatic.com/steam/apps/{app_id}/header.jpg")
}

fn resolve_feed_cover_url(app_id: u32, cover_url: Option<String>) -> Option<String> {
    match cover_url {
        Some(url) if !url.trim().is_empty() => Some(url),
        _ => Some(steam_header_cover_url(app_id)),
    }
}

#[utoipa::path(
    get,
    path = "/v1/feeds/{section}",
    params(
        ("section" = FeedSection, Path, description = "Recommendation section"),
        FeedQuery
    ),
    responses(
        (status = 200, body = FeedResponseSchema),
        (status = 304, description = "Not modified"),
        (status = 400, body = ErrorBody),
        (status = 409, body = ErrorBody)
    ),
    tag = "recommendations"
)]
async fn get_feed(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(section): Path<String>,
    Query(query): Query<FeedQuery>,
) -> impl IntoResponse {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let active_config = match storage_result(repo, |repo| repo.active_algorithm_config()).await {
        Ok(config) => config,
        Err(error) => return map_storage_error(error, None),
    };
    let section = match FeedSection::parse(&section) {
        Some(s) => s,
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "unknown feed section",
                None,
            );
        }
    };
    let (mut prefs, user_id) = match user_context(repo, &headers).await {
        Ok(context) => context,
        Err(response) => return *response,
    };
    if let Some(party_size) = query.party_size {
        if !(1..=64).contains(&party_size) {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "party_size must be between 1 and 64",
                None,
            );
        }
        prefs.party_size = party_size;
    }
    if let Err(response) = apply_feed_overrides(&mut prefs, &query) {
        return *response;
    }
    let requested_limit = query.limit.unwrap_or(20);
    if !(1..=100).contains(&requested_limit) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "limit must be between 1 and 100",
            None,
        );
    }
    let limit = requested_limit as usize;
    let feed_sort = match FeedSort::parse(query.sort.as_deref()) {
        Ok(value) => value,
        Err(()) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "sort must be recommended, ccu, reviews, or release_date",
                None,
            );
        }
    };
    let feed_order = match FeedSortOrder::parse(query.order.as_deref(), feed_sort) {
        Ok(value) => value,
        Err(()) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "order must be asc or desc",
                None,
            );
        }
    };
    let active_feedback = match user_id {
        Some(ref user_id) => {
            let user_id = user_id.clone();
            match storage_result(repo, move |repo| repo.list_active_feedback(&user_id)).await {
                Ok(feedback) => feedback,
                Err(error) => return map_storage_error(error, None),
            }
        }
        None => Vec::new(),
    };
    let feedback_by_app: HashMap<_, _> = active_feedback
        .iter()
        .map(|feedback| (feedback.app_id, feedback.feedback_type.as_str()))
        .collect();
    let preference_context = recommendation_context(
        &prefs,
        &active_feedback,
        &active_config.version,
        &active_config.config,
    );
    let snapshot_ms = match storage_result(repo, |repo| repo.data_updated_at_ms()).await {
        Ok(value) => value,
        Err(error) => return map_storage_error(error, None),
    };
    let snapshot_user_id = user_id.clone();
    let play_intent = match storage_result(repo, move |repo| {
        repo.play_intent_feed_snapshot(snapshot_user_id.as_deref())
    })
    .await
    {
        Ok(value) => value,
        Err(error) => return map_storage_error(error, None),
    };
    let offset = if let Some(page) = query.page {
        if page < 1 {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "page must be >= 1",
                None,
            );
        }
        let page_index = usize::try_from(page - 1).unwrap_or(usize::MAX);
        page_index.saturating_mul(limit)
    } else {
        match decode_cursor(
            query.cursor.as_deref(),
            section,
            snapshot_ms,
            &preference_context,
            play_intent.epoch.revision,
        ) {
            Ok(value) => value,
            Err(CursorError::Invalid) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    "invalid feed cursor",
                    None,
                );
            }
            Err(CursorError::Stale) => {
                return error_response(
                    StatusCode::CONFLICT,
                    "cursor_stale",
                    "feed cursor snapshot is stale; restart pagination",
                    None,
                );
            }
        }
    };
    let cache_identity = user_id.as_deref().unwrap_or("public");
    // v4: feed items include cover fallbacks + review/ccu presentation fields + sort.
    let etag = weak_etag(&format!(
        "feed:v4:{}:{snapshot_ms}:{preference_context}:{offset}:{limit}:{}:pi{}:user{cache_identity}:sort{}:order{}",
        section.as_str(),
        active_config.version,
        play_intent.epoch.revision,
        feed_sort.as_str(),
        feed_order.as_str(),
    ));
    if let Some(response) = if_none_match_ok(&headers, &etag) {
        return response;
    }

    let now_ms = repo.database().now_ms();
    let today = mpgs_storage::util::day_utc_from_ms(now_ms);
    let cutoff = mpgs_storage::util::day_utc_from_ms(
        now_ms.saturating_sub(i64::from(active_config.config.recent_days) * 24 * 60 * 60 * 1000),
    );

    let cutoff_query = cutoff.clone();
    let today_query = today.clone();
    let currency = prefs.budget_currency.clone();
    let recommendation_config = active_config.config.clone();
    let candidate_limit = i64::from(active_config.config.candidate_limit);
    let candidates = match storage_result(repo, move |repo| {
        repo.list_candidates(
            section,
            &cutoff_query,
            &today_query,
            &currency,
            &recommendation_config,
            candidate_limit,
        )
    })
    .await
    {
        Ok(rows) => rows,
        Err(error) => return map_storage_error(error, None),
    };
    let mut presentation_by_app: HashMap<u32, FeedPresentation> = HashMap::new();
    let inputs: Vec<RankingInput> = candidates
        .into_iter()
        .filter_map(|row| {
            if query.demo_only == Some(true) && !row.has_demo {
                return None;
            }
            let signals = row.to_ranking_signals();
            let feedback = feedback_by_app.get(&row.app_id).copied();
            if matches!(feedback, Some("not_interested" | "party_size_mismatch")) {
                return None;
            }
            let personal_adjustment = match feedback {
                Some("like") => 0.20,
                Some("played") => 0.05,
                Some("too_competitive") if signals.multiplayer.matchmaking_core >= 0.5 => -0.30,
                Some("too_competitive" | "hosting_friction") => -0.15,
                _ => 0.0,
            };
            let availability = row.availability();
            let matches_section = mpgs_storage::query::section_matches(
                section,
                &row,
                &signals,
                &cutoff,
                &today,
                &active_config.config,
            );
            if !matches_section {
                return None;
            }
            presentation_by_app.insert(
                row.app_id,
                FeedPresentation {
                    release_date: row.release_date.clone(),
                    release_date_raw: row.release_date_raw.clone(),
                    release_date_precision: row.release_date_precision.clone(),
                    cover_url: resolve_feed_cover_url(row.app_id, row.cover_url.clone()),
                    cover_updated_at_ms: row.cover_updated_at_ms,
                    total_reviews: row.total_reviews,
                    total_positive: row.total_positive,
                    latest_ccu: row.latest_ccu,
                    typical_ccu_7d: row.typical_ccu_7d,
                },
            );
            Some(RankingInput {
                app_id: row.app_id,
                name: row.name,
                dominant_mode: row.dominant_mode,
                recommended_min: row.recommended_min,
                recommended_max: row.recommended_max,
                availability,
                signals,
                personal_adjustment,
                play_intent_count: play_intent.counts.get(&row.app_id).copied().unwrap_or(0),
            })
        })
        .collect();

    let ranked = rank_feed_configured(
        section,
        &inputs,
        &prefs,
        &active_config.config,
        &active_config.version,
    );
    let mut ranked_items = ranked.items;
    apply_feed_sort(
        &mut ranked_items,
        feed_sort,
        feed_order,
        &presentation_by_app,
    );
    let total = ranked_items.len();
    let limit_usize = limit;
    let page_number = (offset / limit_usize) + 1;
    let total_pages = if total == 0 {
        0
    } else {
        total.div_ceil(limit_usize)
    };
    let page: Vec<_> = ranked_items
        .into_iter()
        .skip(offset)
        .take(limit_usize)
        .map(|item| {
            let presentation = presentation_by_app.get(&item.app_id);
            json!({
                "app_id": item.app_id,
                "name": item.name,
                "section": section.as_str(),
                "release_date": presentation.and_then(|value| value.release_date.clone()),
                "release_date_raw": presentation.and_then(|value| value.release_date_raw.clone()),
                "release_date_precision": presentation.and_then(|value| value.release_date_precision.clone()),
                "cover_url": presentation.and_then(|value| value.cover_url.clone())
                    .or_else(|| resolve_feed_cover_url(item.app_id, None)),
                "cover_updated_at_ms": presentation.and_then(|value| value.cover_updated_at_ms),
                "total_reviews": presentation.and_then(|value| value.total_reviews),
                "total_positive": presentation.and_then(|value| value.total_positive),
                "latest_ccu": presentation.and_then(|value| value.latest_ccu),
                "typical_ccu_7d": presentation.and_then(|value| value.typical_ccu_7d),
                "score": item.score.final_score,
                "confidence": item.score.friend_fit,
                "party": {
                    "recommended_min": item.recommended_min,
                    "recommended_max": item.recommended_max,
                },
                "multiplayer": {
                    "dominant_mode": item.dominant_mode,
                },
                "play_intent": {
                    "count": play_intent.counts.get(&item.app_id).copied().unwrap_or(0),
                    "voted": play_intent.user_votes.contains(&item.app_id),
                },
                "reasons": item.explanation.reasons,
                "cautions": item.explanation.cautions,
                "evidence_ids": item.explanation.evidence_ids,
                "components": {
                    "friend_fit": item.score.friend_fit,
                    "section_score": item.score.section_score,
                    "personalized_score": item.score.personalized_score,
                    "final_score": item.score.final_score,
                },
                "algorithm_version": item.algorithm_version,
            })
        })
        .collect();

    let next_offset = offset.saturating_add(limit_usize);
    let next_cursor = if next_offset < total {
        Some(encode_cursor(
            section,
            snapshot_ms,
            &preference_context,
            play_intent.epoch.revision,
            next_offset,
        ))
    } else {
        None
    };
    let body = json!({
        "items": page,
        "next_cursor": next_cursor,
        "total": total,
        "limit": limit,
        "offset": offset,
        "page": page_number,
        "total_pages": total_pages,
        "snapshot_at_ms": snapshot_ms,
        "algorithm_version": active_config.version,
        "data_updated_at_ms": snapshot_ms,
        "sort": feed_sort.as_str(),
        "order": feed_order.as_str(),
    });
    (StatusCode::OK, [(header::ETAG, etag)], Json(body)).into_response()
}

fn apply_feed_sort(
    items: &mut [mpgs_recommender::RankedCandidate],
    sort: FeedSort,
    order: FeedSortOrder,
    presentation: &HashMap<u32, FeedPresentation>,
) {
    if matches!(sort, FeedSort::Recommended) || items.len() < 2 {
        return;
    }
    items.sort_by(|left, right| {
        let left_p = presentation.get(&left.app_id);
        let right_p = presentation.get(&right.app_id);
        let primary = match sort {
            FeedSort::Recommended => std::cmp::Ordering::Equal,
            FeedSort::Ccu => {
                let left_ccu = left_p
                    .and_then(|value| value.typical_ccu_7d.or(value.latest_ccu))
                    .unwrap_or(0);
                let right_ccu = right_p
                    .and_then(|value| value.typical_ccu_7d.or(value.latest_ccu))
                    .unwrap_or(0);
                left_ccu.cmp(&right_ccu)
            }
            FeedSort::Reviews => {
                let left_reviews = left_p.and_then(|value| value.total_reviews).unwrap_or(0);
                let right_reviews = right_p.and_then(|value| value.total_reviews).unwrap_or(0);
                left_reviews.cmp(&right_reviews)
            }
            FeedSort::ReleaseDate => {
                let left_date = left_p
                    .and_then(|value| value.release_date.as_deref())
                    .unwrap_or("");
                let right_date = right_p
                    .and_then(|value| value.release_date.as_deref())
                    .unwrap_or("");
                // Missing dates always sort last regardless of direction.
                match (left_date.is_empty(), right_date.is_empty()) {
                    (true, true) => std::cmp::Ordering::Equal,
                    (true, false) => std::cmp::Ordering::Greater,
                    (false, true) => std::cmp::Ordering::Less,
                    (false, false) => left_date.cmp(right_date),
                }
            }
        };
        let primary = match order {
            FeedSortOrder::Asc => primary,
            FeedSortOrder::Desc => primary.reverse(),
        };
        primary
            .then_with(|| {
                left.score
                    .final_score
                    .partial_cmp(&right.score.final_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .reverse()
            })
            .then_with(|| left.app_id.cmp(&right.app_id))
    });
}

#[derive(Debug, Clone, Serialize)]
struct NaturalLanguageInterpretation {
    party_size: Option<u8>,
    session_minutes_max: Option<u32>,
    coop_competitive: Option<f64>,
    self_hosting_willingness: Option<f64>,
    platforms: Vec<String>,
    demo_only: bool,
    selected_section: FeedSection,
}

#[utoipa::path(
    post,
    path = "/v1/recommendations/natural-language",
    request_body = NaturalLanguageRequest,
    responses(
        (status = 200, body = NaturalLanguageResponseSchema),
        (status = 400, body = ErrorBody),
        (status = 503, body = ErrorBody)
    ),
    tag = "recommendations"
)]
async fn natural_language_recommendations(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<NaturalLanguageRequest>,
) -> Response {
    let NaturalLanguageRequest {
        query: raw_query,
        limit,
        custom_ai,
    } = body;
    let query = raw_query.trim().to_owned();
    if query.len() < 3 || query.len() > 500 {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "query must contain between 3 and 500 characters",
            None,
        );
    }
    let output_limit = limit.unwrap_or(6);
    if !(3..=10).contains(&output_limit) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "limit must be between 3 and 10",
            None,
        );
    }

    let account_user_id = match state.repo.as_ref() {
        Some(repo) => optional_account_user_id(repo, &headers).await,
        None => None,
    };
    let interpreted = interpret_natural_language(&query);
    let feed_query = FeedQuery {
        // Rank and validate the required Top 20, then truncate the public response.
        limit: Some(20),
        page: None,
        cursor: None,
        party_size: interpreted.party_size,
        coop_competitive: interpreted.coop_competitive,
        self_hosting_willingness: interpreted.self_hosting_willingness,
        platforms: (!interpreted.platforms.is_empty()).then(|| interpreted.platforms.join(",")),
        languages: None,
        session_minutes_min: None,
        session_minutes_max: interpreted.session_minutes_max,
        max_price_minor: None,
        currency: None,
        demo_only: Some(interpreted.demo_only),
        sort: None,
        order: None,
    };
    let feed_response = get_feed(
        State(state.clone()),
        headers.clone(),
        Path(interpreted.selected_section.as_str().to_owned()),
        Query(feed_query),
    )
    .await
    .into_response();
    if feed_response.status() != StatusCode::OK {
        return feed_response;
    }

    let (_, feed_body) = feed_response.into_parts();
    let bytes = match axum::body::to_bytes(feed_body, 2 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "failed to assemble recommendation response",
                None,
            );
        }
    };
    let feed: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(feed) => feed,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "failed to decode recommendation response",
                None,
            );
        }
    };
    let mut items = feed
        .get("items")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    // Best-effort hybrid retrieval boost among the deterministic candidate set.
    if let Some(repo) = state.repo.as_ref() {
        let _ = storage_result(repo, |repo| {
            // Ensure demo/local catalogs have retrieval docs without blocking requests on large DBs.
            if repo.document_count().unwrap_or(0) == 0 {
                let _ = repo.sync_retrieval_from_catalog(2_000, 0, true);
            }
            Ok(())
        })
        .await;
        let query_embedding = if state.embedding.is_available() {
            tokio::time::timeout(
                Duration::from_secs(2),
                state.embedding.embed(&[EmbeddingInput {
                    id: "nl-query".into(),
                    text: query.clone(),
                }]),
            )
            .await
            .ok()
            .and_then(Result::ok)
            .and_then(|mut embeddings| (embeddings.len() == 1).then(|| embeddings.remove(0)))
        } else {
            None
        };
        let query_for_search = query.clone();
        let search_result = if let Some(embedding) = query_embedding {
            let provider = state.embedding.name().to_owned();
            storage_result(repo, move |repo| {
                repo.hybrid_search_with_vector(
                    &query_for_search,
                    &embedding.vector,
                    &provider,
                    &embedding.model,
                    40,
                )
            })
            .await
        } else {
            storage_result(repo, move |repo| repo.hybrid_search(&query_for_search, 40)).await
        };
        if let Ok(hits) = search_result {
            reorder_items_by_hybrid(&mut items, &hits);
        }
    }
    let resolved_ai = match custom_ai {
        Some(custom) => {
            let Some(account_user_id) = account_user_id.as_deref() else {
                return error_response(
                    StatusCode::UNAUTHORIZED,
                    "unauthenticated",
                    "sign in to use a device-local custom AI provider",
                    None,
                );
            };
            let mode = match state.repo.as_ref() {
                Some(repo) => {
                    let user_id = account_user_id.to_owned();
                    match storage_result(repo, move |repo| repo.account_ai_settings(&user_id)).await
                    {
                        Ok(settings) => settings.mode,
                        Err(_) => AiMode::Off,
                    }
                }
                None => AiMode::Off,
            };
            if mode == AiMode::Custom {
                match transient_custom_ai_gateway(custom).await {
                    Ok(gateway) => ResolvedAiGateway {
                        gateway,
                        builtin_quota: None,
                    },
                    Err(response) => return response,
                }
            } else {
                // A stale device credential must not override a server-side
                // builtin/off selection. Fall back to the configured mode.
                account_ai_gateway(&state, Some(account_user_id)).await
            }
        }
        None => account_ai_gateway(&state, account_user_id.as_deref()).await,
    };
    let cache_scope = account_user_id
        .as_deref()
        .map(|user_id| format!("account:{user_id}"))
        .unwrap_or_else(|| "anonymous".to_owned());
    let ai_started = std::time::Instant::now();
    // Multi-model routing only applies to the server-built-in provider.
    let task_router = resolved_ai
        .builtin_quota
        .as_ref()
        .map(|_| state.task_router.as_ref())
        .or_else(|| {
            // Anonymous / builtin gateway without quota still uses the router
            // when the deployment gateway is the same built-in endpoint.
            (!resolved_ai.gateway.provider_name().is_empty()
                && resolved_ai.gateway.provider_name() == state.ai.provider_name()
                && state.task_router.is_available())
            .then_some(state.task_router.as_ref())
        });
    let (ai_status, fallback_reason, ai_summary, ai_summary_evidence_ids) =
        enhance_natural_language_with_ai(
            &resolved_ai.gateway,
            task_router,
            state.repo.as_ref(),
            &cache_scope,
            &query,
            &mut items,
            resolved_ai.builtin_quota.as_ref(),
        )
        .await;
    items.truncate(output_limit as usize);
    Json(json!({
        "query": query,
        "interpreted": interpreted,
        "items": items,
        "ai_status": ai_status.as_str(),
        "ai_provider": resolved_ai.gateway.provider_name(),
        "ai_latency_ms": ai_started.elapsed().as_millis() as u64,
        "fallback_reason": fallback_reason,
        "ai_summary": ai_summary,
        "ai_summary_evidence_ids": ai_summary_evidence_ids,
        "algorithm_version": feed.get("algorithm_version").cloned().unwrap_or(json!(ALGORITHM_VERSION)),
        "data_updated_at_ms": feed.get("data_updated_at_ms").cloned().unwrap_or(json!(0)),
    }))
    .into_response()
}

fn reorder_items_by_hybrid(items: &mut [serde_json::Value], hits: &[mpgs_storage::HybridHit]) {
    if items.is_empty() || hits.is_empty() {
        return;
    }
    let score_by_id: std::collections::HashMap<u32, f64> =
        hits.iter().map(|h| (h.app_id, h.score)).collect();
    items.sort_by(|a, b| {
        let id_a = a.get("app_id").and_then(|v| v.as_u64()).map(|v| v as u32);
        let id_b = b.get("app_id").and_then(|v| v.as_u64()).map(|v| v as u32);
        let ha = id_a
            .and_then(|id| score_by_id.get(&id).copied())
            .unwrap_or(0.0);
        let hb = id_b
            .and_then(|id| score_by_id.get(&id).copied())
            .unwrap_or(0.0);
        match hb.partial_cmp(&ha).unwrap_or(std::cmp::Ordering::Equal) {
            std::cmp::Ordering::Equal => {
                let sa = a.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let sb = b.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
                sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
            }
            other => other,
        }
    });
    for item in items.iter_mut() {
        if let Some(app_id) = item
            .get("app_id")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            && let Some(score) = score_by_id.get(&app_id)
            && let Some(obj) = item.as_object_mut()
        {
            obj.insert("hybrid_score".into(), json!(score));
        }
    }
}

/// Resolve an account's AI mode without ever returning the custom key to the
/// caller. Anonymous natural-language requests use the deployment's built-in
/// mode but cannot select a custom provider.
struct ResolvedAiGateway {
    gateway: AiGateway,
    builtin_quota: Option<BuiltinAiQuota>,
}

#[derive(Clone)]
struct BuiltinAiQuota {
    user_id: String,
    limiter: AccountAiLimiter,
}

async fn account_ai_gateway(state: &AppState, user_id: Option<&str>) -> ResolvedAiGateway {
    let Some(user_id) = user_id else {
        return ResolvedAiGateway {
            gateway: state.ai.clone(),
            builtin_quota: None,
        };
    };
    let Some(repo) = state.repo.as_ref() else {
        return ResolvedAiGateway {
            gateway: AiGateway::disabled(),
            builtin_quota: None,
        };
    };
    let lookup_user_id = user_id.to_owned();
    let settings =
        match storage_result(repo, move |repo| repo.account_ai_settings(&lookup_user_id)).await {
            Ok(settings) => settings,
            Err(_) => {
                return ResolvedAiGateway {
                    gateway: AiGateway::disabled(),
                    builtin_quota: None,
                };
            }
        };
    match settings.mode {
        AiMode::Off => ResolvedAiGateway {
            gateway: AiGateway::disabled(),
            builtin_quota: None,
        },
        AiMode::Builtin => ResolvedAiGateway {
            gateway: state.ai.clone(),
            builtin_quota: Some(BuiltinAiQuota {
                user_id: user_id.to_owned(),
                limiter: state.account_ai_limits.clone(),
            }),
        },
        AiMode::Custom => ResolvedAiGateway {
            // Custom credentials are device-owned and must arrive transiently
            // with the recommendation request.
            gateway: AiGateway::disabled(),
            builtin_quota: None,
        },
    }
}

async fn transient_custom_ai_gateway(
    custom: TransientCustomAiRequest,
) -> Result<AiGateway, Response> {
    if custom.provider != "openai_compat" {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "custom provider must be openai_compat",
            None,
        ));
    }
    let resolution = mpgs_ai::resolve_custom_base_url(&custom.base_url)
        .await
        .map_err(|_| {
            error_response(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "custom AI endpoint is not allowed",
                None,
            )
        })?;
    let provider = OpenAiCompatProvider::new_with_custom_resolution(
        resolution.base_url.clone(),
        custom.api_key,
        custom.model,
        Duration::from_secs(12),
        &resolution,
    )
    .map_err(|_| {
        error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "custom AI configuration is invalid",
            None,
        )
    })?;
    Ok(AiGateway::new(Arc::new(provider), AiPolicy::default()))
}

/// Apply optional AI Top-N analysis. Always preserves deterministic items on failure.
async fn enhance_natural_language_with_ai(
    gateway: &AiGateway,
    task_router: Option<&TaskRouter>,
    repo: Option<&Repository>,
    cache_scope: &str,
    query: &str,
    items: &mut Vec<serde_json::Value>,
    builtin_quota: Option<&BuiltinAiQuota>,
) -> (AiStatus, Option<String>, Option<String>, Vec<String>) {
    const AI_MAX_RECOMMENDATIONS: usize = 8;
    if items.is_empty() {
        if gateway.is_available() {
            return (AiStatus::Used, None, None, Vec::new());
        }
        return (
            AiStatus::Disabled,
            Some(mpgs_ai::AiError::Disabled.fallback_reason().to_owned()),
            None,
            Vec::new(),
        );
    }
    if !gateway.is_available() {
        // Keep reason text compatible with M4 acceptance wording while exposing disabled status.
        return (
            AiStatus::Fallback,
            Some(mpgs_ai::AiError::Disabled.fallback_reason().to_owned()),
            None,
            Vec::new(),
        );
    }

    let mut candidates = Vec::new();
    let mut compact = Vec::new();
    for item in items.iter() {
        let Some(app_id) = item
            .get("app_id")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
        else {
            continue;
        };
        let evidence_ids = item
            .get("evidence_ids")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect::<std::collections::HashSet<_>>()
            })
            .unwrap_or_default();
        let mut evidence_list: Vec<String> = evidence_ids.iter().cloned().collect();
        evidence_list.sort();
        compact.push(json!({
            "app_id": app_id,
            "name": item.get("name").cloned().unwrap_or(json!("")),
            // Keep provider-prompt data independent of the current natural-language
            // query so different queries can reuse the longest possible prefix.
            "section": item.get("section").cloned().unwrap_or(json!(null)),
            "release_date": item.get("release_date").cloned().unwrap_or(json!(null)),
            "party": item.get("party").cloned().unwrap_or(json!(null)),
            "multiplayer": item.get("multiplayer").cloned().unwrap_or(json!(null)),
            "evidence_ids": evidence_list,
        }));
        candidates.push(CandidateEvidence {
            app_id,
            evidence_ids,
        });
    }

    // Provider-side prompt caches match exact prefixes. A stable candidate order
    // keeps the shared prefix identical even when deterministic ranking order varies.
    compact.sort_by_key(|item| {
        item.get("app_id")
            .and_then(|value| value.as_u64())
            .unwrap_or(0)
    });
    if candidates.is_empty() {
        return (
            AiStatus::Fallback,
            Some("no valid candidates for AI analysis".into()),
            None,
            Vec::new(),
        );
    }

    let provider_identity = gateway.provider_cache_identity();
    let route_version = task_router
        .map(|router| router.route_version(AiTaskType::RankExplain).to_owned())
        .unwrap_or_else(|| DEFAULT_ROUTE_VERSION.to_owned());
    let cache_key = nl_ai_cache_key(
        query,
        &compact,
        &provider_identity,
        cache_scope,
        &route_version,
    );
    if let Some(repo) = repo {
        let now_ms = chrono_like_now_ms();
        let lookup_key = cache_key.clone();
        match storage_result(repo, move |repo| repo.get_ai_cache(&lookup_key, now_ms)).await {
            Ok(Some(entry)) => {
                match serde_json::from_str::<serde_json::Value>(&entry.output_json)
                    .map_err(|error| error.to_string())
                    .and_then(|content| {
                        validate_rank_result(&content, &candidates, AI_MAX_RECOMMENDATIONS)
                            .map_err(|error| error.to_string())
                    }) {
                    Ok(validated) => {
                        tracing::info!(provider = %entry.provider, model = %entry.model, "AI response cache hit");
                        apply_validated_ai_rank(items, &validated);
                        let summary = if validated.summary.is_empty() {
                            None
                        } else {
                            Some(validated.summary)
                        };
                        return (
                            AiStatus::Cached,
                            None,
                            summary,
                            validated.summary_evidence_ids,
                        );
                    }
                    Err(error) => {
                        tracing::warn!(%error, "AI response cache entry failed validation");
                    }
                }
            }
            Ok(None) => tracing::info!("AI response cache miss"),
            Err(error) => tracing::warn!(%error, "AI response cache lookup failed"),
        }
    }

    let data_prompt = format!(
        "{}\n\n{}",
        wrap_untrusted_data_block(
            "candidates_json",
            &serde_json::to_string(&compact).unwrap_or_else(|_| "[]".into()),
            12_000
        ),
        // Dynamic content belongs last so it cannot break reuse of the stable prefix.
        wrap_untrusted_data_block("user_query", query, 500),
    );
    let request = StructuredRequest {
        task: AiTaskType::RankExplain,
        system_prompt: rank_analysis_system_prompt().to_owned(),
        data_prompt,
        json_schema_name: "rank_analysis".into(),
        json_schema: rank_analysis_schema(),
        max_output_tokens: 1_800,
        temperature: 0.2,
        model: None,
        protocol: None,
    };

    // Cache hits above return before this point. Only a real built-in model
    // request consumes the account's durable daily quota and a short-lived
    // per-account concurrency permit.
    let _permit = if let Some(quota) = builtin_quota {
        match quota.limiter.try_acquire(&quota.user_id) {
            Some(permit) => Some(permit),
            None => {
                return (
                    AiStatus::Fallback,
                    Some(
                        mpgs_ai::AiError::BudgetExhausted
                            .fallback_reason()
                            .to_owned(),
                    ),
                    None,
                    Vec::new(),
                );
            }
        }
    } else {
        None
    };
    if let Some(quota) = builtin_quota {
        let Some(repo) = repo else {
            return (
                AiStatus::Fallback,
                Some(mpgs_ai::AiError::Disabled.fallback_reason().to_owned()),
                None,
                Vec::new(),
            );
        };
        let user_id = quota.user_id.clone();
        let day_utc = chrono_like_now_ms() / 86_400_000;
        let daily_limit = quota.limiter.daily_budget();
        match storage_result(repo, move |repo| {
            repo.consume_account_ai_quota(&user_id, day_utc, daily_limit)
        })
        .await
        {
            Ok(Some(_)) => {}
            Ok(None) => {
                return (
                    AiStatus::Fallback,
                    Some(
                        mpgs_ai::AiError::BudgetExhausted
                            .fallback_reason()
                            .to_owned(),
                    ),
                    None,
                    Vec::new(),
                );
            }
            Err(_) => {
                return (
                    AiStatus::Fallback,
                    Some("AI account quota is temporarily unavailable; deterministic recommendations are shown".to_owned()),
                    None,
                    Vec::new(),
                );
            }
        }
    }

    let response = match run_rank_completion(gateway, task_router, request).await {
        Ok(response) => response,
        Err(error) => {
            return (
                AiStatus::Fallback,
                Some(safe_ai_failure_reason(&error)),
                None,
                Vec::new(),
            );
        }
    };
    tracing::info!(
        prompt_cache_hit_tokens = ?response.prompt_cache_hit_tokens,
        prompt_cache_miss_tokens = ?response.prompt_cache_miss_tokens,
        "AI provider prompt-cache usage"
    );

    let (validated, degraded_notice) = match validate_rank_result_with_safe_degradation(
        &response.content,
        &candidates,
        AI_MAX_RECOMMENDATIONS,
    ) {
        Ok(result) => result,
        Err(error) => {
            return (
                AiStatus::Fallback,
                Some(safe_ai_failure_reason(&error)),
                None,
                Vec::new(),
            );
        }
    };

    if let Some(repo) = repo {
        let now_ms = chrono_like_now_ms();
        let output_json = serde_json::to_string(&validated).unwrap_or_else(|_| "{}".to_owned());
        let entry = mpgs_storage::AiCacheEntry {
            cache_key: nl_ai_cache_key(
                query,
                &compact,
                &provider_identity,
                cache_scope,
                &route_version,
            ),
            task_type: "rank_explain".into(),
            provider: response.provider.clone(),
            model: response.model.clone(),
            prompt_version: RANK_PROMPT_VERSION.into(),
            input_hash: nl_ai_cache_key(
                query,
                &compact,
                &provider_identity,
                cache_scope,
                &route_version,
            ),
            output_json,
            // The persisted payload has already been strictly validated. In the
            // degradation path it is the sanitized ranking-only payload, which
            // still belongs to the schema's durable `accepted` state.
            validation_status: "accepted".into(),
            usage_input: i64::from(response.usage_input),
            usage_output: i64::from(response.usage_output),
            created_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(6 * 60 * 60 * 1000),
        };
        match storage_result(repo, move |repo| repo.put_ai_cache(&entry)).await {
            Ok(()) => tracing::info!("AI response cache stored"),
            Err(error) => tracing::warn!(%error, "AI response cache write failed"),
        }
    }

    apply_validated_ai_rank(items, &validated);

    let summary = if validated.summary.is_empty() {
        None
    } else {
        Some(validated.summary)
    };
    (
        AiStatus::Used,
        degraded_notice,
        summary,
        validated.summary_evidence_ids,
    )
}

fn safe_ai_failure_reason(error: &AiError) -> String {
    match error {
        AiError::InvalidOutput(detail) => format!(
            "AI output failed validation: {detail}; deterministic recommendations are shown"
        ),
        _ => error.fallback_reason().to_owned(),
    }
}

fn validate_rank_result_with_safe_degradation(
    value: &serde_json::Value,
    candidates: &[CandidateEvidence],
    max_items: usize,
) -> Result<(AiRankResult, Option<String>), AiError> {
    match validate_rank_result(value, candidates, max_items) {
        Ok(result) => Ok((result, None)),
        Err(AiError::InvalidOutput(strict_reason)) => {
            let recommendations = value
                .get("recommendations")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| AiError::InvalidOutput(strict_reason.clone()))?;
            let ranking_only = json!({
                "recommendations": recommendations.iter().map(|item| json!({
                    "app_id": item.get("app_id").cloned().unwrap_or(serde_json::Value::Null),
                    "fit_score": item.get("fit_score").cloned().unwrap_or(serde_json::Value::Null),
                    "confidence": item.get("confidence").cloned().unwrap_or(serde_json::Value::Null),
                    "reason_evidence_ids": [],
                    "reasons": [],
                    "cautions": [],
                })).collect::<Vec<_>>(),
                "summary": "",
                "summary_evidence_ids": [],
            });
            let result = validate_rank_result(&ranking_only, candidates, max_items)
                .map_err(|_| AiError::InvalidOutput(strict_reason.clone()))?;
            Ok((
                result,
                Some(format!(
                    "AI ranking applied; generated explanations were discarded because strict validation reported: {strict_reason}"
                )),
            ))
        }
        Err(error) => Err(error),
    }
}

fn apply_validated_ai_rank(items: &mut Vec<serde_json::Value>, validated: &mpgs_ai::AiRankResult) {
    let mut by_id: std::collections::HashMap<u32, serde_json::Value> = items
        .drain(..)
        .filter_map(|item| {
            let app_id = item
                .get("app_id")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32)?;
            Some((app_id, item))
        })
        .collect();

    let mut reordered = Vec::new();
    for ai_item in &validated.recommendations {
        let Some(mut item) = by_id.remove(&ai_item.app_id) else {
            continue;
        };
        let base = item
            .get("components")
            .and_then(|c| c.get("personalized_score"))
            .and_then(|v| v.as_f64())
            .or_else(|| item.get("score").and_then(|v| v.as_f64()))
            .unwrap_or(0.0);
        let blended = blend_ai(
            base,
            Some(AiAdjustment {
                fit: ai_item.fit_score,
                confidence: ai_item.confidence,
            }),
        );
        if let Some(obj) = item.as_object_mut() {
            obj.insert("score".into(), json!(blended));
            obj.insert("ai_fit".into(), json!(ai_item.fit_score));
            obj.insert("ai_confidence".into(), json!(ai_item.confidence));
            if !ai_item.reasons.is_empty() {
                obj.insert("ai_reasons".into(), json!(ai_item.reasons));
            }
            if !ai_item.cautions.is_empty() {
                let mut cautions = obj
                    .get("cautions")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                for caution in &ai_item.cautions {
                    cautions.push(json!(caution));
                }
                obj.insert("cautions".into(), json!(cautions));
            }
            if let Some(components) = obj.get_mut("components").and_then(|v| v.as_object_mut()) {
                components.insert("final_score".into(), json!(blended));
            }
        }
        reordered.push(item);
    }
    let mut rest: Vec<_> = by_id.into_values().collect();
    rest.sort_by(|a, b| {
        let sa = a.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let sb = b.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
    });
    reordered.extend(rest);
    *items = reordered;
}

fn nl_ai_cache_key(
    query: &str,
    compact: &[serde_json::Value],
    provider_identity: &str,
    cache_scope: &str,
    route_version: &str,
) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(provider_identity.as_bytes());
    hasher.update([0]);
    hasher.update(cache_scope.as_bytes());
    hasher.update([0]);
    hasher.update(RANK_PROMPT_VERSION.as_bytes());
    hasher.update([0]);
    hasher.update(route_version.as_bytes());
    hasher.update([0]);
    hasher.update(ALGORITHM_VERSION.as_bytes());
    hasher.update([0]);
    hasher.update(query.as_bytes());
    hasher.update([0]);
    // Stable ordering: sort by app_id so cache keys don't depend on feed order.
    let mut ordered = compact.to_vec();
    ordered.sort_by(|a, b| {
        let id_a = a.get("app_id").and_then(|v| v.as_u64()).unwrap_or(0);
        let id_b = b.get("app_id").and_then(|v| v.as_u64()).unwrap_or(0);
        id_a.cmp(&id_b)
    });
    if let Ok(bytes) = serde_json::to_vec(&ordered) {
        hasher.update(bytes);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    format!("nl-rank:{hex}")
}

async fn run_rank_completion(
    gateway: &AiGateway,
    task_router: Option<&TaskRouter>,
    request: StructuredRequest,
) -> Result<mpgs_ai::StructuredResponse, AiError> {
    if let Some(router) = task_router
        && router.is_available()
    {
        return router
            .structured_completion(request)
            .await
            .map(|routed| routed.response);
    }
    gateway.structured_completion(request).await
}

fn new_analysis_id() -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(chrono_like_now_ms().to_le_bytes());
    hasher.update(getrandom_u64().to_le_bytes());
    let digest = hasher.finalize();
    format!(
        "an_{}",
        digest
            .iter()
            .take(12)
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

fn getrandom_u64() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
        ^ std::process::id() as u64
}

#[derive(Debug, Deserialize)]
struct AiSearchRequest {
    query: String,
    #[serde(default)]
    limit: Option<u8>,
    /// When true (default), return base candidates immediately with analysis_id.
    #[serde(default = "default_true")]
    r#async: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct AiCompareRequest {
    app_ids: Vec<u32>,
}

async fn ai_search(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<AiSearchRequest>,
) -> Response {
    let query = body.query.trim().to_owned();
    if query.len() < 3 || query.len() > 500 {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "query must contain between 3 and 500 characters",
            None,
        );
    }
    let output_limit = body.limit.unwrap_or(6);
    if !(3..=10).contains(&output_limit) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "limit must be between 3 and 10",
            None,
        );
    }

    // Reuse the existing natural-language path for deterministic base results.
    let nl_body = NaturalLanguageRequest {
        query: query.clone(),
        limit: Some(i64::from(output_limit)),
        custom_ai: None,
    };
    let nl_response =
        natural_language_recommendations(State(state.clone()), headers.clone(), Json(nl_body))
            .await
            .into_response();
    if nl_response.status() != StatusCode::OK {
        return nl_response;
    }
    let (_, body_bytes) = nl_response.into_parts();
    let bytes = match axum::body::to_bytes(body_bytes, 2 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "failed to assemble AI search response",
                None,
            );
        }
    };
    let mut payload: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "failed to parse AI search base results",
                None,
            );
        }
    };

    // Progressive mode records an analysis_id so clients can poll enhancement
    // without blocking the first paint. When async=false, the NL path already
    // waited for AI and we still expose a completed analysis id when storage
    // is available.
    let analysis_id = new_analysis_id();
    let ai_status = payload
        .get("ai_status")
        .and_then(|v| v.as_str())
        .unwrap_or("fallback")
        .to_owned();
    if let Some(repo) = state.repo.as_ref() {
        let now_ms = chrono_like_now_ms();
        let request_json = json!({ "query": query, "limit": output_limit }).to_string();
        let base_result_json = payload.to_string();
        let status_for_row = if body.r#async && ai_status == "used" {
            // Enhancement already finished synchronously via NL path.
            "used"
        } else if body.r#async && matches!(ai_status.as_str(), "fallback" | "disabled") {
            ai_status.as_str()
        } else if body.r#async {
            "pending"
        } else {
            ai_status.as_str()
        };
        let insert = mpgs_storage::InsertProgressiveAnalysis {
            analysis_id: analysis_id.clone(),
            task_type: "rank_explain".into(),
            status: status_for_row.into(),
            prompt_version: RANK_PROMPT_VERSION.into(),
            input_hash: analysis_id.clone(),
            preference_hash: String::new(),
            data_snapshot_hash: String::new(),
            request_json,
            base_result_json: Some(base_result_json),
            created_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(6 * 60 * 60 * 1000),
        };
        let _ = storage_result(repo, move |repo| repo.insert_progressive_analysis(&insert)).await;
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("analysis_id".into(), json!(analysis_id));
            if body.r#async && status_for_row == "pending" {
                obj.insert("ai_status".into(), json!("pending"));
            }
        }
    } else if let Some(obj) = payload.as_object_mut() {
        obj.insert("analysis_id".into(), json!(analysis_id));
    }

    Json(payload).into_response()
}

async fn get_ai_analysis(
    State(state): State<Arc<AppState>>,
    Path(analysis_id): Path<String>,
) -> Response {
    let Some(repo) = state.repo.as_ref() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "database is not configured",
            None,
        );
    };
    if analysis_id.len() > 64 || analysis_id.trim().is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "analysis_id is invalid",
            None,
        );
    }
    let now_ms = chrono_like_now_ms();
    let id = analysis_id.clone();
    match storage_result(repo, move |repo| repo.get_progressive_analysis(&id, now_ms)).await {
        Ok(Some(row)) => Json(json!({
            "analysis_id": row.analysis_id,
            "task_type": row.task_type,
            "ai_status": row.status,
            "provider": row.provider,
            "model": row.model,
            "protocol": row.protocol,
            "route_version": row.route_version,
            "prompt_version": row.prompt_version,
            "base_result": row.base_result_json.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()),
            "result": row.result_json.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()),
            "fallback_reason": row.fallback_reason,
            "created_at_ms": row.created_at_ms,
            "updated_at_ms": row.updated_at_ms,
            "completed_at_ms": row.completed_at_ms,
            "expires_at_ms": row.expires_at_ms,
        }))
        .into_response(),
        Ok(None) => error_response(
            StatusCode::NOT_FOUND,
            "not_found",
            "analysis not found or expired",
            None,
        ),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "failed to load analysis",
            None,
        ),
    }
}

async fn ai_compare(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AiCompareRequest>,
) -> Response {
    let app_ids = body.app_ids;
    if !(2..=4).contains(&app_ids.len()) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "app_ids must contain 2 to 4 games",
            None,
        );
    }
    let mut unique = HashSet::new();
    for id in &app_ids {
        if *id == 0 || !unique.insert(*id) {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "app_ids must be unique positive AppIDs",
                None,
            );
        }
    }

    let Some(repo) = state.repo.as_ref() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "database is not configured",
            None,
        );
    };

    // Fact matrix is server-generated; the model may only explain later (AI-009).
    let mut matrix = Vec::new();
    for app_id in app_ids {
        let app = match storage_result(repo, move |repo| repo.get_app(app_id)).await {
            Ok(Some(app)) => app,
            Ok(None) => {
                return error_response(
                    StatusCode::NOT_FOUND,
                    "not_found",
                    &format!("app_id {app_id} was not found"),
                    None,
                );
            }
            Err(_) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "failed to load game facts",
                    None,
                );
            }
        };
        let profile = storage_result(repo, move |repo| repo.get_profile(app_id))
            .await
            .ok()
            .flatten();
        matrix.push(json!({
            "app_id": app.app_id,
            "name": app.canonical_name,
            "release_state": app.release_state,
            "release_date": app.release_date,
            "is_early_access": app.is_early_access,
            "party": profile.as_ref().map(|p| json!({
                "min": p.recommended_min_players,
                "max": p.recommended_max_players,
            })),
            "multiplayer": profile.as_ref().map(|p| json!({
                "dominant_mode": p.dominant_mode,
                "private_session": p.private_session,
                "online_coop": p.online_coop,
                "crossplay": p.crossplay,
                "self_hosted_server": p.self_hosted_server,
                "drop_in_out": p.drop_in_out,
            })),
            "data_updated_at_ms": app.updated_at_ms,
        }));
    }

    Json(json!({
        "fact_matrix": matrix,
        "ai_status": "fallback",
        "explanation": null,
        "fallback_reason": "compare explanation uses the fact matrix until the compare_games route is online",
        "columns": [
            "party_size",
            "platforms",
            "price",
            "multiplayer_mode",
            "service_dependency",
            "content_pacing",
            "review_quality",
            "data_updated_at"
        ]
    }))
    .into_response()
}

async fn get_game_ai_summary(
    State(state): State<Arc<AppState>>,
    Path(app_id): Path<u32>,
) -> Response {
    if app_id == 0 {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "app_id must be positive",
            None,
        );
    }
    let Some(repo) = state.repo.as_ref() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "database is not configured",
            None,
        );
    };
    // Offline summaries are filled by M8.3 batch jobs. Until then return an
    // honest empty payload rather than inventing content.
    let exists = match storage_result(repo, move |repo| repo.get_app(app_id)).await {
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "failed to load game",
                None,
            );
        }
    };
    if !exists {
        return error_response(StatusCode::NOT_FOUND, "not_found", "game not found", None);
    }
    Json(json!({
        "app_id": app_id,
        "ai_status": "disabled",
        "summary": null,
        "sections": {
            "who_it_fits": null,
            "how_to_play": null,
            "multiplayer_dependency": null,
            "review_strengths": null,
            "common_issues": null,
            "unknowns": null
        },
        "updated_at_ms": null,
        "fallback_reason": "offline game summaries are not generated yet"
    }))
    .into_response()
}

fn chrono_like_now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn interpret_natural_language(query: &str) -> NaturalLanguageInterpretation {
    let normalized = query.to_lowercase();
    let party_size = extract_party_size(&normalized);
    let session_minutes_max = extract_session_minutes(&normalized);
    let coop_competitive = if contains_any(
        &normalized,
        &[
            "不要太卷",
            "不太卷",
            "不要太竞技",
            "不竞技",
            "休闲",
            "轻松",
            "合作",
            "coop",
            "casual",
        ],
    ) {
        Some(0.1)
    } else if contains_any(&normalized, &["竞技", "排位", "competitive", "ranked"]) {
        Some(0.85)
    } else {
        None
    };
    let self_hosting_willingness = contains_any(
        &normalized,
        &[
            "自建服",
            "自建服务器",
            "专用服务器",
            "self-host",
            "dedicated server",
        ],
    )
    .then_some(1.0);
    let mut platforms = Vec::new();
    if normalized.contains("windows") {
        platforms.push("windows".to_owned());
    }
    if contains_any(&normalized, &["macos", "mac os", "mac版", "mac 版"]) {
        platforms.push("macos".to_owned());
    }
    if normalized.contains("linux") {
        platforms.push("linux".to_owned());
    }
    let demo_only = contains_any(&normalized, &["demo", "试玩", "playtest", "测试版"]);
    let selected_section =
        if contains_any(&normalized, &["即将", "未发售", "coming soon"]) || demo_only {
            FeedSection::Upcoming
        } else if contains_any(
            &normalized,
            &["经典", "老游戏", "反复刷", "耐玩", "replayable"],
        ) {
            FeedSection::ClassicLegacy
        } else if contains_any(&normalized, &["热门", "人多", "popular"]) {
            FeedSection::PopularLegacy
        } else {
            FeedSection::RecentRelease
        };
    NaturalLanguageInterpretation {
        party_size,
        session_minutes_max,
        coop_competitive,
        self_hosting_willingness,
        platforms,
        demo_only,
        selected_section,
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn extract_party_size(query: &str) -> Option<u8> {
    const NUMBERS: [(&str, u8); 16] = [
        ("1", 1),
        ("一", 1),
        ("2", 2),
        ("二", 2),
        ("3", 3),
        ("三", 3),
        ("4", 4),
        ("四", 4),
        ("5", 5),
        ("五", 5),
        ("6", 6),
        ("六", 6),
        ("7", 7),
        ("七", 7),
        ("8", 8),
        ("八", 8),
    ];
    for (token, value) in NUMBERS {
        if [
            format!("{token}个人"),
            format!("{token} 人"),
            format!("{token}人"),
            format!("{token} people"),
            format!("{token} players"),
        ]
        .iter()
        .any(|pattern| query.contains(pattern))
        {
            return Some(value);
        }
    }
    for (token, value) in [
        ("one", 1),
        ("two", 2),
        ("three", 3),
        ("four", 4),
        ("five", 5),
        ("six", 6),
        ("seven", 7),
        ("eight", 8),
    ] {
        if [format!("{token} people"), format!("{token} players")]
            .iter()
            .any(|pattern| query.contains(pattern))
        {
            return Some(value);
        }
    }
    None
}

fn extract_session_minutes(query: &str) -> Option<u32> {
    const HOURS: [(&str, u32); 8] = [
        ("1", 60),
        ("一", 60),
        ("2", 120),
        ("二", 120),
        ("3", 180),
        ("三", 180),
        ("4", 240),
        ("四", 240),
    ];
    for (token, minutes) in HOURS {
        if [
            format!("{token}小时"),
            format!("{token} 小时"),
            format!("{token} hour"),
        ]
        .iter()
        .any(|pattern| query.contains(pattern))
        {
            return Some(minutes);
        }
    }
    for (token, minutes) in [("one", 60), ("two", 120), ("three", 180), ("four", 240)] {
        if [format!("{token} hour"), format!("{token} hours")]
            .iter()
            .any(|pattern| query.contains(pattern))
        {
            return Some(minutes);
        }
    }
    for minutes in [15_u32, 20, 30, 45, 60, 90, 120, 180, 240] {
        if [
            format!("{minutes}分钟"),
            format!("{minutes} 分钟"),
            format!("{minutes} min"),
        ]
        .iter()
        .any(|pattern| query.contains(pattern))
        {
            return Some(minutes);
        }
    }
    None
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct CalendarQuery {
    from: Option<String>,
    to: Option<String>,
    state: Option<String>,
}

#[utoipa::path(
    get,
    path = "/v1/calendar",
    params(CalendarQuery),
    responses(
        (status = 200, body = CalendarResponseSchema),
        (status = 304, description = "Not modified"),
        (status = 400, body = ErrorBody)
    ),
    tag = "catalog"
)]
async fn get_calendar(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<CalendarQuery>,
) -> impl IntoResponse {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let now_ms = repo.database().now_ms();
    let calendar_state = query.state.unwrap_or_else(|| "upcoming".to_owned());
    if !matches!(calendar_state.as_str(), "upcoming" | "recent") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "calendar state must be upcoming or recent",
            None,
        );
    }
    let default_from_ms = if calendar_state == "recent" {
        now_ms.saturating_sub(180_i64 * 24 * 60 * 60 * 1000)
    } else {
        now_ms
    };
    let default_to_ms = if calendar_state == "recent" {
        now_ms
    } else {
        now_ms.saturating_add(365_i64 * 24 * 60 * 60 * 1000)
    };
    let from = query
        .from
        .unwrap_or_else(|| mpgs_storage::util::day_utc_from_ms(default_from_ms));
    let to = query
        .to
        .unwrap_or_else(|| mpgs_storage::util::day_utc_from_ms(default_to_ms));
    let Some(from_day) = mpgs_storage::util::iso_day_to_unix_days(&from) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "calendar dates must use valid YYYY-MM-DD values",
            None,
        );
    };
    let Some(to_day) = mpgs_storage::util::iso_day_to_unix_days(&to) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "calendar dates must use valid YYYY-MM-DD values",
            None,
        );
    };
    if to_day < from_day || to_day - from_day > 366 {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "calendar range must be ordered and not exceed one year",
            None,
        );
    }
    let data_updated_at_ms = match storage_result(repo, |repo| repo.data_updated_at_ms()).await {
        Ok(value) => value,
        Err(error) => return map_storage_error(error, None),
    };
    let etag = weak_etag(&format!(
        "calendar:v3:{calendar_state}:{from}:{to}:{data_updated_at_ms}"
    ));
    if let Some(response) = if_none_match_ok(&headers, &etag) {
        return response;
    }
    match storage_result(repo, move |repo| {
        let (dated, undated) = repo.list_calendar(&from, &to, &calendar_state)?;
        let attach_review_state = |items: Vec<mpgs_storage::AppRecord>| {
            items
                .into_iter()
                .map(|item| {
                    let detail = repo.game_detail(item.app_id)?;
                    let review_total = detail.as_ref().and_then(|row| row.total_reviews);
                    let cover_url = detail.and_then(|row| row.cover_url);
                    Ok(calendar_item_json(item, review_total, cover_url))
                })
                .collect::<Result<Vec<_>, StorageError>>()
        };
        Ok((attach_review_state(dated)?, attach_review_state(undated)?))
    })
    .await
    {
        Ok((dated, undated)) => {
            let body = json!({
                "dated_items": dated,
                "undated_items": undated,
                "data_updated_at_ms": data_updated_at_ms,
            });
            (StatusCode::OK, [(header::ETAG, etag)], Json(body)).into_response()
        }
        Err(error) => map_storage_error(error, None),
    }
}

fn calendar_item_json(
    item: mpgs_storage::AppRecord,
    review_total: Option<u32>,
    cover_url: Option<String>,
) -> serde_json::Value {
    json!({
        "app_id": item.app_id,
        "app_type": item.app_type,
        "canonical_name": item.canonical_name,
        "cover_url": cover_url.or_else(|| resolve_feed_cover_url(item.app_id, None)),
        "release_state": item.release_state,
        "release_date": item.release_date,
        "release_date_raw": item.release_date_raw,
        "release_date_precision": item.release_date_precision,
        "is_early_access": item.is_early_access,
        "current_data_confidence": item.current_data_confidence,
        "review_total": review_total,
        "early_data": review_total.unwrap_or(0) < 100,
        "source_modified_at_ms": item.source_modified_at_ms,
        "created_at_ms": item.created_at_ms,
        "updated_at_ms": item.updated_at_ms,
    })
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct SearchQuery {
    q: Option<String>,
    limit: Option<i64>,
}

#[utoipa::path(
    get,
    path = "/v1/search",
    params(SearchQuery),
    responses(
        (status = 200, body = SearchResponseSchema),
        (status = 400, body = ErrorBody)
    ),
    tag = "catalog"
)]
async fn search_games(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SearchQuery>,
) -> impl IntoResponse {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let q = query.q.unwrap_or_default();
    if q.trim().is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "q is required",
            None,
        );
    }
    let limit = query.limit.unwrap_or(20);
    if !(1..=100).contains(&limit) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "limit must be between 1 and 100",
            None,
        );
    }
    let active_config = match storage_result(repo, |repo| repo.active_algorithm_config()).await {
        Ok(value) => value,
        Err(error) => return map_storage_error(error, None),
    };
    match storage_result(repo, move |repo| repo.search_games(&q, limit)).await {
        Ok(items) => {
            let body = json!({
                "items": items.iter().map(|g| json!({
                    "app_id": g.app_id,
                    "name": g.name,
                    "release_state": g.release_state,
                    "release_date": g.release_date,
                })).collect::<Vec<_>>(),
                "algorithm_version": active_config.version,
            });
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(error) => map_storage_error(error, None),
    }
}

#[utoipa::path(
    get,
    path = "/v1/games/{app_id}",
    params(("app_id" = u32, Path, description = "Steam AppID")),
    responses(
        (status = 200, body = GameResponseSchema),
        (status = 304, description = "Not modified"),
        (status = 404, body = ErrorBody)
    ),
    tag = "catalog"
)]
async fn get_game(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(app_id): Path<u32>,
) -> impl IntoResponse {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let data_updated_at_ms = match storage_result(repo, |repo| repo.data_updated_at_ms()).await {
        Ok(value) => value,
        Err(error) => return map_storage_error(error, None),
    };
    let active_config = match storage_result(repo, |repo| repo.active_algorithm_config()).await {
        Ok(value) => value,
        Err(error) => return map_storage_error(error, None),
    };
    // Optional identity: a valid bearer reveals this user's vote; anonymous omits it.
    let user_id = optional_user_id(repo, &headers).await;
    let snapshot_user_id = user_id.clone();
    let play_intent = match storage_result(repo, move |repo| {
        repo.community_game_previews(app_id, snapshot_user_id.as_deref())
    })
    .await
    {
        Ok(value) => value,
        Err(error) => return map_storage_error(error, None),
    };
    let cache_identity = user_id.as_deref().unwrap_or("public");
    let etag = weak_etag(&format!(
        "game:v3:{app_id}:{data_updated_at_ms}:{}:pi{}:{}:{}:user{cache_identity}",
        active_config.version, play_intent.0.revision, play_intent.1, play_intent.2,
    ));
    if let Some(response) = if_none_match_ok(&headers, &etag) {
        return response;
    }
    let popular_reviews = match storage_result(repo, move |repo| repo.popular_reviews(app_id)).await
    {
        Ok(value) => value,
        Err(error) => return map_storage_error(error, None),
    };
    match storage_result(repo, move |repo| repo.game_detail(app_id)).await {
        Ok(Some(game)) => {
            let body = json!({
                "app_id": game.app_id,
                "name": game.name,
                "app_type": game.app_type,
                "release_state": game.release_state,
                "release_date": game.release_date,
                "release_date_raw": game.release_date_raw,
                "release_date_precision": game.release_date_precision,
                "cover_url": game.cover_url,
                "cover_updated_at_ms": game.cover_updated_at_ms,
                "short_description": game.short_description,
                "steam_url": format!("https://store.steampowered.com/app/{app_id}/"),
                "multiplayer": {
                    "dominant_mode": game.dominant_mode,
                    "private_session": game.private_session,
                    "online_coop": game.online_coop,
                    "self_hosted_server": game.self_hosted_server,
                    "recommended_min": game.recommended_min,
                    "recommended_max": game.recommended_max,
                    "profile_confidence": game.profile_confidence,
                },
                "play_intent": {
                    "count": play_intent.1,
                    "voted": play_intent.2,
                    "voters_preview": play_intent.3.into_iter().map(|voter| json!({
                        "display_name": voter.display_name,
                        "avatar_url": avatar_url(&voter.avatar_public_id, voter.avatar_version),
                    })).collect::<Vec<_>>(),
                    "omitted_count": play_intent.4,
                },
                "reviews": {
                    "total": game.total_reviews,
                    "positive": game.total_positive,
                    "featured": popular_reviews.into_iter().map(|review| json!({
                        "recommendation_id": review.recommendation_id,
                        "rank": review.rank,
                        "author_name": review.author_name,
                        "author_profile_url": review.author_profile_url,
                        "text": review.review_text,
                        "voted_up": review.voted_up,
                        "votes_up": review.votes_up,
                        "votes_funny": review.votes_funny,
                        "comment_count": review.comment_count,
                        "playtime_forever_minutes": review.playtime_forever_minutes,
                        "playtime_at_review_minutes": review.playtime_at_review_minutes,
                        "created_at_ms": review.created_at_ms,
                        "written_during_early_access": review.written_during_early_access,
                    })).collect::<Vec<_>>(),
                },
                "latest_ccu": game.latest_ccu,
                "availability": {
                    "platforms": game.platforms,
                    "languages": game.languages,
                    "typical_session_minutes_min": game.typical_session_minutes_min,
                    "typical_session_minutes_max": game.typical_session_minutes_max,
                    "is_free": game.is_free,
                    "final_price_minor": game.final_price_minor,
                    "price_currency": game.price_currency,
                    "has_demo": game.has_demo,
                },
                "algorithm_version": active_config.version,
                "data_updated_at_ms": data_updated_at_ms,
            });
            (StatusCode::OK, [(header::ETAG, etag)], Json(body)).into_response()
        }
        Ok(None) => error_response(StatusCode::NOT_FOUND, "not_found", "game not found", None),
        Err(error) => map_storage_error(error, None),
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct EvidenceQuery {
    feature: Option<String>,
}

#[utoipa::path(
    get,
    path = "/v1/games/{app_id}/evidence",
    params(
        ("app_id" = u32, Path, description = "Steam AppID"),
        EvidenceQuery
    ),
    responses(
        (status = 200, body = EvidenceResponseSchema),
        (status = 400, body = ErrorBody),
        (status = 404, body = ErrorBody)
    ),
    tag = "catalog"
)]
async fn get_evidence(
    State(state): State<Arc<AppState>>,
    Path(app_id): Path<u32>,
    Query(query): Query<EvidenceQuery>,
) -> impl IntoResponse {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    if query
        .feature
        .as_ref()
        .is_some_and(|value| value.trim().is_empty() || value.len() > 64)
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "feature must contain between 1 and 64 bytes",
            None,
        );
    }
    let game = match storage_result(repo, move |repo| repo.game_detail(app_id)).await {
        Ok(Some(game)) => game,
        Ok(None) => {
            return error_response(StatusCode::NOT_FOUND, "not_found", "game not found", None);
        }
        Err(error) => return map_storage_error(error, None),
    };
    let feature = query.feature.clone();
    match storage_result(repo, move |repo| {
        repo.list_evidence(app_id, feature.as_deref())
    })
    .await
    {
        Ok(items) => {
            let mut seen_features = HashSet::new();
            let mut public_items: Vec<_> = items
                .iter()
                .map(|e| {
                    let canonical = seen_features.insert(e.feature_name.clone());
                    let evidence_id = if canonical {
                        format!("feature:{}:{app_id}", e.feature_name)
                    } else {
                        format!("feature:{}:{app_id}:{}", e.feature_name, e.evidence_id)
                    };
                    json!({
                        "evidence_id": evidence_id,
                        "feature": e.feature_name,
                        "value": serde_json::from_str::<serde_json::Value>(&e.value_json).unwrap_or(json!(null)),
                        "source_type": e.source_type,
                        "source_label": e.source_ref,
                        "confidence": e.confidence,
                        "observed_at_ms": e.observed_at_ms,
                    })
                })
                .collect();
            let data_updated_at_ms =
                match storage_result(repo, |repo| repo.data_updated_at_ms()).await {
                    Ok(value) => value,
                    Err(error) => return map_storage_error(error, None),
                };
            for (feature, value) in [
                ("private_session", game.private_session),
                ("self_hosted_server", game.self_hosted_server),
                ("online_coop", game.online_coop),
            ] {
                let requested = query.feature.as_deref().is_none_or(|name| name == feature);
                if requested
                    && !seen_features.contains(feature)
                    && let Some(value) = value
                {
                    public_items.push(json!({
                        "evidence_id": format!("feature:{feature}:{app_id}"),
                        "feature": feature,
                        "value": value,
                        "source_type": "computed_profile",
                        "source_label": "normalized multiplayer profile",
                        "confidence": game.profile_confidence.unwrap_or(0.4),
                        "observed_at_ms": data_updated_at_ms,
                    }));
                }
            }
            if query
                .feature
                .as_deref()
                .is_none_or(|name| name == "review_summary")
                && let Some(total) = game.total_reviews
            {
                public_items.push(json!({
                    "evidence_id": format!("review:{app_id}:summary"),
                    "feature": "review_summary",
                    "value": {
                        "total": total,
                        "positive": game.total_positive,
                        "wilson_lower": game.wilson_lower,
                    },
                    "source_type": "steam_reviews",
                    "source_label": "latest normalized review summary",
                    "confidence": 0.9,
                    "observed_at_ms": data_updated_at_ms,
                }));
            }
            let body = json!({
                "items": public_items,
            });
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(error) => map_storage_error(error, None),
    }
}

#[derive(Debug, Deserialize, ToSchema)]
struct FeedbackBody {
    app_id: u32,
    #[serde(rename = "type")]
    feedback_type: String,
    recommendation_run_id: Option<String>,
    client_created_at_ms: Option<i64>,
}

#[utoipa::path(
    post,
    path = "/v1/feedback",
    request_body = FeedbackBody,
    params(("Idempotency-Key" = String, Header, description = "Client-generated idempotency key")),
    responses(
        (status = 201, body = FeedbackResponseSchema),
        (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody),
        (status = 409, body = ErrorBody)
    ),
    security(("bearer_auth" = [])),
    tag = "feedback"
)]
async fn post_feedback(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<FeedbackBody>,
) -> impl IntoResponse {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let user_id = match require_user(repo, &headers).await {
        Ok(id) => id,
        Err(resp) => return *resp,
    };
    let Some(feedback_type) = FeedbackType::parse(&body.feedback_type) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "unknown feedback type",
            None,
        );
    };
    let Some(idem) = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
    else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "Idempotency-Key header is required",
            None,
        );
    };
    match storage_result(repo, move |repo| {
        repo.create_feedback(
            &user_id,
            body.app_id,
            feedback_type,
            body.recommendation_run_id.as_deref(),
            &idem,
            body.client_created_at_ms,
        )
    })
    .await
    {
        Ok(record) => (StatusCode::CREATED, Json(record_json(&record))).into_response(),
        Err(error) => map_storage_error(error, None),
    }
}

#[utoipa::path(
    post,
    path = "/v1/feedback/{feedback_id}/undo",
    params(("feedback_id" = i64, Path, description = "Feedback event ID")),
    responses(
        (status = 200, body = FeedbackResponseSchema),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody)
    ),
    security(("bearer_auth" = [])),
    tag = "feedback"
)]
async fn undo_feedback(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(feedback_id): Path<i64>,
) -> impl IntoResponse {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let user_id = match require_user(repo, &headers).await {
        Ok(id) => id,
        Err(resp) => return *resp,
    };
    match storage_result(repo, move |repo| repo.undo_feedback(&user_id, feedback_id)).await {
        Ok(record) => (StatusCode::OK, Json(record_json(&record))).into_response(),
        Err(error) => map_storage_error(error, None),
    }
}

fn record_json(record: &mpgs_storage::feedback::FeedbackRecord) -> serde_json::Value {
    json!({
        "feedback_id": record.feedback_id,
        "app_id": record.app_id,
        "type": record.feedback_type,
        "recommendation_run_id": record.recommendation_run_id,
        "created_at_ms": record.created_at_ms,
    })
}

#[derive(Debug, Deserialize, ToSchema)]
struct PlayIntentBody {
    /// True to cast a "want to play" vote, false to withdraw it.
    intent: bool,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct PlayIntentResponseSchema {
    app_id: u32,
    count: u32,
    voted: bool,
}

#[utoipa::path(
    post,
    path = "/v1/games/{app_id}/play-intent",
    params(("app_id" = u32, Path, description = "Steam AppID")),
    request_body = PlayIntentBody,
    responses(
        (status = 200, body = PlayIntentResponseSchema),
        (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody)
    ),
    security(("bearer_auth" = [])),
    tag = "feedback"
)]
async fn set_play_intent(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(app_id): Path<u32>,
    Json(body): Json<PlayIntentBody>,
) -> impl IntoResponse {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let user_id = match require_user(repo, &headers).await {
        Ok(id) => id,
        Err(resp) => return *resp,
    };
    let intent = body.intent;
    let vote_user_id = user_id.clone();
    let vote = match storage_result(repo, move |repo| {
        repo.set_play_intent(&vote_user_id, app_id, intent)
    })
    .await
    {
        Ok(vote) => vote,
        Err(error) => return map_storage_error(error, None),
    };
    let preview_user_id = user_id.clone();
    match storage_result(repo, move |repo| {
        repo.community_game_previews(app_id, Some(&preview_user_id))
    })
    .await
    {
        Ok((_epoch, count, voted, previews, omitted_count)) => (
            StatusCode::OK,
            Json(json!({
                "app_id": vote.app_id,
                "count": count,
                "voted": voted,
                "voters_preview": previews.into_iter().map(|voter| json!({
                    "display_name": voter.display_name,
                    "avatar_url": avatar_url(&voter.avatar_public_id, voter.avatar_version),
                })).collect::<Vec<_>>(),
                "omitted_count": omitted_count,
            })),
        )
            .into_response(),
        Err(error) => map_storage_error(error, None),
    }
}

// --- admin / internal (M2) ---

#[derive(Debug, Deserialize)]
struct CreateOverrideBody {
    feature_name: String,
    value: serde_json::Value,
    reason: String,
    external_evidence: Option<String>,
    operator: String,
}

#[derive(Debug, Deserialize)]
struct RevokeOverrideBody {
    operator: String,
    reason: String,
}

#[derive(Debug, Deserialize)]
struct LeaseJobsBody {
    owner: String,
    limit: Option<i64>,
    lease_ms: Option<i64>,
    source: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CompleteJobBody {
    owner: String,
    idempotency_key: String,
}

#[derive(Debug, Deserialize)]
struct FailJobBody {
    owner: String,
    error_category: String,
    retry_delay_ms: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct EnqueueJobBody {
    source: String,
    task_type: String,
    entity_key: String,
    priority: Option<i64>,
    due_at_ms: Option<i64>,
    idempotency_key: String,
    payload_json: Option<serde_json::Value>,
    max_attempts: Option<i64>,
}

async fn create_override(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(app_id): Path<u32>,
    Json(body): Json<CreateOverrideBody>,
) -> impl IntoResponse {
    if let Err(resp) = require_admin(&state, &headers) {
        return *resp;
    }
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let request = CreateOverrideRequest {
        feature_name: body.feature_name,
        value_json: body.value,
        reason: body.reason,
        external_evidence: body.external_evidence,
        operator: body.operator,
        request_id: header_str(&headers, "x-request-id"),
    };
    match storage_result(repo, move |repo| repo.create_override(app_id, &request)).await {
        Ok(over) => (StatusCode::CREATED, Json(over)).into_response(),
        Err(error) => map_storage_error(error, None),
    }
}

async fn revoke_override(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(body): Json<RevokeOverrideBody>,
) -> impl IntoResponse {
    if let Err(resp) = require_admin(&state, &headers) {
        return *resp;
    }
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let request_id = header_str(&headers, "x-request-id");
    match storage_result(repo, move |repo| {
        repo.revoke_override(id, &body.operator, &body.reason, request_id.as_deref())
    })
    .await
    {
        Ok(over) => (StatusCode::OK, Json(over)).into_response(),
        Err(error) => map_storage_error(error, None),
    }
}

async fn game_debug(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(app_id): Path<u32>,
) -> impl IntoResponse {
    if let Err(resp) = require_admin(&state, &headers) {
        return *resp;
    }
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let (app, profile) = match storage_result(repo, move |repo| {
        Ok((repo.get_app(app_id)?, repo.get_profile(app_id)?))
    })
    .await
    {
        Ok(v) => v,
        Err(error) => return map_storage_error(error, None),
    };
    (
        StatusCode::OK,
        Json(json!({"app": app, "multiplayer_profile": profile})),
    )
        .into_response()
}

async fn data_status(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(response) = require_admin(&state, &headers) {
        return *response;
    }
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let tasks = match storage_result(repo, |repo| repo.data_refresh_status()).await {
        Ok(tasks) => tasks,
        Err(error) => return map_storage_error(error, None),
    };
    let coverage = match storage_result(repo, |repo| repo.m3_catalog_coverage()).await {
        Ok(coverage) => coverage,
        Err(error) => return map_storage_error(error, None),
    };
    let m7_coverage = match storage_result(repo, |repo| {
        let active = repo.active_algorithm_config()?;
        repo.m7_data_coverage(&active.config)
    })
    .await
    {
        Ok(coverage) => coverage,
        Err(error) => return map_storage_error(error, None),
    };
    (
        StatusCode::OK,
        Json(json!({
            "tasks": tasks,
            "coverage": coverage,
            "m7_coverage": m7_coverage,
        })),
    )
        .into_response()
}

async fn enqueue_job(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<EnqueueJobBody>,
) -> impl IntoResponse {
    if let Err(resp) = require_admin(&state, &headers) {
        return *resp;
    }
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let now = repo.database().now_ms();
    let job = EnqueueJob {
        source: body.source,
        task_type: body.task_type,
        entity_key: body.entity_key,
        priority: body.priority.unwrap_or(100),
        due_at_ms: body.due_at_ms.unwrap_or(now),
        idempotency_key: body.idempotency_key,
        payload_json: body.payload_json.map(|v| v.to_string()),
        max_attempts: body.max_attempts.unwrap_or(5),
    };
    match storage_result(repo, move |repo| repo.enqueue_job(&job)).await {
        Ok(job_id) => (StatusCode::CREATED, Json(json!({"job_id": job_id}))).into_response(),
        Err(error) => map_storage_error(error, None),
    }
}

async fn lease_jobs(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<LeaseJobsBody>,
) -> impl IntoResponse {
    if let Err(resp) = require_admin(&state, &headers) {
        return *resp;
    }
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    match storage_result(repo, move |repo| {
        repo.lease_jobs(
            &body.owner,
            body.limit.unwrap_or(10),
            body.lease_ms.unwrap_or(60_000),
            body.source.as_deref(),
        )
    })
    .await
    {
        Ok(jobs) => (StatusCode::OK, Json(jobs)).into_response(),
        Err(error) => map_storage_error(error, None),
    }
}

async fn complete_job(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(job_id): Path<i64>,
    Json(body): Json<CompleteJobBody>,
) -> impl IntoResponse {
    if let Err(resp) = require_admin(&state, &headers) {
        return *resp;
    }
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    match storage_result(repo, move |repo| {
        repo.complete_job(job_id, &body.owner, &body.idempotency_key)
    })
    .await
    {
        Ok(job) => (StatusCode::OK, Json(job)).into_response(),
        Err(error) => map_storage_error(error, None),
    }
}

async fn fail_job(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(job_id): Path<i64>,
    Json(body): Json<FailJobBody>,
) -> impl IntoResponse {
    if let Err(resp) = require_admin(&state, &headers) {
        return *resp;
    }
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    match storage_result(repo, move |repo| {
        repo.fail_job(
            job_id,
            &body.owner,
            &body.error_category,
            body.retry_delay_ms.unwrap_or(60_000),
        )
    })
    .await
    {
        Ok(job) => (StatusCode::OK, Json(job)).into_response(),
        Err(error) => map_storage_error(error, None),
    }
}

// --- helpers ---

fn require_repo(state: &AppState) -> Option<&Repository> {
    state.repo.as_ref()
}

#[derive(Debug, Clone)]
struct AccountAuth {
    user_id: String,
    access_token: String,
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
}

async fn require_account(
    repo: &Repository,
    headers: &HeaderMap,
) -> Result<AccountAuth, Box<Response>> {
    let token = bearer_token(headers).ok_or_else(|| {
        Box::new(error_response(
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
            "sign in to continue",
            None,
        ))
    })?;
    let token_for_lookup = token.clone();
    storage_result(repo, move |repo| {
        repo.resolve_account_access_token(&token_for_lookup)
    })
    .await
    .map(|user_id| AccountAuth {
        user_id,
        access_token: token,
    })
    .map_err(|error| Box::new(map_account_error(error)))
}

async fn require_user(repo: &Repository, headers: &HeaderMap) -> Result<String, Box<Response>> {
    require_account(repo, headers)
        .await
        .map(|account| account.user_id)
}

async fn optional_account_user_id(repo: &Repository, headers: &HeaderMap) -> Option<String> {
    let token = bearer_token(headers)?;
    storage_result(repo, move |repo| repo.resolve_account_access_token(&token))
        .await
        .ok()
}

async fn optional_anonymous_user_id(repo: &Repository, headers: &HeaderMap) -> Option<String> {
    let token = bearer_token(headers)?;
    storage_result(repo, move |repo| {
        repo.resolve_anonymous_access_token(&token)
    })
    .await
    .ok()
}

async fn user_context(
    repo: &Repository,
    headers: &HeaderMap,
) -> Result<(UserPreferences, Option<String>), Box<Response>> {
    let Some(user_id) = optional_account_user_id(repo, headers).await else {
        return Ok((UserPreferences::default(), None));
    };
    let lookup_user_id = user_id.clone();
    let preferences = storage_result(repo, move |repo| repo.get_preferences(&lookup_user_id))
        .await
        .map_err(|error| Box::new(map_storage_error(error, None)))?;
    Ok((preferences, Some(user_id)))
}

/// Resolve a user id when a valid bearer token is present; `None` for anonymous
/// requests or stale tokens (public endpoints must not 401 on an old token).
async fn optional_user_id(repo: &Repository, headers: &HeaderMap) -> Option<String> {
    optional_account_user_id(repo, headers).await
}

fn map_account_error(error: StorageError) -> Response {
    if matches!(error, StorageError::NotFound { .. }) {
        error_response(
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
            "sign in to continue",
            None,
        )
    } else {
        map_storage_error(error, None)
    }
}

async fn storage_result<T, F>(repo: &Repository, operation: F) -> Result<T, StorageError>
where
    T: Send + 'static,
    F: FnOnce(Repository) -> Result<T, StorageError> + Send + 'static,
{
    let repo = repo.clone();
    tokio::task::spawn_blocking(move || operation(repo))
        .await
        .map_err(|error| StorageError::Io(std::io::Error::other(error.to_string())))?
}

fn encode_cursor(
    section: FeedSection,
    snapshot_ms: i64,
    preference_context: &str,
    play_intent_revision: u64,
    offset: usize,
) -> String {
    format!(
        "v2:{}:{snapshot_ms}:{preference_context}:{play_intent_revision}:{offset}",
        section.as_str()
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CursorError {
    Invalid,
    Stale,
}

fn decode_cursor(
    cursor: Option<&str>,
    expected_section: FeedSection,
    expected_snapshot_ms: i64,
    expected_preference_context: &str,
    expected_play_intent_revision: u64,
) -> Result<usize, CursorError> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let mut parts = cursor.split(':');
    let version = parts.next();
    if version == Some("v1") {
        return Err(CursorError::Stale);
    }
    let valid_version = version == Some("v2");
    let section = parts.next();
    let snapshot = parts.next().and_then(|value| value.parse::<i64>().ok());
    let preference_context = parts.next();
    let play_intent_revision = parts.next().and_then(|value| value.parse::<u64>().ok());
    let offset = parts.next().and_then(|value| value.parse::<usize>().ok());
    if !valid_version
        || section != Some(expected_section.as_str())
        || snapshot.is_none()
        || preference_context.is_none()
        || play_intent_revision.is_none()
        || offset.is_none()
        || parts.next().is_some()
    {
        return Err(CursorError::Invalid);
    }
    if snapshot != Some(expected_snapshot_ms)
        || preference_context != Some(expected_preference_context)
        || play_intent_revision != Some(expected_play_intent_revision)
    {
        return Err(CursorError::Stale);
    }
    Ok(offset.expect("validated above"))
}

fn recommendation_context(
    prefs: &UserPreferences,
    feedback: &[mpgs_storage::feedback::ActiveFeedback],
    algorithm_version: &str,
    config: &RecommendationConfig,
) -> String {
    let payload = serde_json::to_string(prefs).unwrap_or_else(|_| format!("v{}", prefs.version));
    let mut context = payload;
    context.push('|');
    context.push_str(algorithm_version);
    context.push('|');
    context.push_str(&serde_json::to_string(config).unwrap_or_default());
    for item in feedback {
        context.push('|');
        context.push_str(&item.app_id.to_string());
        context.push(':');
        context.push_str(&item.feedback_type);
    }
    format!("{:x}", hash64(&context))
}

fn apply_feed_overrides(
    prefs: &mut UserPreferences,
    query: &FeedQuery,
) -> Result<(), Box<Response>> {
    if let Some(value) = query.coop_competitive {
        prefs.coop_competitive = value;
    }
    if let Some(value) = query.self_hosting_willingness {
        prefs.self_hosting_willingness = value;
    }
    if let Some(platforms) = query.platforms.as_deref() {
        prefs.platforms = parse_csv_filter("platforms", platforms)?;
    }
    if let Some(languages) = query.languages.as_deref() {
        prefs.languages = parse_csv_filter("languages", languages)?;
    }
    if let Some(value) = query.session_minutes_min {
        prefs.session_minutes_min = value;
    }
    if let Some(value) = query.session_minutes_max {
        prefs.session_minutes_max = value;
    }
    if let Some(value) = query.max_price_minor {
        prefs.budget_max_each_minor = Some(value);
    }
    if let Some(value) = query.currency.as_deref() {
        prefs.budget_currency = value.to_ascii_uppercase();
    }
    prefs.validate().map_err(|message| {
        Box::new(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            &message,
            None,
        ))
    })
}

fn parse_csv_filter(name: &str, value: &str) -> Result<Vec<String>, Box<Response>> {
    let values: Vec<_> = value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    if values.is_empty() || values.len() > 32 || values.iter().any(|item| item.len() > 64) {
        return Err(Box::new(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            &format!("{name} must be a comma-separated list of 1 to 32 values"),
            None,
        )));
    }
    Ok(values)
}

fn weak_etag(payload: &str) -> HeaderValue {
    let hash = hash64(payload);
    HeaderValue::from_str(&format!("W/\"{hash:x}\"")).unwrap_or(HeaderValue::from_static("W/\"0\""))
}

fn hash64(payload: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in payload.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn hex_sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn if_none_match_ok(headers: &HeaderMap, etag: &HeaderValue) -> Option<Response> {
    let inm = headers.get(header::IF_NONE_MATCH)?.to_str().ok()?;
    if inm == etag.to_str().ok()? {
        return Some(StatusCode::NOT_MODIFIED.into_response());
    }
    None
}

fn require_admin(state: &AppState, headers: &HeaderMap) -> Result<(), Box<Response>> {
    let Some(expected) = state.admin_token.as_deref() else {
        return Err(Box::new(error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            "MPGS_ADMIN_TOKEN is not configured",
            None,
        )));
    };
    let provided = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if provided != Some(expected) {
        return Err(Box::new(error_response(
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
            "invalid admin token",
            None,
        )));
    }
    Ok(())
}

fn storage_disabled() -> Response {
    error_response(
        StatusCode::SERVICE_UNAVAILABLE,
        "temporarily_unavailable",
        "storage is disabled",
        None,
    )
}

fn map_storage_error(error: StorageError, request_id: Option<String>) -> Response {
    let (status, code) = match &error {
        StorageError::NotFound { .. } => (StatusCode::NOT_FOUND, "not_found"),
        StorageError::Validation { .. } => (StatusCode::BAD_REQUEST, "invalid_argument"),
        StorageError::Conflict { .. } => (StatusCode::CONFLICT, "version_conflict"),
        StorageError::Lease { .. } => (StatusCode::CONFLICT, "version_conflict"),
        StorageError::Migration { .. } => {
            (StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable")
        }
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
    };
    let message = match status {
        StatusCode::INTERNAL_SERVER_ERROR => "internal server error".to_owned(),
        StatusCode::SERVICE_UNAVAILABLE => "storage is temporarily unavailable".to_owned(),
        _ => error.to_string(),
    };
    if status.is_server_error() {
        tracing::error!(%error, http_status = status.as_u16(), "storage request failed");
    }
    error_response(status, code, &message, request_id)
}

fn error_response(
    status: StatusCode,
    code: &str,
    message: &str,
    request_id: Option<String>,
) -> Response {
    let request_id = request_id.or_else(|| CURRENT_REQUEST_ID.try_with(Clone::clone).ok());
    (
        status,
        Json(ErrorBody {
            error: ErrorDetail {
                code: code.into(),
                message: message.into(),
                request_id,
            },
        }),
    )
        .into_response()
}

fn header_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn avatar_processing_crops_to_square_webp_and_rejects_mismatched_media() {
        let source =
            image::RgbaImage::from_fn(320, 160, |x, y| image::Rgba([x as u8, y as u8, 180, 255]));
        let mut png = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(source)
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
        let png = png.into_inner();

        let encoded = process_avatar_image(&png, Some("image/png")).unwrap();
        let avatar = image::load_from_memory_with_format(&encoded, ImageFormat::WebP).unwrap();
        assert_eq!((avatar.width(), avatar.height()), (128, 128));

        // Generic browser content-type + magic-byte sniffing.
        let via_octet =
            process_avatar_image(&png, Some("application/octet-stream")).expect("octet-stream");
        assert_eq!(via_octet.len(), encoded.len());
        let via_empty = process_avatar_image(&png, None).expect("missing content-type");
        assert_eq!(via_empty.len(), encoded.len());
        // Mislabeled content-type falls back to sniffing.
        let via_mislabeled =
            process_avatar_image(&png, Some("image/jpeg")).expect("mislabeled jpeg header");
        assert_eq!(via_mislabeled.len(), encoded.len());

        assert!(process_avatar_image(b"<svg/>", Some("image/svg+xml")).is_err());
        assert!(process_avatar_image(b"<svg/>", Some("application/octet-stream")).is_err());
    }

    #[test]
    fn ai_degradation_keeps_safe_scores_but_discards_unverified_text() {
        let candidates = vec![CandidateEvidence {
            app_id: 42,
            evidence_ids: HashSet::from(["allowed".to_owned()]),
        }];
        let raw = json!({
            "recommendations": [{
                "app_id": 42,
                "fit_score": 0.8,
                "confidence": 0.7,
                "reason_evidence_ids": ["invented"],
                "reasons": ["unsupported claim"],
                "cautions": []
            }],
            "summary": "unsupported summary",
            "summary_evidence_ids": ["invented"]
        });
        let (result, notice) =
            validate_rank_result_with_safe_degradation(&raw, &candidates, 1).unwrap();
        assert_eq!(result.recommendations[0].fit_score, 0.8);
        assert!(result.recommendations[0].reasons.is_empty());
        assert!(result.summary.is_empty());
        assert!(notice.is_some());

        let unknown = json!({
            "recommendations": [{"app_id": 999, "fit_score": 0.8, "confidence": 0.7}],
            "summary": "",
            "summary_evidence_ids": []
        });
        assert!(validate_rank_result_with_safe_degradation(&unknown, &candidates, 1).is_err());
    }
}
