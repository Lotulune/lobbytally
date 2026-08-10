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
use image::{DynamicImage, ImageFormat, ImageReader, Limits, imageops::FilterType};
use mpgs_ai::{
    AiError, AiGateway, AiPolicy, AiProvider, AiRankResult, AiStatus, AiTaskType, AppVoteCount,
    COMPARE_PROMPT_VERSION, CandidateEvidence, DEFAULT_ROUTE_VERSION, EmbeddingInput,
    EmbeddingProvider, GroupAdviceRequest as AiGroupAdviceRequest, OpenAiCompatProvider,
    RANK_PROMPT_VERSION, RuleIntentBaseline, SUMMARY_PROMPT_VERSION, StructuredRequest, TaskRouter,
    compare_schema, compare_system_prompt, deterministic_group_advice, group_advice_schema,
    group_advice_system_prompt, intent_parse_schema, intent_parse_system_prompt,
    merge_intent_with_rules, parse_compare_explanation, parse_group_advice,
    parse_structured_intent, rank_analysis_schema, rank_analysis_system_prompt, rule_game_summary,
    validate_rank_result, wrap_untrusted_data_block,
};
use mpgs_domain::{FeedSection, FeedbackType, ModeFamily, RecommendationConfig, UserPreferences};
use mpgs_recommender::{
    ALGORITHM_VERSION, AiAdjustment, Explanation, HardConstraints, RankedCandidate, RankingInput,
    ScoreBreakdown, SlotReason, blend_ai, mmr_rerank_with_tie_seed,
    rank_feed_configured_with_constraints_and_tie_seed,
};
use mpgs_storage::{
    CreateOverrideRequest, EnqueueJob, HybridHit, InsertRecommendationEvent,
    InsertRecommendationItem, InsertRecommendationRun, Repository, StorageError,
    accounts::{AiMode, LoginAccount, PreferenceChoice, PutAiSettings, RegisterAccount},
    community::{CommunityFilters, CommunitySort},
    hash_candidate_set, hash_structured_context,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::{OpenApi, ToSchema};

use crate::ai_limits::{AccountAiLimiter, AccountAiPermit};
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
const AI_CALL_WINDOW: Duration = Duration::from_secs(60);
const AI_CALLS_PER_ACCOUNT: u32 = 10;
const MAX_AI_CALL_IDENTITIES: usize = 100_000;
const SCORE_SEMANTICS: &str = "context_percentile_v1";
const SCORE_CALIBRATION_VERSION: &str = "context-percentile-v1";
const MIN_INDEX_POOL_SIZE: usize = 10;
const MIN_INDEX_DATA_CONFIDENCE: f64 = 0.45;
const MIN_INDEX_FEATURES: usize = 3;
const NATURAL_LANGUAGE_RECALL_LIMIT: usize = 300;

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

#[derive(Debug, Clone, Copy)]
struct AiCallCounter {
    started: Instant,
    attempts: u32,
}

#[derive(Default)]
struct AiCallLimiter {
    counters: Mutex<HashMap<String, AiCallCounter>>,
}

impl AiCallLimiter {
    fn allow(&self, user_id: &str) -> bool {
        let now = Instant::now();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        user_id.hash(&mut hasher);
        let key = format!("{:016x}", hasher.finish());
        let mut counters = self
            .counters
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if counters.len() >= MAX_AI_CALL_IDENTITIES {
            counters.retain(|_, counter| now.duration_since(counter.started) < AI_CALL_WINDOW);
        }
        if counters.len() >= MAX_AI_CALL_IDENTITIES {
            return false;
        }
        let counter = counters.entry(key).or_insert(AiCallCounter {
            started: now,
            attempts: 0,
        });
        if now.duration_since(counter.started) >= AI_CALL_WINDOW {
            *counter = AiCallCounter {
                started: now,
                attempts: 0,
            };
        }
        if counter.attempts >= AI_CALLS_PER_ACCOUNT {
            return false;
        }
        counter.attempts = counter.attempts.saturating_add(1);
        true
    }
}

fn ai_call_limiter() -> &'static AiCallLimiter {
    static LIMITER: OnceLock<AiCallLimiter> = OnceLock::new();
    LIMITER.get_or_init(AiCallLimiter::default)
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

/// Stable, storage-independent handshake for clients configured with only a
/// server base URL. Keep paths relative so the same response works behind any
/// HTTPS reverse proxy without trusting forwarded host headers.
#[derive(Debug, Serialize, ToSchema)]
struct ClientDiscoveryResponse {
    service: &'static str,
    discovery_version: u8,
    service_version: &'static str,
    api_version: &'static str,
    api_base_path: &'static str,
    readiness_path: &'static str,
    openapi_path: &'static str,
    authentication: [&'static str; 2],
}

#[derive(Debug, Serialize, ToSchema)]
struct MetaResponse {
    api_version: &'static str,
    service_version: &'static str,
    algorithm_version: String,
    /// Active rule-parameter configuration; may lag the compiled algorithm on
    /// an existing database until operators activate the new config.
    config_version: Option<String>,
    /// SQLite schema migration version when storage is enabled.
    schema_version: Option<i64>,
    /// Compile-time git SHA from `MPGS_BUILD_GIT_SHA`, or `unknown` for local builds.
    build_git_sha: &'static str,
    /// Latest catalog / snapshot freshness marker from storage, when available.
    data_updated_at_ms: Option<i64>,
    supported_sections: Vec<&'static str>,
    ai_available: bool,
    /// Built-in multi-model task routing is active (not a single collapsed model).
    ai_multi_model: bool,
    /// Per-task primary models for the built-in router (empty when disabled).
    ai_task_models: Vec<String>,
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
    group_fit: f64,
    mode_fit: f64,
    access_fit: f64,
    hosting_fit: f64,
    session_fit: f64,
    quality: f64,
    activity: f64,
    freshness: f64,
    risk: f64,
    relevance_score: f64,
    final_score: f64,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct FeatureFreshnessValueSchema {
    status: String,
    observed_at_ms: Option<i64>,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct FeatureFreshnessSchema {
    multiplayer: FeatureFreshnessValueSchema,
    reviews: FeatureFreshnessValueSchema,
    activity: FeatureFreshnessValueSchema,
    price: FeatureFreshnessValueSchema,
    release: FeatureFreshnessValueSchema,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct FeedItemSchema {
    app_id: u32,
    name: String,
    section: FeedSection,
    /// One-based position in the final response ordering.
    rank: usize,
    release_date: Option<String>,
    release_date_raw: Option<String>,
    release_date_precision: Option<String>,
    cover_url: Option<String>,
    cover_updated_at_ms: Option<i64>,
    total_reviews: Option<u32>,
    total_positive: Option<u32>,
    latest_ccu: Option<u32>,
    typical_ccu_7d: Option<u32>,
    /// Deprecated raw ranking score. Prefer `recommendation_index`.
    score: f64,
    /// Backward-compatible alias of `data_confidence`.
    confidence: f64,
    data_confidence: f64,
    friend_fit: f64,
    /// Context-relative percentile index; absent when evidence is insufficient.
    recommendation_index: Option<u8>,
    fit_band: String,
    slot_reason: String,
    score_calibration_version: String,
    party: PartySchema,
    multiplayer: MultiplayerSummarySchema,
    play_intent: PlayIntentSummarySchema,
    reasons: Vec<String>,
    cautions: Vec<String>,
    evidence_ids: Vec<String>,
    reason_evidence: Vec<String>,
    feature_freshness: FeatureFreshnessSchema,
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
    config_version: String,
    data_updated_at_ms: i64,
    recommendation_run_id: Option<String>,
    score_semantics: String,
    sort: String,
    /// Omitted for recommended ordering because direction does not apply.
    order: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
struct NaturalLanguageRequest {
    query: String,
    limit: Option<i64>,
    custom_ai: Option<TransientCustomAiRequest>,
    /// When true, return deterministic base results without waiting for rank AI.
    #[serde(default)]
    r#async: Option<bool>,
    /// Multi-turn structured intent delta only (no full chat transcript).
    #[serde(default)]
    intent_delta: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, ToSchema)]
struct TransientCustomAiRequest {
    provider: String,
    base_url: String,
    /// Default / construction-time model.
    model: String,
    api_key: String,
    /// When true (default), use `routes` or primary+fallback multi-model routing.
    #[serde(default)]
    multi_model: Option<bool>,
    /// Optional shared fallback when `routes` is empty.
    #[serde(default)]
    fallback_model: Option<String>,
    /// Per-task routes from the device (省心配置 result). Never persisted server-side.
    #[serde(default)]
    routes: Option<Vec<TransientCustomRoute>>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
struct TransientCustomRoute {
    task: String,
    primary_model: String,
    #[serde(default)]
    fallback_models: Vec<String>,
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
    /// Explicitly selected section; absent when recall spans all sections.
    selected_section: Option<FeedSection>,
    selected_section_explicit: bool,
    max_price_minor: Option<i64>,
    currency: Option<String>,
    modes_preferred: Vec<String>,
    modes_excluded: Vec<String>,
    hard_constraints: Vec<String>,
    /// Structured dimensions that were translated into recall, filtering, or scoring.
    applied_constraints: Vec<String>,
    /// Parsed dimensions that could not be represented faithfully by the current ranker.
    unapplied_constraints: Vec<String>,
    intent_confidence: Option<f64>,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct NaturalLanguageResponseSchema {
    query: String,
    interpreted: NaturalLanguageInterpretationSchema,
    items: Vec<FeedItemSchema>,
    ai_status: String,
    ai_provider: String,
    ai_model: Option<String>,
    ai_protocol: Option<String>,
    ai_route_version: Option<String>,
    ai_used_model_fallback: bool,
    ai_attempted_models: Vec<String>,
    ai_multi_model: bool,
    #[schema(value_type = Object)]
    ai_routes: serde_json::Value,
    ai_latency_ms: u64,
    fallback_reason: Option<String>,
    ai_summary: Option<String>,
    ai_summary_evidence_ids: Vec<String>,
    algorithm_version: String,
    config_version: String,
    recommendation_run_id: Option<String>,
    data_updated_at_ms: i64,
    score_semantics: String,
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
    config_version: String,
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
    config_version: String,
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

#[allow(dead_code)]
#[derive(ToSchema)]
struct RecommendationEventResponseSchema {
    recommendation_event_id: i64,
    recommendation_run_id: String,
    app_id: u32,
    event_type: String,
    client_created_at_ms: Option<i64>,
    #[schema(value_type = Object)]
    metadata: serde_json::Value,
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
        client_discovery,
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
        discover_custom_models,
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
        post_recommendation_event,
        post_feedback,
        undo_feedback,
        set_play_intent
    ),
    components(schemas(
        ClientDiscoveryResponse,
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
        DiscoverCustomModelsBody,
        DiscoverCustomModelsResponse,
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
        FeatureFreshnessValueSchema,
        FeatureFreshnessSchema,
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
        RecommendationEventBody,
        RecommendationEventResponseSchema,
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
        .route("/.well-known/mpgs", get(client_discovery))
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
        .route(
            "/v1/me/avatar",
            put(put_avatar)
                .delete(delete_avatar)
                .layer(DefaultBodyLimit::max(2 * 1024 * 1024)),
        )
        .route(
            "/v1/me/ai-settings",
            get(get_ai_settings).put(put_ai_settings),
        )
        .route("/v1/me/ai-settings/test", post(test_ai_settings))
        .route("/v1/me/ai-settings/discover", post(discover_custom_models))
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
        .route("/v1/ai/group-advice", post(ai_group_advice))
        .route("/v1/games/{app_id}/ai-summary", get(get_game_ai_summary))
        .route(
            "/admin/v1/bootstrap",
            get(get_bootstrap_status).post(start_bootstrap),
        )
        .route("/v1/calendar", get(get_calendar))
        .route("/v1/search", get(search_games))
        .route("/v1/games/{app_id}", get(get_game))
        .route("/v1/games/{app_id}/evidence", get(get_evidence))
        .route("/v1/games/{app_id}/play-intent", post(set_play_intent))
        .route("/v1/recommendation-events", post(post_recommendation_event))
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
        // JSON endpoints are deliberately small. The avatar route overrides this
        // limit locally because encoded image uploads may be as large as 2 MiB.
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

#[utoipa::path(
    get,
    path = "/.well-known/mpgs",
    responses((status = 200, description = "MPGS client connection discovery", body = ClientDiscoveryResponse)),
    tag = "public"
)]
async fn client_discovery() -> Json<ClientDiscoveryResponse> {
    Json(ClientDiscoveryResponse {
        service: "mpgs-server",
        discovery_version: 1,
        service_version: env!("CARGO_PKG_VERSION"),
        api_version: "v1",
        api_base_path: "/v1",
        readiness_path: "/health/ready",
        openapi_path: "/openapi.json",
        authentication: ["anonymous", "account"],
    })
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
    let (config_version, schema_version, data_updated_at_ms) = match state.repo.as_ref() {
        Some(repo) => {
            match storage_result(repo, |repo| {
                let algorithm = repo.active_algorithm_config()?.version;
                let schema = repo.database().schema_version()?;
                let data_updated = repo.data_updated_at_ms().ok();
                Ok((Some(algorithm), Some(schema), data_updated))
            })
            .await
            {
                Ok(values) => values,
                Err(error) => return map_storage_error(error, None),
            }
        }
        None => (None, None, None),
    };
    let ai_task_models: Vec<String> = state
        .task_router
        .route_snapshot()
        .into_iter()
        .filter(|r| r.enabled)
        .map(|r| format!("{}:{}", r.task, r.primary_model))
        .collect();
    let body = MetaResponse {
        api_version: "v1",
        service_version: env!("CARGO_PKG_VERSION"),
        algorithm_version: ALGORITHM_VERSION.to_owned(),
        config_version,
        schema_version,
        build_git_sha: build_git_sha(),
        data_updated_at_ms,
        supported_sections: FeedSection::ALL
            .into_iter()
            .map(FeedSection::as_str)
            .collect(),
        ai_available: state.ai.is_available() || state.task_router.is_available(),
        ai_multi_model: state.task_router.multi_model_active(),
        ai_task_models,
        storage_enabled: state.repo.is_some(),
        demo_mode: std::env::var("MPGS_SEED_DEMO").ok().is_some_and(|value| {
            matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes")
        }),
    };
    let etag = weak_etag(&format!(
        "{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
        body.service_version,
        body.algorithm_version,
        body.config_version.as_deref().unwrap_or("none"),
        body.schema_version.unwrap_or(-1),
        body.build_git_sha,
        body.data_updated_at_ms.unwrap_or(0),
        body.storage_enabled,
        body.demo_mode,
        body.ai_multi_model,
        body.ai_task_models.join(",")
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
    let decode_permit = match avatar_decode_limiter().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "temporarily_unavailable",
                "avatar processing is busy; retry later",
                None,
            );
        }
    };
    let encoded = match tokio::task::spawn_blocking(move || {
        let _decode_permit = decode_permit;
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

fn avatar_decode_limiter() -> Arc<tokio::sync::Semaphore> {
    const MAX_CONCURRENT_AVATAR_DECODES: usize = 2;
    static LIMITER: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    Arc::clone(
        LIMITER
            .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_AVATAR_DECODES))),
    )
}

fn validate_avatar_dimensions(width: u32, height: u32) -> Result<(), String> {
    const MAX_AVATAR_PIXELS: u64 = 16_000_000;
    const MAX_AVATAR_DIMENSION: u32 = 16_000;
    if width == 0 || height == 0 {
        return Err("avatar image has no pixels".to_owned());
    }
    if width > MAX_AVATAR_DIMENSION
        || height > MAX_AVATAR_DIMENSION
        || u64::from(width)
            .checked_mul(u64::from(height))
            .is_none_or(|pixels| pixels > MAX_AVATAR_PIXELS)
    {
        return Err("avatar dimensions are too large".to_owned());
    }
    Ok(())
}

fn decode_avatar_image(raw: &[u8], format: ImageFormat) -> Result<DynamicImage, String> {
    let dimensions = ImageReader::with_format(Cursor::new(raw), format)
        .into_dimensions()
        .map_err(|_| "avatar data does not match its declared image type".to_owned())?;
    validate_avatar_dimensions(dimensions.0, dimensions.1)?;

    let mut limits = Limits::default();
    limits.max_image_width = Some(16_000);
    limits.max_image_height = Some(16_000);
    limits.max_alloc = Some(96 * 1024 * 1024);
    let mut reader = ImageReader::with_format(Cursor::new(raw), format);
    reader.limits(limits);
    reader
        .decode()
        .map_err(|_| "avatar data could not be decoded within safe limits".to_owned())
}

fn process_avatar_image(raw: &[u8], content_type: Option<&str>) -> Result<Vec<u8>, String> {
    const MAX_AVATAR_BYTES: usize = 2 * 1024 * 1024;
    if raw.is_empty() || raw.len() > MAX_AVATAR_BYTES {
        return Err("avatar must be a non-empty JPEG, PNG, or WebP smaller than 2 MiB".to_owned());
    }
    let declared_format = resolve_avatar_image_format(raw, content_type)?;
    // A renamed file is harmless, but decoding must use the magic-byte format so
    // dimension preflight and decoder limits apply to the actual codec.
    let format = sniff_avatar_image_format(raw).unwrap_or(declared_format);
    let image = decode_avatar_image(raw, format)?;
    let side = image.width().min(image.height());
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

#[derive(Debug, Clone, Deserialize, utoipa::IntoParams)]
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

/// List upstream models with a device-local key (never stored server-side).
#[derive(Debug, Deserialize, ToSchema)]
struct DiscoverCustomModelsBody {
    base_url: String,
    api_key: String,
}

#[derive(Debug, Serialize, ToSchema)]
struct DiscoverCustomModelsResponse {
    models: Vec<String>,
}

#[utoipa::path(
    post,
    path = "/v1/me/ai-settings/discover",
    request_body = DiscoverCustomModelsBody,
    responses(
        (status = 200, body = DiscoverCustomModelsResponse),
        (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody),
        (status = 502, body = ErrorBody)
    ),
    security(("bearer_auth" = [])),
    tag = "account"
)]
async fn discover_custom_models(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<DiscoverCustomModelsBody>,
) -> Response {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    if let Err(response) = require_account(repo, &headers).await {
        return *response;
    }
    if body.api_key.trim().is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "api_key is required",
            None,
        );
    }
    let resolution = match mpgs_ai::resolve_custom_base_url(&body.base_url).await {
        Ok(r) => r,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "custom AI endpoint is not allowed",
                None,
            );
        }
    };
    let provider = match OpenAiCompatProvider::new_with_custom_resolution(
        resolution.base_url.clone(),
        body.api_key,
        "probe-model",
        Duration::from_secs(12),
        &resolution,
    ) {
        Ok(p) => p,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "custom AI configuration is invalid",
                None,
            );
        }
    };
    match provider.list_models().await {
        Ok(models) => {
            let ids: Vec<String> = models.into_iter().map(|m| m.model).collect();
            Json(json!({ "models": ids })).into_response()
        }
        Err(_) => error_response(
            StatusCode::BAD_GATEWAY,
            "ai_connection_failed",
            "could not list models from the custom endpoint",
            None,
        ),
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
    let routes = state.task_router.route_snapshot();
    let discovered = state.task_router.registry().available_models();
    json!({
        "mode": settings.mode.as_str(),
        "provider": settings.provider,
        "base_url": settings.base_url,
        "model": settings.model,
        "configured": settings.configured,
        "key_mask": settings.key_mask,
        "updated_at_ms": settings.updated_at_ms,
        "builtin": {
            "available": state.ai.is_available() || state.task_router.is_available(),
            "provider": state.ai.provider_name(),
            "multi_model": state.task_router.multi_model_active(),
            "route_version": DEFAULT_ROUTE_VERSION,
            "routes": routes,
            "discovered_models": discovered,
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
    Json(raw_body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let user_id = match require_user(repo, &headers).await {
        Ok(id) => id,
        Err(resp) => return *resp,
    };
    let confidence_was_supplied = raw_body.get("preference_confidence").is_some();
    let mut body = match serde_json::from_value::<UserPreferences>(raw_body) {
        Ok(body) => body,
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                &format!("invalid preferences payload: {error}"),
                None,
            );
        }
    };
    if !confidence_was_supplied {
        let lookup_user_id = user_id.clone();
        match storage_result(repo, move |repo| repo.get_preferences(&lookup_user_id)).await {
            Ok(current) => body.preference_confidence = current.preference_confidence,
            Err(error) => return map_storage_error(error, None),
        }
    }
    match storage_result(repo, move |repo| repo.put_preferences(&user_id, &body)).await {
        Ok(prefs) => (StatusCode::OK, Json(prefs)).into_response(),
        Err(error) => map_storage_error(error, None),
    }
}

#[derive(Debug, Clone, Deserialize, utoipa::IntoParams)]
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
    excluded_modes: Option<String>,
    session_minutes_min: Option<u32>,
    session_minutes_max: Option<u32>,
    max_price_minor: Option<i64>,
    currency: Option<String>,
    demo_only: Option<bool>,
    /// Ranking override: recommendation score (`recommended` or `fit_index`),
    /// strict `ccu`, `reviews`, or `release_date`.
    sort: Option<String>,
    /// `desc` (default for ccu/reviews) or `asc` (default for release_date).
    /// Ignored when `sort=recommended`.
    order: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FeedSort {
    Recommended,
    FitIndex,
    Ccu,
    Reviews,
    ReleaseDate,
}

impl FeedSort {
    fn parse(raw: Option<&str>) -> Result<Self, ()> {
        match raw.map(str::trim).filter(|value| !value.is_empty()) {
            None | Some("recommended") | Some("score") => Ok(Self::Recommended),
            Some("fit_index") | Some("relevance") | Some("fit") => Ok(Self::FitIndex),
            Some("ccu") | Some("players") | Some("player_count") => Ok(Self::Ccu),
            Some("reviews") | Some("review_count") => Ok(Self::Reviews),
            Some("release_date") | Some("release") | Some("date") => Ok(Self::ReleaseDate),
            _ => Err(()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Recommended => "recommended",
            Self::FitIndex => "fit_index",
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
        // Recommended is an authored ranking (including diversity), not a scalar
        // sort. Direction is deliberately ignored and canonicalized so it cannot
        // change cursors, ETags, or the response contract.
        if matches!(sort, FeedSort::Recommended) {
            return Ok(Self::Desc);
        }
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

fn feed_result_context(
    preference_context: &str,
    sort: FeedSort,
    order: FeedSortOrder,
    demo_only: bool,
) -> String {
    let order = if matches!(sort, FeedSort::Recommended) {
        "n/a"
    } else {
        order.as_str()
    };
    format!(
        "{:016x}",
        hash64(&format!(
            "{preference_context}|sort={}|order={}|demo_only={demo_only}",
            sort.as_str(),
            order
        ))
    )
}

fn feedback_personal_adjustment(feedback: Option<&str>, matchmaking_core: f64) -> f64 {
    match feedback {
        Some("like") => 0.20,
        // "Played" means the discovery feed should make room for something new.
        Some("played") => -0.10,
        Some("too_competitive") if matchmaking_core >= 0.5 => -0.30,
        Some("too_competitive" | "hosting_friction") => -0.15,
        _ => 0.0,
    }
}

fn combined_feedback_personal_adjustment(feedback: &[&str], matchmaking_core: f64) -> f64 {
    feedback
        .iter()
        .map(|feedback| feedback_personal_adjustment(Some(feedback), matchmaking_core))
        .sum::<f64>()
        .clamp(-0.5, 0.5)
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
    data_confidence: f64,
    effective_feature_count: usize,
    profile_observed_at_ms: Option<i64>,
    reviews_observed_at_ms: Option<i64>,
    activity_observed_at_ms: Option<i64>,
    price_observed_at_ms: Option<i64>,
    release_observed_at_ms: Option<i64>,
}

fn feature_freshness_value(observed_at_ms: Option<i64>) -> serde_json::Value {
    json!({
        "status": if observed_at_ms.is_some() { "fresh" } else { "unknown" },
        "observed_at_ms": observed_at_ms,
    })
}

fn context_percentile_indices(scores: impl IntoIterator<Item = (u32, f64)>) -> HashMap<u32, u8> {
    let mut scores: Vec<_> = scores
        .into_iter()
        .filter(|(_, score)| score.is_finite())
        .collect();
    if scores.len() < MIN_INDEX_POOL_SIZE {
        return HashMap::new();
    }

    scores.sort_by(|(left_id, left), (right_id, right)| {
        right.total_cmp(left).then_with(|| left_id.cmp(right_id))
    });
    let total = scores.len() as f64;
    let mut indices = HashMap::with_capacity(scores.len());
    let mut start = 0usize;
    while start < scores.len() {
        let score = scores[start].1;
        let mut end = start + 1;
        while end < scores.len() && scores[end].1.total_cmp(&score).is_eq() {
            end += 1;
        }
        // Ranks are one-based. `end` is exclusive, and therefore also equals
        // the one-based rank of the last member in the tie group.
        let midrank = ((start + 1) as f64 + end as f64) / 2.0;
        let index = (100.0 * (total - midrank + 0.5) / total)
            .round()
            .clamp(0.0, 100.0) as u8;
        for (app_id, _) in &scores[start..end] {
            indices.insert(*app_id, index);
        }
        start = end;
    }
    indices
}

/// Compute the public index inside the exact response window. The index is a
/// request-context comparison, not a probability or an all-catalog percentile;
/// global position is communicated separately by `rank`.
fn context_percentile_indices_for_window(
    scores: impl IntoIterator<Item = (u32, f64)>,
    offset: usize,
    limit: usize,
) -> HashMap<u32, u8> {
    context_percentile_indices(scores.into_iter().skip(offset).take(limit))
}

fn visible_recommendation_index(
    app_id: u32,
    indices: &HashMap<u32, u8>,
    presentation: Option<&FeedPresentation>,
) -> Option<u8> {
    let presentation = presentation?;
    recommendation_index_is_eligible(presentation)
        .then(|| indices.get(&app_id).copied())
        .flatten()
}

fn recommendation_index_is_eligible(presentation: &FeedPresentation) -> bool {
    presentation.data_confidence >= MIN_INDEX_DATA_CONFIDENCE
        && presentation.effective_feature_count >= MIN_INDEX_FEATURES
}

fn fit_band(index: Option<u8>) -> &'static str {
    match index {
        Some(80..) => "excellent",
        Some(60..) => "good",
        Some(_) => "consider",
        None => "insufficient_data",
    }
}

fn slot_reason_str(reason: mpgs_recommender::SlotReason) -> &'static str {
    match reason {
        mpgs_recommender::SlotReason::Base => "base",
        mpgs_recommender::SlotReason::Diversity => "diversity",
        mpgs_recommender::SlotReason::Explore => "explore",
    }
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
) -> Response {
    get_feed_inner(state, headers, section, query, true, None).await
}

async fn get_feed_inner(
    state: Arc<AppState>,
    headers: HeaderMap,
    section: String,
    query: FeedQuery,
    record_telemetry: bool,
    internal_retrieval_ids: Option<&[u32]>,
) -> Response {
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
    let max_limit = if internal_retrieval_ids.is_some() {
        NATURAL_LANGUAGE_RECALL_LIMIT as i64
    } else {
        100
    };
    if !(1..=max_limit).contains(&requested_limit) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            &format!("limit must be between 1 and {max_limit}"),
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
                "sort must be recommended, fit_index, ccu, reviews, or release_date",
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
    let mut feedback_by_app: HashMap<u32, Vec<&str>> = HashMap::new();
    for feedback in &active_feedback {
        feedback_by_app
            .entry(feedback.app_id)
            .or_default()
            .push(feedback.feedback_type.as_str());
    }
    let preference_context = recommendation_context(
        &prefs,
        &active_feedback,
        &active_config.version,
        &active_config.config,
    );
    let now_ms = repo.database().now_ms();
    let today = mpgs_storage::util::day_utc_from_ms(now_ms);
    let tie_seed = recommendation_tie_seed(user_id.as_deref(), &today);
    let demo_only = query.demo_only.unwrap_or(false);
    let result_context = format!(
        "{}|tie_day={today}",
        feed_result_context(&preference_context, feed_sort, feed_order, demo_only),
    );
    let snapshot_ms = match storage_result(repo, |repo| repo.feed_snapshot_at_ms()).await {
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
            &result_context,
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
    // v6: include build revision so query/ranking semantics changes invalidate
    // client ETags even when data_updated_at_ms is unchanged.
    let etag = weak_etag(&format!(
        "feed:v6:{}:{}:{snapshot_ms}:{result_context}:{offset}:{limit}:{}:pi{}:user{cache_identity}:sort{}:order{}:demo{demo_only}",
        build_git_sha(),
        section.as_str(),
        active_config.version,
        play_intent.epoch.revision,
        feed_sort.as_str(),
        feed_order.as_str(),
    ));
    if let Some(response) = if_none_match_ok(&headers, &etag) {
        return response;
    }

    let cutoff = mpgs_storage::util::day_utc_from_ms(
        now_ms.saturating_sub(i64::from(active_config.config.recent_days) * 24 * 60 * 60 * 1000),
    );

    let cutoff_query = cutoff.clone();
    let today_query = today.clone();
    let currency = prefs.budget_currency.clone();
    let recommendation_config = active_config.config.clone();
    let candidate_limit = i64::from(active_config.config.candidate_limit);
    let candidates = match storage_result(repo, move |repo| {
        repo.list_candidates_cached(
            snapshot_ms,
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
            if demo_only && !row.has_demo {
                return None;
            }
            let signals = row.to_ranking_signals_at(&today);
            let feedback = feedback_by_app
                .get(&row.app_id)
                .map(Vec::as_slice)
                .unwrap_or_default();
            if feedback
                .iter()
                .any(|value| matches!(*value, "not_interested" | "party_size_mismatch"))
            {
                return None;
            }
            let personal_adjustment = combined_feedback_personal_adjustment(
                feedback,
                signals.multiplayer.matchmaking_core,
            );
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
            let effective_feature_count = usize::from(
                row.total_reviews.is_some_and(|reviews| reviews > 0)
                    && row.total_positive.is_some(),
            ) + usize::from(
                row.latest_ccu.is_some() || row.typical_ccu_7d.is_some(),
            ) + usize::from(row.release_date.is_some())
                + usize::from(
                    [
                        signals.multiplayer_confidence.private_session,
                        signals.multiplayer_confidence.self_host_or_dedicated,
                        signals.multiplayer_confidence.online_coop,
                        signals.multiplayer_confidence.group_size_fit,
                        signals.multiplayer_confidence.drop_in_out,
                        signals.multiplayer_confidence.cross_platform_fit,
                        signals.multiplayer_confidence.matchmaking_core,
                        signals.multiplayer_confidence.public_world_dependency,
                    ]
                    .into_iter()
                    .any(|confidence| confidence > 0.0),
                )
                + usize::from(row.has_demo);
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
                    data_confidence: signals.data_confidence.clamp(0.0, 1.0),
                    effective_feature_count,
                    profile_observed_at_ms: row.profile_observed_at_ms,
                    reviews_observed_at_ms: row.reviews_observed_at_ms,
                    activity_observed_at_ms: row.activity_observed_at_ms,
                    price_observed_at_ms: row.price_observed_at_ms,
                    release_observed_at_ms: row
                        .release_date
                        .as_ref()
                        .and(row.release_observed_at_ms),
                },
            );
            let dominant_mode = row.display_dominant_mode();
            Some(RankingInput {
                app_id: row.app_id,
                name: row.name,
                dominant_mode,
                taxonomy_tags: row.taxonomy_tags,
                publisher: row.publisher,
                series: None,
                recommended_min: row.recommended_min,
                recommended_max: row.recommended_max,
                availability,
                signals,
                personal_adjustment,
                play_intent_count: play_intent.counts.get(&row.app_id).copied().unwrap_or(0),
            })
        })
        .collect();

    let hard_constraints = hard_constraints_from_feed_query(&query);
    let ranked = rank_feed_configured_with_constraints_and_tie_seed(
        section,
        &inputs,
        &prefs,
        &hard_constraints,
        &active_config.config,
        ALGORITHM_VERSION,
        tie_seed,
    );
    let mut ranked_items = ranked.items;
    apply_feed_sort(
        &mut ranked_items,
        feed_sort,
        feed_order,
        &presentation_by_app,
    );
    if let Some(retrieval_ids) = internal_retrieval_ids {
        ranked_items = select_internal_recall_candidates(ranked_items, retrieval_ids, limit);
    }
    let recommendation_indices = context_percentile_indices_for_window(
        ranked_items
            .iter()
            .map(|item| (item.app_id, item.score.relevance_score)),
        offset,
        limit,
    );
    let total = ranked_items.len();
    let limit_usize = limit;
    let page_number = (offset / limit_usize) + 1;
    let total_pages = if total == 0 {
        0
    } else {
        total.div_ceil(limit_usize)
    };
    let candidate_ids: Vec<u32> = ranked_items.iter().map(|item| item.app_id).collect();
    let page: Vec<_> = ranked_items
        .into_iter()
        .skip(offset)
        .take(limit_usize)
        .enumerate()
        .map(|(page_index, item)| {
            let presentation = presentation_by_app.get(&item.app_id);
            let recommendation_index = visible_recommendation_index(
                item.app_id,
                &recommendation_indices,
                presentation,
            );
            let data_confidence = presentation
                .map(|value| value.data_confidence)
                .unwrap_or(0.0);
            let ranking_metadata = (!record_telemetry).then(|| {
                json!({
                    "taxonomy_tags": item.taxonomy_tags,
                    "publisher": item.publisher,
                    "series": item.series,
                })
            });
            let score_components = json!({
                "friend_fit": item.score.friend_fit,
                "section_score": item.score.section_score,
                "personalized_score": item.score.personalized_score,
                "group_fit": item.score.group_fit,
                "mode_fit": item.score.mode_fit,
                "access_fit": item.score.access_fit,
                "hosting_fit": item.score.hosting_fit,
                "session_fit": item.score.session_fit,
                "quality": item.score.quality,
                "activity": item.score.activity,
                "freshness": item.score.freshness,
                "risk": item.score.risk,
                "relevance_score": item.score.relevance_score,
                "final_score": item.score.final_score,
            });
            let feature_freshness = json!({
                "multiplayer": feature_freshness_value(
                    presentation.and_then(|value| value.profile_observed_at_ms),
                ),
                "reviews": feature_freshness_value(
                    presentation.and_then(|value| value.reviews_observed_at_ms),
                ),
                "activity": feature_freshness_value(
                    presentation.and_then(|value| value.activity_observed_at_ms),
                ),
                "price": feature_freshness_value(
                    presentation.and_then(|value| value.price_observed_at_ms),
                ),
                "release": feature_freshness_value(
                    presentation.and_then(|value| value.release_observed_at_ms),
                ),
            });
            let reason_evidence = item.explanation.evidence_ids.clone();
            let mut response_item = json!({
                "app_id": item.app_id,
                "name": item.name,
                "section": section.as_str(),
                "rank": offset + page_index + 1,
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
                "confidence": data_confidence,
                "data_confidence": data_confidence,
                "friend_fit": item.score.friend_fit,
                "recommendation_index": recommendation_index,
                "fit_band": fit_band(recommendation_index),
                "slot_reason": "base",
                "score_calibration_version": SCORE_CALIBRATION_VERSION,
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
                "reason_evidence": reason_evidence,
                "feature_freshness": feature_freshness,
                "components": score_components,
                "algorithm_version": item.algorithm_version,
            });
            if let Some(metadata) = ranking_metadata
                && let Some(object) = response_item.as_object_mut()
            {
                // Only internal natural-language recall receives these fields;
                // the regular feed contract remains unchanged.
                object.insert("_ranking_metadata".into(), metadata);
            }
            response_item
        })
        .collect();

    let recommendation_run_id = if record_telemetry {
        record_recommendation_run(
            repo,
            "feed",
            section.as_str(),
            &active_config.version,
            &result_context,
            snapshot_ms,
            offset,
            limit,
            &candidate_ids,
            &page,
        )
        .await
    } else {
        None
    };

    let next_offset = offset.saturating_add(limit_usize);
    let next_cursor = if next_offset < total {
        Some(encode_cursor(
            section,
            snapshot_ms,
            &result_context,
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
        "algorithm_version": ALGORITHM_VERSION,
        "config_version": active_config.version,
        "data_updated_at_ms": snapshot_ms,
        "recommendation_run_id": recommendation_run_id,
        "score_semantics": SCORE_SEMANTICS,
        "sort": feed_sort.as_str(),
        "order": (!matches!(feed_sort, FeedSort::Recommended)).then(|| feed_order.as_str()),
    });
    (StatusCode::OK, [(header::ETAG, etag)], Json(body)).into_response()
}

#[allow(clippy::too_many_arguments)]
async fn record_recommendation_run(
    repo: &Repository,
    request_kind: &str,
    feed_section: &str,
    config_version: &str,
    result_context: &str,
    snapshot_ms: i64,
    offset: usize,
    limit: usize,
    candidate_ids: &[u32],
    page: &[serde_json::Value],
) -> Option<String> {
    let context_hash = match hash_structured_context(&json!({
        "schema": 1,
        "request_kind": request_kind,
        "section": feed_section,
        "result_context": result_context,
        "snapshot_ms": snapshot_ms,
        "offset": offset,
        "limit": limit,
    })) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%error, "failed to hash recommendation context");
            return None;
        }
    };
    let candidate_set_hash = match hash_candidate_set(candidate_ids) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%error, "failed to hash recommendation candidate set");
            return None;
        }
    };
    let run_input = InsertRecommendationRun {
        // A deployment-keyed subject pseudonym is not configured yet. Never
        // fall back to hashing a raw account id with an unkeyed digest.
        subject_hash: None,
        request_kind: request_kind.to_owned(),
        feed_section: feed_section.to_owned(),
        algorithm_version: ALGORITHM_VERSION.into(),
        config_version: config_version.to_owned(),
        score_semantics: SCORE_SEMANTICS.into(),
        context_schema_version: 1,
        context_hash,
        candidate_set_hash,
        candidate_count: u32::try_from(candidate_ids.len()).unwrap_or(u32::MAX),
    };
    let items: Vec<InsertRecommendationItem> = page
        .iter()
        .filter_map(|item| {
            let app_id = u32::try_from(item.get("app_id")?.as_u64()?).ok()?;
            let rank = u32::try_from(item.get("rank")?.as_u64()?).ok()?;
            let components = item.get("components")?.clone();
            Some(InsertRecommendationItem {
                app_id,
                rank,
                relevance_score: components.get("relevance_score")?.as_f64()?,
                recommendation_index: item
                    .get("recommendation_index")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| u8::try_from(value).ok()),
                data_confidence: item.get("data_confidence")?.as_f64()?,
                slot_reason: item.get("slot_reason")?.as_str()?.to_owned(),
                score_components: components,
            })
        })
        .collect();
    if items.len() != page.len() {
        tracing::warn!(
            expected = page.len(),
            actual = items.len(),
            "recommendation item attribution was incomplete"
        );
        return None;
    }
    match storage_result(repo, move |repo| {
        repo.insert_recommendation_run_with_items(&run_input, &items)
    })
    .await
    {
        Ok(run) => Some(run.recommendation_run_id),
        Err(error) => {
            tracing::warn!(%error, "failed to atomically persist recommendation attribution");
            None
        }
    }
}

fn apply_feed_sort(
    items: &mut [mpgs_recommender::RankedCandidate],
    sort: FeedSort,
    order: FeedSortOrder,
    presentation: &HashMap<u32, FeedPresentation>,
) {
    if items.len() < 2 {
        return;
    }
    items.sort_by(|left, right| {
        let left_p = presentation.get(&left.app_id);
        let right_p = presentation.get(&right.app_id);
        let primary = match sort {
            FeedSort::Recommended => compare_recommendation_score(
                left.score.relevance_score,
                right.score.relevance_score,
                left_p,
                right_p,
                FeedSortOrder::Desc,
            ),
            FeedSort::FitIndex => compare_recommendation_score(
                left.score.relevance_score,
                right.score.relevance_score,
                left_p,
                right_p,
                order,
            ),
            FeedSort::Ccu => compare_optional_missing_last(
                left_p.and_then(|value| value.typical_ccu_7d.or(value.latest_ccu)),
                right_p.and_then(|value| value.typical_ccu_7d.or(value.latest_ccu)),
                order,
            ),
            FeedSort::Reviews => compare_optional_missing_last(
                left_p.and_then(|value| value.total_reviews),
                right_p.and_then(|value| value.total_reviews),
                order,
            ),
            FeedSort::ReleaseDate => compare_optional_missing_last(
                left_p
                    .and_then(|value| value.release_date.as_deref())
                    .filter(|value| !value.is_empty()),
                right_p
                    .and_then(|value| value.release_date.as_deref())
                    .filter(|value| !value.is_empty()),
                order,
            ),
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

fn compare_recommendation_score(
    left_score: f64,
    right_score: f64,
    left: Option<&FeedPresentation>,
    right: Option<&FeedPresentation>,
    order: FeedSortOrder,
) -> std::cmp::Ordering {
    match (
        left.is_some_and(recommendation_index_is_eligible),
        right.is_some_and(recommendation_index_is_eligible),
    ) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => match order {
            FeedSortOrder::Asc => left_score.total_cmp(&right_score),
            FeedSortOrder::Desc => right_score.total_cmp(&left_score),
        },
    }
}

/// Select an internal natural-language recall page from the complete ranked
/// section. Retrieval IDs are considered only when they survived all filtering
/// and ranking admission; absent (for example hard-filtered) IDs cannot be
/// materialized by this function. Remaining capacity is filled by strict
/// relevance so the later cross-section MMR is the only diversity pass that
/// influences the natural-language response.
fn select_internal_recall_candidates(
    mut ranked: Vec<RankedCandidate>,
    retrieval_ids: &[u32],
    limit: usize,
) -> Vec<RankedCandidate> {
    if ranked.is_empty() || limit == 0 {
        return Vec::new();
    }
    ranked.sort_by(|left, right| {
        right
            .score
            .relevance_score
            .total_cmp(&left.score.relevance_score)
            .then_with(|| right.data_confidence.total_cmp(&left.data_confidence))
            .then_with(|| left.app_id.cmp(&right.app_id))
    });
    let fill_order: Vec<u32> = ranked.iter().map(|item| item.app_id).collect();
    let mut eligible: HashMap<u32, RankedCandidate> =
        ranked.into_iter().map(|item| (item.app_id, item)).collect();
    let mut selected = Vec::with_capacity(limit.min(eligible.len()));
    for app_id in retrieval_ids.iter().chain(fill_order.iter()) {
        if let Some(item) = eligible.remove(app_id) {
            selected.push(item);
            if selected.len() == limit {
                break;
            }
        }
    }
    selected
}

fn compare_optional_missing_last<T: Ord>(
    left: Option<T>,
    right: Option<T>,
    order: FeedSortOrder,
) -> std::cmp::Ordering {
    match (left, right) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        (Some(left), Some(right)) => match order {
            FeedSortOrder::Asc => left.cmp(&right),
            FeedSortOrder::Desc => right.cmp(&left),
        },
    }
}

#[derive(Debug, Clone, Serialize)]
struct NaturalLanguageInterpretation {
    party_size: Option<u8>,
    session_minutes_max: Option<u32>,
    coop_competitive: Option<f64>,
    self_hosting_willingness: Option<f64>,
    platforms: Vec<String>,
    demo_only: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    selected_section: Option<FeedSection>,
    selected_section_explicit: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_price_minor: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    currency: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    modes_preferred: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    modes_excluded: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    hard_constraints: Vec<String>,
    #[serde(default)]
    applied_constraints: Vec<String>,
    #[serde(default)]
    unapplied_constraints: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    intent_confidence: Option<f64>,
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
        r#async: async_mode,
        intent_delta,
    } = body;
    let Some(query) = normalized_ai_query(&raw_query) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "query must contain between 3 and 500 characters",
            None,
        );
    };
    let output_limit = limit.unwrap_or(6);
    if !(3..=10).contains(&output_limit) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "limit must be between 3 and 10",
            None,
        );
    }
    let skip_rank_ai = async_mode.unwrap_or(false);

    let account_user_id = match state.repo.as_ref() {
        Some(repo) => optional_account_user_id(repo, &headers).await,
        None => None,
    };
    let mut interpreted = interpret_natural_language(&query);
    // Optional multi-turn structured delta (never full chat history).
    if let Some(delta) = intent_delta {
        apply_intent_delta(&mut interpreted, &delta);
    }

    // Resolve AI early so intent_parse can refine soft preferences before ranking.
    let resolved_ai_early = match &custom_ai {
        Some(_) => None, // custom credentials are applied after feed for rank only
        None => Some(account_ai_gateway(&state, account_user_id.as_deref()).await),
    };
    if let Some(resolved) = resolved_ai_early.as_ref()
        && let Some(router) = resolved_task_router(resolved, &state)
        && let Some(merged) = try_ai_intent_parse(
            router,
            &resolved.gateway,
            state.repo.as_ref(),
            resolved.admission.as_ref(),
            &query,
            &interpreted,
        )
        .await
    {
        apply_structured_intent(&mut interpreted, &merged);
    }
    finalize_natural_language_constraints(&mut interpreted);

    // Recall before section pagination. The RRF list is only an admission hint:
    // each section still applies objective filters, request hard constraints,
    // negative feedback, and the complete deterministic ranking before a hit is
    // allowed into the shared pool.
    let hybrid_hits = if let Some(repo) = state.repo.as_ref() {
        natural_language_hybrid_hits(&state, repo, &query).await
    } else {
        Vec::new()
    };
    let retrieval_ids: Vec<u32> = hybrid_hits.iter().map(|hit| hit.app_id).collect();

    let feed_query = FeedQuery {
        // Internal recall may inspect up to 300 admitted games from each fully
        // ranked section. The cross-section union is capped separately below.
        limit: Some(NATURAL_LANGUAGE_RECALL_LIMIT as i64),
        page: None,
        cursor: None,
        party_size: interpreted.party_size,
        coop_competitive: interpreted.coop_competitive,
        self_hosting_willingness: interpreted.self_hosting_willingness,
        platforms: (!interpreted.platforms.is_empty()).then(|| interpreted.platforms.join(",")),
        languages: None,
        excluded_modes: (!interpreted.modes_excluded.is_empty())
            .then(|| interpreted.modes_excluded.join(",")),
        session_minutes_min: None,
        session_minutes_max: interpreted.session_minutes_max,
        max_price_minor: interpreted.max_price_minor,
        currency: interpreted.currency.clone(),
        demo_only: Some(interpreted.demo_only),
        sort: None,
        order: None,
    };
    let sections: Vec<FeedSection> = if interpreted.selected_section_explicit {
        interpreted
            .selected_section
            .map_or_else(|| FeedSection::ALL.to_vec(), |section| vec![section])
    } else {
        FeedSection::ALL.to_vec()
    };
    let mut feed = serde_json::Value::Null;
    let mut items_by_app = HashMap::<u32, serde_json::Value>::new();
    for recall_section in sections {
        let feed_response = get_feed_inner(
            state.clone(),
            headers.clone(),
            recall_section.as_str().to_owned(),
            feed_query.clone(),
            false,
            Some(&retrieval_ids),
        )
        .await;
        if feed_response.status() != StatusCode::OK {
            return feed_response;
        }
        let (_, feed_body) = feed_response.into_parts();
        let bytes = match axum::body::to_bytes(feed_body, 4 * 1024 * 1024).await {
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
        let recalled: serde_json::Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "failed to decode recommendation response",
                    None,
                );
            }
        };
        if feed.is_null() {
            feed = recalled.clone();
        }
        for item in recalled
            .get("items")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(app_id) = item
                .get("app_id")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
            else {
                continue;
            };
            let item_score = json_item_relevance(item);
            let replace = items_by_app
                .get(&app_id)
                .is_none_or(|current| item_score > json_item_relevance(current));
            if replace {
                items_by_app.insert(app_id, item.clone());
            }
        }
    }
    let mut items = build_natural_language_candidate_pool(
        items_by_app,
        &hybrid_hits,
        NATURAL_LANGUAGE_RECALL_LIMIT,
    );
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
            let settings = match state.repo.as_ref() {
                Some(repo) => {
                    let user_id = account_user_id.to_owned();
                    storage_result(repo, move |repo| repo.account_ai_settings(&user_id))
                        .await
                        .ok()
                }
                None => None,
            };
            if settings
                .as_ref()
                .is_some_and(|row| row.mode == AiMode::Custom)
            {
                let Some(saved_base_url) =
                    settings.as_ref().and_then(|row| row.base_url.as_deref())
                else {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        "invalid_argument",
                        "saved custom AI endpoint is missing",
                        None,
                    );
                };
                match transient_custom_ai_resolved(
                    custom,
                    saved_base_url,
                    account_user_id,
                    state.account_ai_limits.clone(),
                )
                .await
                {
                    Ok(resolved) => resolved,
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
    // Prefer device-local multi-model custom router; otherwise built-in TaskRouter.
    let task_router = resolved_task_router(&resolved_ai, &state);

    let enhance = if skip_rank_ai {
        if resolved_ai.gateway.is_available() || task_router.is_some_and(|r| r.is_available()) {
            AiEnhanceOutcome::pending("base ranking returned; AI enhancement is pending")
        } else {
            AiEnhanceOutcome::fallback(mpgs_ai::AiError::Disabled.fallback_reason())
        }
    } else {
        enhance_natural_language_with_ai(
            &resolved_ai.gateway,
            task_router,
            state.repo.as_ref(),
            &cache_scope,
            &query,
            &mut items,
            resolved_ai.admission.as_ref(),
        )
        .await
    };
    // Section feeds have already been diversified independently. Once their
    // candidates are unioned and AI has supplied its bounded numeric blend,
    // rebuild one global baseline and run MMR exactly once for the response.
    let nl_mmr_lambda = if let Some(repo) = state.repo.as_ref() {
        storage_result(repo, |repo| repo.active_algorithm_config())
            .await
            .map(|config| config.config.mmr_lambda)
            .unwrap_or_else(|_| RecommendationConfig::default().mmr_lambda)
    } else {
        RecommendationConfig::default().mmr_lambda
    };
    let nl_tie_seed = state.repo.as_ref().map_or(0, |repo| {
        let day = mpgs_storage::util::day_utc_from_ms(repo.database().now_ms());
        recommendation_tie_seed(account_user_id.as_deref(), &day)
    });
    global_natural_language_rerank(&mut items, nl_mmr_lambda, nl_tie_seed);
    let nl_candidate_ids: Vec<u32> = items.iter().map(json_app_id).collect();
    items.truncate(output_limit as usize);
    strip_internal_ranking_metadata(&mut items);
    let config_version = feed
        .get("config_version")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let data_updated_at_ms = feed
        .get("data_updated_at_ms")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    let nl_context = hash_structured_context(&interpreted)
        .unwrap_or_else(|_| "structured-intent-unavailable".to_owned());
    let nl_section = interpreted
        .selected_section
        .map(FeedSection::as_str)
        .unwrap_or("all");
    let recommendation_run_id = if let Some(repo) = state.repo.as_ref() {
        record_recommendation_run(
            repo,
            "natural_language",
            nl_section,
            &config_version,
            &nl_context,
            data_updated_at_ms,
            0,
            items.len(),
            &nl_candidate_ids,
            &items,
        )
        .await
    } else {
        None
    };
    let rank_route = task_router.and_then(|router| router.route_for(AiTaskType::RankExplain));
    let intent_route = task_router.and_then(|router| router.route_for(AiTaskType::IntentParse));
    Json(json!({
        "query": query,
        "interpreted": interpreted,
        "items": items,
        "ai_status": enhance.status.as_str(),
        "ai_provider": resolved_ai.gateway.provider_name(),
        "ai_model": enhance.model,
        "ai_protocol": enhance.protocol,
        "ai_route_version": enhance.route_version,
        "ai_used_model_fallback": enhance.used_model_fallback,
        "ai_attempted_models": enhance.attempted_models,
        "ai_multi_model": task_router.is_some_and(TaskRouter::multi_model_active),
        "ai_routes": {
            "rank_explain": rank_route.map(|r| json!({
                "primary": r.primary_model,
                "fallbacks": r.fallback_models,
            })),
            "intent_parse": intent_route.map(|r| json!({
                "primary": r.primary_model,
                "fallbacks": r.fallback_models,
            })),
        },
        "ai_latency_ms": ai_started.elapsed().as_millis() as u64,
        "fallback_reason": enhance.fallback_reason,
        "ai_summary": enhance.summary,
        "ai_summary_evidence_ids": enhance.summary_evidence_ids,
        "algorithm_version": feed.get("algorithm_version").cloned().unwrap_or(json!(ALGORITHM_VERSION)),
        "config_version": config_version,
        "recommendation_run_id": recommendation_run_id,
        "data_updated_at_ms": data_updated_at_ms,
        "score_semantics": SCORE_SEMANTICS,
    }))
    .into_response()
}

async fn natural_language_hybrid_hits(
    state: &AppState,
    repo: &Repository,
    query: &str,
) -> Vec<HybridHit> {
    // Catalog indexing is cursor-driven by the scheduler. A request never runs
    // a partial first-N sync: an unavailable or not-yet-populated index simply
    // falls back to the deterministic relevance pool below.
    let query_embedding = if state.embedding.is_available() {
        tokio::time::timeout(
            Duration::from_secs(2),
            state.embedding.embed(&[EmbeddingInput {
                id: "nl-query".into(),
                text: query.to_owned(),
            }]),
        )
        .await
        .ok()
        .and_then(Result::ok)
        .and_then(|mut embeddings| (embeddings.len() == 1).then(|| embeddings.remove(0)))
    } else {
        None
    };
    let query_for_search = query.to_owned();
    let result = if let Some(embedding) = query_embedding {
        let provider = state.embedding.name().to_owned();
        storage_result(repo, move |repo| {
            repo.hybrid_search_with_vector(
                &query_for_search,
                &embedding.vector,
                &provider,
                &embedding.model,
                NATURAL_LANGUAGE_RECALL_LIMIT as u32,
            )
        })
        .await
    } else {
        storage_result(repo, move |repo| {
            repo.hybrid_search(&query_for_search, NATURAL_LANGUAGE_RECALL_LIMIT as u32)
        })
        .await
    };
    match result {
        Ok(hits) => hits,
        Err(error) => {
            tracing::warn!(%error, "natural-language hybrid recall unavailable; using deterministic fallback");
            Vec::new()
        }
    }
}

/// Form one cross-section pool. RRF hits retain admission priority, but only if
/// a fully-ranked section emitted them after all filters. Strict relevance fills
/// the remaining slots deterministically when retrieval is empty or incomplete.
fn build_natural_language_candidate_pool(
    mut eligible: HashMap<u32, serde_json::Value>,
    hits: &[HybridHit],
    limit: usize,
) -> Vec<serde_json::Value> {
    if eligible.is_empty() || limit == 0 {
        return Vec::new();
    }
    let mut fill_order: Vec<u32> = eligible.keys().copied().collect();
    fill_order.sort_by(|left, right| {
        json_item_relevance(&eligible[right])
            .total_cmp(&json_item_relevance(&eligible[left]))
            .then_with(|| left.cmp(right))
    });

    let mut selected = Vec::with_capacity(limit.min(eligible.len()));
    for hit in hits {
        let Some(mut item) = eligible.remove(&hit.app_id) else {
            continue;
        };
        if let Some(object) = item.as_object_mut() {
            object.insert("hybrid_score".into(), json!(hit.score));
        }
        selected.push(item);
        if selected.len() == limit {
            return selected;
        }
    }
    for app_id in fill_order {
        let Some(item) = eligible.remove(&app_id) else {
            continue;
        };
        selected.push(item);
        if selected.len() == limit {
            break;
        }
    }
    selected
}

fn json_app_id(item: &serde_json::Value) -> u32 {
    item.get("app_id")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(u32::MAX)
}

fn json_item_relevance(item: &serde_json::Value) -> f64 {
    item.pointer("/components/relevance_score")
        .and_then(serde_json::Value::as_f64)
        .or_else(|| item.get("score").and_then(serde_json::Value::as_f64))
        .filter(|value| value.is_finite())
        .unwrap_or(f64::NEG_INFINITY)
}

fn global_natural_language_rerank(items: &mut Vec<serde_json::Value>, lambda: f64, tie_seed: u64) {
    if items.is_empty() {
        return;
    }
    let mut json_by_app = HashMap::with_capacity(items.len());
    let mut ranked = Vec::with_capacity(items.len());
    let mut malformed = Vec::new();
    for item in std::mem::take(items) {
        let Some(candidate) = ranked_candidate_from_json(&item) else {
            malformed.push(item);
            continue;
        };
        json_by_app.insert(candidate.app_id, item);
        ranked.push(candidate);
    }

    let reranked = mmr_rerank_with_tie_seed(ranked, lambda, 2, tie_seed);
    let mut ordered = Vec::with_capacity(json_by_app.len() + malformed.len());
    for candidate in reranked {
        let Some(mut item) = json_by_app.remove(&candidate.app_id) else {
            continue;
        };
        if let Some(object) = item.as_object_mut() {
            object.insert(
                "slot_reason".into(),
                json!(slot_reason_str(candidate.slot_reason)),
            );
        }
        ordered.push(item);
    }
    // Generated feed items are always well-formed. This defensive tail keeps a
    // malformed internal item from being silently lost if that contract regresses.
    for mut item in malformed {
        if let Some(object) = item.as_object_mut() {
            object
                .entry("slot_reason".to_owned())
                .or_insert_with(|| json!("base"));
        }
        ordered.push(item);
    }
    *items = ordered;
    refresh_json_ranking_metadata(items);
}

fn ranked_candidate_from_json(item: &serde_json::Value) -> Option<RankedCandidate> {
    let app_id = item
        .get("app_id")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())?;
    let component = |field: &str| {
        item.pointer(&format!("/components/{field}"))
            .and_then(serde_json::Value::as_f64)
    };
    let relevance_score = json_item_relevance(item);
    let final_score = item
        .get("score")
        .and_then(serde_json::Value::as_f64)
        .or_else(|| component("final_score"))
        .unwrap_or(relevance_score);
    let strings = |field: &str| {
        item.get(field)
            .and_then(serde_json::Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    Some(RankedCandidate {
        app_id,
        name: item
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        dominant_mode: item
            .pointer("/multiplayer/dominant_mode")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        taxonomy_tags: item
            .pointer("/_ranking_metadata/taxonomy_tags")
            .and_then(serde_json::Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        publisher: item
            .pointer("/_ranking_metadata/publisher")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        series: item
            .pointer("/_ranking_metadata/series")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        recommended_min: item
            .pointer("/party/recommended_min")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u8::try_from(value).ok()),
        recommended_max: item
            .pointer("/party/recommended_max")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u8::try_from(value).ok()),
        data_confidence: item
            .get("data_confidence")
            .and_then(serde_json::Value::as_f64)
            .or_else(|| item.get("confidence").and_then(serde_json::Value::as_f64))
            .unwrap_or(0.0),
        score: ScoreBreakdown {
            friend_fit: component("friend_fit")
                .or_else(|| item.get("friend_fit").and_then(serde_json::Value::as_f64))
                .unwrap_or(0.0),
            section_score: component("section_score").unwrap_or(0.0),
            personalized_score: component("personalized_score").unwrap_or(0.0),
            group_fit: component("group_fit").unwrap_or(0.0),
            mode_fit: component("mode_fit").unwrap_or(0.0),
            access_fit: component("access_fit").unwrap_or(0.0),
            hosting_fit: component("hosting_fit").unwrap_or(0.0),
            session_fit: component("session_fit").unwrap_or(0.0),
            quality: component("quality").unwrap_or(0.0),
            activity: component("activity").unwrap_or(0.0),
            freshness: component("freshness").unwrap_or(0.0),
            risk: component("risk").unwrap_or(0.0),
            relevance_score,
            final_score,
        },
        explanation: Explanation {
            reasons: strings("reasons"),
            cautions: strings("cautions"),
            evidence_ids: strings("evidence_ids"),
        },
        slot_reason: SlotReason::Base,
        algorithm_version: item
            .get("algorithm_version")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(ALGORITHM_VERSION)
            .to_owned(),
    })
}

fn strip_internal_ranking_metadata(items: &mut [serde_json::Value]) {
    for item in items {
        if let Some(object) = item.as_object_mut() {
            object.remove("_ranking_metadata");
        }
    }
}

/// Resolve an account's AI mode without ever returning the custom key to the
/// caller. Anonymous requests deliberately receive a disabled gateway: every
/// paid provider call must have an accountable, admitted account identity.
#[derive(Clone)]
struct ResolvedAiGateway {
    gateway: AiGateway,
    /// When set (device-local multi-model custom API), prefer this over `state.task_router`.
    task_router: Option<Arc<TaskRouter>>,
    use_builtin_router: bool,
    admission: Option<AiCallAdmission>,
}

#[derive(Clone)]
struct AiCallAdmission {
    user_id: String,
    limiter: AccountAiLimiter,
    charge_daily_quota: bool,
}

#[derive(Debug, Clone, Copy)]
enum AiAdmissionError {
    BudgetExhausted,
    TemporarilyUnavailable,
}

fn disabled_resolved_ai() -> ResolvedAiGateway {
    ResolvedAiGateway {
        gateway: AiGateway::disabled(),
        task_router: None,
        use_builtin_router: false,
        admission: None,
    }
}

fn resolved_task_router<'a>(
    resolved: &'a ResolvedAiGateway,
    state: &'a AppState,
) -> Option<&'a TaskRouter> {
    resolved
        .task_router
        .as_deref()
        .or_else(|| {
            resolved
                .use_builtin_router
                .then_some(state.task_router.as_ref())
        })
        .filter(|router| router.is_available())
}

fn ai_admission_failure_reason(error: AiAdmissionError) -> String {
    match error {
        AiAdmissionError::BudgetExhausted => AiError::BudgetExhausted.fallback_reason().to_owned(),
        AiAdmissionError::TemporarilyUnavailable => {
            "AI account quota is temporarily unavailable; deterministic results are shown"
                .to_owned()
        }
    }
}

async fn acquire_ai_call(
    repo: Option<&Repository>,
    admission: &AiCallAdmission,
) -> Result<AccountAiPermit, AiAdmissionError> {
    let permit = admission
        .limiter
        .try_acquire(&admission.user_id)
        .ok_or(AiAdmissionError::BudgetExhausted)?;
    if !ai_call_limiter().allow(&admission.user_id) {
        return Err(AiAdmissionError::BudgetExhausted);
    }
    if admission.charge_daily_quota {
        let repo = repo.ok_or(AiAdmissionError::TemporarilyUnavailable)?;
        let user_id = admission.user_id.clone();
        let day_utc = chrono_like_now_ms() / 86_400_000;
        let daily_limit = admission.limiter.daily_budget();
        match storage_result(repo, move |repo| {
            repo.consume_account_ai_quota(&user_id, day_utc, daily_limit)
        })
        .await
        {
            Ok(Some(_)) => {}
            Ok(None) => return Err(AiAdmissionError::BudgetExhausted),
            Err(_) => return Err(AiAdmissionError::TemporarilyUnavailable),
        }
    }
    Ok(permit)
}

async fn account_ai_gateway(state: &AppState, user_id: Option<&str>) -> ResolvedAiGateway {
    let Some(user_id) = user_id else {
        return disabled_resolved_ai();
    };
    let Some(repo) = state.repo.as_ref() else {
        return disabled_resolved_ai();
    };
    let lookup_user_id = user_id.to_owned();
    let settings =
        match storage_result(repo, move |repo| repo.account_ai_settings(&lookup_user_id)).await {
            Ok(settings) => settings,
            Err(_) => {
                return disabled_resolved_ai();
            }
        };
    match settings.mode {
        AiMode::Off => disabled_resolved_ai(),
        AiMode::Builtin => ResolvedAiGateway {
            gateway: state.ai.clone(),
            task_router: None,
            use_builtin_router: true,
            admission: Some(AiCallAdmission {
                user_id: user_id.to_owned(),
                limiter: state.account_ai_limits.clone(),
                charge_daily_quota: true,
            }),
        },
        AiMode::Custom => ResolvedAiGateway {
            // Custom credentials are device-owned and must arrive transiently
            // with the recommendation request.
            gateway: AiGateway::disabled(),
            task_router: None,
            use_builtin_router: false,
            admission: None,
        },
    }
}

fn custom_ai_hosts_match(
    requested: &mpgs_ai::CustomBaseUrlResolution,
    saved: &mpgs_ai::CustomBaseUrlResolution,
) -> bool {
    requested.host.eq_ignore_ascii_case(&saved.host)
        && requested.address.port() == saved.address.port()
}

async fn transient_custom_ai_resolved(
    custom: TransientCustomAiRequest,
    saved_base_url: &str,
    user_id: &str,
    limiter: AccountAiLimiter,
) -> Result<ResolvedAiGateway, Response> {
    if custom.provider != "openai_compat" {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "custom provider must be openai_compat",
            None,
        ));
    }
    if custom.model.trim().is_empty() || custom.api_key.trim().is_empty() {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "custom model and api_key are required",
            None,
        ));
    }
    let (resolution, saved_resolution) = tokio::join!(
        mpgs_ai::resolve_custom_base_url(&custom.base_url),
        mpgs_ai::resolve_custom_base_url(saved_base_url)
    );
    let resolution = resolution.map_err(|_| {
        error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "custom AI endpoint is not allowed",
            None,
        )
    })?;
    let saved_resolution = saved_resolution.map_err(|_| {
        error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "saved custom AI endpoint is no longer valid",
            None,
        )
    })?;
    if !custom_ai_hosts_match(&resolution, &saved_resolution) {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "custom AI endpoint does not match saved account settings",
            None,
        ));
    }
    let timeout = Duration::from_secs(20);
    let provider = OpenAiCompatProvider::new_with_custom_resolution(
        resolution.base_url.clone(),
        custom.api_key.clone(),
        custom.model.clone(),
        timeout,
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
    let provider = Arc::new(provider);
    let gateway = AiGateway::new(
        provider.clone(),
        AiPolicy {
            online_timeout: timeout,
            ..AiPolicy::default()
        },
    );

    let multi = custom.multi_model.unwrap_or(true);
    let task_router = if multi {
        let routes = build_transient_custom_routes(&custom);
        let router = TaskRouter::new(
            provider,
            routes,
            Arc::new(mpgs_ai::ModelRegistry::new()),
            mpgs_ai::RouterPolicy::default(),
        );
        // Keep the registry unknown/optimistic here. Explicit discovery has its
        // own authenticated endpoint; recommendation requests must not add an
        // unadmitted `/models` call or block first paint before AI admission.
        Some(Arc::new(router))
    } else {
        None
    };

    Ok(ResolvedAiGateway {
        gateway,
        task_router,
        use_builtin_router: false,
        admission: Some(AiCallAdmission {
            user_id: user_id.to_owned(),
            limiter,
            charge_daily_quota: false,
        }),
    })
}

fn build_transient_custom_routes(
    custom: &TransientCustomAiRequest,
) -> std::collections::HashMap<AiTaskType, mpgs_ai::TaskRouteConfig> {
    use mpgs_ai::{DEFAULT_ROUTE_VERSION, default_task_routes};
    use std::time::Duration;

    let mut routes = default_task_routes();
    let fallback = custom
        .fallback_model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    if let Some(custom_routes) = &custom.routes {
        for row in custom_routes {
            let Ok(task) = parse_ai_task(&row.task) else {
                continue;
            };
            let primary = row.primary_model.trim();
            if primary.is_empty() {
                continue;
            }
            let entry = routes
                .entry(task)
                .or_insert_with(|| mpgs_ai::TaskRouteConfig {
                    task,
                    primary_model: primary.to_owned(),
                    fallback_models: vec![],
                    protocol_preference: vec![
                        mpgs_ai::ApiProtocol::ChatCompletions,
                        mpgs_ai::ApiProtocol::Responses,
                    ],
                    timeout: Duration::from_secs(20),
                    max_output_tokens: 1_800,
                    enabled: true,
                    route_version: DEFAULT_ROUTE_VERSION.to_owned(),
                });
            entry.primary_model = primary.to_owned();
            entry.fallback_models = row
                .fallback_models
                .iter()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty() && s != primary)
                .collect();
            entry.enabled = true;
        }
    } else {
        // Simple multi-model: same primary for all online tasks + optional shared fallback.
        for route in routes.values_mut() {
            if matches!(
                route.task,
                AiTaskType::IntentParse
                    | AiTaskType::RankExplain
                    | AiTaskType::CompareGames
                    | AiTaskType::GroupAdvice
                    | AiTaskType::GameSummary
            ) {
                route.primary_model = custom.model.clone();
                route.fallback_models = fallback
                    .clone()
                    .into_iter()
                    .filter(|m| m != &custom.model)
                    .collect();
            }
        }
    }
    routes
}

fn parse_ai_task(raw: &str) -> Result<AiTaskType, ()> {
    match raw.trim() {
        "intent_parse" => Ok(AiTaskType::IntentParse),
        "rank_explain" | "rank_analysis" => Ok(AiTaskType::RankExplain),
        "compare_games" => Ok(AiTaskType::CompareGames),
        "group_advice" => Ok(AiTaskType::GroupAdvice),
        "game_summary" => Ok(AiTaskType::GameSummary),
        "data_quality" => Ok(AiTaskType::DataQuality),
        "feature_extract" => Ok(AiTaskType::FeatureExtract),
        "embed" => Ok(AiTaskType::Embed),
        _ => Err(()),
    }
}

#[derive(Debug, Clone)]
struct AiEnhanceOutcome {
    status: AiStatus,
    fallback_reason: Option<String>,
    summary: Option<String>,
    summary_evidence_ids: Vec<String>,
    model: Option<String>,
    protocol: Option<String>,
    route_version: Option<String>,
    used_model_fallback: bool,
    attempted_models: Vec<String>,
}

impl Default for AiEnhanceOutcome {
    fn default() -> Self {
        Self {
            status: AiStatus::Fallback,
            fallback_reason: None,
            summary: None,
            summary_evidence_ids: Vec::new(),
            model: None,
            protocol: None,
            route_version: None,
            used_model_fallback: false,
            attempted_models: Vec::new(),
        }
    }
}

impl AiEnhanceOutcome {
    fn disabled(reason: impl Into<String>) -> Self {
        Self {
            status: AiStatus::Disabled,
            fallback_reason: Some(reason.into()),
            ..Self::default()
        }
    }

    fn fallback(reason: impl Into<String>) -> Self {
        Self {
            status: AiStatus::Fallback,
            fallback_reason: Some(reason.into()),
            ..Self::default()
        }
    }

    fn pending(reason: impl Into<String>) -> Self {
        Self {
            status: AiStatus::Pending,
            fallback_reason: Some(reason.into()),
            ..Self::default()
        }
    }
}

/// Apply optional AI Top-N analysis. Always preserves deterministic items on failure.
async fn enhance_natural_language_with_ai(
    gateway: &AiGateway,
    task_router: Option<&TaskRouter>,
    repo: Option<&Repository>,
    cache_scope: &str,
    query: &str,
    items: &mut [serde_json::Value],
    admission: Option<&AiCallAdmission>,
) -> AiEnhanceOutcome {
    const AI_MAX_RECOMMENDATIONS: usize = 8;
    let online_available =
        gateway.is_available() || task_router.is_some_and(TaskRouter::is_available);
    if items.is_empty() {
        if online_available {
            return AiEnhanceOutcome {
                status: AiStatus::Used,
                ..AiEnhanceOutcome::default()
            };
        }
        return AiEnhanceOutcome::disabled(mpgs_ai::AiError::Disabled.fallback_reason());
    }
    if !online_available {
        // Keep reason text compatible with M4 acceptance wording while exposing disabled status.
        return AiEnhanceOutcome::fallback(mpgs_ai::AiError::Disabled.fallback_reason());
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
        let base_evidence = item
            .get("evidence_ids")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect::<std::collections::HashSet<_>>()
            })
            .unwrap_or_default();
        let compact_item = json!({
            "app_id": app_id,
            "name": item.get("name").cloned().unwrap_or(json!("")),
            // Keep provider-prompt data independent of the current natural-language
            // query so different queries can reuse the longest possible prefix.
            "section": item.get("section").cloned().unwrap_or(json!(null)),
            "release_date": item.get("release_date").cloned().unwrap_or(json!(null)),
            "party": item.get("party").cloned().unwrap_or(json!(null)),
            "multiplayer": item.get("multiplayer").cloned().unwrap_or(json!(null)),
        });
        let evidence_ids =
            mpgs_ai::expand_candidate_evidence_ids(app_id, &compact_item, base_evidence);
        let mut evidence_list: Vec<String> = evidence_ids.iter().cloned().collect();
        evidence_list.sort();
        let mut compact_item = compact_item;
        if let Some(obj) = compact_item.as_object_mut() {
            obj.insert("evidence_ids".into(), json!(evidence_list));
        }
        compact.push(compact_item);
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
        return AiEnhanceOutcome::fallback("no valid candidates for AI analysis");
    }

    let provider_identity = ai_route_cache_identity(gateway, task_router, AiTaskType::RankExplain);
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
                        return AiEnhanceOutcome {
                            status: AiStatus::Cached,
                            summary,
                            summary_evidence_ids: validated.summary_evidence_ids,
                            model: Some(entry.model),
                            route_version: Some(route_version),
                            ..AiEnhanceOutcome::default()
                        };
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

    // Cache hits above return before this point. Every real provider call,
    // including a device-local custom provider, needs the same account-scoped
    // concurrency and minute admission. Built-in calls additionally charge the
    // durable daily budget.
    let Some(admission) = admission else {
        return AiEnhanceOutcome::fallback(AiError::Disabled.fallback_reason());
    };
    let _permit = match acquire_ai_call(repo, admission).await {
        Ok(permit) => permit,
        Err(error) => return AiEnhanceOutcome::fallback(ai_admission_failure_reason(error)),
    };

    let (response, used_model_fallback, attempted_models) =
        match run_rank_completion(gateway, task_router, request).await {
            Ok(result) => result,
            Err(error) => {
                return AiEnhanceOutcome::fallback(safe_ai_failure_reason(&error));
            }
        };
    tracing::info!(
        model = %response.model,
        protocol = ?response.protocol,
        used_model_fallback,
        ?attempted_models,
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
            return AiEnhanceOutcome::fallback(safe_ai_failure_reason(&error));
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
    AiEnhanceOutcome {
        status: AiStatus::Used,
        fallback_reason: degraded_notice,
        summary,
        summary_evidence_ids: validated.summary_evidence_ids,
        model: Some(response.model),
        protocol: response.protocol.map(|p| p.as_str().to_owned()),
        route_version: Some(route_version),
        used_model_fallback,
        attempted_models,
    }
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
            // Prefer sanitizing forged evidence ids over discarding all AI text.
            if let Ok((sanitized, stripped)) =
                mpgs_ai::sanitize_rank_result(value, candidates, max_items)
            {
                let notice = if stripped {
                    Some(format!(
                        "AI output was sanitized after strict validation reported: {strict_reason}"
                    ))
                } else {
                    None
                };
                return Ok((sanitized, notice));
            }
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

fn apply_validated_ai_rank(items: &mut [serde_json::Value], validated: &mpgs_ai::AiRankResult) {
    let ai_by_id: HashMap<_, _> = validated
        .recommendations
        .iter()
        .map(|item| (item.app_id, item))
        .collect();
    let original_positions: HashMap<u32, usize> = items
        .iter()
        .enumerate()
        .filter_map(|(position, item)| {
            item.get("app_id")
                .and_then(|value| value.as_u64())
                .and_then(|value| u32::try_from(value).ok())
                .map(|app_id| (app_id, position))
        })
        .collect();

    // AI output is a set of bounded numeric adjustments, never an authoritative
    // ordering. Mutating the existing set in place also guarantees an AI response
    // cannot add, restore, or accidentally drop a deterministic candidate.
    for item in items.iter_mut() {
        let Some(app_id) = item
            .get("app_id")
            .and_then(|value| value.as_u64())
            .and_then(|value| u32::try_from(value).ok())
        else {
            continue;
        };
        let Some(ai_item) = ai_by_id.get(&app_id) else {
            continue;
        };
        let base = item
            .get("components")
            .and_then(|components| components.get("relevance_score"))
            .and_then(|value| value.as_f64())
            .or_else(|| item.get("score").and_then(|value| value.as_f64()))
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
            if !ai_item.reason_evidence_ids.is_empty() {
                for field in ["evidence_ids", "reason_evidence"] {
                    let mut evidence = obj
                        .get(field)
                        .and_then(serde_json::Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    for evidence_id in &ai_item.reason_evidence_ids {
                        if !evidence
                            .iter()
                            .any(|value| value.as_str() == Some(evidence_id.as_str()))
                        {
                            evidence.push(json!(evidence_id));
                        }
                    }
                    obj.insert(field.into(), json!(evidence));
                }
            }
            if let Some(components) = obj.get_mut("components").and_then(|v| v.as_object_mut()) {
                components.insert("relevance_score".into(), json!(blended));
                components.insert("final_score".into(), json!(blended));
            }
        }
    }

    items.sort_by(|left, right| {
        let left_score = left
            .get("score")
            .and_then(|value| value.as_f64())
            .unwrap_or(f64::NEG_INFINITY);
        let right_score = right
            .get("score")
            .and_then(|value| value.as_f64())
            .unwrap_or(f64::NEG_INFINITY);
        let left_id = left
            .get("app_id")
            .and_then(|value| value.as_u64())
            .and_then(|value| u32::try_from(value).ok());
        let right_id = right
            .get("app_id")
            .and_then(|value| value.as_u64())
            .and_then(|value| u32::try_from(value).ok());
        right_score
            .total_cmp(&left_score)
            .then_with(|| {
                left_id
                    .and_then(|id| original_positions.get(&id).copied())
                    .unwrap_or(usize::MAX)
                    .cmp(
                        &right_id
                            .and_then(|id| original_positions.get(&id).copied())
                            .unwrap_or(usize::MAX),
                    )
            })
            .then_with(|| left_id.cmp(&right_id))
    });
    refresh_json_ranking_metadata(items);
}

fn refresh_json_ranking_metadata(items: &mut [serde_json::Value]) {
    let indices = context_percentile_indices(items.iter().filter_map(|item| {
        Some((
            item.get("app_id")
                .and_then(|value| value.as_u64())
                .and_then(|value| u32::try_from(value).ok())?,
            item.get("components")
                .and_then(|components| components.get("relevance_score"))
                .and_then(|value| value.as_f64())
                .or_else(|| item.get("score").and_then(|value| value.as_f64()))?,
        ))
    }));
    for (position, item) in items.iter_mut().enumerate() {
        let app_id = item
            .get("app_id")
            .and_then(|value| value.as_u64())
            .and_then(|value| u32::try_from(value).ok());
        let had_visible_index = item
            .get("recommendation_index")
            .is_some_and(|value| !value.is_null());
        let data_confidence = item
            .get("data_confidence")
            .and_then(|value| value.as_f64())
            .or_else(|| item.get("confidence").and_then(|value| value.as_f64()))
            .unwrap_or(0.0);
        let recommendation_index = app_id
            .filter(|_| had_visible_index && data_confidence >= MIN_INDEX_DATA_CONFIDENCE)
            .and_then(|id| indices.get(&id).copied());
        if let Some(object) = item.as_object_mut() {
            object.insert("rank".into(), json!(position + 1));
            object.insert("recommendation_index".into(), json!(recommendation_index));
            object.insert("fit_band".into(), json!(fit_band(recommendation_index)));
            object
                .entry("slot_reason".to_owned())
                .or_insert_with(|| json!("base"));
            object.insert(
                "score_calibration_version".into(),
                json!(SCORE_CALIBRATION_VERSION),
            );
        }
    }
}

fn ai_route_cache_identity(
    gateway: &AiGateway,
    task_router: Option<&TaskRouter>,
    task: AiTaskType,
) -> String {
    let route = task_router
        .and_then(|router| router.route_for(task))
        .map(|route| {
            json!({
                "task": task.as_str(),
                "primary_model": route.primary_model,
                "fallback_models": route.fallback_models,
                "protocol_preference": route
                    .protocol_preference
                    .iter()
                    .map(|protocol| protocol.as_str())
                    .collect::<Vec<_>>(),
                "timeout_ms": route.timeout.as_millis(),
                "max_output_tokens": route.max_output_tokens,
                "enabled": route.enabled,
                "route_version": route.route_version,
            })
        })
        .unwrap_or_else(|| json!({ "task": task.as_str(), "route": "direct" }));
    let identity = json!({
        "provider": gateway.provider_cache_identity(),
        "route": route,
    });
    let bytes = serde_json::to_vec(&identity).unwrap_or_default();
    format!("ai-route:{}", hex_sha256(&bytes))
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
) -> Result<(mpgs_ai::StructuredResponse, bool, Vec<String>), AiError> {
    if let Some(router) = task_router
        && router.is_available()
    {
        let routed = router.structured_completion(request).await?;
        return Ok((
            routed.response,
            routed.used_fallback,
            routed.attempted_models,
        ));
    }
    gateway
        .structured_completion(request)
        .await
        .map(|response| (response, false, Vec::new()))
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

fn normalized_ai_query(raw: &str) -> Option<String> {
    let query = raw.trim();
    (3..=500)
        .contains(&query.chars().count())
        .then(|| query.to_owned())
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
    let Some(query) = normalized_ai_query(&body.query) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "query must contain between 3 and 500 characters",
            None,
        );
    };
    let output_limit = body.limit.unwrap_or(6);
    if !(3..=10).contains(&output_limit) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "limit must be between 3 and 10",
            None,
        );
    }

    // Progressive search: base ranking first without waiting for rank AI.
    let nl_body = NaturalLanguageRequest {
        query: query.clone(),
        limit: Some(i64::from(output_limit)),
        custom_ai: None,
        r#async: Some(body.r#async),
        intent_delta: None,
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
    let Some(repo) = state.repo.as_ref() else {
        return storage_disabled();
    };
    let account_user_id = optional_account_user_id(repo, &headers).await;
    let resolved_ai = account_ai_gateway(&state, account_user_id.as_deref()).await;
    let cache_scope = account_user_id
        .as_deref()
        .map(|user_id| format!("account:{user_id}"))
        .unwrap_or_else(|| "anonymous".to_owned());
    {
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
        if let Err(error) =
            storage_result(repo, move |repo| repo.insert_progressive_analysis(&insert)).await
        {
            return map_storage_error(error, None);
        }
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("analysis_id".into(), json!(analysis_id));
            if body.r#async && status_for_row == "pending" {
                obj.insert("ai_status".into(), json!("pending"));
            }
        }
        // Finish rank_explain in the background so the first paint never waits.
        if body.r#async && status_for_row == "pending" {
            let state_bg = state.clone();
            let analysis_id_bg = analysis_id.clone();
            let query_bg = query.clone();
            let items_bg = payload.get("items").cloned().unwrap_or_else(|| json!([]));
            let resolved_ai_bg = resolved_ai.clone();
            let cache_scope_bg = cache_scope.clone();
            tokio::spawn(async move {
                enhance_progressive_analysis_in_background(
                    state_bg,
                    analysis_id_bg,
                    query_bg,
                    items_bg,
                    resolved_ai_bg,
                    cache_scope_bg,
                )
                .await;
            });
        }
    }

    Json(payload).into_response()
}

async fn enhance_progressive_analysis_in_background(
    state: Arc<AppState>,
    analysis_id: String,
    query: String,
    items_value: serde_json::Value,
    resolved_ai: ResolvedAiGateway,
    cache_scope: String,
) {
    let Some(repo) = state.repo.as_ref() else {
        return;
    };
    let mut items = items_value.as_array().cloned().unwrap_or_default();
    let router = resolved_task_router(&resolved_ai, &state);
    let enhance = enhance_natural_language_with_ai(
        &resolved_ai.gateway,
        router,
        Some(repo),
        &cache_scope,
        &query,
        &mut items,
        resolved_ai.admission.as_ref(),
    )
    .await;
    let now_ms = chrono_like_now_ms();
    let provider_name = resolved_ai.gateway.provider_name().to_owned();
    let result = json!({
        "items": items,
        "ai_status": enhance.status.as_str(),
        "fallback_reason": enhance.fallback_reason,
        "ai_summary": enhance.summary,
        "ai_summary_evidence_ids": enhance.summary_evidence_ids,
        "ai_provider": provider_name,
        "ai_model": enhance.model,
        "ai_protocol": enhance.protocol,
        "ai_route_version": enhance.route_version,
        "ai_used_model_fallback": enhance.used_model_fallback,
        "ai_attempted_models": enhance.attempted_models,
    });
    let result_json = result.to_string();
    let status = enhance.status.as_str().to_owned();
    let update = mpgs_storage::CompleteProgressiveAnalysis {
        analysis_id: analysis_id.clone(),
        status,
        provider: Some(provider_name),
        model: enhance.model.clone(),
        protocol: enhance.protocol.clone(),
        route_version: enhance.route_version.or_else(|| {
            router.map(|router| router.route_version(AiTaskType::RankExplain).to_owned())
        }),
        result_json: Some(result_json),
        error_category: enhance
            .fallback_reason
            .as_ref()
            .map(|_| "ai_fallback".into()),
        fallback_reason: enhance.fallback_reason,
        completed_at_ms: now_ms,
    };
    if let Err(error) = storage_result(repo, move |repo| {
        repo.complete_progressive_analysis(&update)
    })
    .await
    {
        tracing::warn!(%analysis_id, %error, "progressive AI enhancement persistence failed");
    } else {
        tracing::info!(
            %analysis_id,
            status = enhance.status.as_str(),
            model = ?enhance.model,
            "progressive AI enhancement completed"
        );
    }
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
    headers: HeaderMap,
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
    let mut allowed_evidence = HashSet::new();
    let mut allowed_columns = HashSet::new();
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
        let mut row_evidence = serde_json::Map::new();
        if profile.as_ref().is_some_and(|profile| {
            profile.recommended_min_players.is_some() || profile.recommended_max_players.is_some()
        }) {
            let evidence_id = format!("app:{app_id}:party_size");
            allowed_evidence.insert(evidence_id.clone());
            allowed_columns.insert("party_size".to_owned());
            row_evidence.insert("party_size".to_owned(), json!(evidence_id));
        }
        if profile.as_ref().is_some_and(|profile| {
            profile.dominant_mode.is_some()
                || profile.online_coop.is_some()
                || profile.crossplay.is_some()
                || profile.drop_in_out.is_some()
        }) {
            let evidence_id = format!("app:{app_id}:multiplayer_mode");
            allowed_evidence.insert(evidence_id.clone());
            allowed_columns.insert("multiplayer_mode".to_owned());
            row_evidence.insert("multiplayer_mode".to_owned(), json!(evidence_id));
        }
        if profile.as_ref().is_some_and(|profile| {
            profile.private_session.is_some() || profile.self_hosted_server.is_some()
        }) {
            let evidence_id = format!("app:{app_id}:service_dependency");
            allowed_evidence.insert(evidence_id.clone());
            allowed_columns.insert("service_dependency".to_owned());
            row_evidence.insert("service_dependency".to_owned(), json!(evidence_id));
        }
        let updated_evidence_id = format!("app:{app_id}:data_updated_at");
        allowed_evidence.insert(updated_evidence_id.clone());
        allowed_columns.insert("data_updated_at".to_owned());
        row_evidence.insert("data_updated_at".to_owned(), json!(updated_evidence_id));
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
            "evidence_ids": row_evidence,
        }));
    }

    let mut allowed_columns_list: Vec<_> = allowed_columns.iter().cloned().collect();
    allowed_columns_list.sort();
    let allowed_ids: Vec<u32> = matrix
        .iter()
        .filter_map(|row| row.get("app_id").and_then(|v| v.as_u64()).map(|v| v as u32))
        .collect();

    let account_user_id = optional_account_user_id(repo, &headers).await;
    let resolved_ai = account_ai_gateway(&state, account_user_id.as_deref()).await;
    let task_router = resolved_task_router(&resolved_ai, &state);
    let provider_available =
        resolved_ai.gateway.is_available() || task_router.is_some_and(TaskRouter::is_available);
    let admission = provider_available
        .then_some(())
        .and(resolved_ai.admission.as_ref());
    let (explanation, ai_status, fallback_reason, model) = if let Some(admission) = admission {
        let data_prompt = format!(
            "{}\n\n{}",
            wrap_untrusted_data_block(
                "fact_matrix_json",
                &serde_json::to_string(&matrix).unwrap_or_else(|_| "[]".into()),
                12_000
            ),
            wrap_untrusted_data_block(
                "allowed_columns",
                &serde_json::to_string(&allowed_columns_list).unwrap_or_else(|_| "[]".into()),
                500
            )
        );
        let request = StructuredRequest {
            task: AiTaskType::CompareGames,
            system_prompt: compare_system_prompt().to_owned(),
            data_prompt,
            json_schema_name: "compare_games".into(),
            json_schema: compare_schema(),
            max_output_tokens: 1_800,
            temperature: 0.2,
            model: None,
            protocol: None,
        };
        match acquire_ai_call(Some(repo), admission).await {
            Ok(_permit) => {
                let completion = if let Some(router) = task_router {
                    router
                        .structured_completion(request)
                        .await
                        .map(|r| r.response)
                } else {
                    resolved_ai.gateway.structured_completion(request).await
                };
                match completion {
                    Ok(response) => {
                        match parse_compare_explanation(
                            &response.content,
                            &allowed_ids,
                            &allowed_evidence,
                        )
                        .and_then(|explanation| {
                            for difference in &explanation.differences {
                                if !allowed_columns.contains(&difference.column)
                                    || difference
                                        .evidence_ids
                                        .iter()
                                        .any(|id| !id.ends_with(&format!(":{}", difference.column)))
                                {
                                    return Err(AiError::InvalidOutput(
                                        "compare evidence does not match the supplied fact column"
                                            .into(),
                                    ));
                                }
                            }
                            Ok(explanation)
                        }) {
                            Ok(expl) => (Some(expl), AiStatus::Used, None, Some(response.model)),
                            Err(error) => (
                                None,
                                AiStatus::Fallback,
                                Some(safe_ai_failure_reason(&error)),
                                Some(response.model),
                            ),
                        }
                    }
                    Err(error) => (
                        None,
                        AiStatus::Fallback,
                        Some(safe_ai_failure_reason(&error)),
                        None,
                    ),
                }
            }
            Err(error) => (
                None,
                AiStatus::Fallback,
                Some(ai_admission_failure_reason(error)),
                None,
            ),
        }
    } else {
        (
            None,
            AiStatus::Disabled,
            Some(AiError::Disabled.fallback_reason().to_owned()),
            None,
        )
    };

    Json(json!({
        "fact_matrix": matrix,
        "ai_status": ai_status.as_str(),
        "explanation": explanation,
        "fallback_reason": fallback_reason,
        "model": model,
        "prompt_version": COMPARE_PROMPT_VERSION,
        "columns": allowed_columns_list
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
    let app = match storage_result(repo, move |repo| repo.get_app(app_id)).await {
        Ok(Some(app)) => app,
        Ok(None) => {
            return error_response(StatusCode::NOT_FOUND, "not_found", "game not found", None);
        }
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "failed to load game",
                None,
            );
        }
    };
    let now_ms = chrono_like_now_ms();
    if let Ok(Some(row)) = storage_result(repo, move |repo| {
        repo.get_game_ai_summary(app_id, SUMMARY_PROMPT_VERSION, now_ms)
    })
    .await
    {
        let summary: serde_json::Value =
            serde_json::from_str(&row.summary_json).unwrap_or(json!({}));
        return Json(json!({
            "app_id": app_id,
            "ai_status": "cached",
            "summary": summary,
            "sections": summary,
            "review_status": row.review_status,
            "model": row.model,
            "prompt_version": row.prompt_version,
            "updated_at_ms": row.updated_at_ms,
            "expires_at_ms": row.expires_at_ms,
            "fallback_reason": null
        }))
        .into_response();
    }

    let profile = storage_result(repo, move |repo| repo.get_profile(app_id))
        .await
        .ok()
        .flatten();
    let evidence = vec![
        format!("app:{app_id}:identity"),
        format!("app:{app_id}:profile"),
    ];
    let summary = rule_game_summary(
        &app.canonical_name,
        profile.as_ref().and_then(|p| p.recommended_min_players),
        profile.as_ref().and_then(|p| p.recommended_max_players),
        profile.as_ref().and_then(|p| p.private_session),
        profile.as_ref().and_then(|p| p.self_hosted_server),
        &evidence,
    );
    let summary_json = serde_json::to_string(&summary).unwrap_or_else(|_| "{}".into());
    let evidence_json = serde_json::to_string(&evidence).unwrap_or_else(|_| "[]".into());
    let _ = storage_result(repo, {
        let summary_json = summary_json.clone();
        let evidence_json = evidence_json.clone();
        move |repo| {
            repo.upsert_game_ai_summary(&mpgs_storage::UpsertGameAiSummary {
                app_id,
                input_hash: format!("rule:{app_id}"),
                prompt_version: SUMMARY_PROMPT_VERSION.into(),
                summary_json,
                evidence_ids_json: evidence_json,
                review_status: "pending_review".into(),
                model: Some("rule".into()),
                created_at_ms: now_ms,
                expires_at_ms: now_ms.saturating_add(7 * 24 * 60 * 60 * 1000),
            })
        }
    })
    .await;

    Json(json!({
        "app_id": app_id,
        "ai_status": "fallback",
        "summary": summary,
        "sections": summary,
        "review_status": "pending_review",
        "model": "rule",
        "prompt_version": SUMMARY_PROMPT_VERSION,
        "updated_at_ms": now_ms,
        "fallback_reason": "offline model summary unavailable; rule summary returned"
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct GroupAdviceHttpRequest {
    party_size: Option<u8>,
    #[serde(default)]
    platforms: Vec<String>,
    #[serde(default)]
    modes_preferred: Vec<String>,
    #[serde(default)]
    modes_excluded: Vec<String>,
    candidate_app_ids: Vec<u32>,
    #[serde(default)]
    vote_counts: Vec<AppVoteCount>,
}

fn valid_group_labels(values: &[String]) -> bool {
    values.len() <= 16
        && values
            .iter()
            .all(|value| !value.trim().is_empty() && value.chars().count() <= 80)
}

async fn ai_group_advice(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<GroupAdviceHttpRequest>,
) -> Response {
    if !(2..=12).contains(&body.candidate_app_ids.len()) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "candidate_app_ids must contain 2 to 12 apps",
            None,
        );
    }
    let mut unique = HashSet::new();
    for id in &body.candidate_app_ids {
        if *id == 0 || !unique.insert(*id) {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "candidate_app_ids must be unique positive AppIDs",
                None,
            );
        }
    }
    if body
        .party_size
        .is_some_and(|party_size| !(1..=64).contains(&party_size))
        || !valid_group_labels(&body.platforms)
        || !valid_group_labels(&body.modes_preferred)
        || !valid_group_labels(&body.modes_excluded)
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "group preferences exceed supported bounds",
            None,
        );
    }
    if body.vote_counts.len() > body.candidate_app_ids.len() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "vote_counts must contain at most one row per candidate",
            None,
        );
    }
    let candidate_ids: HashSet<u32> = body.candidate_app_ids.iter().copied().collect();
    let mut vote_ids = HashSet::new();
    if body
        .vote_counts
        .iter()
        .any(|row| !candidate_ids.contains(&row.app_id) || !vote_ids.insert(row.app_id))
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "vote_counts must be unique and refer to candidate AppIDs",
            None,
        );
    }
    let request = AiGroupAdviceRequest {
        party_size: body.party_size,
        platforms: body.platforms,
        modes_preferred: body.modes_preferred,
        modes_excluded: body.modes_excluded,
        candidate_app_ids: body.candidate_app_ids.clone(),
        vote_counts: body.vote_counts.clone(),
    };
    let mut allowed_evidence = HashSet::new();
    let candidate_facts: Vec<_> = request
        .candidate_app_ids
        .iter()
        .map(|app_id| {
            let evidence_id = format!("candidate:{app_id}");
            allowed_evidence.insert(evidence_id.clone());
            json!({ "app_id": app_id, "evidence_id": evidence_id })
        })
        .collect();
    let vote_facts: Vec<_> = request
        .vote_counts
        .iter()
        .map(|row| {
            let evidence_id = format!("vote:aggregate:{}", row.app_id);
            allowed_evidence.insert(evidence_id.clone());
            json!({
                "app_id": row.app_id,
                "votes": row.votes,
                "evidence_id": evidence_id,
            })
        })
        .collect();
    let det = deterministic_group_advice(&request.candidate_app_ids, &request.vote_counts);
    let account_user_id = match state.repo.as_ref() {
        Some(repo) => optional_account_user_id(repo, &headers).await,
        None => None,
    };
    let resolved_ai = account_ai_gateway(&state, account_user_id.as_deref()).await;
    let task_router = resolved_task_router(&resolved_ai, &state);
    let provider_available =
        resolved_ai.gateway.is_available() || task_router.is_some_and(TaskRouter::is_available);
    let admission = provider_available
        .then_some(())
        .and(resolved_ai.admission.as_ref());
    let (advice, ai_status, fallback_reason) = if let Some(admission) = admission {
        let group_facts = json!({
            "party_size": request.party_size,
            "platforms": request.platforms,
            "modes_preferred": request.modes_preferred,
            "modes_excluded": request.modes_excluded,
            "candidates": candidate_facts,
            "vote_counts": vote_facts,
        });
        let data_prompt = wrap_untrusted_data_block(
            "group_aggregate_json",
            &serde_json::to_string(&group_facts).unwrap_or_else(|_| "{}".into()),
            6_000,
        );
        let structured = StructuredRequest {
            task: AiTaskType::GroupAdvice,
            system_prompt: group_advice_system_prompt().to_owned(),
            data_prompt,
            json_schema_name: "group_advice".into(),
            json_schema: group_advice_schema(),
            max_output_tokens: 1_200,
            temperature: 0.2,
            model: None,
            protocol: None,
        };
        match acquire_ai_call(state.repo.as_ref(), admission).await {
            Ok(_permit) => {
                let completion = if let Some(router) = task_router {
                    router
                        .structured_completion(structured)
                        .await
                        .map(|r| r.response)
                } else {
                    resolved_ai.gateway.structured_completion(structured).await
                };
                match completion {
                    Ok(response) => {
                        match parse_group_advice(
                            &response.content,
                            &request.candidate_app_ids,
                            &allowed_evidence,
                        ) {
                            Ok(parsed) => (Some(parsed), AiStatus::Used, None),
                            Err(error) => (
                                det,
                                AiStatus::Fallback,
                                Some(safe_ai_failure_reason(&error)),
                            ),
                        }
                    }
                    Err(error) => (
                        det,
                        AiStatus::Fallback,
                        Some(safe_ai_failure_reason(&error)),
                    ),
                }
            }
            Err(error) => (
                det,
                AiStatus::Fallback,
                Some(ai_admission_failure_reason(error)),
            ),
        }
    } else {
        (
            det,
            AiStatus::Fallback,
            Some(AiError::Disabled.fallback_reason().to_owned()),
        )
    };

    Json(json!({
        "advice": advice,
        "ai_status": ai_status.as_str(),
        "fallback_reason": fallback_reason,
        "prompt_version": mpgs_ai::GROUP_ADVICE_PROMPT_VERSION
    }))
    .into_response()
}

async fn get_bootstrap_status(State(state): State<Arc<AppState>>) -> Response {
    let Some(repo) = state.repo.as_ref() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "database is not configured",
            None,
        );
    };
    let raw = storage_result(repo, |repo| repo.get_bootstrap_state("first_start"))
        .await
        .ok()
        .flatten();
    let value = raw
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or_else(|| {
            json!({
                "mode": "normal",
                "priority_target": 300,
                "priority_remaining": null,
                "store_only": false
            })
        });
    Json(json!({ "bootstrap": value })).into_response()
}

#[derive(Debug, Deserialize)]
struct BootstrapStartRequest {
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    priority_target: Option<u32>,
}

async fn start_bootstrap(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<BootstrapStartRequest>,
) -> Response {
    if let Err(response) = require_admin(&state, &headers) {
        return *response;
    }
    let Some(repo) = state.repo.as_ref() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "database is not configured",
            None,
        );
    };
    let mode = body.mode.unwrap_or_else(|| "store_only".into());
    if mode != "store_only" && mode != "normal" {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "mode must be store_only or normal",
            None,
        );
    }
    let priority_target = body.priority_target.unwrap_or(300).clamp(1, 5_000);
    let targets = storage_result(repo, move |repo| {
        repo.list_enrichment_targets_after_filtered(
            priority_target,
            None,
            "CN",
            "schinese",
            mpgs_storage::EnrichmentNeedFilter {
                store: true,
                reviews: false,
                review_excerpts: false,
                ccu: false,
                price: false,
                media_backfill: false,
                english_name: false,
            },
        )
    })
    .await
    .unwrap_or_default();
    let mut enqueued = 0_u32;
    for target in &targets {
        let app_id = target.app_id;
        let name = storage_result(repo, move |repo| repo.get_app(app_id))
            .await
            .ok()
            .flatten()
            .map(|a| a.canonical_name)
            .unwrap_or_else(|| format!("app-{app_id}"));
        let missing = vec!["store_details".into()];
        if storage_result(repo, {
            let name = name.clone();
            let missing = missing.clone();
            move |repo| repo.enqueue_web_discovery(app_id, &name, &missing)
        })
        .await
        .is_ok()
        {
            enqueued = enqueued.saturating_add(1);
        }
    }
    let payload = json!({
        "mode": mode,
        "store_only": mode == "store_only",
        "priority_target": priority_target,
        "priority_remaining": priority_target.saturating_sub(enqueued),
        "web_discovery_enqueued": enqueued,
        "started_at_ms": chrono_like_now_ms()
    });
    let payload_text = payload.to_string();
    let _ = storage_result(repo, move |repo| {
        repo.put_bootstrap_state("first_start", &payload_text)
    })
    .await;
    Json(json!({ "bootstrap": payload })).into_response()
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
    let upcoming_explicit =
        contains_any(&normalized, &["即将", "未发售", "coming soon"]) || demo_only;
    let classic_explicit = contains_any(
        &normalized,
        &["经典", "老游戏", "反复刷", "耐玩", "replayable"],
    );
    let popular_explicit = contains_any(&normalized, &["热门", "人多", "popular"]);
    let detected_section = if upcoming_explicit {
        FeedSection::Upcoming
    } else if classic_explicit {
        FeedSection::ClassicLegacy
    } else if popular_explicit {
        FeedSection::PopularLegacy
    } else {
        FeedSection::RecentRelease
    };
    let mut modes_preferred = Vec::new();
    if contains_any(&normalized, &["合作", "coop", "co-op", "co op"]) {
        modes_preferred.push("private_coop".to_owned());
    }
    if contains_any(&normalized, &["竞技", "排位", "competitive", "ranked"])
        && coop_competitive.is_some_and(|value| value >= 0.65)
    {
        modes_preferred.push("matchmade_pvp".to_owned());
    }
    if self_hosting_willingness.is_some() {
        modes_preferred.push("self_hosted".to_owned());
    }
    let mut modes_excluded = Vec::new();
    if contains_any(
        &normalized,
        &[
            "不要pvp",
            "不要 pvp",
            "不玩pvp",
            "不玩 pvp",
            "排除竞技",
            "拒绝竞技",
            "no pvp",
        ],
    ) {
        modes_excluded.push("matchmade_pvp".to_owned());
    }
    let mut hard_constraints = Vec::new();
    if party_size.is_some() {
        hard_constraints.push("party_size".to_owned());
    }
    if session_minutes_max.is_some() {
        hard_constraints.push("session_minutes".to_owned());
    }
    if !platforms.is_empty() {
        hard_constraints.push("platforms".to_owned());
    }
    if demo_only {
        hard_constraints.push("demo_required".to_owned());
    }
    if self_hosting_willingness.is_some() {
        hard_constraints.push("self_hosting".to_owned());
    }
    if !modes_excluded.is_empty() {
        hard_constraints.push("modes_excluded".to_owned());
    }
    NaturalLanguageInterpretation {
        party_size,
        session_minutes_max,
        coop_competitive,
        self_hosting_willingness,
        platforms,
        demo_only,
        selected_section: (upcoming_explicit || classic_explicit || popular_explicit)
            .then_some(detected_section),
        selected_section_explicit: upcoming_explicit || classic_explicit || popular_explicit,
        max_price_minor: None,
        currency: None,
        modes_preferred,
        modes_excluded,
        hard_constraints,
        applied_constraints: Vec::new(),
        unapplied_constraints: Vec::new(),
        intent_confidence: None,
    }
}

fn apply_intent_delta(interpreted: &mut NaturalLanguageInterpretation, delta: &serde_json::Value) {
    if let Some(size) = delta.get("party_size").and_then(|v| v.as_u64())
        && (1..=64).contains(&size)
    {
        interpreted.party_size = Some(size as u8);
        push_hard(&mut interpreted.hard_constraints, "party_size");
    }
    if let Some(max) = delta
        .pointer("/session_minutes/max")
        .and_then(|v| v.as_u64())
    {
        interpreted.session_minutes_max = u32::try_from(max).ok();
        push_hard(&mut interpreted.hard_constraints, "session_minutes");
    }
    if let Some(platforms) = delta.get("platforms").and_then(|v| v.as_array()) {
        let mut next = Vec::new();
        for p in platforms {
            if let Some(s) = p.as_str() {
                match s {
                    "windows" | "macos" | "linux" | "steamdeck" => next.push(s.to_owned()),
                    _ => {}
                }
            }
        }
        if !next.is_empty() {
            interpreted.platforms = next;
            push_hard(&mut interpreted.hard_constraints, "platforms");
        }
    }
    if let Some(budget) = delta.pointer("/budget/max_each").and_then(|v| v.as_i64())
        && budget >= 0
    {
        interpreted.max_price_minor = Some(budget);
        interpreted.currency = delta
            .pointer("/budget/currency")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
            .or_else(|| Some("CNY".into()));
        push_hard(&mut interpreted.hard_constraints, "budget");
    }
}

fn apply_structured_intent(
    interpreted: &mut NaturalLanguageInterpretation,
    intent: &mpgs_ai::StructuredIntent,
) {
    if (intent.is_hard("party_size") || interpreted.party_size.is_none())
        && let Some(size) = intent.party_size
    {
        interpreted.party_size = Some(size);
    }
    if (intent.is_hard("platforms") || interpreted.platforms.is_empty())
        && !intent.platforms.is_empty()
    {
        interpreted.platforms = intent.platforms.clone();
    }
    if let Some(session) = &intent.session_minutes
        && (intent.is_hard("session_minutes") || interpreted.session_minutes_max.is_none())
        && let Some(max) = session.max
    {
        interpreted.session_minutes_max = Some(max);
    }
    if let Some(budget) = &intent.budget
        && intent.is_hard("budget")
    {
        interpreted.max_price_minor = budget.max_each;
        interpreted.currency = budget.currency.clone();
    }
    if intent.self_hosting.as_deref() == Some("required") {
        interpreted.self_hosting_willingness = Some(1.0);
    }
    if intent.demo_required == Some(true) {
        interpreted.demo_only = true;
        interpreted.selected_section = Some(FeedSection::Upcoming);
        interpreted.selected_section_explicit = true;
    }
    interpreted.modes_preferred = intent.modes_preferred.clone();
    interpreted.modes_excluded = intent.modes_excluded.clone();
    interpreted.hard_constraints = intent.hard_constraints.clone();
    interpreted.intent_confidence = Some(intent.confidence);
}

/// Normalize parsed mode aliases and report which structured dimensions the
/// deterministic feed can actually honor. Keeping this accounting next to the
/// query translation prevents the UI from presenting an inferred field as an
/// enforced filter when the current ranker only has a soft approximation.
fn finalize_natural_language_constraints(interpreted: &mut NaturalLanguageInterpretation) {
    interpreted.applied_constraints.clear();
    interpreted.unapplied_constraints.clear();

    if interpreted.party_size.is_some() {
        push_hard(&mut interpreted.applied_constraints, "party_size");
    }
    if interpreted.session_minutes_max.is_some() {
        push_hard(&mut interpreted.applied_constraints, "session_minutes");
    }
    if !interpreted.platforms.is_empty() {
        push_hard(&mut interpreted.applied_constraints, "platforms");
    }
    if interpreted.max_price_minor.is_some() {
        push_hard(&mut interpreted.applied_constraints, "budget");
    }
    if interpreted.demo_only {
        push_hard(&mut interpreted.applied_constraints, "demo_required");
    }
    if interpreted.selected_section_explicit {
        push_hard(&mut interpreted.applied_constraints, "selected_section");
    }
    if interpreted.coop_competitive.is_some() {
        push_hard(&mut interpreted.applied_constraints, "coop_competitive");
    }
    if interpreted.self_hosting_willingness.is_some() {
        push_hard(
            &mut interpreted.applied_constraints,
            "self_hosting_preference",
        );
    }

    let mut normalized_preferred = Vec::new();
    let mut prefers_coop = false;
    let mut prefers_pvp = false;
    let mut prefers_self_hosting = false;
    for raw in std::mem::take(&mut interpreted.modes_preferred) {
        let family = ModeFamily::from_alias(&raw);
        if family == ModeFamily::Unknown {
            push_hard(
                &mut interpreted.unapplied_constraints,
                &format!("modes_preferred:{raw}"),
            );
            normalized_preferred.push(raw);
            continue;
        }
        push_hard(&mut normalized_preferred, family.as_str());
        match family {
            ModeFamily::PrivateCoop => prefers_coop = true,
            ModeFamily::MatchmadePvp => prefers_pvp = true,
            ModeFamily::SelfHosted => prefers_self_hosting = true,
            ModeFamily::PublicWorld | ModeFamily::Mixed | ModeFamily::GenericMultiplayer => {
                push_hard(
                    &mut interpreted.unapplied_constraints,
                    &format!("modes_preferred:{}", family.as_str()),
                );
            }
            ModeFamily::Unknown => unreachable!("handled above"),
        }
    }
    interpreted.modes_preferred = normalized_preferred;
    if prefers_coop && prefers_pvp {
        push_hard(
            &mut interpreted.unapplied_constraints,
            "modes_preferred:coop_and_pvp",
        );
    } else if prefers_coop {
        interpreted.coop_competitive = Some(0.1);
        push_hard(&mut interpreted.applied_constraints, "modes_preferred");
    } else if prefers_pvp {
        interpreted.coop_competitive = Some(0.85);
        push_hard(&mut interpreted.applied_constraints, "modes_preferred");
    }
    if prefers_self_hosting {
        interpreted.self_hosting_willingness = Some(1.0);
        push_hard(&mut interpreted.applied_constraints, "modes_preferred");
    }

    let mut normalized_excluded = Vec::new();
    for raw in std::mem::take(&mut interpreted.modes_excluded) {
        let family = ModeFamily::from_alias(&raw);
        if family == ModeFamily::Unknown {
            push_hard(
                &mut interpreted.unapplied_constraints,
                &format!("modes_excluded:{raw}"),
            );
            continue;
        }
        push_hard(&mut normalized_excluded, family.as_str());
    }
    interpreted.modes_excluded = normalized_excluded;
    if !interpreted.modes_excluded.is_empty() {
        push_hard(&mut interpreted.applied_constraints, "modes_excluded");
    }

    // Hosting willingness is represented by a scoring signal today, but there
    // is no objective self-hosting hard-filter dimension in HardConstraints.
    if interpreted
        .hard_constraints
        .iter()
        .any(|field| field == "self_hosting")
    {
        push_hard(
            &mut interpreted.unapplied_constraints,
            "self_hosting_hard_filter",
        );
    }
    for field in interpreted.hard_constraints.clone() {
        let represented = matches!(
            field.as_str(),
            "party_size"
                | "platforms"
                | "budget"
                | "session_minutes"
                | "demo_required"
                | "modes_excluded"
                | "self_hosting"
        );
        if !represented {
            push_hard(
                &mut interpreted.unapplied_constraints,
                &format!("hard_constraint:{field}"),
            );
        }
    }
}

fn push_hard(list: &mut Vec<String>, field: &str) {
    if !list.iter().any(|f| f == field) {
        list.push(field.to_owned());
    }
}

async fn try_ai_intent_parse(
    router: &TaskRouter,
    gateway: &AiGateway,
    repo: Option<&Repository>,
    admission: Option<&AiCallAdmission>,
    query: &str,
    rules: &NaturalLanguageInterpretation,
) -> Option<mpgs_ai::StructuredIntent> {
    if !router.is_available() && !gateway.is_available() {
        return None;
    }
    let baseline = RuleIntentBaseline {
        party_size: rules.party_size,
        platforms: rules.platforms.clone(),
        session_minutes_max: rules.session_minutes_max,
        demo_required: rules.demo_only,
        self_hosting_required: rules.self_hosting_willingness.is_some_and(|v| v >= 0.5),
        coop_competitive: rules.coop_competitive,
    };
    let data_prompt = wrap_untrusted_data_block("user_query", query, 500);
    let request = StructuredRequest {
        task: AiTaskType::IntentParse,
        system_prompt: intent_parse_system_prompt().to_owned(),
        data_prompt,
        json_schema_name: "intent_parse".into(),
        json_schema: intent_parse_schema(),
        max_output_tokens: 512,
        temperature: 0.0,
        model: None,
        protocol: None,
    };
    let admission = admission?;
    let _permit = acquire_ai_call(repo, admission).await.ok()?;
    let response = if router.is_available() {
        router.structured_completion(request).await.ok()?.response
    } else {
        gateway.structured_completion(request).await.ok()?
    };
    let ai = parse_structured_intent(&response.content).ok()?;
    Some(merge_intent_with_rules(ai, &baseline))
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

const UPCOMING_CALENDAR_DAYS: i64 = 60;

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
        now_ms.saturating_add(UPCOMING_CALENDAR_DAYS * 24 * 60 * 60 * 1000)
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
        "calendar:v4:{calendar_state}:{from}:{to}:{data_updated_at_ms}:{}",
        build_git_sha()
    ));
    if let Some(response) = if_none_match_ok(&headers, &etag) {
        return response;
    }
    match storage_result(repo, move |repo| {
        let (dated, undated) =
            repo.list_calendar_cached(data_updated_at_ms, &from, &to, &calendar_state)?;
        Ok((
            dated
                .into_iter()
                .map(calendar_item_json)
                .collect::<Vec<_>>(),
            undated
                .into_iter()
                .map(calendar_item_json)
                .collect::<Vec<_>>(),
        ))
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

fn calendar_item_json(row: mpgs_storage::query::CalendarItemRow) -> serde_json::Value {
    let item = row.app;
    let review_total = row.review_total;
    json!({
        "app_id": item.app_id,
        "app_type": item.app_type,
        "canonical_name": item.canonical_name,
        "cover_url": row.cover_url.or_else(|| resolve_feed_cover_url(item.app_id, None)),
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
                "algorithm_version": ALGORITHM_VERSION,
                "config_version": active_config.version,
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
    let media_assets = match storage_result(repo, move |repo| repo.game_media_assets(app_id)).await
    {
        Ok(value) => value,
        Err(error) => return map_storage_error(error, None),
    };
    let media_updated_at_ms = media_assets
        .iter()
        .map(|asset| asset.updated_at_ms)
        .max()
        .unwrap_or(0);
    let cache_identity = user_id.as_deref().unwrap_or("public");
    let etag = weak_etag(&format!(
        "game:v4:{app_id}:{data_updated_at_ms}:media{media_updated_at_ms}:{}:pi{}:{}:{}:user{cache_identity}",
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
            let media = game_media_json(&media_assets);
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
                "media": media,
                "multiplayer": {
                    "dominant_mode": mpgs_storage::resolve_display_dominant_mode(
                        game.dominant_mode.as_deref(),
                        game.online_coop,
                    ),
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
                "algorithm_version": ALGORITHM_VERSION,
                "config_version": active_config.version,
                "data_updated_at_ms": data_updated_at_ms,
            });
            (StatusCode::OK, [(header::ETAG, etag)], Json(body)).into_response()
        }
        Ok(None) => error_response(StatusCode::NOT_FOUND, "not_found", "game not found", None),
        Err(error) => map_storage_error(error, None),
    }
}

/// Build the always-present `media` object for game detail responses.
fn game_media_json(assets: &[mpgs_storage::GameMediaAssetRow]) -> serde_json::Value {
    let mut screenshots = Vec::new();
    let mut videos = Vec::new();
    let mut updated_at_ms: Option<i64> = None;
    for asset in assets {
        updated_at_ms = Some(
            updated_at_ms
                .map(|prev| prev.max(asset.updated_at_ms))
                .unwrap_or(asset.updated_at_ms),
        );
        match asset.kind.as_str() {
            "screenshot" => {
                if let Some(full_url) = asset.full_url.as_ref() {
                    screenshots.push(json!({
                        "id": asset.source_id,
                        "thumbnail_url": asset.thumbnail_url,
                        "full_url": full_url,
                    }));
                }
            }
            "movie" => {
                videos.push(json!({
                    "id": asset.source_id,
                    "title": asset.title,
                    "poster_url": asset.thumbnail_url,
                    "highlight": asset.is_highlight,
                    "mp4_url": asset.mp4_url,
                    "hls_h264_url": asset.hls_h264_url,
                    "dash_h264_url": asset.dash_h264_url,
                }));
            }
            _ => {}
        }
    }
    json!({
        "updated_at_ms": updated_at_ms,
        "screenshots": screenshots,
        "videos": videos,
    })
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
                (
                    "private_session",
                    game.private_session.map(serde_json::Value::from),
                ),
                (
                    "self_hosted_server",
                    game.self_hosted_server.map(serde_json::Value::from),
                ),
                ("online_coop", game.online_coop.map(serde_json::Value::from)),
                (
                    "matchmaking_core",
                    game.resolved_matchmaking_core()
                        .map(serde_json::Value::from),
                ),
                (
                    "public_world_dependency",
                    game.resolved_public_world_dependency()
                        .map(serde_json::Value::from),
                ),
                (
                    "service_shutdown_risk",
                    game.resolved_service_shutdown_risk()
                        .map(serde_json::Value::from),
                ),
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
#[serde(deny_unknown_fields)]
struct RecommendationEventBody {
    recommendation_run_id: String,
    app_id: u32,
    event_type: String,
    client_created_at_ms: Option<i64>,
}

#[utoipa::path(
    post,
    path = "/v1/recommendation-events",
    request_body = RecommendationEventBody,
    params(("Idempotency-Key" = String, Header, description = "Client-generated idempotency key")),
    responses(
        (status = 201, body = RecommendationEventResponseSchema),
        (status = 400, body = ErrorBody),
        (status = 404, body = ErrorBody),
        (status = 409, body = ErrorBody)
    ),
    tag = "recommendations"
)]
async fn post_recommendation_event(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<RecommendationEventBody>,
) -> impl IntoResponse {
    let Some(repo) = require_repo(&state) else {
        return storage_disabled();
    };
    let Some((event_type, event_category)) = public_recommendation_event(&body.event_type) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "event_type must be one of exposure, detail_open, steam_click, or play_intent",
            None,
        );
    };
    let Some(idempotency_key) = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .filter(|value| !value.trim().is_empty() && value.len() <= 128)
    else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "Idempotency-Key header must contain between 1 and 128 bytes",
            None,
        );
    };
    if body.recommendation_run_id.trim().is_empty() || body.recommendation_run_id.len() > 128 {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "recommendation_run_id must contain between 1 and 128 bytes",
            None,
        );
    }

    let recommendation_run_id = body.recommendation_run_id;
    let app_id = body.app_id;
    let attribution_run_id = recommendation_run_id.clone();
    match storage_result(repo, move |repo| {
        repo.recommendation_item_attribution(&attribution_run_id, app_id)
    })
    .await
    {
        Ok(Some(_)) => {}
        Ok(None) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "recommendation_run_id does not contain this app",
                None,
            );
        }
        Err(error) => return map_storage_error(error, None),
    }

    let event = InsertRecommendationEvent {
        recommendation_run_id,
        app_id,
        event_type: event_type.to_owned(),
        idempotency_key,
        client_created_at_ms: body.client_created_at_ms,
        metadata: json!({
            "schema_version": 1,
            "event_category": event_category,
            "source": "public_recommendation_event_api",
        }),
    };
    match storage_result(repo, move |repo| repo.insert_recommendation_event(&event)).await {
        Ok(record) => (
            StatusCode::CREATED,
            Json(recommendation_event_json(&record)),
        )
            .into_response(),
        Err(error) => map_storage_error(error, None),
    }
}

fn public_recommendation_event(event_type: &str) -> Option<(&'static str, &'static str)> {
    match event_type {
        "exposure" => Some(("exposure", "impression")),
        "detail_open" => Some(("detail_open", "engagement")),
        "steam_click" => Some(("steam_click", "outbound")),
        "play_intent" => Some(("play_intent", "intent")),
        _ => None,
    }
}

fn recommendation_event_json(
    record: &mpgs_storage::RecommendationEventRecord,
) -> serde_json::Value {
    json!({
        "recommendation_event_id": record.recommendation_event_id,
        "recommendation_run_id": record.recommendation_run_id,
        "app_id": record.app_id,
        "event_type": record.event_type,
        "client_created_at_ms": record.client_created_at_ms,
        "metadata": record.metadata,
        "created_at_ms": record.created_at_ms,
    })
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
    let app_id = body.app_id;
    let recommendation_run_id = body.recommendation_run_id.clone();
    let client_created_at_ms = body.client_created_at_ms;
    if let Some(run_id) = recommendation_run_id.as_ref() {
        let run_id = run_id.clone();
        match storage_result(repo, move |repo| {
            repo.recommendation_item_attribution(&run_id, app_id)
        })
        .await
        {
            Ok(Some(_)) => {}
            Ok(None) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    "recommendation_run_id does not contain this app",
                    None,
                );
            }
            Err(error) => return map_storage_error(error, None),
        }
    }
    let feedback_user_id = user_id.clone();
    let feedback_run_id = recommendation_run_id.clone();
    let feedback_idem = idem.clone();
    match storage_result(repo, move |repo| {
        repo.create_feedback(
            &feedback_user_id,
            app_id,
            feedback_type,
            feedback_run_id.as_deref(),
            &feedback_idem,
            client_created_at_ms,
        )
    })
    .await
    {
        Ok(record) => {
            if let Some(run_id) = recommendation_run_id {
                let event = InsertRecommendationEvent {
                    recommendation_run_id: run_id,
                    app_id,
                    event_type: feedback_type.as_str().to_owned(),
                    idempotency_key: idem,
                    client_created_at_ms,
                    metadata: json!({ "feedback_type": feedback_type.as_str() }),
                };
                if let Err(error) =
                    storage_result(repo, move |repo| repo.insert_recommendation_event(&event)).await
                {
                    tracing::warn!(%error, app_id, "failed to persist attributed feedback event");
                }
            }
            (StatusCode::CREATED, Json(record_json(&record))).into_response()
        }
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
    let snapshot = match storage_result(repo, |repo| repo.pipeline_status_snapshot()).await {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => mpgs_storage::PipelineStatusSnapshot::default(),
        Err(error) => return map_storage_error(error, None),
    };
    let latest_runs = match storage_result(repo, |repo| repo.latest_source_runs()).await {
        Ok(runs) => runs,
        Err(error) => return map_storage_error(error, None),
    };
    let worker_queue = match storage_result(repo, |repo| repo.pipeline_worker_queue_status()).await
    {
        Ok(queue) => queue,
        Err(error) => return map_storage_error(error, None),
    };
    (
        StatusCode::OK,
        Json(json!({
            "tasks": tasks,
            "coverage": snapshot.coverage,
            "m7_coverage": snapshot.m7_coverage,
            "dimension_coverage": snapshot.dimension_coverage,
            "latest_runs": latest_runs,
            "worker_queue": worker_queue,
            "integrated_ingestion": snapshot.integrated_ingestion,
            "inventory": snapshot.inventory,
            "generated_at_ms": snapshot.generated_at_ms,
            "build_git_sha": build_git_sha(),
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
    if let Some(excluded_modes) = query.excluded_modes.as_deref() {
        prefs.excluded_modes = parse_csv_filter("excluded_modes", excluded_modes)?;
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

fn hard_constraints_from_feed_query(query: &FeedQuery) -> HardConstraints {
    HardConstraints {
        modes: query.excluded_modes.is_some()
            || query.coop_competitive.is_some()
            || query.self_hosting_willingness.is_some(),
        party_size: query.party_size.is_some(),
        platforms: query.platforms.is_some(),
        languages: query.languages.is_some(),
        session_length: query.session_minutes_min.is_some() || query.session_minutes_max.is_some(),
        budget: query.max_price_minor.is_some() || query.currency.is_some(),
    }
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

/// Derive ranking entropy without returning, logging, or persisting the raw
/// account identifier. The UTC day is part of both the seed and feed context,
/// so cursors/ETags cannot silently reuse yesterday's exact-tie ordering.
fn recommendation_tie_seed(user_id: Option<&str>, utc_day: &str) -> u64 {
    hash64(&format!(
        "recommendation-tie-v1|{}|{utc_day}",
        user_id.unwrap_or("public")
    ))
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
    use axum::http::Request;
    use std::collections::HashSet;
    use tower::ServiceExt;

    fn api_test_state(ai: AiGateway) -> AppState {
        AppState {
            repo: None,
            admin_token: None,
            rate_limits: RateLimitConfig {
                enabled: false,
                ..RateLimitConfig::default()
            },
            cors: CorsConfig::default(),
            account_ai_limits: AccountAiLimiter::default(),
            ai,
            task_router: Arc::new(TaskRouter::from_provider(Arc::new(
                mpgs_ai::DisabledProvider,
            ))),
            embedding: Arc::new(mpgs_ai::HashEmbeddingProvider { dimensions: 64 }),
        }
    }

    fn recall_candidate(app_id: u32, relevance_score: f64) -> RankedCandidate {
        RankedCandidate {
            app_id,
            name: format!("game-{app_id}"),
            dominant_mode: Some("private_coop".into()),
            taxonomy_tags: vec!["coop".into()],
            publisher: None,
            series: None,
            recommended_min: Some(1),
            recommended_max: Some(4),
            data_confidence: 0.8,
            score: ScoreBreakdown {
                friend_fit: 0.5,
                section_score: relevance_score,
                personalized_score: relevance_score,
                group_fit: 0.5,
                mode_fit: 0.5,
                access_fit: 0.5,
                hosting_fit: 0.5,
                session_fit: 0.5,
                quality: 0.5,
                activity: 0.5,
                freshness: 0.5,
                risk: 0.0,
                relevance_score,
                final_score: relevance_score.clamp(0.0, 1.0),
            },
            explanation: Explanation {
                reasons: Vec::new(),
                cautions: Vec::new(),
                evidence_ids: Vec::new(),
            },
            slot_reason: SlotReason::Base,
            algorithm_version: ALGORITHM_VERSION.into(),
        }
    }

    fn feed_presentation(data_confidence: f64, effective_feature_count: usize) -> FeedPresentation {
        FeedPresentation {
            release_date: None,
            release_date_raw: None,
            release_date_precision: None,
            cover_url: None,
            cover_updated_at_ms: None,
            total_reviews: None,
            total_positive: None,
            latest_ccu: None,
            typical_ccu_7d: None,
            data_confidence,
            effective_feature_count,
            profile_observed_at_ms: None,
            reviews_observed_at_ms: None,
            activity_observed_at_ms: None,
            price_observed_at_ms: None,
            release_observed_at_ms: None,
        }
    }

    fn test_router_with_rank_models(primary: &str, fallbacks: &[&str]) -> TaskRouter {
        let mut routes = mpgs_ai::default_task_routes();
        let route = routes
            .get_mut(&AiTaskType::RankExplain)
            .expect("default rank route");
        route.primary_model = primary.to_owned();
        route.fallback_models = fallbacks.iter().map(|model| (*model).to_owned()).collect();
        TaskRouter::new(
            Arc::new(mpgs_ai::FakeProvider::default()),
            routes,
            Arc::new(mpgs_ai::ModelRegistry::new()),
            mpgs_ai::RouterPolicy::default(),
        )
    }

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
    fn avatar_dimensions_are_rejected_before_decode_allocation() {
        assert!(validate_avatar_dimensions(4_000, 4_000).is_ok());
        assert!(validate_avatar_dimensions(4_001, 4_000).is_err());
        assert!(validate_avatar_dimensions(16_001, 1).is_err());
        assert!(validate_avatar_dimensions(0, 128).is_err());
    }

    #[test]
    fn ai_query_limits_count_unicode_characters_not_utf8_bytes() {
        assert_eq!(
            normalized_ai_query("  三人合作  ").as_deref(),
            Some("三人合作")
        );
        assert!(normalized_ai_query("游戏").is_none());
        assert!(normalized_ai_query(&"游".repeat(500)).is_some());
        assert!(normalized_ai_query(&"游".repeat(501)).is_none());
    }

    #[test]
    fn feed_result_context_binds_sort_order_and_demo_filter() {
        let base = feed_result_context("prefs", FeedSort::Recommended, FeedSortOrder::Desc, false);
        assert_ne!(
            base,
            feed_result_context("prefs", FeedSort::Ccu, FeedSortOrder::Desc, false)
        );
        assert_eq!(
            base,
            feed_result_context("prefs", FeedSort::Recommended, FeedSortOrder::Asc, false)
        );
        assert_ne!(
            base,
            feed_result_context("prefs", FeedSort::Recommended, FeedSortOrder::Desc, true)
        );
    }

    #[test]
    fn generic_natural_language_recall_is_not_pinned_to_recent() {
        let generic = interpret_natural_language("四人合作，单局一小时以内，不要太竞技");
        assert_eq!(generic.selected_section, None);
        assert!(!generic.selected_section_explicit);

        let classic = interpret_natural_language("想找四人耐玩的经典老游戏");
        assert_eq!(classic.selected_section, Some(FeedSection::ClassicLegacy));
        assert!(classic.selected_section_explicit);

        let upcoming = interpret_natural_language("想玩即将发售的多人 demo");
        assert_eq!(upcoming.selected_section, Some(FeedSection::Upcoming));
        assert!(upcoming.selected_section_explicit);
    }

    #[test]
    fn natural_language_constraints_report_what_is_really_applied() {
        let mut interpreted = interpret_natural_language("四人合作，不要 pvp，想要自建服 demo");
        finalize_natural_language_constraints(&mut interpreted);

        assert_eq!(interpreted.party_size, Some(4));
        assert!(interpreted.demo_only);
        assert!(
            interpreted
                .modes_preferred
                .iter()
                .any(|mode| mode == "private_coop")
        );
        assert!(
            interpreted
                .modes_excluded
                .iter()
                .any(|mode| mode == "matchmade_pvp")
        );
        assert!(
            interpreted
                .applied_constraints
                .iter()
                .any(|field| field == "modes_excluded")
        );
        assert!(
            interpreted
                .unapplied_constraints
                .iter()
                .any(|field| field == "self_hosting_hard_filter")
        );
    }

    #[test]
    fn rrf_hit_beyond_the_old_top_75_is_admitted_from_the_full_ranked_section() {
        let ranked = (1..=100)
            .map(|app_id| recall_candidate(app_id, 1.0 - f64::from(app_id) / 1_000.0))
            .collect();

        let selected = select_internal_recall_candidates(ranked, &[100], 75);
        let ids: Vec<u32> = selected.iter().map(|item| item.app_id).collect();

        assert_eq!(selected.len(), 75);
        assert_eq!(
            ids[0], 100,
            "RRF admission must happen before page truncation"
        );
        assert!(ids.contains(&1), "relevance still fills remaining capacity");

        let eligible = selected
            .into_iter()
            .map(|item| {
                let app_id = item.app_id;
                (
                    app_id,
                    json!({
                        "app_id": app_id,
                        "score": item.score.final_score,
                        "components": {"relevance_score": item.score.relevance_score},
                    }),
                )
            })
            .collect();
        let hit = HybridHit {
            app_id: 100,
            score: 1.0 / 61.0,
            fts_rank: Some(1.0),
            vector_score: None,
        };
        let union = build_natural_language_candidate_pool(eligible, &[hit], 75);
        assert_eq!(json_app_id(&union[0]), 100);
        assert!(union[0].get("hybrid_score").is_some());
    }

    #[test]
    fn retrieval_cannot_revive_an_id_absent_after_hard_filters() {
        // `999` represents a game removed before ranking by an objective/request
        // hard filter or negative feedback. It is deliberately absent here.
        let ranked = vec![recall_candidate(1, 0.9), recall_candidate(2, 0.8)];
        let selected = select_internal_recall_candidates(ranked, &[999, 2], 2);
        let ids: Vec<u32> = selected.iter().map(|item| item.app_id).collect();

        assert_eq!(ids, vec![2, 1]);
        assert!(!ids.contains(&999));

        let eligible = selected
            .into_iter()
            .map(|item| {
                let app_id = item.app_id;
                (
                    app_id,
                    json!({
                        "app_id": app_id,
                        "score": item.score.final_score,
                        "components": {"relevance_score": item.score.relevance_score},
                    }),
                )
            })
            .collect();
        let hits = vec![
            HybridHit {
                app_id: 999,
                score: 1.0,
                fts_rank: Some(1.0),
                vector_score: None,
            },
            HybridHit {
                app_id: 2,
                score: 0.9,
                fts_rank: Some(0.9),
                vector_score: None,
            },
        ];
        let union = build_natural_language_candidate_pool(eligible, &hits, 2);
        assert_eq!(
            union.iter().map(json_app_id).collect::<Vec<_>>(),
            vec![2, 1]
        );
    }

    #[test]
    fn natural_language_global_mmr_is_stable_and_preserves_honest_slot_reasons() {
        let item = |app_id: u32, score: f64, mode: &str, tags: &[&str]| {
            json!({
                "app_id": app_id,
                "name": format!("game-{app_id}"),
                "score": score,
                "confidence": 0.8,
                "data_confidence": 0.8,
                "recommendation_index": 80,
                "party": {"recommended_min": 1, "recommended_max": 4},
                "multiplayer": {"dominant_mode": mode},
                "components": {
                    "friend_fit": 0.8,
                    "section_score": score,
                    "personalized_score": score,
                    "relevance_score": score,
                    "final_score": score,
                },
                "reasons": [],
                "cautions": [],
                "evidence_ids": [],
                "algorithm_version": ALGORITHM_VERSION,
                "_ranking_metadata": {
                    "taxonomy_tags": tags,
                    "publisher": null,
                    "series": null,
                },
            })
        };
        let original = vec![
            item(2, 0.99, "private_coop", &["action", "coop"]),
            item(3, 0.98, "matchmade_pvp", &["strategy", "pvp"]),
            item(1, 1.0, "private_coop", &["action", "coop"]),
        ];
        let mut forward = original.clone();
        let mut reverse = original.into_iter().rev().collect();

        global_natural_language_rerank(&mut forward, 0.5, 123);
        global_natural_language_rerank(&mut reverse, 0.5, 123);

        let ids = |items: &[serde_json::Value]| items.iter().map(json_app_id).collect::<Vec<_>>();
        assert_eq!(ids(&forward), vec![1, 3, 2]);
        assert_eq!(ids(&forward), ids(&reverse));
        assert_eq!(forward[0]["slot_reason"], "base");
        assert_eq!(forward[1]["slot_reason"], "diversity");
        assert_eq!(forward[0]["rank"], 1);
        assert_eq!(forward[1]["rank"], 2);
        assert_eq!(forward[2]["rank"], 3);
    }

    #[test]
    fn recommendation_tie_seed_is_stable_per_user_day_and_rotates_context() {
        let seed = recommendation_tie_seed(Some("account-a"), "2026-08-02");
        assert_eq!(
            seed,
            recommendation_tie_seed(Some("account-a"), "2026-08-02")
        );
        assert_ne!(
            seed,
            recommendation_tie_seed(Some("account-a"), "2026-08-03")
        );
        assert_ne!(
            seed,
            recommendation_tie_seed(Some("account-b"), "2026-08-02")
        );
        assert_eq!(
            recommendation_tie_seed(None, "2026-08-02"),
            recommendation_tie_seed(Some("public"), "2026-08-02")
        );
    }

    #[test]
    fn recommended_order_is_ignored_but_scalar_sort_order_is_validated() {
        assert_eq!(
            FeedSortOrder::parse(Some("asc"), FeedSort::Recommended),
            Ok(FeedSortOrder::Desc)
        );
        assert_eq!(
            FeedSortOrder::parse(Some("not-an-order"), FeedSort::Recommended),
            Ok(FeedSortOrder::Desc)
        );
        assert_eq!(
            FeedSortOrder::parse(Some("asc"), FeedSort::Ccu),
            Ok(FeedSortOrder::Asc)
        );
        assert_eq!(
            FeedSortOrder::parse(Some("invalid"), FeedSort::Ccu),
            Err(())
        );
    }

    #[test]
    fn recommendation_and_fit_index_use_the_same_strict_score_order() {
        assert_eq!(FeedSort::parse(Some("fit_index")), Ok(FeedSort::FitIndex));
        assert_eq!(FeedSort::parse(Some("relevance")), Ok(FeedSort::FitIndex));
        assert_eq!(FeedSort::parse(Some("fit")), Ok(FeedSort::FitIndex));
        assert_eq!(
            FeedSortOrder::parse(None, FeedSort::FitIndex),
            Ok(FeedSortOrder::Desc)
        );

        let presentation = HashMap::from([
            (1, feed_presentation(0.8, 3)),
            (2, feed_presentation(0.8, 3)),
            (3, feed_presentation(0.8, 3)),
            (4, feed_presentation(0.4, 2)),
        ]);
        let original = vec![
            recall_candidate(1, 0.2),
            recall_candidate(2, 0.9),
            recall_candidate(3, 0.5),
            recall_candidate(4, 0.99),
        ];
        let mut descending = original.clone();
        apply_feed_sort(
            &mut descending,
            FeedSort::FitIndex,
            FeedSortOrder::Desc,
            &presentation,
        );
        assert_eq!(
            descending
                .iter()
                .map(|item| item.app_id)
                .collect::<Vec<_>>(),
            vec![2, 3, 1, 4]
        );

        let mut recommended = original.clone();
        apply_feed_sort(
            &mut recommended,
            FeedSort::Recommended,
            FeedSortOrder::Desc,
            &presentation,
        );
        assert_eq!(
            recommended
                .iter()
                .map(|item| item.app_id)
                .collect::<Vec<_>>(),
            vec![2, 3, 1, 4]
        );

        let mut ascending = original;
        apply_feed_sort(
            &mut ascending,
            FeedSort::FitIndex,
            FeedSortOrder::Asc,
            &presentation,
        );
        assert_eq!(
            ascending.iter().map(|item| item.app_id).collect::<Vec<_>>(),
            vec![1, 3, 2, 4]
        );
    }

    #[test]
    fn optional_feed_sort_values_are_missing_last_in_both_directions() {
        use std::cmp::Ordering::{Greater, Less};

        assert_eq!(
            compare_optional_missing_last(None, Some(1_u32), FeedSortOrder::Asc),
            Greater
        );
        assert_eq!(
            compare_optional_missing_last(None, Some(1_u32), FeedSortOrder::Desc),
            Greater
        );
        assert_eq!(
            compare_optional_missing_last(Some(1_u32), Some(2), FeedSortOrder::Asc),
            Less
        );
        assert_eq!(
            compare_optional_missing_last(Some(1_u32), Some(2), FeedSortOrder::Desc),
            Greater
        );
    }

    #[test]
    fn context_percentile_uses_midrank_for_exact_ties() {
        let mut scores = vec![(1, 0.9), (2, 0.9)];
        scores.extend((3..=10).map(|app_id| (app_id, 1.0 - f64::from(app_id) / 10.0)));
        let indices = context_percentile_indices(scores);

        assert_eq!(indices.get(&1), indices.get(&2));
        assert_eq!(indices.get(&1), Some(&90));
        assert!(indices[&1] > indices[&10]);
        assert!(context_percentile_indices((1..10).map(|id| (id, 1.0))).is_empty());
    }

    #[test]
    fn recommendation_index_uses_the_returned_request_window() {
        let full_section = (1_u32..=524)
            .map(|app_id| (app_id, 1.0 - f64::from(app_id) / 1_000.0))
            .collect::<Vec<_>>();

        let indices = context_percentile_indices_for_window(full_section, 0, 20);

        assert_eq!(indices.len(), 20);
        assert_eq!(indices.values().copied().collect::<HashSet<_>>().len(), 20);
        assert_eq!(indices.get(&1), Some(&98));
        assert_eq!(indices.get(&20), Some(&3));
        assert!(!indices.contains_key(&21));
    }

    #[test]
    fn recommendation_index_is_hidden_when_evidence_is_insufficient() {
        let indices = context_percentile_indices((1..=10).map(|id| (id, f64::from(11 - id))));
        let presentation = |data_confidence, effective_feature_count| FeedPresentation {
            release_date: None,
            release_date_raw: None,
            release_date_precision: None,
            cover_url: None,
            cover_updated_at_ms: None,
            total_reviews: None,
            total_positive: None,
            latest_ccu: None,
            typical_ccu_7d: None,
            data_confidence,
            effective_feature_count,
            profile_observed_at_ms: None,
            reviews_observed_at_ms: None,
            activity_observed_at_ms: None,
            price_observed_at_ms: None,
            release_observed_at_ms: None,
        };

        assert_eq!(
            visible_recommendation_index(1, &indices, Some(&presentation(0.8, 3))),
            Some(95)
        );
        assert_eq!(
            visible_recommendation_index(1, &indices, Some(&presentation(0.44, 3))),
            None
        );
        assert_eq!(
            visible_recommendation_index(1, &indices, Some(&presentation(0.8, 2))),
            None
        );
        assert_eq!(fit_band(None), "insufficient_data");
    }

    #[test]
    fn ai_rank_is_a_bounded_score_adjustment_not_array_order() {
        let mut items = vec![
            json!({
                "app_id": 1,
                "score": 0.9,
                "confidence": 0.8,
                "data_confidence": 0.8,
                "recommendation_index": 90,
                "components": {"personalized_score": 0.1, "final_score": 0.9}
            }),
            json!({
                "app_id": 2,
                "score": 0.8,
                "confidence": 0.8,
                "data_confidence": 0.8,
                "recommendation_index": 80,
                "evidence_ids": [],
                "reason_evidence": [],
                "components": {"personalized_score": 0.8, "final_score": 0.8}
            }),
            json!({
                "app_id": 3,
                "score": 0.79,
                "confidence": 0.8,
                "data_confidence": 0.8,
                "recommendation_index": 70,
                "components": {"personalized_score": 0.79, "final_score": 0.79}
            }),
        ];
        let validated = mpgs_ai::AiRankResult {
            // This order deliberately disagrees with the bounded blended scores.
            recommendations: vec![
                mpgs_ai::AiRankItem {
                    app_id: 1,
                    fit_score: 0.0,
                    confidence: 1.0,
                    reason_evidence_ids: Vec::new(),
                    reasons: Vec::new(),
                    cautions: Vec::new(),
                },
                mpgs_ai::AiRankItem {
                    app_id: 2,
                    fit_score: 1.0,
                    confidence: 1.0,
                    reason_evidence_ids: vec!["feature:test:2".into()],
                    reasons: vec!["evidence-backed reason".into()],
                    cautions: Vec::new(),
                },
                // A forged/restored id is ignored even if this helper is called
                // without the normal upstream candidate validation.
                mpgs_ai::AiRankItem {
                    app_id: 999,
                    fit_score: 1.0,
                    confidence: 1.0,
                    reason_evidence_ids: Vec::new(),
                    reasons: Vec::new(),
                    cautions: Vec::new(),
                },
            ],
            summary: String::new(),
            summary_evidence_ids: Vec::new(),
        };

        apply_validated_ai_rank(&mut items, &validated);

        let ids: Vec<_> = items
            .iter()
            .filter_map(|item| item.get("app_id").and_then(|value| value.as_u64()))
            .collect();
        assert_eq!(ids, vec![2, 3, 1]);
        assert_eq!(items.len(), 3);
        assert!((items[2]["score"].as_f64().unwrap() - 0.765).abs() < 1e-12);
        assert!((items[0]["score"].as_f64().unwrap() - 0.83).abs() < 1e-12);
        assert_eq!(items[0]["rank"], 1);
        assert_eq!(items[0]["evidence_ids"], json!(["feature:test:2"]));
        assert_eq!(items[0]["reason_evidence"], json!(["feature:test:2"]));
        assert_eq!(items[1]["rank"], 2);
        assert_eq!(items[2]["rank"], 3);
    }

    #[test]
    fn played_feedback_demotes_discovery_results() {
        assert!(feedback_personal_adjustment(Some("played"), 0.0) < 0.0);
        assert!(feedback_personal_adjustment(Some("like"), 0.0) > 0.0);
        assert!(
            (combined_feedback_personal_adjustment(&["played", "like"], 0.0) - 0.1).abs() < 1e-12
        );
    }

    #[test]
    fn custom_ai_request_must_match_saved_host_and_port() {
        let saved = mpgs_ai::CustomBaseUrlResolution {
            base_url: "https://ai.example.test/v1".to_owned(),
            host: "ai.example.test".to_owned(),
            address: "203.0.113.10:443".parse().unwrap(),
        };
        let same_host_new_address = mpgs_ai::CustomBaseUrlResolution {
            base_url: "https://AI.EXAMPLE.TEST/other".to_owned(),
            host: "AI.EXAMPLE.TEST".to_owned(),
            address: "203.0.113.11:443".parse().unwrap(),
        };
        let other_host = mpgs_ai::CustomBaseUrlResolution {
            base_url: "https://other.example.test/v1".to_owned(),
            host: "other.example.test".to_owned(),
            address: "203.0.113.10:443".parse().unwrap(),
        };
        let other_port = mpgs_ai::CustomBaseUrlResolution {
            address: "203.0.113.10:8443".parse().unwrap(),
            ..saved.clone()
        };
        assert!(custom_ai_hosts_match(&same_host_new_address, &saved));
        assert!(!custom_ai_hosts_match(&other_host, &saved));
        assert!(!custom_ai_hosts_match(&other_port, &saved));
    }

    #[test]
    fn ai_cache_identity_changes_with_the_model_chain() {
        let gateway = AiGateway::new(
            Arc::new(mpgs_ai::FakeProvider::default()),
            AiPolicy::default(),
        );
        let first = test_router_with_rank_models("model-a", &["fallback-a"]);
        let second = test_router_with_rank_models("model-b", &["fallback-a"]);
        let third = test_router_with_rank_models("model-a", &["fallback-b"]);
        let first_identity =
            ai_route_cache_identity(&gateway, Some(&first), AiTaskType::RankExplain);
        assert_ne!(
            first_identity,
            ai_route_cache_identity(&gateway, Some(&second), AiTaskType::RankExplain)
        );
        assert_ne!(
            first_identity,
            ai_route_cache_identity(&gateway, Some(&third), AiTaskType::RankExplain)
        );
    }

    #[tokio::test]
    async fn anonymous_requests_never_resolve_the_builtin_ai_provider() {
        let gateway = AiGateway::new(
            Arc::new(mpgs_ai::FakeProvider::default()),
            AiPolicy::default(),
        );
        assert!(gateway.is_available());
        let state = api_test_state(gateway);
        let resolved = account_ai_gateway(&state, None).await;
        assert!(!resolved.gateway.is_available());
        assert!(resolved.admission.is_none());
        assert!(resolved_task_router(&resolved, &state).is_none());
    }

    #[tokio::test]
    async fn each_builtin_upstream_call_consumes_one_daily_quota_unit() {
        let database = mpgs_storage::Database::open_in_memory().unwrap();
        let repo = Repository::new(database);
        repo.migrate().unwrap();
        repo.ensure_runtime_defaults().unwrap();
        let session = repo
            .register_account(
                &RegisterAccount {
                    username: "api_quota_test".to_owned(),
                    display_name: "API quota test".to_owned(),
                    password: "long-enough-test-password".to_owned(),
                    device_label: "test".to_owned(),
                },
                None,
            )
            .unwrap();
        let admission = AiCallAdmission {
            user_id: session.user_id.clone(),
            limiter: AccountAiLimiter::default(),
            charge_daily_quota: true,
        };
        let first = acquire_ai_call(Some(&repo), &admission).await.unwrap();
        drop(first);
        let second = acquire_ai_call(Some(&repo), &admission).await.unwrap();
        drop(second);
        let day_utc = chrono_like_now_ms() / 86_400_000;
        assert_eq!(
            repo.account_ai_daily_usage(&session.user_id, day_utc)
                .unwrap(),
            2
        );
    }

    #[tokio::test]
    async fn json_body_limit_is_smaller_than_the_avatar_upload_limit() {
        let app = build_router(api_test_state(AiGateway::disabled()));
        let oversized_json = format!(r#"{{"query":"{}"}}"#, "x".repeat(70 * 1024));
        let json_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/ai/search")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(oversized_json))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(json_response.status(), StatusCode::PAYLOAD_TOO_LARGE);

        let avatar_response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/v1/me/avatar")
                    .header(header::CONTENT_TYPE, "application/octet-stream")
                    .body(Body::from(vec![0_u8; 70 * 1024]))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(avatar_response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn ai_degradation_keeps_safe_scores_and_sanitizes_unverified_evidence() {
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
        assert!(result.recommendations[0].reason_evidence_ids.is_empty());
        assert!(result.summary.is_empty());
        assert!(result.summary_evidence_ids.is_empty());
        assert!(notice.is_some());

        let unknown = json!({
            "recommendations": [{"app_id": 999, "fit_score": 0.8, "confidence": 0.7}],
            "summary": "",
            "summary_evidence_ids": []
        });
        assert!(validate_rank_result_with_safe_degradation(&unknown, &candidates, 1).is_err());
    }
}
