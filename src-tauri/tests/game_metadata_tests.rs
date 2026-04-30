use rusqlite::Connection;
use tauri_app_lib::db;
use tauri_app_lib::models::{GameCard, ReviewSnippet, StoreReleaseState, UserGameState};
use tauri_app_lib::recommendation::DemoStatus;

fn empty_db() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory db");
    db::migrate(&conn).expect("migrate");
    conn
}

#[test]
fn sqlite_round_trips_extended_store_metadata() {
    let conn = empty_db();
    let card = GameCard {
        appid: 3_990_001,
        name: "Orbital Bakers".to_string(),
        section: "new".to_string(),
        short_description: Some("Bake, brawl, and coordinate your orbiting kitchen.".to_string()),
        release_date: Some("2026-06-01".to_string()),
        release_date_text: "Jun 1, 2026".to_string(),
        release_state: StoreReleaseState::Upcoming,
        demo_status: DemoStatus::ReleasedWithDemo,
        supported_languages: vec!["english".to_string(), "schinese".to_string()],
        is_adult_content: false,
        price_text: Some("$19.99".to_string()),
        discount_percent: Some(10),
        positive_review_pct: Some(91.5),
        total_reviews: Some(432),
        current_players: Some(87),
        recommendation_score: 84.2,
        ai_score: Some(80.0),
        ai_summary: "Metadata round-trip coverage.".to_string(),
        capsule_url: "https://cdn.example.test/orbital-bakers.jpg".to_string(),
        store_screenshot_urls: vec![
            "https://cdn.example.test/orbital-bakers-1.jpg".to_string(),
            "https://cdn.example.test/orbital-bakers-2.jpg".to_string(),
        ],
        tags: vec!["Co-op".to_string(), "Cooking".to_string()],
        multiplayer_modes: vec!["Online Co-op".to_string()],
        review_snippets: vec![ReviewSnippet {
            voted_up: true,
            review: "A delightful mess with friends.".to_string(),
            playtime_hours: Some(7.5),
        }],
        user_state: UserGameState::default(),
    };

    db::upsert_game(&conn, &card).expect("upsert game");

    let loaded = db::load_game(&conn, card.appid)
        .expect("load game")
        .expect("game exists");

    assert_eq!(loaded.release_state, StoreReleaseState::Upcoming);
    assert_eq!(
        loaded.short_description.as_deref(),
        Some("Bake, brawl, and coordinate your orbiting kitchen.")
    );
    assert_eq!(
        loaded.supported_languages,
        vec!["english".to_string(), "schinese".to_string()]
    );
    assert!(!loaded.is_adult_content);
    assert_eq!(loaded.price_text.as_deref(), Some("$19.99"));
    assert_eq!(loaded.discount_percent, Some(10));
    assert_eq!(
        loaded.store_screenshot_urls,
        vec![
            "https://cdn.example.test/orbital-bakers-1.jpg".to_string(),
            "https://cdn.example.test/orbital-bakers-2.jpg".to_string(),
        ]
    );
}

#[test]
fn dashboard_separates_upcoming_games_and_load_game_includes_them() {
    let conn = empty_db();
    let released = GameCard {
        appid: 3_990_010,
        name: "Released Squad".to_string(),
        section: "new".to_string(),
        short_description: Some("A co-op release metadata fixture.".to_string()),
        release_date: Some("2026-04-01".to_string()),
        release_date_text: "Apr 1, 2026".to_string(),
        release_state: StoreReleaseState::Released,
        demo_status: DemoStatus::Released,
        supported_languages: vec!["english".to_string()],
        is_adult_content: false,
        price_text: Some("$9.99".to_string()),
        discount_percent: None,
        positive_review_pct: Some(88.0),
        total_reviews: Some(210),
        current_players: Some(44),
        recommendation_score: 70.0,
        ai_score: Some(72.0),
        ai_summary: "Released metadata coverage.".to_string(),
        capsule_url: "https://cdn.example.test/released-squad.jpg".to_string(),
        store_screenshot_urls: vec![],
        tags: vec!["Co-op".to_string()],
        multiplayer_modes: vec!["Online Co-op".to_string()],
        review_snippets: vec![],
        user_state: UserGameState::default(),
    };
    let mut upcoming = released.clone();
    upcoming.appid = 3_990_011;
    upcoming.name = "Launch Watch".to_string();
    upcoming.section = "classic".to_string();
    upcoming.release_date = Some("2026-12-01".to_string());
    upcoming.release_date_text = "Dec 1, 2026".to_string();
    upcoming.release_state = StoreReleaseState::Upcoming;
    upcoming.price_text = None;

    db::upsert_game(&conn, &released).expect("upsert released");
    db::upsert_game(&conn, &upcoming).expect("upsert upcoming");

    let dashboard = db::load_dashboard(&conn).expect("load dashboard");
    assert_eq!(
        dashboard
            .upcoming
            .iter()
            .map(|game| game.appid)
            .collect::<Vec<_>>(),
        vec![upcoming.appid]
    );
    assert!(dashboard
        .new_games
        .iter()
        .any(|game| game.appid == released.appid));
    assert!(dashboard
        .classics
        .iter()
        .all(|game| game.appid != upcoming.appid));

    let loaded = db::load_game(&conn, upcoming.appid)
        .expect("load game")
        .expect("upcoming exists");
    assert_eq!(loaded.appid, upcoming.appid);
    assert_eq!(loaded.release_state, StoreReleaseState::Upcoming);
}
