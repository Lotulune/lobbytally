use axum::body::Body;
use axum::http::{Request, StatusCode};
use mpgs_server::{build_openapi, build_router, ServiceInfoConfig};
use tower::ServiceExt;

fn test_config() -> ServiceInfoConfig {
    ServiceInfoConfig {
        service_instance_id: "018fb770-8998-7699-a6e4-b7b59f2f9c01".to_string(),
        service_name: "MPGS Test Service".to_string(),
        service_version: "0.1.0".to_string(),
    }
}

#[tokio::test]
async fn service_info_returns_public_identity_payload() {
    let app = build_router(test_config());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/service-info")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(value["apiVersion"], "v1");
    assert_eq!(value["serviceName"], "MPGS Test Service");
    assert_eq!(value["capabilities"][0], "public_catalog_read");
}

#[test]
fn openapi_includes_service_info_route() {
    let value = serde_json::to_value(build_openapi()).unwrap();

    assert!(value["paths"]["/api/v1/service-info"]["get"].is_object());
    assert!(value["paths"]["/healthz"]["get"].is_object());
    assert!(value["paths"]["/api/v1/discovery-home"]["get"].is_object());
    assert!(value["paths"]["/api/v1/games"]["get"].is_object());
    assert!(value["paths"]["/api/v1/games/{appid}"]["get"].is_object());
    assert!(value["paths"]["/api/v1/games/{appid}/analysis"]["get"].is_object());
    assert!(value["paths"]["/api/v1/admin/session"]["post"].is_object());
    assert!(value["paths"]["/api/v1/admin/overview"]["get"].is_object());
    assert!(value["paths"]["/api/v1/admin/diagnostics"]["get"].is_object());
    assert!(value["paths"]["/api/v1/admin/tasks"]["get"].is_object());
    assert!(value["paths"]["/api/v1/admin/tasks"]["post"].is_object());
    assert!(value["paths"]["/api/v1/admin/config-state"]["get"].is_object());
    assert!(value["paths"]["/api/v1/admin/connection-share"]["get"].is_object());
    assert!(value["paths"]["/api/v1/admin/config/pending/service-identity"]["post"].is_object());
    assert!(value["paths"]["/api/v1/admin/restart"]["post"].is_object());
    assert!(value["paths"]["/api/v1/setup/status"]["get"].is_object());
    assert!(value["paths"]["/api/v1/setup/complete"]["post"].is_object());
}

#[test]
fn openapi_documents_public_read_conditional_cache_contract() {
    let value = serde_json::to_value(build_openapi()).unwrap();

    for path in [
        "/api/v1/discovery-home",
        "/api/v1/games",
        "/api/v1/games/{appid}",
        "/api/v1/games/{appid}/analysis",
    ] {
        let get = &value["paths"][path]["get"];
        let parameters = get["parameters"].as_array().unwrap();
        assert!(parameters
            .iter()
            .any(|parameter| parameter["name"] == "If-None-Match"));
        assert!(get["responses"]["304"].is_object());
    }
}
