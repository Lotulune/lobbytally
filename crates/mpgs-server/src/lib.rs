pub mod admin;
pub mod config;
pub mod config_files;
pub mod cors;
pub mod db;
pub mod health;
pub mod public_catalog;
pub mod rate_limit;
pub mod restart;
pub mod setup;

pub use admin::AdminAuthConfig;
use admin::{
    AdminAuditEventSummary, AdminAuditEventsResponse, AdminDiagnosticsResponse,
    AdminOverviewResponse, AdminSessionRequest, AdminSessionResponse,
};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
pub use config::{ConfigError, ConfigHealth, ServerConfig, StartupConfig};
use config_files::{
    ConfigDeploymentDiagnostics, ConfigFileManager, ConfigStateResponse, PendingConfigResponse,
    PendingServiceIdentityRequest, ServiceConnectionFileResponse,
};
pub use cors::PublicCorsConfig;
use health::{HealthResponse, HealthStatus};
use mpgs_core::models::{PublicCatalogStatus, ServiceCapability, ServiceInfo};
use public_catalog::{
    DiscoveryHomeResponse, GamesQuery, PublicGameAnalysis, PublicGameDetail, PublicGamesPage,
    ServiceErrorEnvelope,
};
use rate_limit::RateLimitBucket;
pub use rate_limit::{RateLimitConfig, RateLimiters};
pub use restart::RestartCoordinator;
use restart::{RestartRequest, RestartResponse};
use setup::{SetupAccess, SetupCompleteRequest, SetupCompleteResponse, SetupStatusResponse};
use sqlx_postgres::PgPool;
use std::{
    path::{Component, Path as FsPath, PathBuf},
    sync::{Arc, Mutex},
};
use utoipa::OpenApi;

#[derive(Debug, Clone)]
pub struct ServiceInfoConfig {
    pub service_instance_id: String,
    pub service_name: String,
    pub service_version: String,
}

impl ServiceInfoConfig {
    pub fn from_env() -> Self {
        Self {
            service_instance_id: std::env::var("MPGS_SERVICE_INSTANCE_ID")
                .unwrap_or_else(|_| "018fb770-8998-7699-a6e4-b7b59f2f9c01".to_string()),
            service_name: std::env::var("MPGS_SERVICE_NAME")
                .unwrap_or_else(|_| "MPGS Public Discovery Service".to_string()),
            service_version: std::env::var("MPGS_SERVICE_VERSION")
                .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string()),
        }
    }

    pub fn service_info(&self) -> ServiceInfo {
        self.service_info_with_catalog_status(PublicCatalogStatus::Empty)
    }

    pub fn service_info_with_catalog_status(
        &self,
        public_catalog_status: PublicCatalogStatus,
    ) -> ServiceInfo {
        ServiceInfo {
            service_instance_id: self.service_instance_id.clone(),
            service_name: self.service_name.clone(),
            service_version: self.service_version.clone(),
            api_version: "v1".to_string(),
            public_catalog_status,
            capabilities: vec![ServiceCapability::PublicCatalogRead],
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    service_info: ServiceInfo,
    database_health: DatabaseHealth,
    config_health: ConfigHealth,
    admin_auth: Option<AdminAuthConfig>,
    setup_access: Option<SetupAccess>,
    config_file_manager: Option<ConfigFileManager>,
    restart: RestartCoordinator,
    audit: AuditSink,
    rate_limits: RateLimiters,
    public_cors: PublicCorsConfig,
    admin_static_dir: Option<PathBuf>,
}

impl AppState {
    pub fn new(service_info: ServiceInfo, database_health: DatabaseHealth) -> Self {
        Self {
            service_info,
            database_health,
            config_health: ConfigHealth::HealthyForTest,
            admin_auth: None,
            setup_access: None,
            config_file_manager: None,
            restart: RestartCoordinator::process_exit(),
            audit: AuditSink::Noop,
            rate_limits: RateLimiters::default(),
            public_cors: PublicCorsConfig::default(),
            admin_static_dir: None,
        }
    }

    pub fn new_with_config_health(
        service_info: ServiceInfo,
        database_health: DatabaseHealth,
        config_health: ConfigHealth,
    ) -> Self {
        Self {
            service_info,
            database_health,
            config_health,
            admin_auth: None,
            setup_access: None,
            config_file_manager: None,
            restart: RestartCoordinator::process_exit(),
            audit: AuditSink::Noop,
            rate_limits: RateLimiters::default(),
            public_cors: PublicCorsConfig::default(),
            admin_static_dir: None,
        }
    }

    pub fn new_with_admin_auth(
        service_info: ServiceInfo,
        database_health: DatabaseHealth,
        admin_auth: AdminAuthConfig,
    ) -> Self {
        Self {
            service_info,
            database_health,
            config_health: ConfigHealth::HealthyForTest,
            admin_auth: Some(admin_auth),
            setup_access: None,
            config_file_manager: None,
            restart: RestartCoordinator::process_exit(),
            audit: AuditSink::Noop,
            rate_limits: RateLimiters::default(),
            public_cors: PublicCorsConfig::default(),
            admin_static_dir: None,
        }
    }

    pub fn with_admin_auth(mut self, admin_auth: AdminAuthConfig) -> Self {
        self.admin_auth = Some(admin_auth);
        self
    }

    pub fn with_config_file_manager(mut self, config_dir: impl Into<std::path::PathBuf>) -> Self {
        self.config_file_manager = Some(ConfigFileManager::new(config_dir));
        self
    }

    pub fn with_config_manager(mut self, config_file_manager: ConfigFileManager) -> Self {
        self.config_file_manager = Some(config_file_manager);
        self
    }

    pub fn with_restart_coordinator(mut self, restart: RestartCoordinator) -> Self {
        self.restart = restart;
        self
    }

    pub fn with_audit_sink(mut self, audit: AuditSink) -> Self {
        self.audit = audit;
        self
    }

    pub fn with_rate_limits(mut self, rate_limits: RateLimiters) -> Self {
        self.rate_limits = rate_limits;
        self
    }

    pub fn with_public_cors(mut self, public_cors: PublicCorsConfig) -> Self {
        self.public_cors = public_cors;
        self
    }

    pub fn with_admin_static_dir(mut self, admin_static_dir: impl Into<PathBuf>) -> Self {
        self.admin_static_dir = Some(admin_static_dir.into());
        self
    }

    pub fn with_setup_access(
        mut self,
        config_dir: impl Into<std::path::PathBuf>,
        setup_token: &str,
    ) -> Self {
        self.setup_access = Some(SetupAccess::for_test_token(config_dir, setup_token));
        self
    }

    pub fn with_setup_config(mut self, setup_access: SetupAccess) -> Self {
        self.setup_access = Some(setup_access);
        self
    }

    pub fn safe_mode(config: ServiceInfoConfig) -> Self {
        Self {
            service_info: config.service_info_with_catalog_status(PublicCatalogStatus::Unavailable),
            database_health: DatabaseHealth::SafeMode,
            config_health: ConfigHealth::HealthyForTest,
            admin_auth: None,
            setup_access: None,
            config_file_manager: None,
            restart: RestartCoordinator::process_exit(),
            audit: AuditSink::Noop,
            rate_limits: RateLimiters::default(),
            public_cors: PublicCorsConfig::default(),
            admin_static_dir: None,
        }
    }
}

#[derive(Clone)]
pub enum AuditSink {
    Noop,
    Pool(PgPool),
    #[doc(hidden)]
    Memory(Arc<Mutex<Vec<AuditRecord>>>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRecord {
    pub event_type: String,
    pub actor: String,
    pub outcome: String,
}

impl AuditSink {
    #[doc(hidden)]
    pub fn memory() -> Self {
        Self::Memory(Arc::new(Mutex::new(Vec::new())))
    }

    #[doc(hidden)]
    pub fn records_for_test(&self) -> Vec<AuditRecord> {
        match self {
            Self::Memory(records) => records
                .lock()
                .map(|records| records.clone())
                .unwrap_or_default(),
            Self::Noop | Self::Pool(_) => Vec::new(),
        }
    }

    #[doc(hidden)]
    pub fn record_for_test(&self, event_type: &str, actor: &str, outcome: &str) {
        if let Self::Memory(records) = self {
            if let Ok(mut records) = records.lock() {
                records.push(AuditRecord {
                    event_type: event_type.to_string(),
                    actor: actor.to_string(),
                    outcome: outcome.to_string(),
                });
            }
        }
    }

    async fn record(&self, event_type: &str, actor: &str, outcome: &str) {
        match self {
            Self::Noop => {}
            Self::Pool(pool) => {
                let _ = db::record_audit_event(pool, event_type, actor, outcome).await;
            }
            Self::Memory(records) => {
                if let Ok(mut records) = records.lock() {
                    records.push(AuditRecord {
                        event_type: event_type.to_string(),
                        actor: actor.to_string(),
                        outcome: outcome.to_string(),
                    });
                }
            }
        }
    }

    async fn latest(&self) -> Option<AuditRecord> {
        match self {
            Self::Noop => None,
            Self::Pool(pool) => db::latest_audit_event(pool)
                .await
                .ok()
                .flatten()
                .map(|event| AuditRecord {
                    event_type: event.event_type,
                    actor: event.actor,
                    outcome: event.outcome,
                }),
            Self::Memory(records) => records
                .lock()
                .ok()
                .and_then(|records| records.last().cloned()),
        }
    }

    async fn recent(&self, limit: usize) -> Vec<AuditRecord> {
        match self {
            Self::Noop => Vec::new(),
            Self::Pool(pool) => db::recent_audit_events(pool, limit as i64)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|event| AuditRecord {
                    event_type: event.event_type,
                    actor: event.actor,
                    outcome: event.outcome,
                })
                .collect(),
            Self::Memory(records) => records
                .lock()
                .map(|records| records.iter().rev().take(limit).cloned().collect())
                .unwrap_or_default(),
        }
    }
}

#[derive(Clone)]
pub enum DatabaseHealth {
    Pool(PgPool),
    #[doc(hidden)]
    HealthyForTest,
    #[doc(hidden)]
    PublicCatalogFixture {
        revision: i64,
        detail: Option<PublicGameDetail>,
        analysis: Option<PublicGameAnalysis>,
    },
    SafeMode,
    Unavailable,
}

impl DatabaseHealth {
    async fn is_healthy(&self) -> bool {
        match self {
            Self::Pool(pool) => db::migration_health_check(pool).await.unwrap_or(false),
            Self::HealthyForTest | Self::PublicCatalogFixture { .. } | Self::SafeMode => true,
            Self::Unavailable => false,
        }
    }

    fn is_safe_mode(&self) -> bool {
        matches!(self, Self::SafeMode)
    }

    async fn service_config_state(&self) -> Option<db::ServiceConfigState> {
        match self {
            Self::Pool(pool) => db::service_config_state(pool).await.ok(),
            _ => None,
        }
    }

    async fn admin_overview_stats(&self) -> db::AdminOverviewStats {
        match self {
            Self::Pool(pool) => db::admin_overview_stats(pool).await.unwrap_or_default(),
            Self::PublicCatalogFixture { detail, .. } => db::AdminOverviewStats {
                public_game_count: detail.iter().count() as i64,
                pending_review_count: 0,
            },
            Self::HealthyForTest | Self::SafeMode | Self::Unavailable => {
                db::AdminOverviewStats::default()
            }
        }
    }

    async fn mark_pending_config(&self, pending_config_version: &str) {
        if let Self::Pool(pool) = self {
            let _ = db::mark_pending_config(pool, pending_config_version).await;
        }
    }
}

pub fn build_router(config: ServiceInfoConfig) -> Router {
    build_router_with_state(AppState::new(
        config.service_info(),
        DatabaseHealth::HealthyForTest,
    ))
}

pub fn build_router_with_state(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route(
            "/api/v1/service-info",
            get(service_info).options(public_preflight),
        )
        .route(
            "/api/v1/discovery-home",
            get(discovery_home).options(public_preflight),
        )
        .route("/api/v1/games", get(games).options(public_preflight))
        .route(
            "/api/v1/games/{appid}",
            get(game_detail).options(public_preflight),
        )
        .route(
            "/api/v1/games/{appid}/analysis",
            get(game_analysis).options(public_preflight),
        )
        .route("/api/v1/admin/session", post(admin_session))
        .route("/api/v1/admin/overview", get(admin_overview))
        .route("/api/v1/admin/diagnostics", get(admin_diagnostics))
        .route("/api/v1/admin/audit-events", get(admin_audit_events))
        .route("/api/v1/admin/config-state", get(admin_config_state))
        .route(
            "/api/v1/admin/connection-share",
            get(admin_connection_share),
        )
        .route(
            "/api/v1/admin/config/pending/service-identity",
            post(admin_pending_service_identity),
        )
        .route("/api/v1/admin/restart", post(admin_restart))
        .route("/api/v1/setup/status", get(setup_status))
        .route("/api/v1/setup/complete", post(setup_complete))
        .route("/openapi.json", get(openapi_json))
        .route("/admin", get(admin_index))
        .route("/admin/", get(admin_index))
        .route("/admin/{*path}", get(admin_deep_link))
        .route("/assets/{*asset_path}", get(static_asset))
        .with_state(state)
}

#[utoipa::path(
    get,
    path = "/api/v1/service-info",
    tag = "public",
    responses(
        (status = 200, description = "Public MPGS service identity information", body = ServiceInfo)
    )
)]
async fn service_info(State(state): State<AppState>) -> Response {
    public_response(&state, Json(state.service_info.clone()).into_response())
}

async fn public_preflight(State(state): State<AppState>) -> Response {
    state.public_cors.preflight_response()
}

#[utoipa::path(
    get,
    path = "/api/v1/discovery-home",
    tag = "public",
    params(("If-None-Match" = Option<String>, Header, description = "Return 304 when the public discovery home ETag still matches")),
    responses(
        (status = 200, description = "Public catalog discovery home summary", body = DiscoveryHomeResponse),
        (status = 304, description = "Public catalog discovery home has not changed"),
        (status = 429, description = "Request rate limited", body = ServiceErrorEnvelope),
        (status = 503, description = "Public catalog unavailable", body = ServiceErrorEnvelope)
    )
)]
async fn discovery_home(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !state.rate_limits.allow(RateLimitBucket::PublicRead) {
        return public_response(&state, rate_limited_response());
    }

    let etag = match state
        .database_health
        .public_catalog_list_etag("discovery-home", None)
        .await
    {
        Ok(etag) => etag,
        Err(error) => {
            if state.database_health.is_safe_mode() {
                return public_response(&state, safe_mode_error_response());
            }
            return public_response(
                &state,
                service_error_response(
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    "public_catalog_unavailable",
                    public_catalog_error_message(error),
                ),
            );
        }
    };

    if request_matches_etag(&headers, &etag) {
        return public_response(&state, not_modified_response(etag));
    }

    match state.database_health.discovery_home().await {
        Ok(payload) => public_response(&state, json_with_etag(payload, etag)),
        Err(error) if state.database_health.is_safe_mode() => {
            public_response(&state, safe_mode_error_response())
        }
        Err(error) => public_response(
            &state,
            service_error_response(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "public_catalog_unavailable",
                public_catalog_error_message(error),
            ),
        ),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/games",
    tag = "public",
    params(GamesQuery, ("If-None-Match" = Option<String>, Header, description = "Return 304 when the paginated public games ETag still matches")),
    responses(
        (status = 200, description = "Paginated public games", body = PublicGamesPage),
        (status = 304, description = "Paginated public games have not changed"),
        (status = 429, description = "Request rate limited", body = ServiceErrorEnvelope),
        (status = 503, description = "Public catalog unavailable", body = ServiceErrorEnvelope)
    )
)]
async fn games(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<GamesQuery>,
) -> Response {
    if !state.rate_limits.allow(RateLimitBucket::PublicRead) {
        return public_response(&state, rate_limited_response());
    }

    let (limit, offset) = query.normalized();
    let etag = match state
        .database_health
        .public_catalog_list_etag("games", Some((limit, offset)))
        .await
    {
        Ok(etag) => etag,
        Err(error) => {
            if state.database_health.is_safe_mode() {
                return public_response(&state, safe_mode_error_response());
            }
            return public_response(
                &state,
                service_error_response(
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    "public_catalog_unavailable",
                    public_catalog_error_message(error),
                ),
            );
        }
    };

    if request_matches_etag(&headers, &etag) {
        return public_response(&state, not_modified_response(etag));
    }

    match state.database_health.public_games_page(limit, offset).await {
        Ok(payload) => public_response(&state, json_with_etag(payload, etag)),
        Err(error) if state.database_health.is_safe_mode() => {
            public_response(&state, safe_mode_error_response())
        }
        Err(error) => public_response(
            &state,
            service_error_response(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "public_catalog_unavailable",
                public_catalog_error_message(error),
            ),
        ),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/games/{appid}",
    tag = "public",
    params(
        ("appid" = u32, Path, description = "Steam AppID"),
        ("If-None-Match" = Option<String>, Header, description = "Return 304 when the public game detail ETag still matches")
    ),
    responses(
        (status = 200, description = "Public game detail", body = PublicGameDetail),
        (status = 304, description = "Public game detail has not changed"),
        (status = 404, description = "Public game not found", body = ServiceErrorEnvelope),
        (status = 429, description = "Request rate limited", body = ServiceErrorEnvelope),
        (status = 503, description = "Public catalog unavailable", body = ServiceErrorEnvelope)
    )
)]
async fn game_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(appid): Path<u32>,
) -> Response {
    if !state.rate_limits.allow(RateLimitBucket::PublicRead) {
        return public_response(&state, rate_limited_response());
    }

    match state.database_health.public_game_detail(appid).await {
        Ok(Some(payload)) => {
            let etag = public_game_detail_etag(&payload);
            if request_matches_etag(&headers, &etag) {
                return public_response(&state, not_modified_response(etag));
            }
            public_response(&state, json_with_etag(payload, etag))
        }
        Ok(None) => public_response(
            &state,
            service_error_response(
                axum::http::StatusCode::NOT_FOUND,
                "public_game_not_found",
                "公开游戏不存在。",
            ),
        ),
        Err(error) if state.database_health.is_safe_mode() => {
            public_response(&state, safe_mode_error_response())
        }
        Err(error) => public_response(
            &state,
            service_error_response(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "public_catalog_unavailable",
                public_catalog_error_message(error),
            ),
        ),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/games/{appid}/analysis",
    tag = "public",
    params(
        ("appid" = u32, Path, description = "Steam AppID"),
        ("If-None-Match" = Option<String>, Header, description = "Return 304 when the public game analysis ETag still matches")
    ),
    responses(
        (status = 200, description = "Public game analysis", body = PublicGameAnalysis),
        (status = 304, description = "Public game analysis has not changed"),
        (status = 404, description = "Public game analysis not found", body = ServiceErrorEnvelope),
        (status = 429, description = "Request rate limited", body = ServiceErrorEnvelope),
        (status = 503, description = "Public catalog unavailable", body = ServiceErrorEnvelope)
    )
)]
async fn game_analysis(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(appid): Path<u32>,
) -> Response {
    if !state.rate_limits.allow(RateLimitBucket::PublicRead) {
        return public_response(&state, rate_limited_response());
    }

    match state.database_health.public_game_analysis(appid).await {
        Ok(Some(payload)) => {
            let etag = public_game_analysis_etag(&payload);
            if request_matches_etag(&headers, &etag) {
                return public_response(&state, not_modified_response(etag));
            }
            public_response(&state, json_with_etag(payload, etag))
        }
        Ok(None) => public_response(
            &state,
            service_error_response(
                axum::http::StatusCode::NOT_FOUND,
                "public_game_analysis_not_found",
                "公开游戏分析不存在。",
            ),
        ),
        Err(error) if state.database_health.is_safe_mode() => {
            public_response(&state, safe_mode_error_response())
        }
        Err(error) => public_response(
            &state,
            service_error_response(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "public_catalog_unavailable",
                public_catalog_error_message(error),
            ),
        ),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/session",
    tag = "admin",
    request_body = AdminSessionRequest,
    responses(
        (status = 200, description = "Admin session established", body = AdminSessionResponse),
        (status = 429, description = "Request rate limited", body = ServiceErrorEnvelope),
        (status = 401, description = "Invalid admin token", body = ServiceErrorEnvelope)
    )
)]
async fn admin_session(
    State(state): State<AppState>,
    Json(request): Json<AdminSessionRequest>,
) -> Response {
    if !state.rate_limits.allow(RateLimitBucket::Admin) {
        return rate_limited_response();
    }

    let Some(admin_auth) = state.admin_auth.as_ref() else {
        return service_error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "admin_auth_unconfigured",
            "管理访问尚未配置。",
        );
    };

    if !admin_auth.verify_token(&request.token) {
        state
            .audit
            .record("admin.session.login", "admin", "failure")
            .await;
        return service_error_response(
            StatusCode::UNAUTHORIZED,
            "admin_token_invalid",
            "管理员令牌无效。",
        );
    }

    let mut response = Json(AdminSessionResponse {
        authenticated: true,
    })
    .into_response();
    if let Ok(cookie) = HeaderValue::from_str(&admin_auth.session_cookie()) {
        response.headers_mut().insert(header::SET_COOKIE, cookie);
    }
    state
        .audit
        .record("admin.session.login", "admin", "success")
        .await;
    response
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/overview",
    tag = "admin",
    responses(
        (status = 200, description = "Admin overview", body = AdminOverviewResponse),
        (status = 429, description = "Request rate limited", body = ServiceErrorEnvelope),
        (status = 401, description = "Admin session required", body = ServiceErrorEnvelope)
    )
)]
async fn admin_overview(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !state.rate_limits.allow(RateLimitBucket::Admin) {
        return rate_limited_response();
    }

    if !admin_session_is_valid(&state, &headers) {
        return admin_session_required_response();
    }

    let stats = state.database_health.admin_overview_stats().await;
    let restart_required = state
        .database_health
        .service_config_state()
        .await
        .map(|state| state.restart_required)
        .unwrap_or_else(|| {
            state
                .config_file_manager
                .as_ref()
                .and_then(|manager| manager.state().ok())
                .map(|state| state.restart_required)
                .unwrap_or(false)
        });
    let connection_share_configured = state
        .config_file_manager
        .as_ref()
        .map(|manager| manager.service_connection_file().is_ok())
        .unwrap_or(false);
    let latest_audit_event = state
        .audit
        .latest()
        .await
        .map(|event| AdminAuditEventSummary {
            event_type: event.event_type,
            actor: event.actor,
            outcome: event.outcome,
        });

    Json(AdminOverviewResponse {
        service_name: state.service_info.service_name,
        public_catalog_status: state.service_info.public_catalog_status,
        public_game_count: stats.public_game_count,
        pending_review_count: stats.pending_review_count,
        restart_required,
        connection_share_configured,
        latest_audit_event,
    })
    .into_response()
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/audit-events",
    tag = "admin",
    responses(
        (status = 200, description = "Recent admin audit events", body = AdminAuditEventsResponse),
        (status = 429, description = "Request rate limited", body = ServiceErrorEnvelope),
        (status = 401, description = "Admin session required", body = ServiceErrorEnvelope)
    )
)]
async fn admin_audit_events(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !state.rate_limits.allow(RateLimitBucket::Admin) {
        return rate_limited_response();
    }

    if !admin_session_is_valid(&state, &headers) {
        return admin_session_required_response();
    }

    let events = state
        .audit
        .recent(20)
        .await
        .into_iter()
        .map(|event| AdminAuditEventSummary {
            event_type: event.event_type,
            actor: event.actor,
            outcome: event.outcome,
        })
        .collect();

    Json(AdminAuditEventsResponse { events }).into_response()
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/diagnostics",
    tag = "admin",
    responses(
        (status = 200, description = "Admin diagnostics", body = AdminDiagnosticsResponse),
        (status = 429, description = "Request rate limited", body = ServiceErrorEnvelope),
        (status = 401, description = "Admin session required", body = ServiceErrorEnvelope)
    )
)]
async fn admin_diagnostics(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !state.rate_limits.allow(RateLimitBucket::Admin) {
        return rate_limited_response();
    }

    if !admin_session_is_valid(&state, &headers) {
        return admin_session_required_response();
    }

    let deployment = state
        .config_file_manager
        .as_ref()
        .map(ConfigFileManager::deployment_diagnostics)
        .unwrap_or_else(ConfigDeploymentDiagnostics::default);

    Json(AdminDiagnosticsResponse {
        postgres: if state.database_health.is_healthy().await {
            "ok".to_string()
        } else {
            "unavailable".to_string()
        },
        active_config: if state.config_health.is_healthy() {
            "ok".to_string()
        } else {
            "unavailable".to_string()
        },
        safe_mode: state.database_health.is_safe_mode(),
        public_base_url: deployment.public_base_url,
        public_base_url_status: deployment.public_base_url_status,
        https_status: deployment.https_status,
        public_cors: deployment.public_cors,
        restart_policy: deployment.restart_policy,
        steam: deployment.steam,
        llm: deployment.llm,
        r2: deployment.r2,
    })
    .into_response()
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/config-state",
    tag = "admin",
    responses(
        (status = 200, description = "Server configuration state", body = ConfigStateResponse),
        (status = 401, description = "Admin session required", body = ServiceErrorEnvelope),
        (status = 429, description = "Request rate limited", body = ServiceErrorEnvelope),
        (status = 503, description = "Config manager unavailable", body = ServiceErrorEnvelope)
    )
)]
async fn admin_config_state(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !state.rate_limits.allow(RateLimitBucket::Admin) {
        return rate_limited_response();
    }

    if !admin_session_is_valid(&state, &headers) {
        return admin_session_required_response();
    }

    let Some(config_file_manager) = state.config_file_manager.as_ref() else {
        return service_error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "config_manager_unavailable",
            "配置管理器不可用。",
        );
    };

    match config_file_manager.state() {
        Ok(mut payload) => {
            if let Some(db_state) = state.database_health.service_config_state().await {
                payload.active_config_version = db_state
                    .active_config_version
                    .unwrap_or(payload.active_config_version);
                payload.pending_config_version = db_state.pending_config_version;
                payload.restart_required = db_state.restart_required;
                payload.last_startup_status = db_state.last_startup_status;
            }
            Json(payload).into_response()
        }
        Err(_) => service_error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "config_state_unavailable",
            "配置状态暂不可用。",
        ),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/connection-share",
    tag = "admin",
    responses(
        (status = 200, description = "Keyless service connection file for client import", body = ServiceConnectionFileResponse),
        (status = 401, description = "Admin session required", body = ServiceErrorEnvelope),
        (status = 429, description = "Request rate limited", body = ServiceErrorEnvelope),
        (status = 503, description = "Connection share unavailable", body = ServiceErrorEnvelope)
    )
)]
async fn admin_connection_share(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !state.rate_limits.allow(RateLimitBucket::Admin) {
        return rate_limited_response();
    }

    if !admin_session_is_valid(&state, &headers) {
        return admin_session_required_response();
    }

    let Some(config_file_manager) = state.config_file_manager.as_ref() else {
        return service_error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "config_manager_unavailable",
            "配置管理器不可用。",
        );
    };

    match config_file_manager.service_connection_file() {
        Ok(payload) => Json(payload).into_response(),
        Err(_) => service_error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "connection_share_unavailable",
            "服务连接分享暂不可用。",
        ),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/config/pending/service-identity",
    tag = "admin",
    request_body = PendingServiceIdentityRequest,
    responses(
        (status = 200, description = "Pending service identity configuration", body = PendingConfigResponse),
        (status = 401, description = "Admin session required", body = ServiceErrorEnvelope),
        (status = 429, description = "Request rate limited", body = ServiceErrorEnvelope),
        (status = 503, description = "Config manager unavailable", body = ServiceErrorEnvelope)
    )
)]
async fn admin_pending_service_identity(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PendingServiceIdentityRequest>,
) -> Response {
    if !state.rate_limits.allow(RateLimitBucket::Admin) {
        return rate_limited_response();
    }

    if !admin_session_is_valid(&state, &headers) {
        return admin_session_required_response();
    }

    let Some(config_file_manager) = state.config_file_manager.as_ref() else {
        return service_error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "config_manager_unavailable",
            "配置管理器不可用。",
        );
    };

    match config_file_manager.write_pending_service_identity(&request) {
        Ok(payload) => {
            state
                .database_health
                .mark_pending_config(&payload.pending_config_version)
                .await;
            state
                .audit
                .record("admin.config.pending_service_identity", "admin", "success")
                .await;
            Json(payload).into_response()
        }
        Err(_) => service_error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "pending_config_write_failed",
            "待生效配置写入失败。",
        ),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/restart",
    tag = "admin",
    request_body = RestartRequest,
    responses(
        (status = 202, description = "Managed restart scheduled through service self-exit", body = RestartResponse),
        (status = 400, description = "Restart confirmation required", body = ServiceErrorEnvelope),
        (status = 401, description = "Admin session required", body = ServiceErrorEnvelope),
        (status = 409, description = "Pending config required", body = ServiceErrorEnvelope),
        (status = 429, description = "Request rate limited", body = ServiceErrorEnvelope),
        (status = 503, description = "Config manager unavailable", body = ServiceErrorEnvelope)
    )
)]
async fn admin_restart(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<RestartRequest>,
) -> Response {
    if !state.rate_limits.allow(RateLimitBucket::Restart) {
        return rate_limited_response();
    }

    if !admin_session_is_valid(&state, &headers) {
        return admin_session_required_response();
    }

    if !request.confirm {
        return service_error_response(
            StatusCode::BAD_REQUEST,
            "restart_confirmation_required",
            "需要确认后才能重启服务。",
        );
    }

    let Some(config_file_manager) = state.config_file_manager.as_ref() else {
        return service_error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "config_manager_unavailable",
            "配置管理器不可用。",
        );
    };

    match config_file_manager.validate_pending_service_config() {
        Ok(true) => {
            state
                .audit
                .record("admin.restart.requested", "admin", "success")
                .await;
            state.restart.request_restart();
            (
                StatusCode::ACCEPTED,
                Json(RestartResponse {
                    restart_scheduled: true,
                    mode: "self_exit".to_string(),
                }),
            )
                .into_response()
        }
        Ok(false) => service_error_response(
            StatusCode::CONFLICT,
            "pending_config_required",
            "没有待生效配置可用于重启。",
        ),
        Err(_) => service_error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "pending_config_invalid",
            "待生效配置校验失败。",
        ),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/setup/status",
    tag = "setup",
    responses(
        (status = 200, description = "First-run setup status", body = SetupStatusResponse),
        (status = 429, description = "Request rate limited", body = ServiceErrorEnvelope)
    )
)]
async fn setup_status(State(state): State<AppState>) -> Response {
    if !state.rate_limits.allow(RateLimitBucket::Setup) {
        return rate_limited_response();
    }

    Json(SetupStatusResponse {
        configured: state
            .setup_access
            .as_ref()
            .map(SetupAccess::is_configured)
            .unwrap_or(true),
    })
    .into_response()
}

#[utoipa::path(
    post,
    path = "/api/v1/setup/complete",
    tag = "setup",
    request_body = SetupCompleteRequest,
    responses(
        (status = 200, description = "First-run setup wrote active configuration", body = SetupCompleteResponse),
        (status = 401, description = "Invalid setup token", body = ServiceErrorEnvelope),
        (status = 409, description = "Setup is already configured", body = ServiceErrorEnvelope),
        (status = 429, description = "Request rate limited", body = ServiceErrorEnvelope),
        (status = 503, description = "Setup is unavailable", body = ServiceErrorEnvelope)
    )
)]
async fn setup_complete(
    State(state): State<AppState>,
    Json(request): Json<SetupCompleteRequest>,
) -> Response {
    if !state.rate_limits.allow(RateLimitBucket::Setup) {
        return rate_limited_response();
    }

    let Some(setup_access) = state.setup_access.as_ref() else {
        return service_error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "setup_unavailable",
            "首次配置入口不可用。",
        );
    };

    if setup_access.is_configured() {
        return service_error_response(
            StatusCode::CONFLICT,
            "setup_already_configured",
            "首次配置已经完成。",
        );
    }

    if !setup_access.verify_token(&request.setup_token) {
        return service_error_response(
            StatusCode::UNAUTHORIZED,
            "setup_token_invalid",
            "引导令牌无效。",
        );
    }

    if setup_access.complete(&request).is_err() {
        return service_error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "setup_config_write_failed",
            "首次配置写入失败。",
        );
    }

    Json(SetupCompleteResponse {
        configured: true,
        restart_required: true,
    })
    .into_response()
}

#[utoipa::path(
    get,
    path = "/healthz",
    tag = "system",
    responses(
        (status = 200, description = "Public service health check", body = HealthResponse),
        (status = 503, description = "Service dependency unavailable", body = HealthResponse)
    )
)]
async fn healthz(State(state): State<AppState>) -> (axum::http::StatusCode, Json<HealthResponse>) {
    if state.database_health.is_healthy().await && state.config_health.is_healthy() {
        (
            axum::http::StatusCode::OK,
            Json(HealthResponse::new(HealthStatus::Ok)),
        )
    } else {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(HealthResponse::new(HealthStatus::Unavailable)),
        )
    }
}

impl DatabaseHealth {
    async fn public_catalog_revision(&self) -> Result<i64, sqlx_core::error::Error> {
        match self {
            Self::Pool(pool) => db::public_catalog_revision(pool).await,
            Self::HealthyForTest => Ok(0),
            Self::PublicCatalogFixture { revision, .. } => Ok(*revision),
            Self::SafeMode => Err(sqlx_core::error::Error::PoolClosed),
            Self::Unavailable => Err(sqlx_core::error::Error::PoolClosed),
        }
    }

    async fn public_catalog_list_etag(
        &self,
        endpoint: &'static str,
        pagination: Option<(u32, u32)>,
    ) -> Result<String, sqlx_core::error::Error> {
        let revision = self.public_catalog_revision().await?;
        let pagination_key = pagination
            .map(|(limit, offset)| format!(":limit={limit}:offset={offset}"))
            .unwrap_or_default();

        Ok(format!(
            "\"public-catalog:{endpoint}:rev={revision}{pagination_key}\""
        ))
    }

    async fn discovery_home(&self) -> Result<DiscoveryHomeResponse, sqlx_core::error::Error> {
        match self {
            Self::Pool(pool) => db::discovery_home(pool).await,
            Self::HealthyForTest => Ok(DiscoveryHomeResponse::empty()),
            Self::PublicCatalogFixture { detail, .. } => {
                let item = detail.iter().map(|detail| detail.game.clone()).collect();
                Ok(DiscoveryHomeResponse {
                    status: if detail.is_some() {
                        PublicCatalogStatus::Ready
                    } else {
                        PublicCatalogStatus::Empty
                    },
                    total_games: detail.iter().count() as i64,
                    sections: public_catalog::DiscoveryHomeSections {
                        newly_published: item,
                        high_confidence: Vec::new(),
                        recently_added: Vec::new(),
                    },
                })
            }
            Self::SafeMode => Err(sqlx_core::error::Error::PoolClosed),
            Self::Unavailable => Err(sqlx_core::error::Error::PoolClosed),
        }
    }

    async fn public_games_page(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<PublicGamesPage, sqlx_core::error::Error> {
        match self {
            Self::Pool(pool) => db::public_games_page(pool, limit, offset).await,
            Self::HealthyForTest => Ok(PublicGamesPage::empty(limit, offset)),
            Self::PublicCatalogFixture { detail, .. } => {
                let items = detail
                    .iter()
                    .map(|detail| detail.game.clone())
                    .skip(offset as usize)
                    .take(limit as usize)
                    .collect();
                Ok(PublicGamesPage {
                    items,
                    page: public_catalog::PageMeta {
                        limit,
                        offset,
                        total: detail.iter().count() as i64,
                    },
                })
            }
            Self::SafeMode => Err(sqlx_core::error::Error::PoolClosed),
            Self::Unavailable => Err(sqlx_core::error::Error::PoolClosed),
        }
    }

    async fn public_game_detail(
        &self,
        appid: u32,
    ) -> Result<Option<PublicGameDetail>, sqlx_core::error::Error> {
        match self {
            Self::Pool(pool) => db::public_game_detail(pool, appid).await,
            Self::HealthyForTest => Ok(None),
            Self::PublicCatalogFixture { detail, .. } => {
                Ok(detail.clone().filter(|detail| detail.game.appid == appid))
            }
            Self::SafeMode => Err(sqlx_core::error::Error::PoolClosed),
            Self::Unavailable => Err(sqlx_core::error::Error::PoolClosed),
        }
    }

    async fn public_game_analysis(
        &self,
        appid: u32,
    ) -> Result<Option<PublicGameAnalysis>, sqlx_core::error::Error> {
        match self {
            Self::Pool(pool) => db::public_game_analysis(pool, appid).await,
            Self::HealthyForTest => Ok(None),
            Self::PublicCatalogFixture { analysis, .. } => {
                Ok(analysis.clone().filter(|analysis| analysis.appid == appid))
            }
            Self::SafeMode => Err(sqlx_core::error::Error::PoolClosed),
            Self::Unavailable => Err(sqlx_core::error::Error::PoolClosed),
        }
    }
}

fn service_error_response(
    status: axum::http::StatusCode,
    code: &'static str,
    message: &'static str,
) -> Response {
    (status, Json(ServiceErrorEnvelope::new(code, message))).into_response()
}

fn safe_mode_error_response() -> Response {
    service_error_response(
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "service_safe_mode",
        "服务处于安全修复模式。",
    )
}

fn public_response(state: &AppState, mut response: Response) -> Response {
    state
        .public_cors
        .insert_public_headers(response.headers_mut());
    response
}

fn admin_session_is_valid(state: &AppState, headers: &HeaderMap) -> bool {
    state
        .admin_auth
        .as_ref()
        .map(|admin_auth| {
            admin_auth.verify_cookie_header(
                headers
                    .get(header::COOKIE)
                    .and_then(|value| value.to_str().ok()),
            )
        })
        .unwrap_or(false)
}

fn admin_session_required_response() -> Response {
    service_error_response(
        StatusCode::UNAUTHORIZED,
        "admin_session_required",
        "需要管理员会话。",
    )
}

fn rate_limited_response() -> Response {
    service_error_response(
        StatusCode::TOO_MANY_REQUESTS,
        "rate_limited",
        "请求过于频繁，请稍后再试。",
    )
}

fn json_with_etag<T>(payload: T, etag: String) -> Response
where
    T: serde::Serialize,
{
    let mut response = Json(payload).into_response();
    insert_etag(response.headers_mut(), &etag);
    response
}

fn not_modified_response(etag: String) -> Response {
    let mut response = StatusCode::NOT_MODIFIED.into_response();
    insert_etag(response.headers_mut(), &etag);
    response
}

fn insert_etag(headers: &mut HeaderMap, etag: &str) {
    if let Ok(value) = HeaderValue::from_str(etag) {
        headers.insert(header::ETAG, value);
    }
}

fn request_matches_etag(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .any(|candidate| candidate == etag)
        })
        .unwrap_or(false)
}

fn public_game_detail_etag(payload: &PublicGameDetail) -> String {
    format!(
        "\"public-catalog:game:{}:updated={}\"",
        payload.game.appid, payload.game.updated_at
    )
}

fn public_game_analysis_etag(payload: &PublicGameAnalysis) -> String {
    format!(
        "\"public-catalog:game-analysis:{}:generated={}\"",
        payload.appid, payload.generated_at
    )
}

fn public_catalog_error_message(_error: sqlx_core::error::Error) -> &'static str {
    "公共游戏库暂不可用。"
}

async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(build_openapi())
}

async fn admin_index(State(state): State<AppState>) -> Response {
    serve_admin_html(&state).await
}

async fn admin_deep_link(State(state): State<AppState>) -> Response {
    serve_admin_html(&state).await
}

async fn static_asset(State(state): State<AppState>, Path(asset_path): Path<String>) -> Response {
    let Some(root) = state.admin_static_dir.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(path) = safe_asset_path(root, &asset_path) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    serve_static_file(&path).await
}

async fn serve_admin_html(state: &AppState) -> Response {
    let Some(root) = state.admin_static_dir.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };

    serve_static_file(&root.join("admin.html")).await
}

async fn serve_static_file(path: &FsPath) -> Response {
    match tokio::fs::read(path).await {
        Ok(contents) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, content_type_for_path(path))],
            contents,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

fn safe_asset_path(root: &FsPath, asset_path: &str) -> Option<PathBuf> {
    let requested = FsPath::new(asset_path);
    if asset_path.trim().is_empty() || requested.is_absolute() {
        return None;
    }

    let mut safe_path = PathBuf::new();
    for component in requested.components() {
        match component {
            Component::Normal(part) => safe_path.push(part),
            _ => return None,
        }
    }

    if safe_path.as_os_str().is_empty() {
        return None;
    }

    Some(root.join("assets").join(safe_path))
}

fn content_type_for_path(path: &FsPath) -> HeaderValue {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("css") => HeaderValue::from_static("text/css; charset=utf-8"),
        Some("html") => HeaderValue::from_static("text/html; charset=utf-8"),
        Some("ico") => HeaderValue::from_static("image/x-icon"),
        Some("jpg") | Some("jpeg") => HeaderValue::from_static("image/jpeg"),
        Some("js") | Some("mjs") => HeaderValue::from_static("text/javascript; charset=utf-8"),
        Some("json") => HeaderValue::from_static("application/json"),
        Some("png") => HeaderValue::from_static("image/png"),
        Some("svg") => HeaderValue::from_static("image/svg+xml"),
        Some("webp") => HeaderValue::from_static("image/webp"),
        Some("woff2") => HeaderValue::from_static("font/woff2"),
        _ => HeaderValue::from_static("application/octet-stream"),
    }
}

#[derive(OpenApi)]
#[openapi(
    paths(
        service_info,
        discovery_home,
        games,
        game_detail,
        game_analysis,
        admin_session,
        admin_overview,
        admin_audit_events,
        admin_diagnostics,
        admin_config_state,
        admin_connection_share,
        admin_pending_service_identity,
        admin_restart,
        setup_status,
        setup_complete,
        healthz
    ),
    components(schemas(
        admin::AdminDiagnosticsResponse,
        admin::AdminAuditEventsResponse,
        admin::AdminAuditEventSummary,
        admin::AdminOverviewResponse,
        admin::AdminSessionRequest,
        admin::AdminSessionResponse,
        config_files::ConfigStateResponse,
        config_files::PendingConfigResponse,
        config_files::PendingServiceIdentityRequest,
        config_files::ServiceConnectionFileResponse,
        restart::RestartRequest,
        restart::RestartResponse,
        setup::SetupCompleteRequest,
        setup::SetupCompleteResponse,
        setup::SetupStatusResponse,
        health::HealthResponse,
        health::HealthStatus,
        public_catalog::DiscoveryHomeResponse,
        public_catalog::DiscoveryHomeSections,
        public_catalog::PageMeta,
        public_catalog::PublicGameAnalysis,
        public_catalog::PublicGameDetail,
        public_catalog::PublicGameListItem,
        public_catalog::PublicGamesPage,
        public_catalog::ServiceErrorBody,
        public_catalog::ServiceErrorEnvelope,
        mpgs_core::models::ServiceInfo,
        mpgs_core::models::PublicCatalogStatus,
        mpgs_core::models::ServiceCapability
    ))
)]
struct ApiDoc;

pub fn build_openapi() -> utoipa::openapi::OpenApi {
    ApiDoc::openapi()
}
