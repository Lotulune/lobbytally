use std::fs;
use std::path::Path;

use mpgs_server::db;

#[test]
fn initial_migration_creates_public_catalog_and_ops_boundaries() {
    let migration_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("migrations")
        .join("0001_public_catalog_ops.sql");
    let sql = fs::read_to_string(&migration_path).expect("initial migration should exist");

    assert!(sql.contains("CREATE SCHEMA IF NOT EXISTS public_catalog"));
    assert!(sql.contains("CREATE SCHEMA IF NOT EXISTS ops"));
    assert!(sql.contains("CREATE TABLE IF NOT EXISTS public_catalog.games"));
    assert!(sql.contains("CREATE TABLE IF NOT EXISTS public_catalog.game_analysis"));
    assert!(sql.contains("CREATE TABLE IF NOT EXISTS public_catalog.public_catalog_state"));
    assert!(sql.contains("CREATE TABLE IF NOT EXISTS ops.service_config_state"));
    assert!(sql.contains("pending_config_version TEXT"));
    assert!(sql.contains("restart_required BOOLEAN NOT NULL DEFAULT FALSE"));
    assert!(!sql.to_lowercase().contains("steam_key"));
    assert!(!sql.to_lowercase().contains("llm_key"));
    assert!(!sql.to_lowercase().contains("admin_token"));
    assert!(!sql.to_lowercase().contains("token_hash"));
    assert!(!sql.to_lowercase().contains("api_key"));
}

#[test]
fn initial_migration_is_embedded_for_binary_startup() {
    let migrator = db::migrator();
    let migrations: Vec<_> = migrator.iter().collect();

    assert_eq!(migrations.len(), 2);
    assert_eq!(migrations[0].version, 1);
    assert_eq!(migrations[0].description, "public_catalog_ops");
    assert!(migrations[0]
        .sql
        .contains("CREATE SCHEMA IF NOT EXISTS public_catalog"));
    assert_eq!(migrations[1].version, 2);
    assert_eq!(migrations[1].description, "ops_audit_events");
    assert!(migrations[1]
        .sql
        .contains("CREATE TABLE IF NOT EXISTS ops.audit_events"));
}

#[test]
fn database_health_checks_the_sqlx_migration_record() {
    let source_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("db.rs");
    let source = fs::read_to_string(&source_path).expect("db source should exist");

    assert!(source.contains("FROM _sqlx_migrations"));
    assert!(source.contains("description = 'public_catalog_ops'"));
    assert!(source.contains("description = 'ops_audit_events'"));
    assert!(source.contains("success = TRUE"));
}

#[test]
fn public_catalog_status_counts_only_anonymous_visible_public_games() {
    let source_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("db.rs");
    let source = fs::read_to_string(&source_path).expect("db source should exist");

    let status_fn = source
        .split("pub async fn public_catalog_status")
        .nth(1)
        .and_then(|tail| tail.split("pub async fn public_catalog_revision").next())
        .expect("public catalog status function should exist");

    assert!(status_fn.contains("review_status = 'accepted'"));
    assert!(status_fn.contains("visibility = 'public'"));
}

#[test]
fn audit_migration_creates_ops_audit_events_without_secret_columns() {
    let migration_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("migrations")
        .join("0002_ops_audit_events.sql");
    let sql = fs::read_to_string(&migration_path).expect("audit migration should exist");

    assert!(sql.contains("CREATE TABLE IF NOT EXISTS ops.audit_events"));
    assert!(sql.contains("event_type TEXT NOT NULL"));
    assert!(sql.contains("actor TEXT NOT NULL DEFAULT 'system'"));
    assert!(sql.contains("outcome TEXT NOT NULL"));
    assert!(sql.contains("detail_json JSONB NOT NULL DEFAULT '{}'::jsonb"));
    assert!(!sql.to_lowercase().contains("token"));
    assert!(!sql.to_lowercase().contains("api_key"));
    assert!(!sql.to_lowercase().contains("secret"));
}
