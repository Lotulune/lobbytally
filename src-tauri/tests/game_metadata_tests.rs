use rusqlite::Connection;
use tauri_app_lib::db;
use tauri_app_lib::models::{
    AnalysisConfidence, AnalysisDimensionScore, AnalysisEvidenceItem, AnalysisEvidenceKind,
    AnalysisPoint, AnalysisReviewEvidenceItem, AnalysisReviewStance, AnalysisSource,
    GameAnalysisReport, GameCard, ReviewSnippet, StoreReleaseState, UserGameState,
};
use tauri_app_lib::recommendation::DemoStatus;

fn empty_db() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory db");
    db::migrate(&conn).expect("migrate");
    conn
}

#[test]
fn migrate_sets_deepseek_as_the_default_llm_provider() {
    let conn = empty_db();
    let config = db::public_config(&conn).expect("load public config");

    assert_eq!(config.llm_base_url, "https://api.deepseek.com");
    assert_eq!(config.llm_model, "deepseek-v4-flash");
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

#[test]
fn sqlite_round_trips_cached_game_analysis_report() {
    let conn = empty_db();
    let card = GameCard {
        appid: 3_990_021,
        name: "Signal Harbor".to_string(),
        section: "new".to_string(),
        short_description: Some("Coordinate lighthouse crews across stormy co-op shifts.".to_string()),
        release_date: Some("2026-08-12".to_string()),
        release_date_text: "Aug 12, 2026".to_string(),
        release_state: StoreReleaseState::Upcoming,
        demo_status: DemoStatus::ReleasedWithDemo,
        supported_languages: vec!["english".to_string()],
        is_adult_content: false,
        price_text: Some("$24.99".to_string()),
        discount_percent: Some(15),
        positive_review_pct: Some(89.0),
        total_reviews: Some(512),
        current_players: Some(133),
        recommendation_score: 82.4,
        ai_score: Some(78.0),
        ai_summary: "Analysis cache fixture.".to_string(),
        capsule_url: "https://cdn.example.test/signal-harbor.jpg".to_string(),
        store_screenshot_urls: vec!["https://cdn.example.test/signal-harbor-1.jpg".to_string()],
        tags: vec!["Co-op".to_string(), "Strategy".to_string()],
        multiplayer_modes: vec!["Online Co-op".to_string()],
        review_snippets: vec![ReviewSnippet {
            voted_up: true,
            review: "Chaotic storms make every rescue memorable.".to_string(),
            playtime_hours: Some(5.2),
        }],
        user_state: UserGameState::default(),
    };

    db::upsert_game(&conn, &card).expect("upsert game");

    let report = GameAnalysisReport {
        appid: card.appid,
        generated_at: "2026-04-30T09:15:00Z".to_string(),
        source: AnalysisSource::Rule,
        confidence: AnalysisConfidence::Medium,
        overall_score: 76.0,
        overview: "Strong co-op signals with some onboarding friction.".to_string(),
        dimension_scores: vec![AnalysisDimensionScore {
            key: "co_op_depth".to_string(),
            label: "Co-op Depth".to_string(),
            score: 81.0,
            reason: "Layered coordination mechanics appear consistently in store metadata.".to_string(),
        }],
        strengths: vec![AnalysisPoint {
            title: "Clear team roles".to_string(),
            reason: "Review and tag signals both point to role-based co-op play.".to_string(),
        }],
        risks: vec![AnalysisPoint {
            title: "Steep onboarding".to_string(),
            reason: "Early-session complexity may slow first-match retention.".to_string(),
        }],
        evidence: vec![AnalysisEvidenceItem {
            kind: AnalysisEvidenceKind::Tags,
            label: "Top tags".to_string(),
            value: "Co-op, Strategy".to_string(),
            interpretation: "Store tags reinforce the coordination-heavy multiplayer angle."
                .to_string(),
        }],
        review_evidence: vec![AnalysisReviewEvidenceItem {
            stance: AnalysisReviewStance::Strength,
            quote: "The teamwork clicks once everyone owns a station.".to_string(),
            playtime_text: "12.4 hrs on record".to_string(),
            interpretation: "Players praise the role clarity after a short learning curve."
                .to_string(),
        }],
    };

    db::save_game_analysis(&conn, card.appid, &report).expect("save game analysis");

    let loaded = db::load_game_analysis(&conn, card.appid)
        .expect("load game analysis")
        .expect("analysis exists");

    let stored_generated_at: Option<String> = conn
        .query_row(
            "SELECT ai_analysis_generated_at FROM games WHERE appid = ?1",
            [card.appid],
            |row| row.get(0),
        )
        .expect("query analysis generated at");

    assert_eq!(loaded.appid, card.appid);
    assert_eq!(loaded.generated_at, "2026-04-30T09:15:00Z");
    assert_eq!(loaded.source, AnalysisSource::Rule);
    assert_eq!(loaded.confidence, AnalysisConfidence::Medium);
    assert_eq!(loaded.overall_score, 76.0);
    assert_eq!(
        loaded.overview,
        "Strong co-op signals with some onboarding friction."
    );
    assert_eq!(stored_generated_at.as_deref(), Some("2026-04-30T09:15:00Z"));
    assert_eq!(loaded.dimension_scores.len(), 1);
    assert_eq!(loaded.dimension_scores[0].key, "co_op_depth");
    assert_eq!(loaded.dimension_scores[0].label, "Co-op Depth");
    assert_eq!(loaded.dimension_scores[0].score, 81.0);
    assert_eq!(
        loaded.dimension_scores[0].reason,
        "Layered coordination mechanics appear consistently in store metadata."
    );
    assert_eq!(loaded.strengths.len(), 1);
    assert_eq!(loaded.strengths[0].title, "Clear team roles");
    assert_eq!(
        loaded.strengths[0].reason,
        "Review and tag signals both point to role-based co-op play."
    );
    assert_eq!(loaded.risks.len(), 1);
    assert_eq!(loaded.risks[0].title, "Steep onboarding");
    assert_eq!(
        loaded.risks[0].reason,
        "Early-session complexity may slow first-match retention."
    );
    assert_eq!(loaded.evidence.len(), 1);
    assert_eq!(loaded.evidence[0].kind, AnalysisEvidenceKind::Tags);
    assert_eq!(loaded.evidence[0].label, "Top tags");
    assert_eq!(loaded.evidence[0].value, "Co-op, Strategy");
    assert_eq!(
        loaded.evidence[0].interpretation,
        "Store tags reinforce the coordination-heavy multiplayer angle."
    );
    assert_eq!(loaded.review_evidence.len(), 1);
    assert_eq!(
        loaded.review_evidence[0].stance,
        AnalysisReviewStance::Strength
    );
    assert_eq!(
        loaded.review_evidence[0].quote,
        "The teamwork clicks once everyone owns a station."
    );
    assert_eq!(
        loaded.review_evidence[0].playtime_text,
        "12.4 hrs on record"
    );
    assert_eq!(
        loaded.review_evidence[0].interpretation,
        "Players praise the role clarity after a short learning curve."
    );
}

#[test]
fn save_game_analysis_rejects_mismatched_report_appid() {
    let conn = empty_db();
    let card = GameCard {
        appid: 3_990_031,
        name: "Mismatch Dock".to_string(),
        section: "new".to_string(),
        short_description: None,
        release_date: None,
        release_date_text: "TBA".to_string(),
        release_state: StoreReleaseState::Tba,
        demo_status: DemoStatus::Released,
        supported_languages: vec!["english".to_string()],
        is_adult_content: false,
        price_text: None,
        discount_percent: None,
        positive_review_pct: None,
        total_reviews: None,
        current_players: None,
        recommendation_score: 40.0,
        ai_score: None,
        ai_summary: "Mismatch fixture.".to_string(),
        capsule_url: "https://cdn.example.test/mismatch-dock.jpg".to_string(),
        store_screenshot_urls: vec![],
        tags: vec![],
        multiplayer_modes: vec!["Online Co-op".to_string()],
        review_snippets: vec![],
        user_state: UserGameState::default(),
    };
    db::upsert_game(&conn, &card).expect("upsert game");

    let report = GameAnalysisReport {
        appid: card.appid + 1,
        generated_at: "2026-04-30T10:00:00Z".to_string(),
        source: AnalysisSource::Rule,
        confidence: AnalysisConfidence::Low,
        overall_score: 10.0,
        overview: "Mismatched report".to_string(),
        dimension_scores: vec![],
        strengths: vec![],
        risks: vec![],
        evidence: vec![],
        review_evidence: vec![],
    };

    let error = db::save_game_analysis(&conn, card.appid, &report).expect_err("should reject");
    assert!(
        error
            .to_string()
            .contains("report appid does not match target appid"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn save_game_analysis_requires_existing_game_row() {
    let conn = empty_db();
    let report = GameAnalysisReport {
        appid: 3_990_041,
        generated_at: "2026-04-30T10:05:00Z".to_string(),
        source: AnalysisSource::Rule,
        confidence: AnalysisConfidence::Low,
        overall_score: 22.0,
        overview: "Missing game row".to_string(),
        dimension_scores: vec![],
        strengths: vec![],
        risks: vec![],
        evidence: vec![],
        review_evidence: vec![],
    };

    let error = db::save_game_analysis(&conn, report.appid, &report).expect_err("should reject");
    assert!(
        error.to_string().contains("no game row found for appid"),
        "unexpected error: {error:#}"
    );
}
