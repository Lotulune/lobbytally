use axum::body::Body;
use axum::http::{Request, StatusCode};
use mpgs_server::{build_router_with_state, AppState, ServiceInfoConfig};
use tower::ServiceExt;

fn test_config() -> ServiceInfoConfig {
    ServiceInfoConfig {
        service_instance_id: "018fb770-8998-7699-a6e4-b7b59f2f9c01".to_string(),
        service_name: "MPGS Safe Mode Test Service".to_string(),
        service_version: "0.1.0".to_string(),
    }
}

async fn get_json(uri: &str) -> (StatusCode, serde_json::Value) {
    let app = build_router_with_state(AppState::safe_mode(test_config()));
    let response = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value = serde_json::from_slice(&body).unwrap();

    (status, value)
}

#[tokio::test]
async fn safe_mode_keeps_healthz_public_and_minimal() {
    let (status, value) = get_json("/healthz").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(value, serde_json::json!({ "status": "ok" }));
}

#[tokio::test]
async fn safe_mode_service_info_reports_unavailable_public_catalog() {
    let (status, value) = get_json("/api/v1/service-info").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["serviceName"], "MPGS Safe Mode Test Service");
    assert_eq!(value["publicCatalogStatus"], "unavailable");
}

#[tokio::test]
async fn safe_mode_blocks_public_catalog_reads_with_sanitized_error() {
    for uri in [
        "/api/v1/discovery-home",
        "/api/v1/games",
        "/api/v1/games/730",
        "/api/v1/games/730/analysis",
    ] {
        let (status, value) = get_json(uri).await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(value["error"]["code"], "service_safe_mode");
        assert_eq!(value["error"]["message"], "服务处于安全修复模式。");
        assert!(value["error"]["details"].as_object().unwrap().is_empty());
    }
}
