#![forbid(unsafe_code)]

mod ai_limits;
mod api;
mod cors;
mod rate_limit;
mod scheduler;

use std::{env, error::Error, io, net::SocketAddr};

use mpgs_ai::{embedding_provider_from_env, gateway_from_env, task_router_from_env};
use mpgs_recommender::ALGORITHM_VERSION;
use mpgs_storage::{Database, Repository, latest_version};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::ai_limits::AccountAiLimiter;
use crate::api::{AppState, build_router};
use crate::cors::CorsConfig;
use crate::rate_limit::RateLimitConfig;

/// Default local bind. 17880 avoids the crowded 8080 range used by many other apps.
const DEFAULT_BIND_ADDR: &str = "127.0.0.1:17880";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    if env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--build-info")
    {
        println!("{}", build_info());
        return Ok(());
    }
    init_tracing();
    let state = build_state().await?;
    scheduler::spawn(state.repo.clone());
    let address: SocketAddr = env::var("MPGS_BIND_ADDR")
        .unwrap_or_else(|_| DEFAULT_BIND_ADDR.to_owned())
        .parse()?;
    let listener = TcpListener::bind(address).await?;
    info!(%address, storage = state.repo.is_some(), "mpgs server listening");
    axum::serve(
        listener,
        build_router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}

fn build_info() -> Value {
    json!({
        "product": "mpgs-server",
        "service_version": env!("CARGO_PKG_VERSION"),
        "git_sha": api::build_git_sha(),
        "rustc_target": env!("MPGS_BUILD_TARGET"),
        "schema_version": latest_version(),
        "algorithm_version": ALGORITHM_VERSION,
    })
}

async fn build_state() -> Result<AppState, Box<dyn Error>> {
    let admin_token = env::var("MPGS_ADMIN_TOKEN").ok().filter(|s| !s.is_empty());
    let demo_seed = demo_seed_enabled()?;
    let repo = match env::var("MPGS_DATABASE_PATH") {
        Ok(path) if !path.is_empty() => {
            let db = Database::open(&path)?;
            let repo = Repository::new(db);
            let version = repo.migrate()?;
            repo.ensure_runtime_defaults()?;
            if demo_seed {
                let seeded = repo.seed_demo_if_empty()?;
                info!(seeded, "demo catalog seed enabled");
            }
            repo.assert_ready()?;
            info!(%path, version, "database ready");
            Some(repo)
        }
        _ if cfg!(debug_assertions) => {
            // Development can use a transient database, but sample content is
            // never implicit: it requires MPGS_SEED_DEMO=true and is visible in
            // service metadata. Release binaries reject this branch below.
            let db = Database::open_in_memory()?;
            let repo = Repository::new(db);
            repo.migrate()?;
            repo.ensure_runtime_defaults()?;
            if demo_seed {
                let seeded = repo.seed_demo_if_empty()?;
                info!(seeded, "development demo catalog seed enabled");
                repo.assert_ready()?;
            } else {
                info!(
                    "using empty development database; set MPGS_DATABASE_PATH or MPGS_SEED_DEMO=true"
                );
            }
            Some(repo)
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "MPGS_DATABASE_PATH is required by release builds; set MPGS_SEED_DEMO=true only for an explicit demo environment",
            )
            .into());
        }
    };
    let ai = gateway_from_env()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    let task_router = task_router_from_env()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    let embedding = embedding_provider_from_env()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    info!(
        provider = ai.provider_name(),
        available = ai.is_available(),
        "AI gateway ready"
    );
    info!(
        provider = task_router.provider_name(),
        available = task_router.is_available(),
        "AI task router ready"
    );
    if task_router.is_available() {
        match task_router.refresh_model_registry().await {
            Ok(count) => info!(
                models = count,
                "AI model registry refreshed from /v1/models"
            ),
            Err(error) => tracing::warn!(
                error = %error,
                "AI model registry refresh skipped; routes stay optimistically open"
            ),
        }
    }
    info!(
        provider = embedding.name(),
        available = embedding.is_available(),
        "embedding provider ready"
    );
    Ok(AppState {
        repo,
        admin_token,
        rate_limits: RateLimitConfig::from_env()?,
        cors: CorsConfig::from_env()?,
        account_ai_limits: AccountAiLimiter::from_env()?,
        ai,
        task_router: std::sync::Arc::new(task_router),
        embedding,
    })
}

fn demo_seed_enabled() -> Result<bool, io::Error> {
    let Ok(value) = env::var("MPGS_SEED_DEMO") else {
        return Ok(false);
    };
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" => Ok(true),
        "0" | "false" | "no" | "" => Ok(false),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "MPGS_SEED_DEMO must be true/false or 1/0",
        )),
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "failed to install Ctrl+C handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(error) => {
                tracing::error!(%error, "failed to install SIGTERM handler");
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {
            tracing::info!("shutdown signal received (ctrl_c)");
        }
        () = terminate => {
            tracing::info!("shutdown signal received (terminate)");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use tower::ServiceExt;

    fn test_embedding() -> std::sync::Arc<dyn mpgs_ai::EmbeddingProvider> {
        std::sync::Arc::new(mpgs_ai::HashEmbeddingProvider { dimensions: 64 })
    }

    fn test_task_router() -> std::sync::Arc<mpgs_ai::TaskRouter> {
        std::sync::Arc::new(mpgs_ai::TaskRouter::from_provider(std::sync::Arc::new(
            mpgs_ai::DisabledProvider,
        )))
    }

    fn test_repo_and_app(rate_limits: RateLimitConfig) -> (Repository, axum::Router) {
        let db = Database::open_in_memory().unwrap();
        let repo = Repository::new(db);
        repo.migrate().unwrap();
        repo.ensure_runtime_defaults().unwrap();
        repo.seed_demo_if_empty().unwrap();
        let app = build_router(AppState {
            repo: Some(repo.clone()),
            admin_token: Some("secret".into()),
            rate_limits,
            cors: CorsConfig::default(),
            account_ai_limits: AccountAiLimiter::default(),
            ai: mpgs_ai::AiGateway::disabled(),
            task_router: test_task_router(),
            embedding: test_embedding(),
        });
        (repo, app)
    }

    fn test_app() -> axum::Router {
        test_repo_and_app(RateLimitConfig::default()).1
    }

    fn test_app_without_repo() -> axum::Router {
        build_router(AppState {
            repo: None,
            admin_token: None,
            rate_limits: RateLimitConfig::default(),
            cors: CorsConfig::default(),
            account_ai_limits: AccountAiLimiter::default(),
            ai: mpgs_ai::AiGateway::disabled(),
            task_router: test_task_router(),
            embedding: test_embedding(),
        })
    }

    #[test]
    fn m6_build_info_matches_compiled_release_metadata() {
        let info = build_info();
        assert_eq!(info["product"], "mpgs-server");
        assert_eq!(info["service_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(info["git_sha"], crate::api::build_git_sha());
        assert_eq!(info["rustc_target"], env!("MPGS_BUILD_TARGET"));
        assert_eq!(info["schema_version"], latest_version());
        assert_eq!(info["algorithm_version"], ALGORITHM_VERSION);
    }

    #[tokio::test]
    async fn cors_preflight_allowed_origin() {
        let app = test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/v1/feeds/recent_release")
                    .header(header::ORIGIN, "http://tauri.localhost")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .and_then(|v| v.to_str().ok()),
            Some("http://tauri.localhost")
        );
        assert!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_METHODS)
                .is_some()
        );
    }

    #[tokio::test]
    async fn cors_get_echoes_allowed_origin() {
        let app = test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/meta")
                    .header(header::ORIGIN, "tauri://localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("x-request-id").is_some());
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        assert_eq!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .and_then(|v| v.to_str().ok()),
            Some("tauri://localhost")
        );
        assert!(
            response
                .headers()
                .get("access-control-expose-headers")
                .is_some()
        );
    }

    #[tokio::test]
    async fn cors_disallowed_origin_gets_no_acao() {
        let app = test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/meta")
                    .header(header::ORIGIN, "https://evil.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none()
        );
    }

    #[tokio::test]
    async fn client_discovery_is_storage_independent_and_cors_enabled() {
        let app = test_app_without_repo();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/.well-known/mpgs")
                    .header(header::ORIGIN, "http://tauri.localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .and_then(|value| value.to_str().ok()),
            Some("http://tauri.localhost")
        );
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let document: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(document["service"], "mpgs-server");
        assert_eq!(document["discovery_version"], 1);
        assert_eq!(document["api_version"], "v1");
        assert_eq!(document["api_base_path"], "/v1");
        assert_eq!(document["readiness_path"], "/health/ready");
        assert_eq!(document["authentication"][0], "anonymous");
        assert_eq!(document["authentication"][1], "account");
    }

    #[tokio::test]
    async fn admin_data_status_exposes_m7_release_coverage() {
        let app = test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/admin/v1/data-status")
                    .header(header::AUTHORIZATION, "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["tasks"].is_array());
        assert!(json["coverage"]["normalized_multiplayer_candidates"].is_number());
        assert!(json["m7_coverage"]["trusted_friend_multiplayer_profiles"].is_number());
        assert!(json["m7_coverage"]["trusted_profiles_with_seven_day_ccu"].is_number());
    }

    async fn account_session_json(app: &axum::Router, username: &str) -> serde_json::Value {
        let session = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/auth/register")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "username": username,
                            "display_name": username,
                            "password": format!("password-{username}-long"),
                            "device_label": "test",
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(session.status(), StatusCode::CREATED);
        let bytes = axum::body::to_bytes(session.into_body(), 1024 * 1024)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn account_token(app: &axum::Router, username: &str) -> String {
        account_session_json(app, username).await["access_token"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn account_for_repo(
        repo: &Repository,
        username: &str,
    ) -> mpgs_storage::accounts::AccountSessionTokens {
        repo.register_account(
            &mpgs_storage::accounts::RegisterAccount {
                username: username.to_owned(),
                display_name: username.to_owned(),
                password: format!("password-{username}-long"),
                device_label: "test".to_owned(),
            },
            None,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn play_intent_vote_requires_auth() {
        let app = test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/games/548430/play-intent")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"intent":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn play_intent_toggles_and_surfaces_in_feed() {
        let app = test_app();
        let token = account_token(&app, "vote_user").await;

        // Cast a vote.
        let vote = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/games/548430/play-intent")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"intent":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(vote.status(), StatusCode::OK);
        let vote_json: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(vote.into_body(), 1 << 20)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(vote_json["count"], 1);
        assert_eq!(vote_json["voted"], true);

        // Feed reflects the count and this user's voted flag.
        let feed = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/feeds/classic_legacy?limit=100")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let feed_json: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(feed.into_body(), 1 << 20)
                .await
                .unwrap(),
        )
        .unwrap();
        let drg = feed_json["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["app_id"] == 548430)
            .expect("DRG present");
        assert_eq!(drg["play_intent"]["count"], 1);
        assert_eq!(drg["play_intent"]["voted"], true);

        // Withdraw the vote.
        let unvote = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/games/548430/play-intent")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"intent":false}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let unvote_json: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(unvote.into_body(), 1 << 20)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(unvote_json["count"], 0);
        assert_eq!(unvote_json["voted"], false);
    }

    #[tokio::test]
    async fn community_filters_and_page_size_are_part_of_the_cache_contract() {
        let app = test_app();
        let token = account_token(&app, "community_cache_user").await;
        let vote = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/games/548430/play-intent")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"intent":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(vote.status(), StatusCode::OK);

        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/community/play-intents?sort=trending&release_state=released&limit=1")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let etag = first.headers().get(header::ETAG).unwrap().clone();

        let different_limit = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/community/play-intents?sort=trending&release_state=released&limit=2")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::IF_NONE_MATCH, etag)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(different_limit.status(), StatusCode::OK);

        let invalid_filter = app
            .oneshot(
                Request::builder()
                    .uri("/v1/community/play-intents?platform=untrusted")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid_filter.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn play_intent_unknown_game_is_not_found() {
        let app = test_app();
        let token = account_token(&app, "missing_vote_user").await;
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/games/40404040/play-intent")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"intent":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn play_intent_change_invalidates_feed_cursor() {
        let (repo, app) = test_repo_and_app(RateLimitConfig::default());
        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/feeds/classic_legacy?limit=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let first_json: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(first.into_body(), 1 << 20)
                .await
                .unwrap(),
        )
        .unwrap();
        let cursor = first_json["next_cursor"]
            .as_str()
            .expect("feed has another page");

        let session = account_for_repo(&repo, "cursor_user");
        repo.set_play_intent(&session.user_id, 548430, true)
            .unwrap();

        let stale = app
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/feeds/classic_legacy?limit=1&cursor={cursor}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stale.status(), StatusCode::CONFLICT);
        let stale_json: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(stale.into_body(), 1 << 20)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(stale_json["error"]["code"], "cursor_stale");
    }

    #[tokio::test]
    async fn detail_etag_is_scoped_to_user_vote_state() {
        let (repo, app) = test_repo_and_app(RateLimitConfig::default());
        let first_user = account_for_repo(&repo, "detail_first");
        let second_user = account_for_repo(&repo, "detail_second");
        repo.set_play_intent(&first_user.user_id, 548430, true)
            .unwrap();

        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/games/548430")
                    .header(
                        header::AUTHORIZATION,
                        format!("Bearer {}", first_user.access_token),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let first_etag = first.headers().get(header::ETAG).unwrap().clone();

        let second = app
            .oneshot(
                Request::builder()
                    .uri("/v1/games/548430")
                    .header(
                        header::AUTHORIZATION,
                        format!("Bearer {}", second_user.access_token),
                    )
                    .header(header::IF_NONE_MATCH, first_etag)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        let second_json: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(second.into_body(), 1 << 20)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(second_json["play_intent"]["count"], 1);
        assert_eq!(second_json["play_intent"]["voted"], false);
    }

    #[tokio::test]
    async fn public_feed_and_default_ranking() {
        let app = test_app();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/feeds/classic_legacy?limit=10")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("etag").is_some());
        assert!(response.headers().get("x-request-id").is_some());
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let items = json["items"].as_array().unwrap();
        assert!(!items.is_empty());
        let ids: Vec<u64> = items.iter().filter_map(|i| i["app_id"].as_u64()).collect();
        // Coop classics should appear before CS2 under default prefs.
        let drg = ids.iter().position(|id| *id == 548430);
        let cs2 = ids.iter().position(|id| *id == 730);
        if let (Some(d), Some(c)) = (drg, cs2) {
            assert!(d < c, "DRG should rank above CS2: {ids:?}");
        }

        let first = &items[0];
        let first_app_id = first["app_id"].as_u64().unwrap();
        let referenced_ids: Vec<_> = first["evidence_ids"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|value| value.as_str())
            .collect();
        let evidence_response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/games/{first_app_id}/evidence"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let evidence_body = axum::body::to_bytes(evidence_response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let evidence_json: serde_json::Value = serde_json::from_slice(&evidence_body).unwrap();
        let available_ids: Vec<_> = evidence_json["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["evidence_id"].as_str())
            .collect();
        assert!(
            referenced_ids
                .iter()
                .all(|evidence_id| available_ids.contains(evidence_id)),
            "feed evidence IDs must resolve: {referenced_ids:?} vs {available_ids:?}"
        );
    }

    #[tokio::test]
    async fn demo_catalog_exercises_all_four_sections() {
        let app = test_app();
        for section in [
            "recent_release",
            "upcoming",
            "popular_legacy",
            "classic_legacy",
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/v1/feeds/{section}?limit=10"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{section}");
            let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert!(
                !json["items"].as_array().unwrap().is_empty(),
                "{section} should have a demo candidate"
            );
        }
    }

    #[tokio::test]
    async fn session_preferences_feedback_flow() {
        let app = test_app();
        let json = account_session_json(&app, "preferences_user").await;
        let token = json["access_token"].as_str().unwrap();
        let refresh_token = json["refresh_token"].as_str().unwrap();

        let prefs = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/preferences")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(prefs.status(), StatusCode::OK);

        let feedback = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/feedback")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header("idempotency-key", "fb-1")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"app_id":548430,"type":"like"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(feedback.status(), StatusCode::CREATED);

        let again = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/feedback")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header("idempotency-key", "fb-1")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"app_id":548430,"type":"like"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(again.status(), StatusCode::CREATED);

        let refreshed = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/auth/refresh")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(
                        r#"{{"refresh_token":"{refresh_token}"}}"#
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(refreshed.status(), StatusCode::OK);

        let old_access = app
            .oneshot(
                Request::builder()
                    .uri("/v1/preferences")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(old_access.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn active_feedback_changes_feed_and_undo_restores_it() {
        let app = test_app();
        let session_json = account_session_json(&app, "feedback_user").await;
        let token = session_json["access_token"].as_str().unwrap();

        let before = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/feeds/classic_legacy?limit=100")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let before_body = axum::body::to_bytes(before.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let before_json: serde_json::Value = serde_json::from_slice(&before_body).unwrap();
        assert!(
            before_json["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["app_id"] == 548430)
        );

        let feedback = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/feedback")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header("idempotency-key", "hide-drg")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"app_id":548430,"type":"not_interested"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let feedback_body = axum::body::to_bytes(feedback.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let feedback_json: serde_json::Value = serde_json::from_slice(&feedback_body).unwrap();
        let feedback_id = feedback_json["feedback_id"].as_i64().unwrap();

        let hidden = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/feeds/classic_legacy?limit=100")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let hidden_body = axum::body::to_bytes(hidden.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let hidden_json: serde_json::Value = serde_json::from_slice(&hidden_body).unwrap();
        assert!(
            !hidden_json["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["app_id"] == 548430)
        );

        let undo = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/feedback/{feedback_id}/undo"))
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(undo.status(), StatusCode::OK);

        let restored = app
            .oneshot(
                Request::builder()
                    .uri("/v1/feeds/classic_legacy?limit=100")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let restored_body = axum::body::to_bytes(restored.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let restored_json: serde_json::Value = serde_json::from_slice(&restored_body).unwrap();
        assert!(
            restored_json["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["app_id"] == 548430)
        );
    }

    #[tokio::test]
    async fn search_and_detail() {
        let app = test_app();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/search?q=Deep&limit=5")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(!json["items"].as_array().unwrap().is_empty());

        let detail = app
            .oneshot(
                Request::builder()
                    .uri("/v1/games/548430")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(detail.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn natural_language_fallback_interprets_constraints_and_returns_candidates() {
        let app = test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/recommendations/natural-language")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"query":"3 people, one hour, casual, replayable, dedicated server","limit":6}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ai_status"], "fallback");
        assert_eq!(json["interpreted"]["party_size"], 3);
        assert_eq!(json["interpreted"]["session_minutes_max"], 60);
        assert_eq!(json["interpreted"]["self_hosting_willingness"], 1.0);
        assert_eq!(json["interpreted"]["selected_section"], "classic_legacy");
        let items = json["items"].as_array().unwrap();
        assert!((3..=10).contains(&items.len()));
        assert!(
            items
                .iter()
                .all(|item| !item["reasons"].as_array().unwrap().is_empty())
        );
    }

    #[tokio::test]
    async fn m8_ai_search_returns_analysis_id_with_base_results() {
        let app = test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/ai/search")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"query":"3 people casual coop windows under budget","limit":5,"async":true}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["analysis_id"].as_str().unwrap().starts_with("an_"));
        assert!(json["items"].as_array().is_some());
        // AI is disabled in test_app → deterministic path remains usable.
        assert!(matches!(
            json["ai_status"].as_str(),
            Some("fallback" | "disabled" | "pending")
        ));

        let analysis_id = json["analysis_id"].as_str().unwrap();
        let poll = test_app()
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/ai/analyses/{analysis_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // Separate app instance has a fresh in-memory DB, so not_found is expected
        // for cross-instance poll. Same-instance poll is covered below.
        assert!(poll.status() == StatusCode::NOT_FOUND || poll.status() == StatusCode::OK);
    }

    #[tokio::test]
    async fn m8_ai_search_analysis_round_trip_same_instance() {
        let (repo, app) = test_repo_and_app(RateLimitConfig {
            enabled: false,
            ..RateLimitConfig::default()
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/ai/search")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"query":"private coop for three","limit":4}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let analysis_id = json["analysis_id"].as_str().unwrap().to_owned();

        let poll = app
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/ai/analyses/{analysis_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(poll.status(), StatusCode::OK);
        let poll_body = axum::body::to_bytes(poll.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let poll_json: serde_json::Value = serde_json::from_slice(&poll_body).unwrap();
        assert_eq!(poll_json["analysis_id"], analysis_id);
        assert_eq!(poll_json["task_type"], "rank_explain");
        assert!(poll_json["base_result"].is_object());
        let _ = repo;
    }

    #[tokio::test]
    async fn m8_ai_compare_returns_server_fact_matrix_only() {
        let app = test_app();
        // Discover two real app ids from demo seed.
        let feed = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/feeds/classic_legacy?limit=2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let feed_body = axum::body::to_bytes(feed.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let feed_json: serde_json::Value = serde_json::from_slice(&feed_body).unwrap();
        let items = feed_json["items"].as_array().unwrap();
        if items.len() < 2 {
            return;
        }
        let a = items[0]["app_id"].as_u64().unwrap();
        let b = items[1]["app_id"].as_u64().unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/ai/compare")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(r#"{{"app_ids":[{a},{b}]}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["fact_matrix"].as_array().unwrap().len(), 2);
        assert!(json["explanation"].is_null());
        // Without a configured provider the matrix still returns and AI is disabled.
        assert!(matches!(
            json["ai_status"].as_str(),
            Some("disabled" | "fallback")
        ));
    }

    #[tokio::test]
    async fn m8_group_advice_returns_deterministic_compromise() {
        let app = test_app();
        let feed = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/feeds/classic_legacy?limit=3")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let feed_body = axum::body::to_bytes(feed.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let feed_json: serde_json::Value = serde_json::from_slice(&feed_body).unwrap();
        let items = feed_json["items"].as_array().unwrap();
        if items.len() < 2 {
            return;
        }
        let ids: Vec<u64> = items
            .iter()
            .take(3)
            .filter_map(|item| item["app_id"].as_u64())
            .collect();
        let body = serde_json::json!({
            "party_size": 3,
            "platforms": ["windows"],
            "candidate_app_ids": ids,
            "vote_counts": [{ "app_id": ids[0], "votes": 5 }, { "app_id": ids[1], "votes": 1 }]
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/ai/group-advice")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["advice"]["primary_app_id"], ids[0]);
        assert!(matches!(
            json["ai_status"].as_str(),
            Some("fallback" | "disabled" | "used")
        ));
    }

    #[tokio::test]
    async fn m8_game_ai_summary_returns_rule_fallback() {
        let app = test_app();
        let feed = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/feeds/classic_legacy?limit=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let feed_body = axum::body::to_bytes(feed.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let feed_json: serde_json::Value = serde_json::from_slice(&feed_body).unwrap();
        let app_id = feed_json["items"][0]["app_id"].as_u64().unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/games/{app_id}/ai-summary"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(matches!(
            json["ai_status"].as_str(),
            Some("fallback" | "cached")
        ));
        assert!(json["summary"]["who_it_fits"]["text"].as_str().is_some());
    }

    #[tokio::test]
    async fn natural_language_uses_fake_ai_when_available_and_valid() {
        use mpgs_ai::{AiGateway, AiPolicy, FakeProvider};
        use std::sync::Arc;

        let db = Database::open_in_memory().unwrap();
        let repo = Repository::new(db);
        repo.migrate().unwrap();
        repo.ensure_runtime_defaults().unwrap();
        repo.seed_demo_if_empty().unwrap();

        // Probe deterministic ranking first to learn a real app_id + evidence.
        let probe = build_router(AppState {
            repo: Some(repo.clone()),
            admin_token: Some("secret".into()),
            rate_limits: RateLimitConfig {
                enabled: false,
                ..RateLimitConfig::default()
            },
            cors: CorsConfig::default(),
            account_ai_limits: AccountAiLimiter::default(),
            ai: AiGateway::disabled(),
            task_router: test_task_router(),
            embedding: test_embedding(),
        });
        let probe_resp = probe
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/recommendations/natural-language")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"query":"3 people casual coop replayable","limit":10}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let probe_body = axum::body::to_bytes(probe_resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let probe_json: serde_json::Value = serde_json::from_slice(&probe_body).unwrap();
        let items = probe_json["items"].as_array().unwrap();
        let first = items
            .iter()
            .skip(3)
            .find(|item| {
                item["evidence_ids"]
                    .as_array()
                    .map(|ids| !ids.is_empty())
                    .unwrap_or(false)
            })
            .expect("expected an evidenced candidate outside the public top 3");
        let app_id = first["app_id"].as_u64().unwrap();
        let evidence = first["evidence_ids"].as_array().unwrap()[0]
            .as_str()
            .unwrap()
            .to_owned();

        let fake = Arc::new(FakeProvider {
            response: serde_json::json!({
                "recommendations": [{
                    "app_id": app_id,
                    "fit_score": 0.95,
                    "confidence": 0.9,
                    "reason_evidence_ids": [evidence.clone()],
                    "reasons": ["AI validated private coop fit"],
                    "cautions": []
                }],
                "summary": "Prefer private cooperative sessions.",
                "summary_evidence_ids": [evidence.clone()]
            }),
            fail_with: None,
            available: true,
            delay: std::time::Duration::ZERO,
            ..FakeProvider::default()
        });
        let app = build_router(AppState {
            repo: Some(repo.clone()),
            admin_token: Some("secret".into()),
            rate_limits: RateLimitConfig {
                enabled: false,
                ..RateLimitConfig::default()
            },
            cors: CorsConfig::default(),
            account_ai_limits: AccountAiLimiter::default(),
            ai: AiGateway::new(fake.clone(), AiPolicy::default()),
            task_router: test_task_router(),
            embedding: test_embedding(),
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/recommendations/natural-language")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"query":"3 people casual coop replayable","limit":3}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ai_status"], "used");
        assert!(json["fallback_reason"].is_null());
        assert_eq!(json["ai_summary"], "Prefer private cooperative sessions.");
        assert_eq!(json["ai_summary_evidence_ids"][0], evidence);
        assert_eq!(json["items"].as_array().unwrap().len(), 3);
        assert_eq!(json["items"][0]["app_id"], app_id);
        assert_eq!(
            json["items"][0]["ai_reasons"][0],
            "AI validated private coop fit"
        );

        // Second identical request should hit the AI analysis cache.
        let cached = build_router(AppState {
            repo: Some(repo.clone()),
            admin_token: Some("secret".into()),
            rate_limits: RateLimitConfig {
                enabled: false,
                ..RateLimitConfig::default()
            },
            cors: CorsConfig::default(),
            account_ai_limits: AccountAiLimiter::default(),
            ai: AiGateway::new(fake, AiPolicy::default()),
            task_router: test_task_router(),
            embedding: test_embedding(),
        })
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/recommendations/natural-language")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"query":"3 people casual coop replayable","limit":3}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
        let cached_body = axum::body::to_bytes(cached.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let cached_json: serde_json::Value = serde_json::from_slice(&cached_body).unwrap();
        assert_eq!(cached_json["ai_status"], "cached");

        let meta = build_router(AppState {
            repo: None,
            admin_token: None,
            rate_limits: RateLimitConfig {
                enabled: false,
                ..RateLimitConfig::default()
            },
            cors: CorsConfig::default(),
            account_ai_limits: AccountAiLimiter::default(),
            ai: AiGateway::new(Arc::new(FakeProvider::default()), AiPolicy::default()),
            task_router: test_task_router(),
            embedding: test_embedding(),
        })
        .oneshot(
            Request::builder()
                .uri("/v1/meta")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        let meta_body = axum::body::to_bytes(meta.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let meta_json: serde_json::Value = serde_json::from_slice(&meta_body).unwrap();
        assert_eq!(meta_json["ai_available"], true);
    }

    #[tokio::test]
    async fn m6_fault_natural_language_times_out_to_deterministic_results() {
        use mpgs_ai::{AiGateway, AiPolicy, FakeProvider};
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        let db = Database::open_in_memory().unwrap();
        let repo = Repository::new(db);
        repo.migrate().unwrap();
        repo.ensure_runtime_defaults().unwrap();
        repo.seed_demo_if_empty().unwrap();
        let provider = FakeProvider {
            delay: Duration::from_secs(2),
            ..FakeProvider::default()
        };
        let policy = AiPolicy {
            online_timeout: Duration::from_millis(10),
            ..AiPolicy::default()
        };
        let app = build_router(AppState {
            repo: Some(repo),
            admin_token: Some("secret".into()),
            rate_limits: RateLimitConfig {
                enabled: false,
                ..RateLimitConfig::default()
            },
            cors: CorsConfig::default(),
            account_ai_limits: AccountAiLimiter::default(),
            ai: AiGateway::new(Arc::new(provider), policy),
            task_router: test_task_router(),
            embedding: test_embedding(),
        });
        let started = Instant::now();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/recommendations/natural-language")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"query":"4 people casual cooperative game","limit":5}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ai_status"], "fallback");
        assert!(!json["items"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn calendar_can_switch_between_recent_and_upcoming() {
        let app = test_app();
        let recent = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/calendar?state=recent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(recent.status(), StatusCode::OK);
        let body = axum::body::to_bytes(recent.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            json["dated_items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["release_state"] == "released" && item["early_data"].is_boolean())
        );

        let upcoming = app
            .oneshot(
                Request::builder()
                    .uri("/v1/calendar?state=upcoming")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(upcoming.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn conditional_requests_and_invalid_inputs_are_handled() {
        let app = test_app();
        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/feeds/classic_legacy?limit=2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let etag = first.headers().get(header::ETAG).unwrap().clone();

        let cached = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/feeds/classic_legacy?limit=2")
                    .header(header::IF_NONE_MATCH, etag)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cached.status(), StatusCode::NOT_MODIFIED);

        for uri in [
            "/v1/feeds/classic_legacy?cursor=broken",
            "/v1/feeds/classic_legacy?party_size=0",
            "/v1/feeds/classic_legacy?limit=101",
            "/v1/calendar?from=2026-02-30&to=2026-12-31",
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{uri}");
        }

        let invalid_token = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/preferences")
                    .header(header::AUTHORIZATION, "Bearer invalid")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid_token.status(), StatusCode::UNAUTHORIZED);
        let response_request_id = invalid_token
            .headers()
            .get("x-request-id")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let invalid_body = axum::body::to_bytes(invalid_token.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let invalid_json: serde_json::Value = serde_json::from_slice(&invalid_body).unwrap();
        assert_eq!(
            invalid_json["error"]["request_id"].as_str(),
            Some(response_request_id.as_str())
        );

        let missing_evidence = app
            .oneshot(
                Request::builder()
                    .uri("/v1/games/4000000000/evidence")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing_evidence.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn generated_openapi_covers_the_public_contract() {
        let response = test_app()
            .oneshot(
                Request::builder()
                    .uri("/openapi.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        let document: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(document["openapi"], "3.1.0");
        for path in [
            "/.well-known/mpgs",
            "/health/live",
            "/health/ready",
            "/v1/meta",
            "/v1/session/anonymous",
            "/v1/session/refresh",
            "/v1/auth/register",
            "/v1/auth/login",
            "/v1/auth/refresh",
            "/v1/auth/logout",
            "/v1/auth/logout-all",
            "/v1/auth/password",
            "/v1/me",
            "/v1/me/avatar",
            "/v1/me/ai-settings",
            "/v1/me/ai-settings/test",
            "/v1/me/ai-settings/discover",
            "/v1/me/ai-settings/custom-key",
            "/v1/community/play-intents",
            "/v1/preferences",
            "/v1/feeds/{section}",
            "/v1/recommendations/natural-language",
            "/v1/calendar",
            "/v1/search",
            "/v1/games/{app_id}",
            "/v1/games/{app_id}/evidence",
            "/v1/games/{app_id}/play-intent",
            "/v1/feedback",
            "/v1/feedback/{feedback_id}/undo",
        ] {
            assert!(document["paths"].get(path).is_some(), "missing {path}");
        }
        assert!(
            document["paths"]
                .get("/admin/v1/games/{app_id}/overrides")
                .is_none()
        );
        let preferences = &document["components"]["schemas"]["UserPreferences"]["properties"];
        for field in [
            "party_size",
            "session_minutes_min",
            "budget_max_each_minor",
            "platforms",
            "languages",
            "excluded_modes",
        ] {
            assert!(
                preferences.get(field).is_some(),
                "missing preference {field}"
            );
        }
        assert!(document["components"]["securitySchemes"]["bearer_auth"].is_object());
    }

    #[tokio::test]
    async fn public_rate_limit_returns_stable_429_contract() {
        let (_, app) = test_repo_and_app(RateLimitConfig {
            read_per_minute: 2,
            global_per_minute: 100,
            ..RateLimitConfig::default()
        });
        for _ in 0..2 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/v1/meta")
                        .header("x-device-id", "test-device")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }
        let limited = app
            .oneshot(
                Request::builder()
                    .uri("/v1/meta")
                    .header("x-device-id", "test-device")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(limited.headers().get(header::RETRY_AFTER).is_some());
        assert_eq!(limited.headers()["x-ratelimit-remaining"], "0");
        let request_id = limited.headers()["x-request-id"]
            .to_str()
            .unwrap()
            .to_owned();
        let body = axum::body::to_bytes(limited.into_body(), 64 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], "rate_limited");
        assert_eq!(json["error"]["request_id"], request_id);
    }

    #[tokio::test]
    async fn active_algorithm_config_and_all_preference_inputs_drive_feed() {
        let (repo, app) = test_repo_and_app(RateLimitConfig::default());
        // Classic is residual of non-popular legacy games; classic_min_* no longer
        // empties the section. Prove active config still gates popular membership.
        repo.database()
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE algorithm_configs
                     SET config_json = json_set(
                         json_set(config_json, '$.popular_min_ccu', 500000000),
                         '$.popular_high_ccu', 500000000
                     )
                     WHERE status = 'active'",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        let empty_popular = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/feeds/popular_legacy?limit=100")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(empty_popular.status(), StatusCode::OK);
        let body = axum::body::to_bytes(empty_popular.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let popular_items = json["items"]
            .as_array()
            .unwrap_or_else(|| panic!("unexpected popular feed body: {json}"));
        assert!(popular_items.is_empty());

        repo.database()
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE algorithm_configs
                     SET config_json = json_set(
                         json_set(config_json, '$.popular_min_ccu', 500),
                         '$.popular_high_ccu', 100000
                     )
                     WHERE status = 'active'",
                    [],
                )?;
                conn.execute_batch(
                    "INSERT INTO app_availability (
                         app_id, platforms_json, languages_json,
                         typical_session_minutes_min, typical_session_minutes_max,
                         is_free, updated_at_ms
                     )
                     SELECT app_id, '[\"windows\"]', '[\"english\"]', 120, 240, 0, 2000
                     FROM apps
                     WHERE 1
                     ON CONFLICT(app_id) DO UPDATE SET
                         platforms_json = excluded.platforms_json,
                         languages_json = excluded.languages_json,
                         typical_session_minutes_min = excluded.typical_session_minutes_min,
                         typical_session_minutes_max = excluded.typical_session_minutes_max,
                         is_free = excluded.is_free,
                         updated_at_ms = excluded.updated_at_ms;

                     UPDATE app_availability
                     SET platforms_json = '[\"linux\"]', languages_json = '[\"schinese\"]',
                         typical_session_minutes_min = 30, typical_session_minutes_max = 90
                     WHERE app_id = 548430;

                     INSERT INTO price_snapshots (
                         app_id, country_code, currency, captured_at_ms,
                         initial_price_minor, final_price_minor, discount_percent,
                         is_purchasable, package_id, source
                     )
                     SELECT app_id, 'CN', 'CNY', 2000, 10000, 10000, 0, 1, NULL, 'test'
                     FROM apps;

                     UPDATE price_snapshots SET final_price_minor = 5000
                     WHERE app_id = 548430 AND currency = 'CNY';",
                )?;
                Ok(())
            })
            .unwrap();

        // Deep Rock Galactic is popular_legacy under residual classic rules (high CCU
        // + friend-fit), so preference filtering is exercised on that section.
        let filtered = app
            .oneshot(
                Request::builder()
                    .uri("/v1/feeds/popular_legacy?limit=100&platforms=linux&languages=schinese&session_minutes_min=30&session_minutes_max=90&max_price_minor=6000&currency=CNY")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(filtered.status(), StatusCode::OK);
        let body = axum::body::to_bytes(filtered.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let ids: Vec<_> = json["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["app_id"].as_u64())
            .collect();
        assert_eq!(ids, vec![548430]);
    }

    #[tokio::test]
    async fn active_algorithm_version_is_consistent_across_public_responses() {
        let (repo, app) = test_repo_and_app(RateLimitConfig::default());
        repo.database()
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE algorithm_configs
                     SET version = 'rules-0.1.0',
                         config_json = json_remove(
                             config_json, '$.play_intent_weight', '$.play_intent_saturation'
                         )
                     WHERE status = 'active'",
                    [],
                )?;
                Ok(())
            })
            .unwrap();

        for uri in [
            "/v1/meta",
            "/v1/search?q=Deep",
            "/v1/feeds/classic_legacy?limit=1",
            "/v1/games/548430",
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{uri}");
            let json: serde_json::Value = serde_json::from_slice(
                &axum::body::to_bytes(response.into_body(), 1 << 20)
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(json["algorithm_version"], "rules-0.1.0", "{uri}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn m6_fault_sqlite_lock_wait_does_not_block_the_async_runtime() {
        use std::sync::{Arc, Barrier};
        use std::time::Duration;

        let (repo, app) = test_repo_and_app(RateLimitConfig::default());
        let barrier = Arc::new(Barrier::new(2));
        let worker_barrier = barrier.clone();
        let locked_repo = repo.clone();
        let holder = std::thread::spawn(move || {
            locked_repo
                .database()
                .with_conn(|_| {
                    worker_barrier.wait();
                    std::thread::sleep(Duration::from_millis(250));
                    Ok(())
                })
                .unwrap();
        });
        barrier.wait();

        let pending_search = tokio::spawn(
            app.clone().oneshot(
                Request::builder()
                    .uri("/v1/search?q=Deep")
                    .body(Body::empty())
                    .unwrap(),
            ),
        );
        tokio::task::yield_now().await;
        let live = tokio::time::timeout(
            Duration::from_millis(100),
            app.oneshot(
                Request::builder()
                    .uri("/health/live")
                    .body(Body::empty())
                    .unwrap(),
            ),
        )
        .await
        .expect("health request must not wait for SQLite")
        .unwrap();
        assert_eq!(live.status(), StatusCode::OK);
        assert_eq!(
            pending_search.await.unwrap().unwrap().status(),
            StatusCode::OK
        );
        holder.join().unwrap();
    }

    #[tokio::test]
    #[ignore = "manual M3 latency gate with a 2,000-game catalog"]
    async fn two_thousand_game_feed_meets_local_p95_gate() {
        let db = Database::open_in_memory().unwrap();
        let repo = Repository::new(db);
        repo.migrate().unwrap();
        repo.ensure_runtime_defaults().unwrap();
        repo.database()
            .with_conn(|conn| {
                conn.execute_batch(
                    "WITH RECURSIVE seq(x) AS (
                         SELECT 1 UNION ALL SELECT x + 1 FROM seq WHERE x < 2000
                     )
                     INSERT INTO apps (
                         app_id, app_type, canonical_name, release_state, release_date,
                         release_date_precision, created_at_ms, updated_at_ms
                     )
                     SELECT 3000000 + x, 'game', 'Perf Game ' || x, 'released', '2020-01-01',
                            'day', 1000, 1000 FROM seq;

                     INSERT INTO multiplayer_profiles (
                         app_id, dominant_mode, private_session, online_coop, self_hosted_server,
                         recommended_min_players, recommended_max_players, profile_confidence,
                         computed_at_ms
                     )
                     SELECT app_id, 'private_coop', 1, 1, 0, 1, 4, 0.9, 1000 FROM apps;

                     INSERT INTO review_snapshots (
                         app_id, region_scope, language_scope, captured_at_ms, total_positive,
                         total_negative, total_reviews, review_score, review_score_desc,
                         wilson_lower, filter_offtopic_activity, parameter_hash, content_hash, source
                     )
                     SELECT app_id, 'all', 'all', 1000, 9000, 1000, 10000, 8, 'Very Positive',
                            0.89, 1, 'perf', 'perf', 'perf' FROM apps;

                     INSERT INTO player_snapshots (
                         app_id, captured_at_ms, player_count, result_code, content_hash, source,
                         offline_players_excluded
                     )
                     SELECT app_id, 1000, 2000, 1, 'perf', 'perf', 1 FROM apps;",
                )?;
                Ok(())
            })
            .unwrap();
        let app = build_router(AppState {
            repo: Some(repo),
            admin_token: Some("secret".into()),
            rate_limits: RateLimitConfig {
                enabled: false,
                ..RateLimitConfig::default()
            },
            cors: CorsConfig::default(),
            account_ai_limits: AccountAiLimiter::default(),
            ai: mpgs_ai::AiGateway::disabled(),
            task_router: test_task_router(),
            embedding: test_embedding(),
        });

        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/feeds/classic_legacy?limit=20")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let etag = first.headers().get(header::ETAG).unwrap().clone();

        let mut uncached = Vec::new();
        for _ in 0..25 {
            let started = std::time::Instant::now();
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/v1/feeds/classic_legacy?limit=20")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            uncached.push(started.elapsed());
        }

        let mut cached = Vec::new();
        for _ in 0..100 {
            let started = std::time::Instant::now();
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/v1/feeds/classic_legacy?limit=20")
                        .header(header::IF_NONE_MATCH, etag.clone())
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
            cached.push(started.elapsed());
        }

        uncached.sort_unstable();
        cached.sort_unstable();
        let uncached_p95 = uncached[uncached.len() * 95 / 100];
        let cached_p95 = cached[cached.len() * 95 / 100];
        eprintln!("uncached_p95={uncached_p95:?}, cached_p95={cached_p95:?}");
        assert!(cached_p95 < std::time::Duration::from_millis(500));
        assert!(uncached_p95 < std::time::Duration::from_millis(500));
    }

    #[tokio::test]
    async fn m6_meta_includes_release_provenance_fields() {
        let app = test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/meta")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["api_version"], "v1");
        assert_eq!(json["service_version"], env!("CARGO_PKG_VERSION"));
        assert!(
            json["algorithm_version"]
                .as_str()
                .is_some_and(|v| !v.is_empty())
        );
        assert!(json["schema_version"].as_i64().is_some_and(|v| v > 0));
        assert!(json["build_git_sha"].as_str().is_some());
        assert!(json["data_updated_at_ms"].as_i64().is_some());
        assert_eq!(json["storage_enabled"], true);
    }

    #[tokio::test]
    async fn m6_soak_concurrent_reads_stay_healthy_for_bounded_window() {
        let (_, app) = test_repo_and_app(RateLimitConfig {
            enabled: false,
            ..RateLimitConfig::default()
        });
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut workers = Vec::new();
        for worker in 0..4 {
            let app = app.clone();
            workers.push(tokio::spawn(async move {
                let mut completed = 0usize;
                while tokio::time::Instant::now() < deadline {
                    let uri = match completed % 4 {
                        0 => "/health/live",
                        1 => "/health/ready",
                        2 => "/v1/meta",
                        _ => "/v1/feeds/classic_legacy?limit=5",
                    };
                    let response = app
                        .clone()
                        .oneshot(
                            Request::builder()
                                .uri(uri)
                                .header("x-device-id", format!("m6-soak-{worker}"))
                                .body(Body::empty())
                                .unwrap(),
                        )
                        .await
                        .unwrap();
                    assert_eq!(
                        response.status(),
                        StatusCode::OK,
                        "worker {worker} request {completed} to {uri} must remain healthy"
                    );
                    completed += 1;
                }
                completed
            }));
        }

        let mut completed = 0usize;
        for worker in workers {
            completed += worker.await.unwrap();
        }
        assert!(
            completed >= 160,
            "bounded soak completed only {completed} requests"
        );
    }

    #[tokio::test]
    async fn m6_fault_ai_unavailable_does_not_break_deterministic_paths() {
        let (repo, app) = test_repo_and_app(RateLimitConfig {
            enabled: false,
            ..RateLimitConfig::default()
        });
        // Force AI off (test helper already uses disabled; re-assert feed + NL fallback).
        let _ = repo;
        let feed = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/feeds/classic_legacy?limit=5")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(feed.status(), StatusCode::OK);

        let nl = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/recommendations/natural-language")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"query":"4人合作自建服 不想太竞技","limit":5}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(nl.status(), StatusCode::OK);
        let body = axum::body::to_bytes(nl.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ai_status"], "fallback");
        assert!(
            json["items"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
        );
    }

    #[tokio::test]
    async fn m6_fault_admin_without_token_stays_denied() {
        let (_, app) = test_repo_and_app(RateLimitConfig {
            enabled: false,
            ..RateLimitConfig::default()
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/v1/games/570/overrides")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"feature_name":"self_hosted_server","value":true,"reason":"m6 fault","operator":"m6"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "admin write without token must be 401"
        );
    }
}
