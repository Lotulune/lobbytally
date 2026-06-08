use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use mpgs_server::{
    admin::hash_token, build_router_with_state, AppState, DatabaseHealth, RateLimitConfig,
    RateLimiters, ServerConfig, ServiceInfoConfig,
};
use serde_json::json;
use std::path::Path;
use tower::ServiceExt;

fn test_config() -> ServiceInfoConfig {
    ServiceInfoConfig {
        service_instance_id: "018fb770-8998-7699-a6e4-b7b59f2f9c01".to_string(),
        service_name: "MPGS Setup Test Service".to_string(),
        service_version: "0.1.0".to_string(),
    }
}

fn setup_app(config_dir: &Path) -> axum::Router {
    build_router_with_state(
        AppState::new(test_config().service_info(), DatabaseHealth::HealthyForTest)
            .with_setup_access(config_dir, "correct-setup-token"),
    )
}

fn rate_limited_setup_app(config_dir: &Path) -> axum::Router {
    build_router_with_state(
        AppState::new(test_config().service_info(), DatabaseHealth::HealthyForTest)
            .with_setup_access(config_dir, "correct-setup-token")
            .with_rate_limits(RateLimiters::new(RateLimitConfig::for_tests(1))),
    )
}

async fn request_json(
    app: axum::Router,
    method: Method,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value, axum::http::HeaderMap) {
    let response = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value = serde_json::from_slice(&body).unwrap();

    (status, value, headers)
}

fn setup_payload(setup_token: &str, admin_token: &str) -> serde_json::Value {
    json!({
        "setupToken": setup_token,
        "serviceName": "MPGS Configured Service",
        "publicBaseUrl": "https://mpgs.example.test",
        "databaseUrl": "postgres://mpgs:secret@postgres:5432/mpgs",
        "adminToken": admin_token,
        "steamApiKey": "fake-steam-key"
    })
}

#[tokio::test]
async fn setup_complete_requires_valid_setup_token() {
    let temp_dir = tempfile::tempdir().unwrap();
    let (status, value, headers) = request_json(
        setup_app(temp_dir.path()),
        Method::POST,
        "/api/v1/setup/complete",
        setup_payload("wrong-token", "new-admin-token"),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(value["error"]["code"], "setup_token_invalid");
    assert!(headers.get(header::SET_COOKIE).is_none());
    assert!(!temp_dir.path().join("active/service.toml").exists());
    assert!(!temp_dir.path().join("active/secrets.toml").exists());
}

#[tokio::test]
async fn setup_complete_writes_active_config_without_storing_admin_token_plaintext() {
    let temp_dir = tempfile::tempdir().unwrap();
    let (status, value, _headers) = request_json(
        setup_app(temp_dir.path()),
        Method::POST,
        "/api/v1/setup/complete",
        setup_payload("correct-setup-token", "new-admin-token"),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["configured"], true);
    assert_eq!(value["restartRequired"], true);

    let service_toml =
        std::fs::read_to_string(temp_dir.path().join("active/service.toml")).unwrap();
    let secrets_toml =
        std::fs::read_to_string(temp_dir.path().join("active/secrets.toml")).unwrap();

    assert!(service_toml.contains("MPGS Configured Service"));
    assert!(service_toml.contains("[service_identity]"));
    assert!(service_toml.contains("[service_connection]"));
    assert!(service_toml.contains("public_base_url = \"https://mpgs.example.test\""));
    assert!(service_toml.contains("[public_cors]"));
    assert!(service_toml.contains("allow_any_origin = true"));
    assert!(service_toml.contains("[deployment]"));
    assert!(service_toml.contains("restart_policy = \"compose:unless-stopped\""));
    assert!(secrets_toml.contains("postgres://mpgs:secret@postgres:5432/mpgs"));
    assert!(secrets_toml.contains("fake-steam-key"));
    assert!(!secrets_toml.contains("new-admin-token"));
    assert!(secrets_toml.contains("token_hash = \"sha256:"));
    let instance_id = service_toml
        .lines()
        .find_map(|line| line.strip_prefix("instance_id = \""))
        .and_then(|value| value.strip_suffix('"'))
        .unwrap();
    assert!(!secrets_toml.contains(&hash_token(&format!("new-admin-token:{instance_id}"))));
    assert!(secrets_toml.contains("session_secret = \""));

    let config = ServerConfig::from_config_dir(temp_dir.path()).unwrap();
    assert_eq!(config.service_info.service_name, "MPGS Configured Service");
    assert!(config
        .admin_auth
        .as_ref()
        .unwrap()
        .verify_token("new-admin-token"));
}

#[tokio::test]
async fn setup_token_does_not_grant_normal_admin_session_after_setup() {
    let temp_dir = tempfile::tempdir().unwrap();
    let _ = request_json(
        setup_app(temp_dir.path()),
        Method::POST,
        "/api/v1/setup/complete",
        setup_payload("correct-setup-token", "new-admin-token"),
    )
    .await;

    let config = ServerConfig::from_config_dir(temp_dir.path()).unwrap();
    let admin_app = build_router_with_state(AppState::new_with_admin_auth(
        config.service_info.service_info(),
        DatabaseHealth::HealthyForTest,
        config.admin_auth.unwrap(),
    ));

    let (status, value, headers) = request_json(
        admin_app,
        Method::POST,
        "/api/v1/admin/session",
        json!({ "token": "correct-setup-token" }),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(value["error"]["code"], "admin_token_invalid");
    assert!(headers.get(header::SET_COOKIE).is_none());
}

#[tokio::test]
async fn setup_complete_is_disabled_after_active_config_exists() {
    let temp_dir = tempfile::tempdir().unwrap();
    let app = setup_app(temp_dir.path());
    let _ = request_json(
        app.clone(),
        Method::POST,
        "/api/v1/setup/complete",
        setup_payload("correct-setup-token", "new-admin-token"),
    )
    .await;

    let (status, value, _headers) = request_json(
        app,
        Method::POST,
        "/api/v1/setup/complete",
        setup_payload("correct-setup-token", "second-admin-token"),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(value["error"]["code"], "setup_already_configured");
}

#[tokio::test]
async fn setup_status_reports_configured_after_active_files_exist() {
    let temp_dir = tempfile::tempdir().unwrap();
    let app = setup_app(temp_dir.path());

    let (initial_status, initial_value, _) =
        request_json(app.clone(), Method::GET, "/api/v1/setup/status", json!({})).await;
    assert_eq!(initial_status, StatusCode::OK);
    assert_eq!(initial_value["configured"], false);

    let _ = request_json(
        app.clone(),
        Method::POST,
        "/api/v1/setup/complete",
        setup_payload("correct-setup-token", "new-admin-token"),
    )
    .await;

    let (final_status, final_value, _) =
        request_json(app, Method::GET, "/api/v1/setup/status", json!({})).await;
    assert_eq!(final_status, StatusCode::OK);
    assert_eq!(final_value["configured"], true);
}

#[tokio::test]
async fn setup_routes_are_rate_limited() {
    let temp_dir = tempfile::tempdir().unwrap();
    let app = rate_limited_setup_app(temp_dir.path());

    let (first_status, _first_value, _) =
        request_json(app.clone(), Method::GET, "/api/v1/setup/status", json!({})).await;
    let (second_status, second_value, _) =
        request_json(app, Method::GET, "/api/v1/setup/status", json!({})).await;

    assert_eq!(first_status, StatusCode::OK);
    assert_eq!(second_status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(second_value["error"]["code"], "rate_limited");
}
