use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use mpgs_server::public_catalog::{AdminReviewFixture, PublicGameDetail, PublicGameListItem};
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
async fn admin_tasks_require_session_cookie() {
    let (status, value) = get_json("/api/v1/admin/tasks", None).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(value["error"]["code"], "admin_session_required");
}

#[tokio::test]
async fn admin_tasks_list_recent_tasks_and_sanitized_failures() {
    let app = build_router_with_state(
        admin_state()
            .with_audit_sink(AuditSink::memory())
            .with_admin_task_fixture(),
    );
    let cookie = admin_cookie_for(app.clone()).await;

    let response = request_json_from(
        app,
        Method::GET,
        "/api/v1/admin/tasks",
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
    assert_eq!(
        value["recentTasks"][0]["taskType"],
        "manual_appid_discovery"
    );
    assert_eq!(value["recentTasks"][0]["status"], "failed");
    assert_eq!(value["failureSummary"]["openFailureCount"], 1);
    assert_eq!(value["failureSummary"]["retryableFailureCount"], 1);
    assert_eq!(value["failures"][0]["reason"], "Steam lookup timed out.");
    assert!(value["failures"][0].get("apiKey").is_none());
    assert!(value["failures"][0].get("secret").is_none());
    assert!(value["failures"][0].get("requestJson").is_none());
    assert!(value["failures"][0].get("responseJson").is_none());
}

#[tokio::test]
async fn admin_create_task_queues_manual_appid_discovery_and_records_audit() {
    let audit = AuditSink::memory();
    let app = build_router_with_state(
        admin_state()
            .with_audit_sink(audit.clone())
            .with_admin_task_fixture(),
    );
    let cookie = admin_cookie_for(app.clone()).await;

    let response = request_json_from(
        app,
        Method::POST,
        "/api/v1/admin/tasks",
        json!({
            "taskType": "manual_appid_discovery",
            "appid": 730
        }),
        Some(&cookie),
    )
    .await;
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(value["task"]["taskType"], "manual_appid_discovery");
    assert_eq!(value["task"]["status"], "queued");
    assert_eq!(value["task"]["targetAppid"], 730);
    assert!(audit.records_for_test().iter().any(|record| {
        record.event_type == "admin.task.manual_appid_discovery.created"
            && record.outcome == "success"
    }));
}

#[tokio::test]
async fn admin_create_task_requires_appid_for_manual_discovery() {
    let app = build_router_with_state(admin_state().with_admin_task_fixture());
    let cookie = admin_cookie_for(app.clone()).await;

    let response = request_json_from(
        app,
        Method::POST,
        "/api/v1/admin/tasks",
        json!({ "taskType": "manual_appid_discovery" }),
        Some(&cookie),
    )
    .await;
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["error"]["code"], "admin_task_appid_required");
}

#[tokio::test]
async fn admin_review_queue_requires_session_cookie() {
    let (status, value) = get_json("/api/v1/admin/review-queue", None).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(value["error"]["code"], "admin_session_required");
}

#[tokio::test]
async fn admin_review_queue_lists_candidates_hidden_from_public_reads() {
    let app = build_router_with_state(AppState::new_with_admin_auth(
        test_config().service_info(),
        DatabaseHealth::ReviewQueueFixture {
            candidates: vec![AdminReviewFixture {
                appid: 440,
                name: "Team Fortress 2".to_string(),
                review_status: "needs_review".to_string(),
                visibility: "hidden".to_string(),
                recommendation_score: Some(86.0),
                updated_at: "2026-06-08 04:00:00+00".to_string(),
                review_note: Some("Needs moderator confirmation.".to_string()),
            }],
        },
        AdminAuthConfig::for_test_token("correct-admin-token"),
    ));
    let cookie = admin_cookie_for(app.clone()).await;

    let response = request_json_from(
        app,
        Method::GET,
        "/api/v1/admin/review-queue",
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
    assert_eq!(value["items"][0]["appid"], 440);
    assert_eq!(value["items"][0]["name"], "Team Fortress 2");
    assert_eq!(value["items"][0]["reviewStatus"], "needs_review");
    assert_eq!(value["items"][0]["visibility"], "hidden");
    assert_eq!(
        value["items"][0]["reviewNote"],
        "Needs moderator confirmation."
    );
}

#[tokio::test]
async fn admin_review_action_accepts_and_publicizes_candidate() {
    let audit = AuditSink::memory();
    let app = build_router_with_state(
        admin_state()
            .with_audit_sink(audit.clone())
            .with_review_action_fixture(),
    );
    let cookie = admin_cookie_for(app.clone()).await;

    let response = request_json_from(
        app,
        Method::POST,
        "/api/v1/admin/review-queue/440/action",
        json!({
            "action": "accept_public",
            "note": "Looks good."
        }),
        Some(&cookie),
    )
    .await;
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["game"]["appid"], 440);
    assert_eq!(value["game"]["reviewStatus"], "accepted");
    assert_eq!(value["game"]["visibility"], "public");
    assert_eq!(value["game"]["reviewNote"], "Looks good.");
    assert!(audit.records_for_test().iter().any(|record| {
        record.event_type == "admin.review.accept_public" && record.outcome == "success"
    }));
}

#[tokio::test]
async fn admin_review_action_maps_all_first_version_actions() {
    let cases = [
        (
            "accept_public",
            "accepted",
            "public",
            "admin.review.accept_public",
        ),
        (
            "accept_hidden",
            "accepted",
            "hidden",
            "admin.review.accept_hidden",
        ),
        ("reject", "rejected", "hidden", "admin.review.reject"),
        ("archive", "archived", "hidden", "admin.review.archive"),
    ];

    for (action, expected_status, expected_visibility, expected_audit_event) in cases {
        let audit = AuditSink::memory();
        let app = build_router_with_state(
            admin_state()
                .with_audit_sink(audit.clone())
                .with_review_action_fixture(),
        );
        let cookie = admin_cookie_for(app.clone()).await;

        let response = request_json_from(
            app,
            Method::POST,
            "/api/v1/admin/review-queue/440/action",
            json!({ "action": action }),
            Some(&cookie),
        )
        .await;
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["game"]["reviewStatus"], expected_status);
        assert_eq!(value["game"]["visibility"], expected_visibility);
        assert!(audit.records_for_test().iter().any(|record| {
            record.event_type == expected_audit_event && record.outcome == "success"
        }));
    }
}

#[tokio::test]
async fn admin_review_action_only_applies_to_pending_candidates() {
    let app = build_router_with_state(admin_state().with_review_action_fixture());
    let cookie = admin_cookie_for(app.clone()).await;

    let first_response = request_json_from(
        app.clone(),
        Method::POST,
        "/api/v1/admin/review-queue/440/action",
        json!({ "action": "accept_hidden" }),
        Some(&cookie),
    )
    .await;
    let second_response = request_json_from(
        app,
        Method::POST,
        "/api/v1/admin/review-queue/440/action",
        json!({ "action": "accept_public" }),
        Some(&cookie),
    )
    .await;
    let status = second_response.status();
    let body = axum::body::to_bytes(second_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(first_response.status(), StatusCode::OK);
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(value["error"]["code"], "admin_review_candidate_not_found");
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
