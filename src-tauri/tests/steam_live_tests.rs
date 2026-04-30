use reqwest::Client;
use rusqlite::Connection;
use tauri_app_lib::discovery::build_discovered_game_card;
use tauri_app_lib::steam::{
    fetch_app_list_preview, fetch_game_snapshot, fetch_store_search_candidates, SteamAppListItem,
    SteamGameSnapshotEnrichment,
};
use tauri_app_lib::{db, recommendation};

fn steam_api_key() -> String {
    std::env::var("MPGS_STEAM_API_KEY")
        .expect("set MPGS_STEAM_API_KEY to run live Steam integration tests")
}

fn steam_http_client() -> Client {
    Client::builder()
        .user_agent("MPGS/0.1 (+https://local.app)")
        .build()
        .expect("build Steam HTTP client")
}

#[tokio::test]
#[ignore = "requires live Steam Web API access and MPGS_STEAM_API_KEY"]
async fn steam_live_app_list_preview_returns_cursor_page() {
    let key = steam_api_key();
    let client = steam_http_client();

    let preview = fetch_app_list_preview(&client, &key, 10, Some(3010))
        .await
        .expect("fetch live Steam app list preview");

    assert!(!preview.apps.is_empty());
    assert!(preview.apps.len() <= 10);
    assert!(preview.last_appid.is_some());
    assert_eq!(preview.have_more_results, Some(true));
}

#[tokio::test]
#[ignore = "requires live Steam Store and Steam Web API access"]
async fn steam_live_game_snapshot_fetches_multiplayer_reviews_and_players() {
    let client = steam_http_client();

    let snapshot = fetch_game_snapshot(
        &client,
        730,
        "US",
        "english",
        SteamGameSnapshotEnrichment::Full,
    )
    .await
    .expect("fetch live Counter-Strike 2 snapshot");

    assert!(snapshot
        .name
        .as_deref()
        .is_some_and(|name| name.contains("Counter-Strike")));
    assert!(!snapshot.multiplayer_modes.is_empty());
    assert!(snapshot.total_reviews.unwrap_or_default() > 1_000_000);
    assert!(snapshot.positive_review_pct.unwrap_or_default() > 0.0);
    assert!(snapshot.current_players.unwrap_or_default() > 0);
    assert!(!snapshot.review_snippets.is_empty());

    let app = SteamAppListItem {
        appid: 730,
        name: "Counter-Strike 2".to_string(),
    };
    let card = build_discovered_game_card(&app, snapshot, &recommendation::today_iso_utc())
        .expect("live multiplayer snapshot should become a game card");

    let conn = Connection::open_in_memory().expect("open in-memory database");
    db::migrate(&conn).expect("migrate in-memory database");
    db::upsert_game(&conn, &card).expect("import live game card");
    let imported = db::load_game(&conn, 730)
        .expect("load imported game")
        .expect("imported game exists");

    assert!(imported.total_reviews.unwrap_or_default() > 1_000_000);
    assert!(imported.positive_review_pct.unwrap_or_default() > 0.0);
    assert!(imported.current_players.unwrap_or_default() > 0);
    assert!(!imported.multiplayer_modes.is_empty());
}

#[tokio::test]
#[ignore = "requires live Steam Store access"]
async fn steam_live_store_search_candidates_page_recent_releases() {
    let client = steam_http_client();

    let first_page = fetch_store_search_candidates(&client, 0, 30, "english")
        .await
        .expect("fetch first Store Search page");
    let second_page = fetch_store_search_candidates(&client, 30, 30, "english")
        .await
        .expect("fetch second Store Search page");

    assert_eq!(first_page.start, 0);
    assert_eq!(second_page.start, 30);
    assert!(!first_page.apps.is_empty());
    assert!(!second_page.apps.is_empty());
    assert_ne!(first_page.apps[0].appid, second_page.apps[0].appid);
    assert!(first_page.have_more_results);
}
