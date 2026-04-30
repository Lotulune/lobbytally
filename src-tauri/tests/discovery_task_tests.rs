use rusqlite::{params, Connection};
use serde_json::json;
use tauri_app_lib::db;
use tauri_app_lib::models::{
    DiscoveryFailureItem, DiscoveryRunSnapshot, DiscoveryRunStatus, DiscoveryTaskRequest, SyncMode,
};

fn seeded_memory_db() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory db");
    db::migrate(&conn).expect("migrate");
    db::seed_default_games(&conn).expect("seed");
    conn
}

fn sample_snapshot(status: DiscoveryRunStatus, added_games: usize) -> DiscoveryRunSnapshot {
    DiscoveryRunSnapshot {
        id: 1,
        status,
        sync_mode: SyncMode::Full,
        target_added_games: 10,
        page_size: 25,
        pages_processed: 2,
        scanned_apps: 50,
        added_games,
        added_new_games: 2,
        added_classic_games: added_games.saturating_sub(2),
        skipped_existing: 20,
        skipped_non_multiplayer: 25,
        failed_games: 3,
        current_appid: Some(600_123),
        last_appid: Some(600_150),
        have_more_results: true,
        started_at: "2026-04-26T12:00:00Z".to_string(),
        updated_at: "2026-04-26T12:02:00Z".to_string(),
        finished_at: None,
        last_error: None,
        failures: Vec::<DiscoveryFailureItem>::new(),
    }
}

#[test]
fn discovery_run_snapshot_round_trips_with_failures() {
    let conn = seeded_memory_db();

    let run = db::create_discovery_run(
        &conn,
        &DiscoveryTaskRequest {
            sync_mode: SyncMode::Quick,
            target_added_games: 6,
            page_size: 25,
        },
        Some(321_000),
    )
    .expect("create run");

    db::update_discovery_run_progress(
        &conn,
        run.id,
        db::DiscoveryProgressPatch {
            status: Some(DiscoveryRunStatus::Running),
            current_appid: Some(Some(321_012)),
            last_appid: Some(Some(321_025)),
            pages_processed: Some(1),
            scanned_apps: Some(25),
            added_games: Some(4),
            added_new_games: Some(1),
            added_classic_games: Some(3),
            skipped_existing: Some(8),
            skipped_non_multiplayer: Some(11),
            failed_games: Some(2),
            have_more_results: Some(true),
            last_error: None,
            finished_at: None,
        },
    )
    .expect("update progress");

    db::append_discovery_failure(
        &conn,
        run.id,
        1,
        Some(321_017),
        "fetch_snapshot",
        "steam timeout",
    )
    .expect("append failure");

    let loaded = db::load_discovery_run(&conn, run.id)
        .expect("load run")
        .expect("run exists");

    assert_eq!(loaded.status, DiscoveryRunStatus::Running);
    assert_eq!(loaded.sync_mode, SyncMode::Quick);
    assert_eq!(loaded.target_added_games, 6);
    assert_eq!(loaded.page_size, 25);
    assert_eq!(loaded.current_appid, Some(321_012));
    assert_eq!(loaded.last_appid, Some(321_025));
    assert_eq!(loaded.pages_processed, 1);
    assert_eq!(loaded.scanned_apps, 25);
    assert_eq!(loaded.added_games, 4);
    assert_eq!(loaded.added_new_games, 1);
    assert_eq!(loaded.added_classic_games, 3);
    assert_eq!(loaded.skipped_existing, 8);
    assert_eq!(loaded.skipped_non_multiplayer, 11);
    assert_eq!(loaded.failed_games, 2);
    assert!(loaded.have_more_results);
    assert_eq!(loaded.failures.len(), 1);
    assert_eq!(loaded.failures[0].page_index, 1);
    assert_eq!(loaded.failures[0].stage, "fetch_snapshot");
    assert_eq!(loaded.failures[0].appid, Some(321_017));
    assert_eq!(loaded.failures[0].reason, "steam timeout");
}

#[test]
fn mark_running_discovery_runs_interrupted_on_startup() {
    let conn = seeded_memory_db();

    let run = db::create_discovery_run(
        &conn,
        &DiscoveryTaskRequest {
            sync_mode: SyncMode::Full,
            target_added_games: 10,
            page_size: 30,
        },
        Some(555_000),
    )
    .expect("create run");

    db::update_discovery_run_progress(
        &conn,
        run.id,
        db::DiscoveryProgressPatch {
            status: Some(DiscoveryRunStatus::Running),
            current_appid: Some(Some(555_012)),
            last_appid: Some(Some(555_030)),
            pages_processed: Some(2),
            scanned_apps: Some(60),
            added_games: Some(5),
            added_new_games: Some(2),
            added_classic_games: Some(3),
            skipped_existing: Some(20),
            skipped_non_multiplayer: Some(33),
            failed_games: Some(2),
            have_more_results: Some(true),
            last_error: None,
            finished_at: None,
        },
    )
    .expect("mark running");

    db::mark_running_discovery_runs_interrupted(&conn).expect("interrupt stale runs");

    let interrupted = db::load_discovery_run(&conn, run.id)
        .expect("load interrupted")
        .expect("run exists");

    assert_eq!(interrupted.status, DiscoveryRunStatus::Interrupted);
    assert_eq!(interrupted.added_games, 5);
    assert_eq!(interrupted.last_appid, Some(555_030));
}

#[test]
fn discovery_progress_patch_can_clear_nullable_fields() {
    let conn = seeded_memory_db();

    let run = db::create_discovery_run(
        &conn,
        &DiscoveryTaskRequest {
            sync_mode: SyncMode::Full,
            target_added_games: 4,
            page_size: 20,
        },
        Some(700_000),
    )
    .expect("create run");

    db::update_discovery_run_progress(
        &conn,
        run.id,
        db::DiscoveryProgressPatch {
            current_appid: Some(Some(700_010)),
            last_appid: Some(Some(700_020)),
            last_error: Some(Some("temporary upstream failure".to_string())),
            finished_at: Some(Some("2026-04-27T10:00:00Z".to_string())),
            ..Default::default()
        },
    )
    .expect("seed nullable fields");

    db::update_discovery_run_progress(
        &conn,
        run.id,
        db::DiscoveryProgressPatch {
            current_appid: Some(None),
            last_appid: Some(None),
            last_error: Some(None),
            finished_at: Some(None),
            ..Default::default()
        },
    )
    .expect("clear nullable fields");

    let loaded = db::load_discovery_run(&conn, run.id)
        .expect("load cleared run")
        .expect("run exists");

    assert_eq!(loaded.current_appid, None);
    assert_eq!(loaded.last_appid, None);
    assert_eq!(loaded.last_error, None);
    assert_eq!(loaded.finished_at, None);
}

#[test]
fn discovery_run_constraints_reject_invalid_status_and_negative_counters() {
    let conn = seeded_memory_db();
    let now = "2026-04-27T10:00:00Z";

    let invalid_status = conn.execute(
        r#"
        INSERT INTO discovery_runs (status, target_added_games, page_size, started_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?4)
        "#,
        params!["not_a_real_status", 1_i64, 25_i64, now],
    );
    assert!(invalid_status.is_err());

    let negative_counter = conn.execute(
        r#"
        INSERT INTO discovery_runs (
            status, target_added_games, page_size, pages_processed, started_at, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?5)
        "#,
        params!["running", 1_i64, 25_i64, -1_i64, now],
    );
    assert!(negative_counter.is_err());

    let negative_current_appid = conn.execute(
        r#"
        INSERT INTO discovery_runs (
            status, target_added_games, page_size, current_appid, started_at, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?5)
        "#,
        params!["running", 1_i64, 25_i64, -7_i64, now],
    );
    assert!(negative_current_appid.is_err());

    let invalid_have_more_results = conn.execute(
        r#"
        INSERT INTO discovery_runs (
            status, target_added_games, page_size, have_more_results, started_at, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?5)
        "#,
        params!["running", 1_i64, 25_i64, 2_i64, now],
    );
    assert!(invalid_have_more_results.is_err());

    let oversized_target_added_games = conn.execute(
        r#"
        INSERT INTO discovery_runs (
            status, target_added_games, page_size, started_at, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?4)
        "#,
        params!["running", 4_294_967_296_i64, 25_i64, now],
    );
    assert!(oversized_target_added_games.is_err());

    let oversized_current_appid = conn.execute(
        r#"
        INSERT INTO discovery_runs (
            status, target_added_games, page_size, current_appid, started_at, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?5)
        "#,
        params!["running", 1_i64, 25_i64, 4_294_967_296_i64, now],
    );
    assert!(oversized_current_appid.is_err());

    let oversized_pages_processed = conn.execute(
        r#"
        INSERT INTO discovery_runs (
            status, target_added_games, page_size, pages_processed, started_at, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?5)
        "#,
        params!["running", 1_i64, 25_i64, 4_294_967_296_i64, now],
    );
    assert!(oversized_pages_processed.is_err());

    let run = db::create_discovery_run(
        &conn,
        &DiscoveryTaskRequest {
            sync_mode: SyncMode::Full,
            target_added_games: 1,
            page_size: 25,
        },
        Some(1),
    )
    .expect("create run for failure constraint checks");

    let oversized_failure_page_index = conn.execute(
        r#"
        INSERT INTO discovery_failures (run_id, page_index, appid, stage, reason, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        "#,
        params![
            run.id,
            4_294_967_296_i64,
            10_i64,
            "fetch_snapshot",
            "oversized page index",
            now
        ],
    );
    assert!(oversized_failure_page_index.is_err());

    let oversized_failure_appid = conn.execute(
        r#"
        INSERT INTO discovery_failures (run_id, page_index, appid, stage, reason, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        "#,
        params![
            run.id,
            0_i64,
            4_294_967_296_i64,
            "fetch_snapshot",
            "oversized appid",
            now
        ],
    );
    assert!(oversized_failure_appid.is_err());
}

#[test]
fn migrate_rejects_invalid_rows_in_legacy_discovery_runs_table() {
    let cases = [
        ("current_appid", -1_i64, "current_appid"),
        ("last_appid", -2_i64, "last_appid"),
        ("have_more_results", 2_i64, "have_more_results"),
        ("pages_processed", 4_294_967_296_i64, "pages_processed"),
        ("current_appid", 4_294_967_296_i64, "current_appid"),
        ("last_appid", 4_294_967_296_i64, "last_appid"),
    ];

    for (column, invalid_value, expected_fragment) in cases {
        let conn = Connection::open_in_memory().expect("open in-memory db");

        conn.execute_batch(
            r#"
            CREATE TABLE discovery_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                status TEXT NOT NULL,
                target_added_games INTEGER NOT NULL,
                page_size INTEGER NOT NULL,
                pages_processed INTEGER NOT NULL DEFAULT 0,
                scanned_apps INTEGER NOT NULL DEFAULT 0,
                added_games INTEGER NOT NULL DEFAULT 0,
                added_new_games INTEGER NOT NULL DEFAULT 0,
                added_classic_games INTEGER NOT NULL DEFAULT 0,
                skipped_existing INTEGER NOT NULL DEFAULT 0,
                skipped_non_multiplayer INTEGER NOT NULL DEFAULT 0,
                failed_games INTEGER NOT NULL DEFAULT 0,
                current_appid INTEGER,
                last_appid INTEGER,
                have_more_results INTEGER NOT NULL DEFAULT 1,
                started_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                finished_at TEXT,
                last_error TEXT
            );
            "#,
        )
        .expect("create legacy discovery_runs");

        let insert_sql = format!(
            r#"
            INSERT INTO discovery_runs (
                status, target_added_games, page_size, {column}, started_at, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?5)
            "#
        );

        conn.execute(
            &insert_sql,
            params![
                "running",
                2_i64,
                25_i64,
                invalid_value,
                "2026-04-27T10:00:00Z"
            ],
        )
        .expect("insert invalid legacy row");

        let error = db::migrate(&conn).expect_err("migration should reject invalid legacy row");
        let message = error.to_string();

        assert!(message.contains("invalid discovery_runs row"));
        assert!(message.contains(expected_fragment));
        assert!(message.contains(&invalid_value.to_string()));
    }

    let conn = Connection::open_in_memory().expect("open in-memory db");

    conn.execute_batch(
        r#"
        CREATE TABLE discovery_runs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            status TEXT NOT NULL,
            target_added_games INTEGER NOT NULL,
            page_size INTEGER NOT NULL,
            pages_processed INTEGER NOT NULL DEFAULT 0,
            scanned_apps INTEGER NOT NULL DEFAULT 0,
            added_games INTEGER NOT NULL DEFAULT 0,
            added_new_games INTEGER NOT NULL DEFAULT 0,
            added_classic_games INTEGER NOT NULL DEFAULT 0,
            skipped_existing INTEGER NOT NULL DEFAULT 0,
            skipped_non_multiplayer INTEGER NOT NULL DEFAULT 0,
            failed_games INTEGER NOT NULL DEFAULT 0,
            current_appid INTEGER,
            last_appid INTEGER,
            have_more_results INTEGER NOT NULL DEFAULT 1,
            started_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            finished_at TEXT,
            last_error TEXT
        );
        "#,
    )
    .expect("create legacy discovery_runs for oversized target");

    conn.execute(
        r#"
        INSERT INTO discovery_runs (
            status, target_added_games, page_size, started_at, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?4)
        "#,
        params!["running", 4_294_967_296_i64, 25_i64, "2026-04-27T10:00:00Z"],
    )
    .expect("insert oversized target_added_games");

    let error = db::migrate(&conn).expect_err("migration should reject oversized target");
    let message = error.to_string();

    assert!(message.contains("invalid discovery_runs row"));
    assert!(message.contains("target_added_games"));
    assert!(message.contains("4294967296"));

    let conn = Connection::open_in_memory().expect("open in-memory db");

    conn.execute_batch(
        r#"
        CREATE TABLE discovery_runs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            status TEXT,
            target_added_games INTEGER NOT NULL,
            page_size INTEGER NOT NULL,
            pages_processed INTEGER NOT NULL DEFAULT 0,
            scanned_apps INTEGER NOT NULL DEFAULT 0,
            added_games INTEGER NOT NULL DEFAULT 0,
            added_new_games INTEGER NOT NULL DEFAULT 0,
            added_classic_games INTEGER NOT NULL DEFAULT 0,
            skipped_existing INTEGER NOT NULL DEFAULT 0,
            skipped_non_multiplayer INTEGER NOT NULL DEFAULT 0,
            failed_games INTEGER NOT NULL DEFAULT 0,
            current_appid INTEGER,
            last_appid INTEGER,
            have_more_results INTEGER NOT NULL DEFAULT 1,
            started_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            finished_at TEXT,
            last_error TEXT
        );
        "#,
    )
    .expect("create nullable-status legacy discovery_runs");

    conn.execute(
        r#"
        INSERT INTO discovery_runs (
            status, target_added_games, page_size, started_at, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?4)
        "#,
        params![rusqlite::types::Null, 1_i64, 25_i64, "2026-04-27T10:00:00Z"],
    )
    .expect("insert null status legacy row");

    let error = db::migrate(&conn).expect_err("migration should reject null status");
    let message = error.to_string();

    assert!(message.contains("invalid discovery_runs row"));
    assert!(message.contains("status"));
    assert!(message.contains("NULL"));
}

#[test]
fn migrate_rejects_invalid_rows_in_legacy_discovery_failures_table() {
    let cases = [
        ("page_index", 4_294_967_296_i64, "page_index"),
        ("appid", 4_294_967_296_i64, "appid"),
    ];

    for (column, invalid_value, expected_fragment) in cases {
        let conn = Connection::open_in_memory().expect("open in-memory db");

        conn.execute_batch(
            r#"
            CREATE TABLE discovery_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                status TEXT NOT NULL,
                target_added_games INTEGER NOT NULL,
                page_size INTEGER NOT NULL,
                pages_processed INTEGER NOT NULL DEFAULT 0,
                scanned_apps INTEGER NOT NULL DEFAULT 0,
                added_games INTEGER NOT NULL DEFAULT 0,
                added_new_games INTEGER NOT NULL DEFAULT 0,
                added_classic_games INTEGER NOT NULL DEFAULT 0,
                skipped_existing INTEGER NOT NULL DEFAULT 0,
                skipped_non_multiplayer INTEGER NOT NULL DEFAULT 0,
                failed_games INTEGER NOT NULL DEFAULT 0,
                current_appid INTEGER,
                last_appid INTEGER,
                have_more_results INTEGER NOT NULL DEFAULT 1,
                started_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                finished_at TEXT,
                last_error TEXT
            );

            CREATE TABLE discovery_failures (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id INTEGER NOT NULL,
                page_index INTEGER NOT NULL,
                appid INTEGER,
                stage TEXT NOT NULL,
                reason TEXT NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY(run_id) REFERENCES discovery_runs(id) ON DELETE CASCADE
            );
            "#,
        )
        .expect("create legacy discovery tables");

        conn.execute(
            r#"
            INSERT INTO discovery_runs (id, status, target_added_games, page_size, started_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?5)
            "#,
            params![1_i64, "running", 1_i64, 25_i64, "2026-04-27T10:00:00Z"],
        )
        .expect("insert valid legacy run");

        if column == "page_index" {
            conn.execute(
                r#"
                INSERT INTO discovery_failures (run_id, page_index, appid, stage, reason, created_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                "#,
                params![
                    1_i64,
                    invalid_value,
                    10_i64,
                    "fetch_snapshot",
                    "invalid legacy failure",
                    "2026-04-27T10:00:00Z"
                ],
            )
            .expect("insert invalid legacy failure page_index row");
        } else {
            conn.execute(
                r#"
                INSERT INTO discovery_failures (run_id, page_index, appid, stage, reason, created_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                "#,
                params![
                    1_i64,
                    0_i64,
                    invalid_value,
                    "fetch_snapshot",
                    "invalid legacy failure",
                    "2026-04-27T10:00:00Z"
                ],
            )
            .expect("insert invalid legacy failure appid row");
        }

        let error =
            db::migrate(&conn).expect_err("migration should reject invalid legacy failure row");
        let message = error.to_string();

        assert!(message.contains("invalid discovery_failures row"));
        assert!(message.contains(expected_fragment));
        assert!(message.contains(&invalid_value.to_string()));
    }

    let conn = Connection::open_in_memory().expect("open in-memory db");

    conn.execute_batch(
        r#"
        CREATE TABLE discovery_runs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            status TEXT NOT NULL,
            target_added_games INTEGER NOT NULL,
            page_size INTEGER NOT NULL,
            pages_processed INTEGER NOT NULL DEFAULT 0,
            scanned_apps INTEGER NOT NULL DEFAULT 0,
            added_games INTEGER NOT NULL DEFAULT 0,
            added_new_games INTEGER NOT NULL DEFAULT 0,
            added_classic_games INTEGER NOT NULL DEFAULT 0,
            skipped_existing INTEGER NOT NULL DEFAULT 0,
            skipped_non_multiplayer INTEGER NOT NULL DEFAULT 0,
            failed_games INTEGER NOT NULL DEFAULT 0,
            current_appid INTEGER,
            last_appid INTEGER,
            have_more_results INTEGER NOT NULL DEFAULT 1,
            started_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            finished_at TEXT,
            last_error TEXT
        );

        CREATE TABLE discovery_failures (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            run_id INTEGER NOT NULL,
            page_index INTEGER NOT NULL,
            appid INTEGER,
            stage TEXT,
            reason TEXT NOT NULL,
            created_at TEXT NOT NULL,
            FOREIGN KEY(run_id) REFERENCES discovery_runs(id) ON DELETE CASCADE
        );
        "#,
    )
    .expect("create legacy discovery tables with nullable stage");

    conn.execute(
        r#"
        INSERT INTO discovery_runs (id, status, target_added_games, page_size, started_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?5)
        "#,
        params![1_i64, "running", 1_i64, 25_i64, "2026-04-27T10:00:00Z"],
    )
    .expect("insert valid legacy run");

    conn.execute(
        r#"
        INSERT INTO discovery_failures (run_id, page_index, appid, stage, reason, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        "#,
        params![
            1_i64,
            0_i64,
            10_i64,
            rusqlite::types::Null,
            "missing stage",
            "2026-04-27T10:00:00Z"
        ],
    )
    .expect("insert null stage legacy failure row");

    let error = db::migrate(&conn).expect_err("migration should reject null stage");
    let message = error.to_string();

    assert!(message.contains("invalid discovery_failures row"));
    assert!(message.contains("stage"));
    assert!(message.contains("NULL"));
}

#[test]
fn discovery_snapshot_progress_percent_uses_target_added_games() {
    assert_eq!(
        sample_snapshot(DiscoveryRunStatus::Running, 4).progress_percent(),
        40
    );
    assert_eq!(
        sample_snapshot(DiscoveryRunStatus::Running, 10).progress_percent(),
        100
    );
}

#[test]
fn discovery_snapshot_serialization_includes_progress_percent() {
    let snapshot = sample_snapshot(DiscoveryRunStatus::Running, 4);
    let serialized = serde_json::to_value(&snapshot).expect("serialize discovery snapshot");

    assert_eq!(serialized["progressPercent"], json!(40));
    assert_eq!(serialized["syncMode"], json!("full"));
    assert_eq!(serialized["targetAddedGames"], json!(10));
    assert_eq!(serialized["addedGames"], json!(4));
    assert_eq!(serialized["status"], json!("running"));
    assert_eq!(serialized["currentAppid"], json!(600_123));
}

#[test]
fn only_paused_or_interrupted_runs_can_resume() {
    assert!(sample_snapshot(DiscoveryRunStatus::Paused, 3).can_resume());
    assert!(sample_snapshot(DiscoveryRunStatus::Interrupted, 3).can_resume());
    assert!(!sample_snapshot(DiscoveryRunStatus::Running, 3).can_resume());
    assert!(!sample_snapshot(DiscoveryRunStatus::Completed, 10).can_resume());
}
