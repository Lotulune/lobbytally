use crate::models::{AiAssessment, GameCard};
use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct LlmRuntimeConfig {
    pub api_key: Option<String>,
    pub base_url: String,
    pub model: String,
}

pub async fn assess_game(
    client: &Client,
    config: &LlmRuntimeConfig,
    game: &GameCard,
) -> Result<AiAssessment> {
    if config.api_key.is_none() {
        return Ok(heuristic_assessment(game));
    }

    let api_key = config.api_key.clone().unwrap();
    let base_url = config.base_url.trim_end_matches('/').to_string();
    let model = config.model.clone();
    let prompt = build_prompt(game);

    let response = client
        .post(format!("{base_url}/v1/chat/completions"))
        .bearer_auth(api_key)
        .json(&ChatRequest {
            model,
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content:
                        "You are a concise Steam multiplayer game curator. Return strict JSON only."
                            .to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: prompt,
                },
            ],
            temperature: 0.2,
        })
        .send()
        .await?
        .error_for_status()?
        .json::<ChatResponse>()
        .await
        .context("decode LLM response")?;

    let content = response
        .choices
        .first()
        .map(|choice| choice.message.content.trim())
        .filter(|content| !content.is_empty())
        .unwrap_or("{}");

    parse_assessment(game.appid, content).or_else(|_| Ok(heuristic_assessment(game)))
}

fn heuristic_assessment(game: &GameCard) -> AiAssessment {
    let score = game
        .ai_score
        .unwrap_or(game.recommendation_score)
        .clamp(0.0, 100.0);
    let player_phrase = match game.current_players.unwrap_or(0) {
        0..=50 => "当前在线样本偏小，适合把它当作小众潜力股观察。",
        51..=1000 => "在线人数不算夸张，但足够支持朋友小队尝试。",
        _ => "当前活跃度不错，临时组局和长期游玩都更安心。",
    };
    let review_phrase = match game.positive_review_pct.unwrap_or(0.0) {
        pct if pct >= 95.0 => "口碑非常稳。",
        pct if pct >= 85.0 => "口碑表现健康。",
        _ => "评价有分歧，需要看差评是否踩中你的雷点。",
    };

    AiAssessment {
        appid: game.appid,
        score,
        summary: format!(
            "{} {} 适合：{}。",
            review_phrase,
            player_phrase,
            game.multiplayer_modes
                .first()
                .cloned()
                .unwrap_or_else(|| "多人联机尝鲜".to_string())
        ),
        best_for: vec![
            "朋友开黑".to_string(),
            game.tags
                .first()
                .cloned()
                .unwrap_or_else(|| "独立游戏".to_string()),
            "多人筛选".to_string(),
        ],
        risks: if game.current_players.unwrap_or(0) < 100 {
            vec![
                "在线人数样本小".to_string(),
                "需要确认好友都能接受题材".to_string(),
            ]
        } else {
            vec!["长期内容量仍需结合近期评测判断".to_string()]
        },
    }
}

fn build_prompt(game: &GameCard) -> String {
    let positive_reviews = game
        .review_snippets
        .iter()
        .filter(|review| review.voted_up)
        .take(8)
        .map(|review| review.review.as_str())
        .collect::<Vec<_>>();
    let negative_reviews = game
        .review_snippets
        .iter()
        .filter(|review| !review.voted_up)
        .take(2)
        .map(|review| review.review.as_str())
        .collect::<Vec<_>>();

    serde_json::json!({
        "task": "Give a short multiplayer recommendation assessment in Simplified Chinese.",
        "output_schema": {
            "score": "0-100 number",
            "summary": "one concise Chinese sentence",
            "best_for": ["2-4 short Chinese labels"],
            "risks": ["1-3 short Chinese labels"]
        },
        "game": {
            "appid": game.appid,
            "name": game.name,
            "release_date": game.release_date,
            "demo_status": game.demo_status,
            "positive_review_pct": game.positive_review_pct,
            "total_reviews": game.total_reviews,
            "current_players": game.current_players,
            "tags": game.tags,
            "multiplayer_modes": game.multiplayer_modes,
            "positive_reviews": positive_reviews,
            "negative_reviews": negative_reviews,
        }
    })
    .to_string()
}

fn parse_assessment(appid: u32, content: &str) -> Result<AiAssessment> {
    #[derive(Debug, Deserialize)]
    struct Raw {
        score: f64,
        summary: String,
        best_for: Vec<String>,
        risks: Vec<String>,
    }

    let trimmed = content
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let raw: Raw = serde_json::from_str(trimmed)?;
    Ok(AiAssessment {
        appid,
        score: raw.score.clamp(0.0, 100.0),
        summary: raw.summary,
        best_for: raw.best_for,
        risks: raw.risks,
    })
}

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f32,
}

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Debug, Deserialize)]
struct ChatChoiceMessage {
    content: String,
}
