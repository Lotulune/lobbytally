use reqwest::Client;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;
use tauri_app_lib::game_analysis::{
    apply_narrative_patch, build_rule_report, generate_game_analysis,
    summarize_report_as_assessment,
};
use tauri_app_lib::llm::{generate_analysis_narrative, AnalysisNarrative, LlmRuntimeConfig};
use tauri_app_lib::models::{
    AnalysisConfidence, AnalysisPoint, AnalysisReviewStance, AnalysisSource, GameCard,
    ReviewSnippet, StoreReleaseState, UserGameState,
};
use tauri_app_lib::recommendation::DemoStatus;

const EXPECTED_DIMENSION_KEYS: [&str; 5] = [
    "approachability",
    "multiplayer_fun",
    "content_depth",
    "reputation_stability",
    "activity_health",
];

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

fn late_negative_fixture_game() -> GameCard {
    let mut game = rich_fixture_game();
    game.review_snippets = vec![
        ReviewSnippet {
            voted_up: true,
            review: "Great onboarding for our group.".to_string(),
            playtime_hours: Some(12.1),
        },
        ReviewSnippet {
            voted_up: true,
            review: "Easy to jump into after work.".to_string(),
            playtime_hours: Some(8.7),
        },
        ReviewSnippet {
            voted_up: true,
            review: "Co-op tasks make communication fun.".to_string(),
            playtime_hours: Some(16.4),
        },
        ReviewSnippet {
            voted_up: true,
            review: "Good value for short weekly sessions.".to_string(),
            playtime_hours: Some(11.0),
        },
        ReviewSnippet {
            voted_up: false,
            review: "The endgame loop gets repetitive too quickly.".to_string(),
            playtime_hours: Some(21.3),
        },
    ];
    game
}

fn local_test_client() -> Client {
    Client::builder()
        .build()
        .expect("build local test HTTP client")
}

fn spawn_chat_completion_server(status_line: &str, body: &str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test server");
    let address = listener
        .local_addr()
        .expect("read local test server address");
    let status_line = status_line.to_string();
    let body = body.to_string();

    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer);
            let response = format!(
                "{status_line}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write test response");
            let _ = stream.flush();
        }
    });

    format!("http://{}", address)
}

fn spawn_recording_server(status_line: &str, body: &str) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test server");
    let address = listener
        .local_addr()
        .expect("read local test server address");
    let status_line = status_line.to_string();
    let body = body.to_string();
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buffer = [0_u8; 16_384];
            let bytes_read = stream.read(&mut buffer).expect("read request");
            let request = String::from_utf8_lossy(&buffer[..bytes_read]).to_string();
            tx.send(request).expect("send recorded request");

            let response = format!(
                "{status_line}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write test response");
            let _ = stream.flush();
        }
    });

    (format!("http://{}", address), rx)
}

#[test]
fn build_rule_report_returns_rich_rule_report() {
    let report = build_rule_report(&rich_fixture_game(), "2026-04-30T12:00:00Z".to_string())
        .expect("rule report should build");

    assert_eq!(report.source, AnalysisSource::Rule);
    assert_eq!(report.confidence, AnalysisConfidence::High);
    assert_eq!(report.dimension_scores.len(), 5);
    assert_eq!(
        report
            .dimension_scores
            .iter()
            .map(|dimension| dimension.key.as_str())
            .collect::<Vec<_>>(),
        EXPECTED_DIMENSION_KEYS
    );
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
fn build_rule_report_review_evidence_keeps_both_stances_when_negative_is_late() {
    let report = build_rule_report(
        &late_negative_fixture_game(),
        "2026-04-30T12:00:00Z".to_string(),
    )
    .expect("rule report should build");

    assert!(report
        .review_evidence
        .iter()
        .any(|item| item.stance == AnalysisReviewStance::Strength));
    assert!(report
        .review_evidence
        .iter()
        .any(|item| item.stance == AnalysisReviewStance::Risk));
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
fn apply_narrative_patch_with_valid_content_flips_source_to_hybrid() {
    let report = build_rule_report(&rich_fixture_game(), "2026-04-30T12:00:00Z".to_string())
        .expect("rule report should build");

    let patched = apply_narrative_patch(
        report,
        AnalysisNarrative {
            overview: "这款作品对固定好友局很友好，合作反馈比规则版文案更完整。".to_string(),
            strengths: vec![AnalysisPoint {
                title: "合作反馈明确".to_string(),
                reason: "正面评测集中提到沟通与分工的乐趣，适合稳定开黑。".to_string(),
            }],
            risks: vec![AnalysisPoint {
                title: "后段耐玩度待看".to_string(),
                reason: "负面反馈主要集中在中后期循环变化不足。".to_string(),
            }],
            dimension_reasons: vec![(
                "approachability".to_string(),
                "教程和合作目标都比较清楚，新玩家较快能加入节奏。".to_string(),
            )],
        },
    );

    assert_eq!(patched.source, AnalysisSource::Hybrid);
    assert_eq!(patched.strengths[0].title, "合作反馈明确");
}

#[test]
fn apply_narrative_patch_rejects_degraded_narrative_and_keeps_rule_report() {
    let report = build_rule_report(&rich_fixture_game(), "2026-04-30T12:00:00Z".to_string())
        .expect("rule report should build");
    let original = serde_json::to_value(&report).expect("serialize original report");

    let patched = apply_narrative_patch(
        report,
        AnalysisNarrative {
            overview: "   ".to_string(),
            strengths: vec![
                AnalysisPoint {
                    title: "".to_string(),
                    reason: "   ".to_string(),
                };
                6
            ],
            risks: vec![AnalysisPoint {
                title: " ".to_string(),
                reason: "".to_string(),
            }],
            dimension_reasons: vec![
                ("unknown_key".to_string(), "   ".to_string()),
                ("approachability".to_string(), " ".to_string()),
            ],
        },
    );

    assert_eq!(patched.source, AnalysisSource::Rule);
    assert_eq!(
        serde_json::to_value(&patched).expect("serialize patched report"),
        original
    );
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

#[tokio::test]
async fn generate_game_analysis_returns_rule_report_when_narrative_request_fails() {
    let client = local_test_client();
    let base_url = spawn_chat_completion_server("HTTP/1.1 500 Internal Server Error", "{}");
    let config = LlmRuntimeConfig {
        api_key: Some("test-key".to_string()),
        base_url,
        model: "gpt-test".to_string(),
    };

    let report = generate_game_analysis(
        &client,
        &config,
        &rich_fixture_game(),
        "2026-04-30T12:00:00Z".to_string(),
    )
    .await
    .expect("fallback to rule report");

    assert_eq!(report.source, AnalysisSource::Rule);
    assert_eq!(
        report
            .dimension_scores
            .iter()
            .map(|dimension| dimension.key.as_str())
            .collect::<Vec<_>>(),
        EXPECTED_DIMENSION_KEYS
    );
}

#[tokio::test]
async fn generate_game_analysis_keeps_rule_report_when_narrative_is_unusable() {
    let client = local_test_client();
    let base_url = spawn_chat_completion_server(
        "HTTP/1.1 200 OK",
        r#"{"choices":[{"message":{"content":"{\"overview\":\"   \",\"strengths\":[{\"title\":\"\",\"reason\":\" \"},{\"title\":\" \",\"reason\":\"\"},{\"title\":\"\",\"reason\":\"\"},{\"title\":\"\",\"reason\":\"\"},{\"title\":\"\",\"reason\":\"\"}],\"risks\":[{\"title\":\" \",\"reason\":\" \"}],\"dimensionReasons\":[[\"approachability\",\" \"],[\"unknown_key\",\"still bad\"]]}"}}]}"#,
    );
    let config = LlmRuntimeConfig {
        api_key: Some("test-key".to_string()),
        base_url,
        model: "gpt-test".to_string(),
    };

    let report = generate_game_analysis(
        &client,
        &config,
        &rich_fixture_game(),
        "2026-04-30T12:00:00Z".to_string(),
    )
    .await
    .expect("fallback to rule report");

    assert_eq!(report.source, AnalysisSource::Rule);
    assert!(!report.strengths.is_empty());
    assert!(!report.risks.is_empty());
    assert_eq!(
        report
            .dimension_scores
            .iter()
            .map(|dimension| dimension.key.as_str())
            .collect::<Vec<_>>(),
        EXPECTED_DIMENSION_KEYS
    );
}

#[tokio::test]
async fn generate_analysis_narrative_uses_single_v1_suffix_for_openai_compatible_base_urls() {
    let client = local_test_client();
    let (base_url, request_rx) = spawn_recording_server(
        "HTTP/1.1 200 OK",
        r#"{"choices":[{"message":{"content":"{\"overview\":\"兼容 /v1 基地址。\",\"strengths\":[{\"title\":\"请求成功\",\"reason\":\"不应重复拼接 /v1。\"}],\"risks\":[{\"title\":\"无\",\"reason\":\"测试夹具。\"}],\"dimensionReasons\":[[\"approachability\",\"路径归一化正确。\"]]}"}}]}"#,
    );
    let config = LlmRuntimeConfig {
        api_key: Some("test-key".to_string()),
        base_url: format!("{base_url}/v1"),
        model: "deepseek-v4-flash".to_string(),
    };

    let narrative = generate_analysis_narrative(
        &client,
        &config,
        &rich_fixture_game(),
        &build_rule_report(&rich_fixture_game(), "2026-04-30T12:00:00Z".to_string())
            .expect("rule report"),
    )
    .await
    .expect("decode narrative");
    let request = request_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("recorded request");

    assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
    assert!(!request.contains("/v1/v1/chat/completions"));
    assert_eq!(narrative.overview, "兼容 /v1 基地址。");
}

#[tokio::test]
async fn generate_analysis_narrative_supports_anthropic_messages_format() {
    let client = local_test_client();
    let (base_url, request_rx) = spawn_recording_server(
        "HTTP/1.1 200 OK",
        r#"{"id":"msg_123","type":"message","role":"assistant","content":[{"type":"text","text":"{\"overview\":\"Anthropic 兼容路径可用。\",\"strengths\":[{\"title\":\"协议兼容\",\"reason\":\"messages 响应已成功解析。\"}],\"risks\":[{\"title\":\"无\",\"reason\":\"测试夹具。\"}],\"dimensionReasons\":[[\"approachability\",\"Anthropic content text 已正确提取。\"]]}"}]}"#,
    );
    let config = LlmRuntimeConfig {
        api_key: Some("test-key".to_string()),
        base_url: format!("{base_url}/anthropic"),
        model: "claude-3-5-sonnet-20241022".to_string(),
    };

    let narrative = generate_analysis_narrative(
        &client,
        &config,
        &rich_fixture_game(),
        &build_rule_report(&rich_fixture_game(), "2026-04-30T12:00:00Z".to_string())
            .expect("rule report"),
    )
    .await
    .expect("decode anthropic narrative");
    let request = request_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("recorded request");
    let request_lower = request.to_lowercase();

    assert!(request.starts_with("POST /anthropic/v1/messages HTTP/1.1\r\n"));
    assert!(request_lower.contains("x-api-key: test-key"));
    assert!(request_lower.contains("anthropic-version: 2023-06-01"));
    assert!(request.contains("\"messages\":[{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\""));
    assert!(request.contains("\"system\":\"You refine rule-based Steam multiplayer analyses. Return strict JSON only.\""));
    assert_eq!(narrative.overview, "Anthropic 兼容路径可用。");
}
