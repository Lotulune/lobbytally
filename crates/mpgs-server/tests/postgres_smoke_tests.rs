use mpgs_core::models::PublicCatalogStatus;
use mpgs_server::admin::AdminReviewAction;
use mpgs_server::admin::AdminTaskKind;
use mpgs_server::{
    build_router_with_state, db, AppState, ConfigHealth, DatabaseHealth, ServiceInfoConfig,
};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use std::fs;
use tower::ServiceExt;

fn test_config() -> ServiceInfoConfig {
    ServiceInfoConfig {
        service_instance_id: "018fb770-8998-7699-a6e4-b7b59f2f9c01".to_string(),
        service_name: "MPGS Postgres Smoke Test Service".to_string(),
        service_version: "0.1.0".to_string(),
    }
}

#[tokio::test]
async fn migrates_empty_postgres_database_when_test_database_is_configured() {
    let Ok(database_url) = std::env::var("MPGS_TEST_DATABASE_URL") else {
        return;
    };

    let pool = db::connect_and_migrate(&database_url)
        .await
        .expect("connect to Postgres and run migrations");
    let status = db::public_catalog_status(&pool)
        .await
        .expect("read public catalog status");

    assert_eq!(status, PublicCatalogStatus::Empty);
}

#[tokio::test]
async fn healthz_is_ok_after_postgres_migration_and_active_config_are_ready() {
    let Ok(database_url) = std::env::var("MPGS_TEST_DATABASE_URL") else {
        return;
    };

    let temp_dir = tempfile::tempdir().unwrap();
    let active_dir = temp_dir.path().join("active");
    fs::create_dir(&active_dir).unwrap();
    let service_path = active_dir.join("service.toml");
    let secrets_path = active_dir.join("secrets.toml");
    fs::write(
        &service_path,
        r#"
bind_addr = "127.0.0.1:4310"

[service_identity]
instance_id = "018fb770-8998-7699-a6e4-b7b59f2f9c01"
name = "MPGS Postgres Smoke Test Service"
version = "0.1.0"
"#,
    )
    .unwrap();
    fs::write(
        &secrets_path,
        format!(
            r#"
[database]
url = "{}"
"#,
            database_url
        ),
    )
    .unwrap();

    let pool = db::connect_and_migrate(&database_url)
        .await
        .expect("connect to Postgres and run migrations");
    let app = build_router_with_state(AppState::new_with_config_health(
        test_config().service_info(),
        DatabaseHealth::Pool(pool),
        ConfigHealth::active_files(service_path, secrets_path),
    ));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn records_pending_config_state_in_ops_schema() {
    let Ok(database_url) = std::env::var("MPGS_TEST_DATABASE_URL") else {
        return;
    };

    let pool = db::connect_and_migrate(&database_url)
        .await
        .expect("connect to Postgres and run migrations");

    db::record_active_config_startup(&pool, "sha256:active-test")
        .await
        .expect("record active config startup state");
    db::mark_pending_config(&pool, "sha256:pending-test")
        .await
        .expect("record pending config state");

    let state = db::service_config_state(&pool)
        .await
        .expect("read service config state");

    assert_eq!(
        state.active_config_version.as_deref(),
        Some("sha256:active-test")
    );
    assert_eq!(
        state.pending_config_version.as_deref(),
        Some("sha256:pending-test")
    );
    assert!(state.restart_required);
    assert_eq!(state.last_startup_status, "ok");
}

#[tokio::test]
async fn records_audit_events_in_ops_schema() {
    let Ok(database_url) = std::env::var("MPGS_TEST_DATABASE_URL") else {
        return;
    };

    let pool = db::connect_and_migrate(&database_url)
        .await
        .expect("connect to Postgres and run migrations");

    db::record_audit_event(&pool, "admin.restart.requested", "admin", "success")
        .await
        .expect("record audit event");

    let event = db::latest_audit_event(&pool)
        .await
        .expect("read latest audit event")
        .expect("audit event should exist");

    assert_eq!(event.event_type, "admin.restart.requested");
    assert_eq!(event.actor, "admin");
    assert_eq!(event.outcome, "success");
}

#[tokio::test]
async fn public_review_action_advances_public_catalog_revision() {
    let Ok(database_url) = std::env::var("MPGS_TEST_DATABASE_URL") else {
        return;
    };

    let pool = db::connect_and_migrate(&database_url)
        .await
        .expect("connect to Postgres and run migrations");

    sqlx_core::query::query::<sqlx_postgres::Postgres>(
        r#"
        INSERT INTO public_catalog.games (
            appid,
            name,
            review_status,
            visibility,
            recommendation_score
        )
        VALUES ($1, 'Review Smoke Candidate', 'needs_review', 'hidden', 87.0)
        ON CONFLICT (appid) DO UPDATE
        SET review_status = 'needs_review',
            visibility = 'hidden',
            recommendation_score = 87.0,
            review_note = NULL,
            updated_at = now()
        "#,
    )
    .bind(901_337_i32)
    .execute(&pool)
    .await
    .expect("insert review candidate");

    let before = db::public_catalog_revision(&pool)
        .await
        .expect("read revision before review action");
    let reviewed = db::apply_admin_review_action(
        &pool,
        901_337,
        AdminReviewAction::AcceptPublic,
        Some("smoke approved"),
    )
    .await
    .expect("apply public review action")
    .expect("review candidate should be updated");
    let after = db::public_catalog_revision(&pool)
        .await
        .expect("read revision after review action");

    assert_eq!(reviewed.review_status, "accepted");
    assert_eq!(reviewed.visibility, "public");
    assert_eq!(reviewed.review_note.as_deref(), Some("smoke approved"));
    assert_eq!(after, before + 1);
}

#[tokio::test]
async fn admin_task_controls_use_ops_schema_in_postgres() {
    let Ok(database_url) = std::env::var("MPGS_TEST_DATABASE_URL") else {
        return;
    };

    let pool = db::connect_and_migrate(&database_url)
        .await
        .expect("connect to Postgres and run migrations");

    let task = db::create_admin_task(&pool, AdminTaskKind::ManualAppidDiscovery, Some(730))
        .await
        .expect("create manual appid task");

    sqlx_core::query::query::<sqlx_postgres::Postgres>(
        r#"
        INSERT INTO ops.task_failures (
            task_id,
            stage,
            target,
            provider,
            retryable,
            attempt,
            reason
        )
        VALUES ($1, 'steam_lookup', 'appid:730', 'steam', TRUE, 1, 'Steam lookup timed out.')
        "#,
    )
    .bind(task.id)
    .execute(&pool)
    .await
    .expect("insert sanitized task failure");

    let state = db::admin_task_control_state(&pool)
        .await
        .expect("read admin task control state");

    assert!(state
        .recent_tasks
        .iter()
        .any(|item| item.id == task.id && item.target_appid == Some(730)));
    assert!(state.failure_summary.open_failure_count >= 1);
    assert!(state.failure_summary.retryable_failure_count >= 1);
    assert!(state.failures.iter().any(|failure| {
        failure.task_id == Some(task.id)
            && failure.stage == "steam_lookup"
            && failure.reason == "Steam lookup timed out."
    }));
}
