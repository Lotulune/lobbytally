use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use mpgs_server::public_catalog::{PublicGameDetail, PublicGameListItem};
use mpgs_server::{
    build_router_with_state, AdminAuthConfig, AppState, AuditSink, DatabaseHealth, RateLimitConfig,
    RateLimiters, ServiceInfoConfig,
};
use serde_json::json;
use tower::ServiceExt;

fn test_config() -> ServiceInfoConfig {
    ServiceInfoConfig {
        service_instance_id: "018fb770-8998-7699-a6e4-b7b59f2f9c01".to_string(),
        service_name: "MPGS Admin Test Service".to_string(),
        service_version: "0.1.0".to_string(),
    }
}

fn admin_state() -> AppState {
    AppState::new_with_admin_auth(
        test_config().service_info(),
        DatabaseHealth::HealthyForTest,
        AdminAuthConfig::for_test_token("correct-admin-token"),
    )
}

fn admin_app() -> axum::Router {
    build_router_with_state(admin_state())
}

async fn request_json(
    method: Method,
    uri: &str,
    body: serde_json::Value,
    cookie: Option<&str>,
) -> axum::response::Response {
    request_json_from(admin_app(), method, uri, body, cookie).await
}

async fn request_json_from(
    app: axum::Router,
    method: Method,
    uri: &str,
    body: serde_json::Value,
    cookie: Option<&str>,
) -> axum::response::Response {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");

    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }

    app.oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}

async fn get_json(uri: &str, cookie: Option<&str>) -> (StatusCode, serde_json::Value) {
    let response = request_json(Method::GET, uri, json!({}), cookie).await;
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value = serde_json::from_slice(&body).unwrap();

    (status, value)
}

async fn admin_cookie_for(app: axum::Router) -> String {
    let response = request_json_from(
        app,
        Method::POST,
        "/api/v1/admin/session",
        json!({ "token": "correct-admin-token" }),
        None,
    )
    .await;

    response
        .headers()
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
async fn admin_overview_requires_session_cookie() {
    let (status, value) = get_json("/api/v1/admin/overview", None).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(value["error"]["code"], "admin_session_required");
}

#[tokio::test]
async fn admin_session_rejects_wrong_token() {
    let response = request_json(
        Method::POST,
        "/api/v1/admin/session",
        json!({ "token": "wrong-token" }),
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(response.headers().get(header::SET_COOKIE).is_none());
}

#[tokio::test]
async fn admin_session_sets_http_only_cookie_for_valid_token() {
    let response = request_json(
        Method::POST,
        "/api/v1/admin/session",
        json!({ "token": "correct-admin-token" }),
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .expect("admin login should set a session cookie")
        .to_str()
        .unwrap();

    assert!(cookie.contains("mpgs_admin_session="));
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Strict"));
}

#[tokio::test]
async fn admin_session_records_login_audit_events() {
    let audit = AuditSink::memory();
    let app = build_router_with_state(admin_state().with_audit_sink(audit.clone()));

    let bad_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/admin/session")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "token": "wrong-token" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad_response.status(), StatusCode::UNAUTHORIZED);

    let good_response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/admin/session")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "token": "correct-admin-token" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(good_response.status(), StatusCode::OK);

    let records = audit.records_for_test();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].event_type, "admin.session.login");
    assert_eq!(records[0].outcome, "failure");
    assert_eq!(records[1].event_type, "admin.session.login");
    assert_eq!(records[1].outcome, "success");
}

#[tokio::test]
async fn admin_session_cookie_allows_overview_and_diagnostics() {
    let login_response = request_json(
        Method::POST,
        "/api/v1/admin/session",
        json!({ "token": "correct-admin-token" }),
        None,
    )
    .await;
    let cookie = login_response
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    let (overview_status, overview) = get_json("/api/v1/admin/overview", Some(&cookie)).await;
    let (diagnostics_status, diagnostics) =
        get_json("/api/v1/admin/diagnostics", Some(&cookie)).await;

    assert_eq!(overview_status, StatusCode::OK);
    assert_eq!(overview["serviceName"], "MPGS Admin Test Service");
    assert_eq!(overview["publicCatalogStatus"], "empty");
    assert_eq!(overview["publicGameCount"], 0);
    assert_eq!(overview["pendingReviewCount"], 0);
    assert_eq!(overview["restartRequired"], false);
    assert_eq!(overview["connectionShareConfigured"], false);
    assert!(overview["latestAuditEvent"].is_null());
    assert_eq!(diagnostics_status, StatusCode::OK);
    assert_eq!(diagnostics["postgres"], "ok");
    assert_eq!(diagnostics["publicBaseUrlStatus"], "missing");
    assert_eq!(diagnostics["httpsStatus"], "unknown");
    assert_eq!(diagnostics["publicCors"], "disabled");
    assert_eq!(diagnostics["restartPolicy"], "external_required");
    assert!(diagnostics.get("adminToken").is_none());
}

#[tokio::test]
async fn admin_overview_reports_public_catalog_fixture_counts() {
    let game = PublicGameListItem {
        appid: 730,
        name: "Counter-Strike 2".to_string(),
        recommendation_score: Some(91.5),
        updated_at: "2026-06-08 03:00:00+00".to_string(),
    };
    let app = build_router_with_state(AppState::new_with_admin_auth(
        test_config()
            .service_info_with_catalog_status(mpgs_core::models::PublicCatalogStatus::Ready),
        DatabaseHealth::PublicCatalogFixture {
            revision: 7,
            detail: Some(PublicGameDetail { game }),
            analysis: None,
        },
        AdminAuthConfig::for_test_token("correct-admin-token"),
    ));
    let cookie = admin_cookie_for(app.clone()).await;

    let response = request_json_from(
        app,
        Method::GET,
        "/api/v1/admin/overview",
        json!({}),
        Some(&cookie),
    )
    .await;
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let overview: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(overview["publicCatalogStatus"], "ready");
    assert_eq!(overview["publicGameCount"], 1);
    assert_eq!(overview["pendingReviewCount"], 0);
    assert_eq!(overview["restartRequired"], false);
    assert_eq!(overview["connectionShareConfigured"], false);
    assert!(overview["latestAuditEvent"].is_null());
}

#[tokio::test]
async fn admin_overview_reports_latest_audit_event_without_secret_values() {
    let audit = AuditSink::memory();
    let app = build_router_with_state(admin_state().with_audit_sink(audit));
    let cookie = admin_cookie_for(app.clone()).await;

    let response = request_json_from(
        app,
        Method::GET,
        "/api/v1/admin/overview",
        json!({}),
        Some(&cookie),
    )
    .await;
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let overview: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        overview["latestAuditEvent"]["eventType"],
        "admin.session.login"
    );
    assert_eq!(overview["latestAuditEvent"]["actor"], "admin");
    assert_eq!(overview["latestAuditEvent"]["outcome"], "success");
    assert!(overview["latestAuditEvent"].get("token").is_none());
    assert!(overview.get("adminToken").is_none());
}

#[tokio::test]
async fn admin_audit_events_requires_session_cookie() {
    let (status, value) = get_json("/api/v1/admin/audit-events", None).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(value["error"]["code"], "admin_session_required");
}

#[tokio::test]
async fn admin_audit_events_lists_recent_records_without_secret_values() {
    let audit = AuditSink::memory();
    audit.record_for_test("admin.config.pending_service_identity", "admin", "success");
    let app = build_router_with_state(admin_state().with_audit_sink(audit));
    let cookie = admin_cookie_for(app.clone()).await;

    let response = request_json_from(
        app,
        Method::GET,
        "/api/v1/admin/audit-events",
        json!({}),
        Some(&cookie),
    )
    .await;
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["events"][0]["eventType"], "admin.session.login");
    assert_eq!(
        value["events"][1]["eventType"],
        "admin.config.pending_service_identity"
    );
    assert_eq!(value["events"][0]["actor"], "admin");
    assert_eq!(value["events"][0]["outcome"], "success");
    assert!(value["events"][0].get("token").is_none());
    assert!(value["events"][0].get("adminToken").is_none());
    assert!(value["events"][0].get("secret").is_none());
}

#[tokio::test]
async fn admin_routes_are_rate_limited() {
    let app = build_router_with_state(
        admin_state().with_rate_limits(RateLimiters::new(RateLimitConfig::for_tests(1))),
    );

    let first_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/admin/session")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "token": "wrong-token" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let second_response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/admin/session")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "token": "wrong-token" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(first_response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(second_response.status(), StatusCode::TOO_MANY_REQUESTS);
}
