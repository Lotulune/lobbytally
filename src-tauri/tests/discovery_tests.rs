use tauri_app_lib::discovery::{
    build_discovered_game_card, clamp_discovery_page_size, clamp_discovery_pages,
    clamp_discovery_target_added_games, next_discovery_cursor, store_search_reached_page_budget,
    store_search_start_for_page,
};
use tauri_app_lib::models::{ReviewSnippet, StoreReleaseState};
use tauri_app_lib::recommendation::DemoStatus;
use tauri_app_lib::steam::{SteamAppListItem, SteamAppListPreview, SteamGameSnapshot};

fn multiplayer_snapshot() -> SteamGameSnapshot {
    SteamGameSnapshot {
        name: Some("Moonbase Kitchen Panic".to_string()),
        short_description: Some("四人合作厨房混乱，但分工很清晰。".to_string()),
        release_date: Some("2026-04-20".to_string()),
        release_date_text: Some("Apr 20, 2026".to_string()),
        release_state: Some(StoreReleaseState::Released),
        demo_status: DemoStatus::ReleasedWithDemo,
        supported_languages: Some(vec!["English".to_string(), "Japanese".to_string()]),
        is_adult_content: Some(false),
        is_free: Some(false),
        price_text: Some("$19.99".to_string()),
        discount_percent: Some(20),
        positive_review_pct: Some(93.0),
        total_reviews: Some(240),
        current_players: Some(88),
        capsule_url: Some("https://cdn.example.test/header.jpg".to_string()),
        store_screenshot_urls: vec![
            "https://cdn.example.test/thumb-1.jpg".to_string(),
            "https://cdn.example.test/thumb-2.jpg".to_string(),
        ],
        tags: vec!["Co-op".to_string(), "Puzzle".to_string()],
        multiplayer_modes: vec!["Online Co-op".to_string(), "Multi-player".to_string()],
        review_snippets: vec![ReviewSnippet {
            voted_up: true,
            review: "Chaotic but readable with three friends.".to_string(),
            playtime_hours: Some(4.5),
        }],
    }
}

#[test]
fn build_discovered_game_card_imports_multiplayer_snapshot() {
    let app = SteamAppListItem {
        appid: 3_900_001,
        name: "Fallback Name".to_string(),
    };

    let card = build_discovered_game_card(&app, multiplayer_snapshot(), "2026-04-26")
        .expect("multiplayer game should be imported");

    assert_eq!(card.appid, 3_900_001);
    assert_eq!(card.name, "Moonbase Kitchen Panic");
    assert_eq!(
        card.short_description.as_deref(),
        Some("四人合作厨房混乱，但分工很清晰。")
    );
    assert_eq!(card.section, "new");
    assert_eq!(card.release_state, StoreReleaseState::Released);
    assert_eq!(card.demo_status, DemoStatus::ReleasedWithDemo);
    assert_eq!(card.supported_languages, vec!["English", "Japanese"]);
    assert!(!card.is_adult_content);
    assert_eq!(card.price_text.as_deref(), Some("$19.99"));
    assert_eq!(card.discount_percent, Some(20));
    assert_eq!(
        card.store_screenshot_urls,
        vec![
            "https://cdn.example.test/thumb-1.jpg".to_string(),
            "https://cdn.example.test/thumb-2.jpg".to_string(),
        ]
    );
    assert_eq!(card.multiplayer_modes, vec!["Online Co-op", "Multi-player"]);
    assert!(card.recommendation_score > 70.0);
    assert!(!card.user_state.favorite);
}

#[test]
fn build_discovered_game_card_rejects_non_multiplayer_snapshot() {
    let app = SteamAppListItem {
        appid: 3_900_002,
        name: "Solo Meadow".to_string(),
    };
    let mut snapshot = multiplayer_snapshot();
    snapshot.multiplayer_modes.clear();

    assert!(build_discovered_game_card(&app, snapshot, "2026-04-26").is_none());
}

#[test]
fn build_discovered_game_card_rejects_low_signal_new_games_without_demo() {
    let app = SteamAppListItem {
        appid: 3_900_003,
        name: "Thin Signal Arena".to_string(),
    };
    let mut snapshot = multiplayer_snapshot();
    snapshot.total_reviews = Some(49);
    snapshot.positive_review_pct = Some(95.0);
    snapshot.demo_status = DemoStatus::Released;

    assert!(build_discovered_game_card(&app, snapshot, "2026-04-26").is_none());

    let mut snapshot = multiplayer_snapshot();
    snapshot.total_reviews = Some(200);
    snapshot.positive_review_pct = Some(39.0);
    snapshot.demo_status = DemoStatus::Released;

    assert!(build_discovered_game_card(&app, snapshot, "2026-04-26").is_none());
}

#[test]
fn build_discovered_game_card_allows_low_signal_new_games_when_demo_is_present() {
    let app = SteamAppListItem {
        appid: 3_900_004,
        name: "Demo Rescue Ops".to_string(),
    };
    let mut snapshot = multiplayer_snapshot();
    snapshot.total_reviews = Some(12);
    snapshot.positive_review_pct = Some(18.0);
    snapshot.demo_status = DemoStatus::ReleasedWithDemo;

    let card = build_discovered_game_card(&app, snapshot, "2026-04-26")
        .expect("demo new game should bypass low-signal thresholds");

    assert_eq!(card.section, "new");
}

#[test]
fn build_discovered_game_card_rejects_legendary_titles_for_new_games() {
    let app = SteamAppListItem {
        appid: 3_900_005,
        name: "多人传奇乱斗".to_string(),
    };
    let mut snapshot = multiplayer_snapshot();
    snapshot.name = Some("多人传奇乱斗".to_string());
    snapshot.total_reviews = Some(500);
    snapshot.positive_review_pct = Some(88.0);

    assert!(build_discovered_game_card(&app, snapshot, "2026-04-26").is_none());
}

#[test]
fn build_discovered_game_card_rejects_legendary_titles_for_classic_games_too() {
    let app = SteamAppListItem {
        appid: 3_900_006,
        name: "挂机传奇大厅".to_string(),
    };
    let mut snapshot = multiplayer_snapshot();
    snapshot.name = Some("挂机传奇大厅".to_string());
    snapshot.release_date = Some("2023-04-20".to_string());
    snapshot.total_reviews = Some(5_000);
    snapshot.positive_review_pct = Some(91.0);

    assert!(build_discovered_game_card(&app, snapshot, "2026-04-26").is_none());
}

#[test]
fn next_discovery_cursor_uses_last_appid_and_falls_back_to_last_app() {
    let explicit_cursor = SteamAppListPreview {
        apps: vec![SteamAppListItem {
            appid: 10,
            name: "Ten".to_string(),
        }],
        last_appid: Some(42),
        have_more_results: Some(true),
    };
    assert_eq!(next_discovery_cursor(&explicit_cursor), Some(42));

    let fallback_cursor = SteamAppListPreview {
        apps: vec![
            SteamAppListItem {
                appid: 11,
                name: "Eleven".to_string(),
            },
            SteamAppListItem {
                appid: 12,
                name: "Twelve".to_string(),
            },
        ],
        last_appid: None,
        have_more_results: Some(true),
    };
    assert_eq!(next_discovery_cursor(&fallback_cursor), Some(12));
}

#[test]
fn discovery_defaults_scan_larger_candidate_pages() {
    assert_eq!(clamp_discovery_pages(None), 2);
    assert_eq!(clamp_discovery_page_size(None), 100);
    assert_eq!(clamp_discovery_target_added_games(None), 200);
    assert_eq!(clamp_discovery_pages(Some(99)), 5);
    assert_eq!(clamp_discovery_page_size(Some(999)), 100);
    assert_eq!(clamp_discovery_target_added_games(Some(999)), 200);
}

#[test]
fn store_search_start_uses_page_offset_instead_of_appid_cursor() {
    assert_eq!(store_search_start_for_page(0, 100), 0);
    assert_eq!(store_search_start_for_page(1, 100), 100);
    assert_eq!(store_search_start_for_page(4, 100), 400);
}

#[test]
fn store_search_discovery_uses_finite_page_budget() {
    assert!(!store_search_reached_page_budget(0));
    assert!(!store_search_reached_page_budget(1));
    assert!(store_search_reached_page_budget(2));
}
