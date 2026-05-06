use tauri_app_lib::recommendation::{
    bucket_game, compute_recommendation_score, DemoStatus, GameFacts, ReleaseBucket,
};

fn base_facts() -> GameFacts {
    GameFacts {
        appid: 1,
        name: "Moonlit Co-op".to_string(),
        release_date: Some("2026-04-20".to_string()),
        positive_review_pct: Some(92.0),
        total_reviews: Some(820),
        current_players: Some(1200),
        multiplayer_modes: vec!["Online Co-op".to_string(), "Co-op".to_string()],
        demo_status: DemoStatus::ReleasedWithDemo,
        ai_score: Some(88.0),
    }
}

#[test]
fn scoring_rewards_balanced_multiplayer_recommendations() {
    let score = compute_recommendation_score(&base_facts(), "2026-04-26");

    assert!((88.0..=94.0).contains(&score));
}

#[test]
fn scoring_keeps_low_player_hidden_gems_above_burial_threshold() {
    let mut facts = base_facts();
    facts.positive_review_pct = Some(97.0);
    facts.total_reviews = Some(180);
    facts.current_players = Some(18);
    facts.ai_score = Some(91.0);

    let score = compute_recommendation_score(&facts, "2026-04-26");

    assert!(score >= 80.0, "score was {score}");
}

#[test]
fn scoring_without_ai_score_does_not_get_a_hidden_default_boost() {
    let mut facts = base_facts();
    facts.ai_score = None;

    let score = compute_recommendation_score(&facts, "2026-04-26");

    assert!(
        score < 80.0,
        "score should stay conservative without a real AI score, got {score}"
    );
}

#[test]
fn bucket_game_splits_recent_and_classic_games() {
    assert_eq!(bucket_game(&base_facts(), "2026-04-26"), ReleaseBucket::New);

    let mut hidden_classic = base_facts();
    hidden_classic.release_date = Some("2020-05-13".to_string());

    assert_eq!(
        bucket_game(&hidden_classic, "2026-04-26"),
        ReleaseBucket::ClassicHidden
    );

    let mut classic = hidden_classic;
    classic.total_reviews = Some(1_200);

    assert_eq!(bucket_game(&classic, "2026-04-26"), ReleaseBucket::Classic);
}
