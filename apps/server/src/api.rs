//! HTTP routes for health, public catalog/recommendation, admin, and internal jobs.

use axum::body::Body;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use mpgs_ai::{
    AiGateway, AiStatus, AiTaskType, CandidateEvidence, EmbeddingInput, EmbeddingProvider,
    RANK_PROMPT_VERSION, StructuredRequest, rank_analysis_schema, rank_analysis_system_prompt,
    validate_rank_result, wrap_untrusted_data_block,
};
use mpgs_domain::{
    FeedSection, FeedbackType, RankingSignals, RecommendationConfig, UserPreferences,
};
use mpgs_recommender::{
    ALGORITHM_VERSION, AiAdjustment, RankingInput, blend_ai, friend_fit, rank_feed_configured,
};
use mpgs_storage::{CreateOverrideRequest, EnqueueJob, Repository, StorageError};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::{OpenApi, ToSchema};

use crate::cors::CorsConfig;
use crate::rate_limit::{RateLimitConfig, RateLimiter};

tokio::task_local! {
    static CURRENT_REQUEST_ID: String;
}
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(Clone)]
pub struct AppState {
    pub repo: Option<Repository>,
    pub admin_token: Option<String>,
    pub rate_limits: RateLimitConfig,
    pub cors: CorsConfig,
    pub ai: AiGateway,
    pub embedding: Arc<dyn EmbeddingProvider>,
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
    snapshot_at_ms: i64,
    algorithm_version: String,
    data_updated_at_ms: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
struct NaturalLanguageRequest {
    query: String,
    limit: Option<i64>,
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
        version = "0.1.0",
        description = "Deterministic friend-group multiplayer game recommendation API"
    ),
    paths(
        health_live,
        health_ready,
        meta,
        create_session,
        refresh_session,
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
        .route("/v1/preferences", get(get_preferences).put(put_preferences))
        .route("/v1/feeds/{section}", get(get_feed))
        .route(
            "/v1/recommendations/natural-language",
            post(natural_language_recommendations),
        )
        .route("/v1/calendar", get(get_calendar))
        .route("/v1/search", get(search_games))
        .route("/v1/games/{app_id}", get(get_game))
        .route("/v1/games/{app_id}/evidence", get(get_evidence))
        .route("/v1/games/{app_id}/play-intent", post(set_play_intent))
        .route("/v1/feedback", post(post_feedback))
        .route("/v1/feedback/{feedback_id}/undo", post(undo_feedback))
        .route("/admin/v1/games/{app_id}/overrides", post(create_override))
        .route("/admin/v1/overrides/{id}/revoke", post(revoke_override))
        .route("/admin/v1/games/{app_id}/debug", get(game_debug))
        .route("/internal/v1/jobs/enqueue", post(enqueue_job))
        .route("/internal/v1/jobs/lease", post(lease_jobs))
        .route("/internal/v1/jobs/{job_id}/complete", post(complete_job))
        .route("/internal/v1/jobs/{job_id}/fail", post(fail_job))
        .layer(DefaultBodyLimit::max(64 * 1024))
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
    };
    let etag = weak_etag(&format!(
        "{}:{}:{}:{}:{}:{}",
        body.service_version,
        body.algorithm_version,
        body.schema_version.unwrap_or(-1),
        body.build_git_sha,
        body.data_updated_at_ms.unwrap_or(0),
        body.storage_enabled
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
    let offset = match decode_cursor(
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
    };
    let cache_identity = user_id.as_deref().unwrap_or("public");
    let etag = weak_etag(&format!(
        "feed:v2:{}:{snapshot_ms}:{preference_context}:{offset}:{limit}:{}:pi{}:user{cache_identity}",
        section.as_str(),
        active_config.version,
        play_intent.epoch.revision,
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
            section_matches(
                section,
                &row,
                &signals,
                &cutoff,
                &today,
                &active_config.config,
            )
            .then_some(RankingInput {
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
    let total = ranked.items.len();
    let page: Vec<_> = ranked
        .items
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|item| {
            json!({
                "app_id": item.app_id,
                "name": item.name,
                "section": section.as_str(),
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

    let next_offset = offset.saturating_add(limit);
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
        "snapshot_at_ms": snapshot_ms,
        "algorithm_version": active_config.version,
        "data_updated_at_ms": snapshot_ms,
    });
    (StatusCode::OK, [(header::ETAG, etag)], Json(body)).into_response()
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

    let interpreted = interpret_natural_language(&query);
    let feed_query = FeedQuery {
        // Rank and validate the required Top 20, then truncate the public response.
        limit: Some(20),
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
    };
    let feed_response = get_feed(
        State(state.clone()),
        headers,
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
    let (ai_status, fallback_reason, ai_summary, ai_summary_evidence_ids) =
        enhance_natural_language_with_ai(&state.ai, state.repo.as_ref(), &query, &mut items).await;
    items.truncate(output_limit as usize);
    Json(json!({
        "query": query,
        "interpreted": interpreted,
        "items": items,
        "ai_status": ai_status.as_str(),
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

/// Apply optional AI Top-N analysis. Always preserves deterministic items on failure.
async fn enhance_natural_language_with_ai(
    gateway: &AiGateway,
    repo: Option<&Repository>,
    query: &str,
    items: &mut Vec<serde_json::Value>,
) -> (AiStatus, Option<String>, Option<String>, Vec<String>) {
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
            // Score is excluded from the cache key material: only identity/evidence matter.
            "reasons": item.get("reasons").cloned().unwrap_or(json!([])),
            "cautions": item.get("cautions").cloned().unwrap_or(json!([])),
            "evidence_ids": evidence_list,
        }));
        candidates.push(CandidateEvidence {
            app_id,
            evidence_ids,
        });
    }
    if candidates.is_empty() {
        return (
            AiStatus::Fallback,
            Some("no valid candidates for AI analysis".into()),
            None,
            Vec::new(),
        );
    }

    let provider_identity = gateway.provider_cache_identity();
    let cache_key = nl_ai_cache_key(query, &compact, &provider_identity);
    if let Some(repo) = repo {
        let now_ms = chrono_like_now_ms();
        if let Ok(Some(entry)) =
            storage_result(repo, move |repo| repo.get_ai_cache(&cache_key, now_ms)).await
            && let Ok(content) = serde_json::from_str::<serde_json::Value>(&entry.output_json)
            && let Ok(validated) = validate_rank_result(&content, &candidates, items.len().max(1))
        {
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
    }

    let data_prompt = format!(
        "{}\n\n{}",
        wrap_untrusted_data_block("user_query", query, 500),
        wrap_untrusted_data_block(
            "candidates_json",
            &serde_json::to_string(&compact).unwrap_or_else(|_| "[]".into()),
            12_000
        )
    );
    let request = StructuredRequest {
        task: AiTaskType::RankAnalysis,
        system_prompt: rank_analysis_system_prompt().to_owned(),
        data_prompt,
        json_schema_name: "rank_analysis".into(),
        json_schema: rank_analysis_schema(),
        max_output_tokens: 1_200,
        temperature: 0.2,
    };

    let response = match gateway.structured_completion(request).await {
        Ok(response) => response,
        Err(error) => {
            return (
                AiStatus::Fallback,
                Some(error.fallback_reason().to_owned()),
                None,
                Vec::new(),
            );
        }
    };

    let validated = match validate_rank_result(&response.content, &candidates, items.len().max(1)) {
        Ok(result) => result,
        Err(error) => {
            return (
                AiStatus::Fallback,
                Some(error.fallback_reason().to_owned()),
                None,
                Vec::new(),
            );
        }
    };

    if let Some(repo) = repo {
        let now_ms = chrono_like_now_ms();
        let output_json = response.content.to_string();
        let entry = mpgs_storage::AiCacheEntry {
            cache_key: nl_ai_cache_key(query, &compact, &provider_identity),
            task_type: "rank_analysis".into(),
            provider: response.provider.clone(),
            model: response.model.clone(),
            prompt_version: RANK_PROMPT_VERSION.into(),
            input_hash: nl_ai_cache_key(query, &compact, &provider_identity),
            output_json,
            validation_status: "accepted".into(),
            usage_input: i64::from(response.usage_input),
            usage_output: i64::from(response.usage_output),
            created_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(6 * 60 * 60 * 1000),
        };
        let _ = storage_result(repo, move |repo| repo.put_ai_cache(&entry)).await;
    }

    apply_validated_ai_rank(items, &validated);

    let summary = if validated.summary.is_empty() {
        None
    } else {
        Some(validated.summary)
    };
    (
        AiStatus::Used,
        None,
        summary,
        validated.summary_evidence_ids,
    )
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

fn nl_ai_cache_key(query: &str, compact: &[serde_json::Value], provider_identity: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(provider_identity.as_bytes());
    hasher.update([0]);
    hasher.update(RANK_PROMPT_VERSION.as_bytes());
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
        "calendar:v2:{calendar_state}:{from}:{to}:{data_updated_at_ms}"
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
                    let review_total = repo
                        .game_detail(item.app_id)?
                        .and_then(|detail| detail.total_reviews);
                    Ok(calendar_item_json(item, review_total))
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
) -> serde_json::Value {
    json!({
        "app_id": item.app_id,
        "app_type": item.app_type,
        "canonical_name": item.canonical_name,
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
        repo.play_intent_game_snapshot(snapshot_user_id.as_deref(), app_id)
    })
    .await
    {
        Ok(value) => value,
        Err(error) => return map_storage_error(error, None),
    };
    let cache_identity = user_id.as_deref().unwrap_or("public");
    let etag = weak_etag(&format!(
        "game:v2:{app_id}:{data_updated_at_ms}:{}:pi{}:{}:{}:user{cache_identity}",
        active_config.version, play_intent.epoch.revision, play_intent.count, play_intent.voted,
    ));
    if let Some(response) = if_none_match_ok(&headers, &etag) {
        return response;
    }
    match storage_result(repo, move |repo| repo.game_detail(app_id)).await {
        Ok(Some(game)) => {
            let body = json!({
                "app_id": game.app_id,
                "name": game.name,
                "app_type": game.app_type,
                "release_state": game.release_state,
                "release_date": game.release_date,
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
                    "count": play_intent.count,
                    "voted": play_intent.voted,
                },
                "reviews": {
                    "total": game.total_reviews,
                    "positive": game.total_positive,
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
    match storage_result(repo, move |repo| {
        repo.set_play_intent(&user_id, app_id, intent)
    })
    .await
    {
        Ok(vote) => (
            StatusCode::OK,
            Json(json!({
                "app_id": vote.app_id,
                "count": vote.count,
                "voted": vote.voted,
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

async fn require_user(repo: &Repository, headers: &HeaderMap) -> Result<String, Box<Response>> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_owned)
        .ok_or_else(|| {
            Box::new(error_response(
                StatusCode::UNAUTHORIZED,
                "unauthenticated",
                "missing bearer token",
                None,
            ))
        })?;
    storage_result(repo, move |repo| repo.resolve_access_token(&token))
        .await
        .map_err(|error| {
            if matches!(error, StorageError::NotFound { .. }) {
                Box::new(error_response(
                    StatusCode::UNAUTHORIZED,
                    "unauthenticated",
                    "invalid or expired bearer token",
                    None,
                ))
            } else {
                Box::new(map_storage_error(error, None))
            }
        })
}

async fn user_context(
    repo: &Repository,
    headers: &HeaderMap,
) -> Result<(UserPreferences, Option<String>), Box<Response>> {
    if !headers.contains_key(header::AUTHORIZATION) {
        return Ok((UserPreferences::default(), None));
    }
    let user_id = require_user(repo, headers).await?;
    let lookup_user_id = user_id.clone();
    let preferences = storage_result(repo, move |repo| repo.get_preferences(&lookup_user_id))
        .await
        .map_err(|error| Box::new(map_storage_error(error, None)))?;
    Ok((preferences, Some(user_id)))
}

/// Resolve a user id when a valid bearer token is present; `None` for anonymous
/// requests or stale tokens (public endpoints must not 401 on an old token).
async fn optional_user_id(repo: &Repository, headers: &HeaderMap) -> Option<String> {
    if !headers.contains_key(header::AUTHORIZATION) {
        return None;
    }
    require_user(repo, headers).await.ok()
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

fn section_matches(
    section: FeedSection,
    row: &mpgs_storage::query::GameCandidateRow,
    signals: &RankingSignals,
    cutoff_date: &str,
    today: &str,
    config: &RecommendationConfig,
) -> bool {
    let friend_score = friend_fit(&signals.multiplayer);
    let activity = row.typical_ccu_7d.or(row.latest_ccu).unwrap_or(0);
    let date = row.release_date.as_deref();
    match section {
        FeedSection::Upcoming => {
            let has_multiplayer_evidence = row.dominant_mode.is_some()
                || row.private_session == Some(true)
                || row.online_coop == Some(true)
                || row.self_hosted_server == Some(true);
            (row.release_state == "upcoming"
                || row.release_state == "coming_soon"
                || row.app_type == "demo"
                || row.app_type == "playtest")
                && has_multiplayer_evidence
        }
        FeedSection::RecentRelease => {
            row.release_state == "released"
                && date.is_some_and(|value| value >= cutoff_date && value <= today)
                && friend_score >= config.recent_min_friend_fit
        }
        FeedSection::PopularLegacy => {
            let quality_floor = if activity >= config.popular_high_ccu {
                config.popular_high_ccu_min_wilson
            } else {
                config.popular_min_wilson
            };
            row.release_state == "released"
                && date.is_some_and(|value| value < cutoff_date)
                && activity >= config.popular_min_ccu
                && row.wilson_lower.is_some_and(|value| value >= quality_floor)
                && friend_score >= config.popular_min_friend_fit
        }
        FeedSection::ClassicLegacy => {
            let friend_group_independent =
                row.private_session == Some(true) || row.self_hosted_server == Some(true);
            row.release_state == "released"
                && date.is_some_and(|value| value < cutoff_date)
                && row
                    .total_reviews
                    .is_some_and(|value| value >= config.classic_min_reviews)
                && row
                    .wilson_lower
                    .is_some_and(|value| value >= config.classic_min_wilson)
                && friend_score >= config.classic_min_friend_fit
                && (friend_group_independent || activity >= config.classic_public_min_ccu)
        }
    }
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
