use tauri_app_lib::game_analysis::{
    apply_narrative_patch, build_rule_report, summarize_report_as_assessment,
};
use tauri_app_lib::llm::AnalysisNarrative;
use tauri_app_lib::models::{
    AnalysisConfidence, AnalysisPoint, AnalysisSource, GameCard, ReviewSnippet, StoreReleaseState,
    UserGameState,
};
use tauri_app_lib::recommendation::DemoStatus;

fn rich_fixture_game() -> GameCard {
    GameCard {
        appid: 7301,
        name: "Harbor Crew".to_string(),
        short_description: Some("A cooperative harbor sim with short-session runs.".to_string()),
        section: "new".to_string(),
        release_date: Some("2026-03-18".to_string()),
        release_date_text: "2026-03-18".to_string(),
        release_state: StoreReleaseState::Released,
        demo_status: DemoStatus::ReleasedWithDemo,
        supported_languages: vec!["English".to_string(), "Simplified Chinese".to_string()],
        is_adult_content: false,
        price_text: Some("$19.99".to_string()),
        discount_percent: Some(15),
        positive_review_pct: Some(91.0),
        total_reviews: Some(1248),
        current_players: Some(1860),
        recommendation_score: 86.0,
        ai_score: Some(88.0),
        ai_summary: "placeholder".to_string(),
        capsule_url: "https://example.com/capsule.jpg".to_string(),
        store_screenshot_urls: vec!["https://example.com/shot-1.jpg".to_string()],
        tags: vec![
            "Co-op".to_string(),
            "Simulation".to_string(),
            "Casual".to_string(),
        ],
        multiplayer_modes: vec!["Online Co-op".to_string(), "LAN Co-op".to_string()],
        review_snippets: vec![
            ReviewSnippet {
                voted_up: true,
                review: "Great with friends and easy to teach.".to_string(),
                playtime_hours: Some(18.4),
            },
            ReviewSnippet {
                voted_up: true,
                review: "Runs are short but the teamwork feels rewarding.".to_string(),
                playtime_hours: Some(9.2),
            },
            ReviewSnippet {
                voted_up: false,
                review: "Late-game variety is still a bit thin.".to_string(),
                playtime_hours: Some(14.7),
            },
        ],
        user_state: UserGameState::default(),
    }
}

#[test]
fn build_rule_report_returns_rich_rule_report() {
    let report = build_rule_report(&rich_fixture_game(), "2026-04-30T12:00:00Z".to_string())
        .expect("rule report should build");

    assert_eq!(report.source, AnalysisSource::Rule);
    assert_eq!(report.confidence, AnalysisConfidence::High);
    assert_eq!(report.dimension_scores.len(), 5);
    assert!(!report.strengths.is_empty());
    assert!(!report.risks.is_empty());
    assert!(
        report.evidence.iter().any(|item| item.label == "好评率"),
        "expected 好评率 evidence, got {:?}",
        report.evidence
    );
    assert!(report.review_evidence.len() >= 2);
}

#[test]
fn apply_narrative_patch_updates_text_without_changing_evidence_shape() {
    let report = build_rule_report(&rich_fixture_game(), "2026-04-30T12:00:00Z".to_string())
        .expect("rule report should build");
    let original_evidence = serde_json::to_value(&report.evidence).expect("serialize evidence");
    let original_review_evidence =
        serde_json::to_value(&report.review_evidence).expect("serialize review evidence");
    let original_dimension_count = report.dimension_scores.len();

    let patched = apply_narrative_patch(
        report,
        AnalysisNarrative {
            overview: "这是一款适合固定朋友局的轻量合作港口经营游戏。".to_string(),
            strengths: vec![AnalysisPoint {
                title: "开黑门槛低".to_string(),
                reason: "规则直观，新手很快就能进入协作节奏。".to_string(),
            }],
            risks: vec![AnalysisPoint {
                title: "后期内容偏薄".to_string(),
                reason: "差评提到中后段变化不足，长线留存要观察。".to_string(),
            }],
            dimension_reasons: vec![
                (
                    "approachability".to_string(),
                    "上手说明直白，前几局就能理解核心循环。".to_string(),
                ),
                (
                    "content_depth".to_string(),
                    "中期内容有亮点，但后续扩展性仍需补强。".to_string(),
                ),
            ],
        },
    );

    assert_eq!(
        patched.overview,
        "这是一款适合固定朋友局的轻量合作港口经营游戏。"
    );
    assert_eq!(patched.strengths[0].title, "开黑门槛低");
    assert_eq!(patched.risks[0].title, "后期内容偏薄");
    assert_eq!(patched.dimension_scores.len(), original_dimension_count);
    assert_eq!(
        serde_json::to_value(&patched.evidence).expect("serialize patched evidence"),
        original_evidence
    );
    assert_eq!(
        serde_json::to_value(&patched.review_evidence).expect("serialize patched review evidence"),
        original_review_evidence
    );
    assert!(patched
        .dimension_scores
        .iter()
        .find(|dimension| dimension.key == "approachability")
        .expect("approachability dimension")
        .reason
        .contains("上手说明直白"));
    assert!(patched
        .dimension_scores
        .iter()
        .find(|dimension| dimension.key == "content_depth")
        .expect("content_depth dimension")
        .reason
        .contains("扩展性仍需补强"));
}

#[test]
fn summarize_report_as_assessment_reuses_core_fields() {
    let report = build_rule_report(&rich_fixture_game(), "2026-04-30T12:00:00Z".to_string())
        .expect("rule report should build");

    let assessment = summarize_report_as_assessment(&report);

    assert_eq!(assessment.appid, report.appid);
    assert_eq!(assessment.summary, report.overview);
    assert_eq!(assessment.score, report.overall_score);
    assert!(!assessment.risks.is_empty());
}
