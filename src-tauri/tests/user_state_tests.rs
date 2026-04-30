use rusqlite::Connection;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri_app_lib::db;
use tauri_app_lib::discovery::DISCOVERY_CURSOR_CONFIG_KEY;
use tauri_app_lib::models::UserGameStatePatch;

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
    assert_eq!(dashboard.recent_discoveries.len(), 8);
    assert!(dashboard
        .stats
        .data_source
        .contains(&dashboard.stats.total_games.to_string()));
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
