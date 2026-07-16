#![forbid(unsafe_code)]

mod api;
mod cors;
mod rate_limit;

use std::{env, error::Error, io, net::SocketAddr};

use mpgs_ai::gateway_from_env;
use mpgs_storage::{Database, Repository};
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::api::{AppState, build_router};
use crate::cors::CorsConfig;
use crate::rate_limit::RateLimitConfig;

const DEFAULT_BIND_ADDR: &str = "127.0.0.1:8080";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    init_tracing();
    let state = build_state()?;
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

fn build_state() -> Result<AppState, Box<dyn Error>> {
    let admin_token = env::var("MPGS_ADMIN_TOKEN").ok().filter(|s| !s.is_empty());
    let repo = match env::var("MPGS_DATABASE_PATH") {
        Ok(path) if !path.is_empty() => {
            let db = Database::open(&path)?;
            let repo = Repository::new(db);
            let version = repo.migrate()?;
            repo.ensure_runtime_defaults()?;
            if demo_seed_enabled()? {
                let seeded = repo.seed_demo_if_empty()?;
                info!(seeded, "demo catalog seed enabled");
            }
            repo.assert_ready()?;
            info!(%path, version, "database ready");
            Some(repo)
        }
        _ => {
            // In-memory DB for local public API demos without configuring a path.
            let db = Database::open_in_memory()?;
            let repo = Repository::new(db);
            repo.migrate()?;
            repo.ensure_runtime_defaults()?;
            repo.seed_demo_if_empty()?;
            info!("using in-memory database (set MPGS_DATABASE_PATH for persistence)");
            Some(repo)
        }
    };
    let ai = gateway_from_env().map_err(|error| {
        io::Error::new(io::ErrorKind::InvalidInput, error.to_string())
    })?;
    info!(provider = ai.provider_name(), available = ai.is_available(), "AI gateway ready");
    Ok(AppState {
        repo,
        admin_token,
        rate_limits: RateLimitConfig::from_env()?,
        cors: CorsConfig::from_env()?,
        ai,
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
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "failed to install Ctrl+C handler");
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
            ai: mpgs_ai::AiGateway::disabled(),
        });
        (repo, app)
    }

    fn test_app() -> axum::Router {
        test_repo_and_app(RateLimitConfig::default()).1
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

    async fn anon_token(app: &axum::Router) -> String {
        let session = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/session/anonymous")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(session.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        json["access_token"].as_str().unwrap().to_owned()
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
        let token = anon_token(&app).await;

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
    async fn play_intent_unknown_game_is_not_found() {
        let app = test_app();
        let token = anon_token(&app).await;
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

        let session = repo.create_anonymous_session().unwrap();
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
        let first_user = repo.create_anonymous_session().unwrap();
        let second_user = repo.create_anonymous_session().unwrap();
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
        let session = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/session/anonymous")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(session.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(session.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
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
                    .uri("/v1/session/refresh")
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
        let session = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/session/anonymous")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let session_body = axum::body::to_bytes(session.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let session_json: serde_json::Value = serde_json::from_slice(&session_body).unwrap();
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
            ai: AiGateway::disabled(),
        });
        let probe_resp = probe
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/recommendations/natural-language")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"query":"3 people casual coop replayable","limit":5}"#,
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
            .find(|item| {
                item["evidence_ids"]
                    .as_array()
                    .map(|ids| !ids.is_empty())
                    .unwrap_or(false)
            })
            .expect("expected at least one item with evidence_ids");
        let app_id = first["app_id"].as_u64().unwrap();
        let evidence = first["evidence_ids"].as_array().unwrap()[0]
            .as_str()
            .unwrap()
            .to_owned();

        let fake = FakeProvider {
            response: serde_json::json!({
                "recommendations": [{
                    "app_id": app_id,
                    "fit_score": 0.95,
                    "confidence": 0.9,
                    "reason_evidence_ids": [evidence],
                    "reasons": ["AI validated private coop fit"],
                    "cautions": []
                }],
                "summary": "Prefer private cooperative sessions."
            }),
            fail_with: None,
            available: true,
        };
        let app = build_router(AppState {
            repo: Some(repo),
            admin_token: Some("secret".into()),
            rate_limits: RateLimitConfig {
                enabled: false,
                ..RateLimitConfig::default()
            },
            cors: CorsConfig::default(),
            ai: AiGateway::new(Arc::new(fake), AiPolicy::default()),
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/recommendations/natural-language")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"query":"3 people casual coop replayable","limit":5}"#,
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
        assert_eq!(json["items"][0]["app_id"], app_id);
        assert_eq!(
            json["items"][0]["ai_reasons"][0],
            "AI validated private coop fit"
        );

        let meta = build_router(AppState {
            repo: None,
            admin_token: None,
            rate_limits: RateLimitConfig {
                enabled: false,
                ..RateLimitConfig::default()
            },
            cors: CorsConfig::default(),
            ai: AiGateway::new(Arc::new(FakeProvider::default()), AiPolicy::default()),
        })
        .oneshot(Request::builder().uri("/v1/meta").body(Body::empty()).unwrap())
        .await
        .unwrap();
        let meta_body = axum::body::to_bytes(meta.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let meta_json: serde_json::Value = serde_json::from_slice(&meta_body).unwrap();
        assert_eq!(meta_json["ai_available"], true);
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
            "/health/live",
            "/health/ready",
            "/v1/meta",
            "/v1/session/anonymous",
            "/v1/session/refresh",
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
        repo.database()
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE algorithm_configs
                     SET config_json = json_set(config_json, '$.classic_min_reviews', 999999999)
                     WHERE status = 'active'",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        let empty = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/feeds/classic_legacy?limit=100")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(empty.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["items"].as_array().unwrap().is_empty());

        repo.database()
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE algorithm_configs
                     SET config_json = json_set(config_json, '$.classic_min_reviews', 3000)
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

        let filtered = app
            .oneshot(
                Request::builder()
                    .uri("/v1/feeds/classic_legacy?limit=100&platforms=linux&languages=schinese&session_minutes_min=30&session_minutes_max=90&max_price_minor=6000&currency=CNY")
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
    async fn sqlite_lock_wait_does_not_block_the_async_runtime() {
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
            ai: mpgs_ai::AiGateway::disabled(),
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
}
