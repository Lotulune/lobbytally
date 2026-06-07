use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use mpgs_server::{
    build_router_with_state, AdminAuthConfig, AppState, DatabaseHealth, ServiceInfoConfig,
};
use serde_json::json;
use std::fs;
use std::path::Path;
use tower::ServiceExt;

fn test_config() -> ServiceInfoConfig {
    ServiceInfoConfig {
        service_instance_id: "018fb770-8998-7699-a6e4-b7b59f2f9c01".to_string(),
        service_name: "MPGS Config Management Test Service".to_string(),
        service_version: "0.1.0".to_string(),
    }
}

fn write_active_config(config_dir: &Path) {
    let active_dir = config_dir.join("active");
    fs::create_dir(&active_dir).unwrap();
    fs::write(
        active_dir.join("service.toml"),
        r#"
bind_addr = "0.0.0.0:4310"

[service_identity]
instance_id = "018fb770-8998-7699-a6e4-b7b59f2f9c01"
name = "MPGS Active Service"
version = "0.1.0"

[service_connection]
public_base_url = "https://mpgs.example.test"
"#,
    )
    .unwrap();
    fs::write(
        active_dir.join("secrets.toml"),
        r#"
[database]
url = "postgres://mpgs:secret@postgres:5432/mpgs"

[admin]
token_hash = "sha256:admin-hash"
session_secret = "admin-session-secret"

[steam]
api_key = "active-steam-key"
"#,
    )
    .unwrap();
}

fn config_management_app(config_dir: &Path) -> axum::Router {
    build_router_with_state(
        AppState::new_with_admin_auth(
            test_config().service_info(),
            DatabaseHealth::HealthyForTest,
            AdminAuthConfig::for_test_token("correct-admin-token"),
        )
        .with_config_file_manager(config_dir),
    )
}

async fn request_json(
    app: axum::Router,
    method: Method,
    uri: &str,
    body: serde_json::Value,
    cookie: Option<&str>,
) -> (StatusCode, serde_json::Value, axum::http::HeaderMap) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");

    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }

    let response = app
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
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

async fn admin_cookie(app: axum::Router) -> String {
    let (_status, _value, headers) = request_json(
        app,
        Method::POST,
        "/api/v1/admin/session",
        json!({ "token": "correct-admin-token" }),
        None,
    )
    .await;

    headers
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn admin_config_state_requires_session_cookie() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_active_config(temp_dir.path());
    let (status, value, _headers) = request_json(
        config_management_app(temp_dir.path()),
        Method::GET,
        "/api/v1/admin/config-state",
        json!({}),
        None,
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(value["error"]["code"], "admin_session_required");
}

#[tokio::test]
async fn admin_pending_service_identity_writes_pending_config_and_preserves_active_secrets() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_active_config(temp_dir.path());
    let app = config_management_app(temp_dir.path());
    let cookie = admin_cookie(app.clone()).await;

    let (status, value, _headers) = request_json(
        app.clone(),
        Method::POST,
        "/api/v1/admin/config/pending/service-identity",
        json!({ "serviceName": "MPGS Pending Service" }),
        Some(&cookie),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["restartRequired"], true);
    assert!(value["pendingConfigVersion"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));

    let pending_service = fs::read_to_string(temp_dir.path().join("pending/service.toml")).unwrap();
    let active_secrets = fs::read_to_string(temp_dir.path().join("active/secrets.toml")).unwrap();

    assert!(pending_service.contains("MPGS Pending Service"));
    assert!(pending_service.contains("018fb770-8998-7699-a6e4-b7b59f2f9c01"));
    assert!(pending_service.contains("[service_connection]"));
    assert!(pending_service.contains("public_base_url = \"https://mpgs.example.test\""));
    assert!(active_secrets.contains("active-steam-key"));
    assert!(!temp_dir.path().join("pending/secrets.toml").exists());

    let (state_status, state, _headers) = request_json(
        app,
        Method::GET,
        "/api/v1/admin/config-state",
        json!({}),
        Some(&cookie),
    )
    .await;

    assert_eq!(state_status, StatusCode::OK);
    assert_eq!(state["restartRequired"], true);
    assert_eq!(state["pendingConfigVersion"], value["pendingConfigVersion"]);
    assert!(state["activeConfigVersion"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    assert_eq!(state["lastStartupStatus"], "ok");
}

#[tokio::test]
async fn admin_connection_share_requires_session_cookie() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_active_config(temp_dir.path());

    let (status, value, _headers) = request_json(
        config_management_app(temp_dir.path()),
        Method::GET,
        "/api/v1/admin/connection-share",
        json!({}),
        None,
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(value["error"]["code"], "admin_session_required");
}

#[tokio::test]
async fn admin_connection_share_returns_keyless_service_connection_file() {
    let temp_dir = tempfile::tempdir().unwrap();
    write_active_config(temp_dir.path());
    let app = config_management_app(temp_dir.path());
    let cookie = admin_cookie(app.clone()).await;

    let (status, value, _headers) = request_json(
        app,
        Method::GET,
        "/api/v1/admin/connection-share",
        json!({}),
        Some(&cookie),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["serviceName"], "MPGS Active Service");
    assert_eq!(
        value["serviceInstanceId"],
        "018fb770-8998-7699-a6e4-b7b59f2f9c01"
    );
    assert_eq!(value["apiVersion"], "v1");
    assert_eq!(value["baseUrl"], "https://mpgs.example.test");
    assert_eq!(
        value["serviceInfoUrl"],
        "https://mpgs.example.test/api/v1/service-info"
    );
    assert_eq!(value["capabilities"][0], "public_catalog_read");
    assert!(value.get("adminToken").is_none());
    assert!(value.get("setupToken").is_none());
}
