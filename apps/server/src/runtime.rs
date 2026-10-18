//! Request-bounded Vercel runtime: HTTP-triggered work, no process lifetime jobs.

use std::{sync::Arc, time::Duration};

use axum::{
    Json,
    body::Body,
    extract::State,
    http::{HeaderMap, Method, Request, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};

use crate::api::AppState;

pub async fn request_deadline(request: Request<Body>, next: Next) -> Response {
    // A bounded worker may use 50 seconds plus brief database release overhead.
    // The platform's configured maximum is larger than this application budget.
    match tokio::time::timeout(Duration::from_secs(60), next.run(request)).await {
        Ok(response) => response,
        Err(_) => failure(
            StatusCode::GATEWAY_TIMEOUT,
            "request_timeout",
            "request deadline exceeded",
        ),
    }
}

fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(), Box<Response>> {
    let Some(expected) = state.cron_secret.as_deref() else {
        return Err(Box::new(failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "maintenance_unavailable",
            "maintenance is not configured",
        )));
    };
    let supplied = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");
    // Compare fixed-size digests so a caller cannot learn a prefix of the secret.
    let expected = Sha256::digest(expected.as_bytes());
    let supplied = Sha256::digest(supplied.as_bytes());
    let mismatch = expected
        .iter()
        .zip(supplied.iter())
        .fold(0_u8, |diff, (left, right)| diff | (left ^ right));
    if mismatch != 0 {
        return Err(Box::new(failure(
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
            "invalid maintenance credentials",
        )));
    }
    Ok(())
}

pub async fn maintenance(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(response) = authorize(&state, &headers) {
        return *response;
    }
    let Some(repo) = state.repo.clone() else {
        return failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "database is not configured",
        );
    };
    let owner = invocation_owner();
    let options = match mpgs_worker::BatchOptions::from_env(owner) {
        Ok(options) => options,
        Err(error) => {
            tracing::error!(%error, "maintenance configuration is invalid");
            return failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "maintenance_unavailable",
                "maintenance configuration is invalid",
            );
        }
    };
    match tokio::task::spawn_blocking(move || mpgs_worker::run_batch(&repo, &options)).await {
        Ok(Ok(stats)) => Json(json!({"status": "ok", "batch": stats})).into_response(),
        Ok(Err(error)) => {
            tracing::warn!(%error, "maintenance batch failed");
            failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "maintenance_failed",
                "maintenance batch failed; inspect server logs",
            )
        }
        Err(error) => {
            tracing::error!(%error, "maintenance batch worker failed");
            failure(
                StatusCode::INTERNAL_SERVER_ERROR,
                "maintenance_failed",
                "maintenance worker failed",
            )
        }
    }
}

pub async fn avatar_cleanup(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(response) = authorize(&state, &headers) {
        return *response;
    }
    match tokio::time::timeout(Duration::from_secs(45), avatar_cleanup_batch(state)).await {
        Ok(response) => response,
        Err(_) => failure(
            StatusCode::GATEWAY_TIMEOUT,
            "cleanup_timeout",
            "avatar cleanup deadline reached; unfinished leases will be retried",
        ),
    }
}

async fn avatar_cleanup_batch(state: Arc<AppState>) -> Response {
    let Some(repo) = state.repo.clone() else {
        return failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "database is not configured",
        );
    };
    let owner = invocation_owner();
    let lease_repo = repo.clone();
    let lease_owner = owner.clone();
    let jobs = match tokio::task::spawn_blocking(move || {
        lease_repo.lease_jobs(&lease_owner, 2, 60_000, Some("avatar_cleanup"))
    })
    .await
    {
        Ok(Ok(jobs)) => jobs,
        Ok(Err(error)) => {
            tracing::warn!(%error, "avatar cleanup could not lease work");
            return failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "storage_unavailable",
                "avatar cleanup could not lease work",
            );
        }
        Err(error) => {
            tracing::error!(%error, "avatar cleanup lease task failed");
            return failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "storage_unavailable",
                "avatar cleanup could not lease work",
            );
        }
    };
    let mut removed = 0;
    let mut retained = 0;
    let mut failed = 0;
    for job in jobs {
        let job_id = job.job_id;
        let lookup_repo = repo.clone();
        let key = job.entity_key.clone();
        let referenced =
            match tokio::task::spawn_blocking(move || lookup_repo.avatar_storage_key_in_use(&key))
                .await
            {
                Ok(Ok(referenced)) => referenced,
                Ok(Err(error)) => {
                    tracing::warn!(job_id, %error, "avatar cleanup reference lookup failed");
                    failed += 1;
                    // The expired job lease is recovered by the next scheduled batch.
                    continue;
                }
                Err(error) => {
                    tracing::error!(job_id, %error, "avatar cleanup reference lookup task failed");
                    failed += 1;
                    continue;
                }
            };
        let deletion = if referenced {
            Ok(())
        } else {
            state.avatar_storage.delete(&job.entity_key).await
        };
        let finish_repo = repo.clone();
        let finish_owner = owner.clone();
        if let Err(error) = &deletion {
            tracing::warn!(job_id = job.job_id, %error, "avatar cleanup delete failed");
            failed += 1;
        } else if referenced {
            retained += 1;
        } else {
            removed += 1;
        }
        let finish = tokio::task::spawn_blocking(move || {
            if deletion.is_ok() {
                finish_repo.complete_job(job_id, &finish_owner, &format!("avatar-cleanup-{job_id}"))
            } else {
                finish_repo.fail_job(job_id, &finish_owner, "network", 60_000)
            }
        })
        .await;
        match finish {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                tracing::warn!(job_id, %error, "avatar cleanup job result could not be recorded");
                failed += 1;
            }
            Err(error) => {
                tracing::error!(job_id, %error, "avatar cleanup job result task failed");
                failed += 1;
            }
        }
    }
    let status = if failed == 0 {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(json!({"removed": removed, "retained": retained, "failed": failed})),
    )
        .into_response()
}

pub async fn web_fallback(State(state): State<Arc<AppState>>, request: Request<Body>) -> Response {
    let Some(directory) = state.web_dir.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let path = request.uri().path();
    if !matches!(*request.method(), Method::GET | Method::HEAD)
        || ["/v1", "/admin", "/internal", "/health", "/.well-known"]
            .iter()
            .any(|prefix| path == *prefix || path.starts_with(&format!("{prefix}/")))
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let accepts_html = !path.rsplit('/').next().unwrap_or("").contains('.')
        && request
            .headers()
            .get(header::ACCEPT)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|accept| {
                accept
                    .split(',')
                    .any(|value| value.trim().starts_with("text/html"))
            });
    let service = ServeDir::new(directory).append_index_html_on_directories(true);
    let response = if accepts_html {
        service
            .fallback(ServeFile::new(directory.join("index.html")))
            .oneshot(request)
            .await
    } else {
        service.oneshot(request).await
    };
    match response {
        Ok(response) => response.map(Body::new),
        Err(error) => match error {},
    }
}

fn invocation_owner() -> String {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)
        .expect("operating-system randomness is required for invocation ownership");
    let nonce: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("vercel-{nonce}")
}

fn failure(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AppState {
        AppState {
            repo: None,
            admin_token: Some("admin-secret-is-separate".into()),
            avatar_storage: crate::avatars::AvatarStorage::memory(),
            cron_secret: Some("0123456789abcdef0123456789abcdef".into()),
            web_dir: None,
            rate_limits: crate::rate_limit::RateLimitConfig {
                enabled: false,
                ..Default::default()
            },
            cors: Default::default(),
            account_ai_limits: Default::default(),
            ai: mpgs_ai::AiGateway::disabled(),
            task_router: Arc::new(mpgs_ai::TaskRouter::from_provider(Arc::new(
                mpgs_ai::DisabledProvider,
            ))),
            embedding: Arc::new(mpgs_ai::HashEmbeddingProvider { dimensions: 64 }),
        }
    }

    #[test]
    fn maintenance_uses_its_own_exact_bearer_secret() {
        let state = state();
        let mut headers = HeaderMap::new();
        assert_eq!(
            authorize(&state, &headers).unwrap_err().status(),
            StatusCode::UNAUTHORIZED
        );
        headers.insert(
            header::AUTHORIZATION,
            "Bearer admin-secret-is-separate".parse().unwrap(),
        );
        assert!(authorize(&state, &headers).is_err());
        headers.insert(
            header::AUTHORIZATION,
            "Bearer 0123456789abcdef0123456789abcde".parse().unwrap(),
        );
        assert!(authorize(&state, &headers).is_err());
        headers.insert(
            header::AUTHORIZATION,
            "Bearer 0123456789abcdef0123456789abcdef".parse().unwrap(),
        );
        assert!(authorize(&state, &headers).is_ok());
    }

    #[tokio::test]
    async fn unknown_api_and_missing_assets_never_return_the_spa() {
        let directory = std::env::temp_dir().join(invocation_owner());
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("index.html"), "<html>LobbyTally</html>").unwrap();
        let mut state = state();
        state.web_dir = Some(directory.clone());
        let app = crate::api::build_router(state);
        for path in ["/v1/unknown", "/internal/unknown", "/assets/missing.js"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header(header::ACCEPT, "text/html")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/settings")
                    .header(header::ACCEPT, "text/html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(bytes.as_ref(), b"<html>LobbyTally</html>");
        std::fs::remove_file(directory.join("index.html")).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
