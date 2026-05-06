use rusqlite::{params, Connection};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri_app_lib::db;
use tauri_app_lib::discovery::DISCOVERY_CURSOR_CONFIG_KEY;
use tauri_app_lib::models::{
    GameCard, ReviewSnippet, StoreReleaseState, UserGameState, UserGameStatePatch,
};
use tauri_app_lib::recommendation::DemoStatus;

fn seeded_memory_db() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory db");
    db::migrate(&conn).expect("migrate");
    db::seed_default_games(&conn).expect("seed");
    conn
}

fn temp_db_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "mpgs-{label}-{}-{nonce}.sqlite3",
        std::process::id()
    ))
}

fn remove_sqlite_files(path: &PathBuf) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
}

fn sample_game(appid: u32, name: &str) -> GameCard {
    GameCard {
        appid,
        name: name.to_string(),
        short_description: Some(format!("{name} short description")),
        section: "classic".to_string(),
        release_date: Some("2024-01-01".to_string()),
        release_date_text: "2024.01".to_string(),
        release_state: StoreReleaseState::Released,
        demo_status: DemoStatus::Released,
        supported_languages: vec!["English".to_string(), "Simplified Chinese".to_string()],
        is_adult_content: false,
        is_free: false,
        price_text: Some("$19.99".to_string()),
        discount_percent: None,
        positive_review_pct: Some(88.0),
        total_reviews: Some(1_250),
        current_players: Some(148),
        recommendation_score: 78.0,
        ai_score: Some(82.0),
        ai_summary: format!("{name} ai summary"),
        capsule_url: format!("https://example.com/{appid}.jpg"),
        store_screenshot_urls: vec![format!("https://example.com/{appid}-1.jpg")],
        tags: vec!["Co-op".to_string(), "Action".to_string()],
        multiplayer_modes: vec!["Online Co-op".to_string()],
        review_snippets: Vec::<ReviewSnippet>::new(),
        user_state: UserGameState::default(),
    }
}

#[test]
fn open_database_skips_default_seed_games_when_steam_key_is_configured() {
    let path = temp_db_path("steam-key-no-seeds");
    {
        let conn = Connection::open(&path).expect("create temp sqlite db");
        db::migrate(&conn).expect("migrate temp db");
        db::set_config(&conn, "steam_api_key", "configured").expect("save steam key marker");
    }

    let conn = db::open_database(&path).expect("open configured database");
    let game_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM games", [], |row| row.get(0))
        .expect("count games");

    assert_eq!(game_count, 0);
    drop(conn);
    remove_sqlite_files(&path);
}

#[test]
fn open_database_keeps_unconfigured_empty_databases_empty() {
    let path = temp_db_path("empty-library");
    let conn = db::open_database(&path).expect("open fresh database");
    let game_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM games", [], |row| row.get(0))
        .expect("count games");

    assert_eq!(game_count, 0);
    let dashboard = db::load_dashboard(&conn).expect("load dashboard");
    assert_eq!(dashboard.stats.total_games, 0);
    assert_eq!(
        dashboard.stats.data_source,
        "当前库为空；请先配置 Steam API Key 后导入多人游戏。"
    );
    drop(conn);
    remove_sqlite_files(&path);
}

#[test]
fn open_database_purges_legacy_bootstrap_seed_rows() {
    let path = temp_db_path("legacy-bootstrap-seeds");
    {
        let conn = Connection::open(&path).expect("create temp sqlite db");
        db::migrate(&conn).expect("migrate temp db");
        db::seed_default_games(&conn).expect("insert legacy bootstrap seeds");
    }

    let conn = db::open_database(&path).expect("reopen legacy bootstrap database");
    let game_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM games", [], |row| row.get(0))
        .expect("count games after purge");

    assert_eq!(game_count, 0);
    drop(conn);
    remove_sqlite_files(&path);
}

#[test]
fn user_state_flags_are_persisted_into_dashboard_cards() {
    let conn = seeded_memory_db();

    db::set_game_user_state(
        &conn,
        548_430,
        UserGameStatePatch {
            favorite: Some(true),
            wishlist: Some(true),
            followed: Some(false),
            viewed: Some(true),
        },
    )
    .expect("set user state");

    let dashboard = db::load_dashboard(&conn).expect("load dashboard");
    let game = dashboard
        .classics
        .iter()
        .find(|game| game.appid == 548_430)
        .expect("Deep Rock Galactic seeded");

    assert!(game.user_state.favorite);
    assert!(game.user_state.wishlist);
    assert!(!game.user_state.followed);
    assert!(game.user_state.viewed);
    assert!(game.user_state.updated_at.is_some());
}

#[test]
fn user_collections_return_only_matching_games() {
    let conn = seeded_memory_db();

    db::set_game_user_state(
        &conn,
        548_430,
        UserGameStatePatch {
            favorite: Some(true),
            wishlist: None,
            followed: None,
            viewed: None,
        },
    )
    .expect("favorite");
    db::set_game_user_state(
        &conn,
        413_150,
        UserGameStatePatch {
            favorite: None,
            wishlist: Some(true),
            followed: Some(true),
            viewed: Some(true),
        },
    )
    .expect("wishlist");

    let collections = db::load_user_collections(&conn).expect("collections");

    assert_eq!(collections.favorites.len(), 1);
    assert_eq!(collections.favorites[0].appid, 548_430);
    assert_eq!(collections.wishlist.len(), 1);
    assert_eq!(collections.wishlist[0].appid, 413_150);
    assert_eq!(collections.followed.len(), 1);
    assert_eq!(collections.history.len(), 1);
}

#[test]
fn dashboard_stats_reflect_library_counts_and_discovery_cursor() {
    let conn = seeded_memory_db();

    db::set_config(&conn, DISCOVERY_CURSOR_CONFIG_KEY, "987654").expect("save cursor");

    let dashboard = db::load_dashboard(&conn).expect("dashboard");

    assert_eq!(
        dashboard.stats.total_games,
        dashboard.new_games.len() + dashboard.classics.len()
    );
    assert_eq!(dashboard.stats.new_games_count, dashboard.new_games.len());
    assert_eq!(
        dashboard.stats.classic_games_count,
        dashboard.classics.len()
    );
    assert_eq!(dashboard.stats.last_discovery_appid, Some(987_654));
    assert_eq!(
        dashboard.recent_discoveries.len(),
        dashboard.stats.total_games as usize
    );
    assert!(dashboard
        .stats
        .data_source
        .contains(&dashboard.stats.total_games.to_string()));
}

#[test]
fn recent_discoveries_follow_first_import_time_instead_of_latest_refresh_time() {
    let conn = Connection::open_in_memory().expect("open in-memory db");
    db::migrate(&conn).expect("migrate");

    let older_game = sample_game(7_100_001, "Older Import");
    let newer_game = sample_game(7_100_002, "Newer Import");

    db::upsert_game(&conn, &older_game).expect("insert older game");
    conn.execute(
        "UPDATE games SET created_at = ?2, updated_at = ?2 WHERE appid = ?1",
        params![older_game.appid, "2026-05-05T10:00:00Z"],
    )
    .expect("stamp older game import time");

    db::upsert_game(&conn, &newer_game).expect("insert newer game");
    conn.execute(
        "UPDATE games SET created_at = ?2, updated_at = ?2 WHERE appid = ?1",
        params![newer_game.appid, "2026-05-05T12:00:00Z"],
    )
    .expect("stamp newer game import time");

    let mut refreshed_older_game = older_game.clone();
    refreshed_older_game.ai_summary = "Older import refreshed by later sync".to_string();
    db::upsert_game(&conn, &refreshed_older_game).expect("refresh older game");

    let (created_at, updated_at): (String, String) = conn
        .query_row(
            "SELECT created_at, updated_at FROM games WHERE appid = ?1",
            params![older_game.appid],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("load stored timestamps");
    assert_eq!(created_at, "2026-05-05T10:00:00Z");
    assert_ne!(updated_at, created_at);

    let dashboard = db::load_dashboard(&conn).expect("load dashboard");
    let ordered_recent_appids = dashboard
        .recent_discoveries
        .iter()
        .map(|game| game.appid)
        .collect::<Vec<_>>();

    assert_eq!(ordered_recent_appids, vec![newer_game.appid, older_game.appid]);
}

#[test]
fn dashboard_stats_expose_idle_ai_batch_refresh_progress_fields() {
    let conn = seeded_memory_db();
    let dashboard = db::load_dashboard(&conn).expect("dashboard");
    let stats_json = serde_json::to_value(&dashboard.stats).expect("serialize stats");

    assert_eq!(stats_json["aiBatchRefreshRunning"], false);
    assert_eq!(stats_json["aiBatchRefreshTotalCount"], 0);
    assert_eq!(stats_json["aiBatchRefreshProcessedCount"], 0);
    assert_eq!(stats_json["aiBatchRefreshUpdatedCount"], 0);
    assert_eq!(stats_json["aiBatchRefreshFailedCount"], 0);
}

#[test]
fn public_config_defaults_to_schinese_language() {
    let conn = Connection::open_in_memory().expect("open in-memory db");
    db::migrate(&conn).expect("migrate");

    let config = db::public_config(&conn).expect("config");

    assert_eq!(config.country, "US");
    assert_eq!(config.language, "schinese");
}

#[test]
fn migrate_upgrades_legacy_default_english_language_to_schinese() {
    let conn = Connection::open_in_memory().expect("open in-memory db");
    db::migrate(&conn).expect("first migrate");
    db::set_config(&conn, "language", "english").expect("set legacy english language");

    db::migrate(&conn).expect("second migrate");

    let config = db::public_config(&conn).expect("config");
    assert_eq!(config.language, "schinese");
}
