use crate::models::{
    AiAssessment, AiRecommendationRequest, AiRecommendationResponse, AnalysisPoint,
    AnalysisSource,
    GameAnalysisReport, GameCard,
};
use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};

const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_ANTHROPIC_MAX_TOKENS: u32 = 1_200;
pub const ANALYSIS_NARRATIVE_SYSTEM_PROMPT: &str =
    "You refine rule-based Steam multiplayer analyses. Return strict JSON only.";
const ANALYSIS_NARRATIVE_CACHE_VERSION: &str = "analysis_narrative_v1";

#[derive(Debug, Clone)]
pub struct LlmRuntimeConfig {
    pub api_key: Option<String>,
    pub base_url: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisNarrative {
    pub overview: String,
    pub strengths: Vec<AnalysisPoint>,
    pub risks: Vec<AnalysisPoint>,
    pub dimension_reasons: Vec<(String, String)>,
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

    let content = request_chat_completion_content(
        client,
        &api_key,
        &base_url,
        &model,
        "You are a concise Steam multiplayer game curator. Return strict JSON only.",
        prompt,
        0.2,
    )
    .await?;

    parse_assessment(game.appid, &content).or_else(|_| Ok(heuristic_assessment(game)))
}

pub async fn generate_analysis_narrative(
    client: &Client,
    config: &LlmRuntimeConfig,
    game: &GameCard,
    rule_report: &GameAnalysisReport,
) -> Result<AnalysisNarrative> {
    let api_key = config
        .api_key
        .clone()
        .context("LLM API key is required for narrative generation")?;
    let base_url = config.base_url.trim_end_matches('/').to_string();
    let model = config.model.clone();
    let prompt = build_analysis_narrative_prompt(game, rule_report);
    let content = request_chat_completion_content(
        client,
        &api_key,
        &base_url,
        &model,
        ANALYSIS_NARRATIVE_SYSTEM_PROMPT,
        prompt,
        0.1,
    )
    .await?;

    Ok(serde_json::from_str(trim_json_content(&content)?)?)
}

pub async fn enhance_recommendation_response(
    client: &Client,
    config: &LlmRuntimeConfig,
    request: &AiRecommendationRequest,
    local_response: AiRecommendationResponse,
) -> AiRecommendationResponse {
    match request_recommendation_enhancement(client, config, request, &local_response).await {
        Ok(enhancement) => {
            let mut response = apply_recommendation_enhancement(local_response, enhancement);
            response.source = AnalysisSource::Hybrid;
            response.llm_used = true;
            response.diagnostic = Some("已调用配置的 LLM，对本地候选的回复、理由和风险提示做了增强。".to_string());
            response
        }
        Err(error) => {
            let mut response = local_response;
            response.source = AnalysisSource::Rule;
            response.llm_used = false;
            response.diagnostic = Some(format!("LLM 增强失败，已回退到规则匹配：{error}"));
            response
        }
    }
}

pub async fn validate_runtime_config(client: &Client, config: &LlmRuntimeConfig) -> Result<()> {
    let api_key = config
        .api_key
        .clone()
        .context("LLM API key is required for connectivity validation")?;
    let base_url = config.base_url.trim_end_matches('/').to_string();
    let model = config.model.trim().to_string();

    let _ = request_chat_completion_content(
        client,
        &api_key,
        &base_url,
        &model,
        "You are a connectivity test assistant. Reply with plain text only.",
        "Reply with OK.".to_string(),
        0.0,
    )
    .await?;

    Ok(())
}

pub fn build_analysis_narrative_cache_key(
    config: &LlmRuntimeConfig,
    game: &GameCard,
    rule_report: &GameAnalysisReport,
) -> String {
    let normalized_base_url = config.base_url.trim().trim_end_matches('/');
    let model = config.model.trim();
    let prompt = build_analysis_narrative_prompt(game, rule_report);
    let fingerprint_payload = serde_json::json!({
        "cache_version": ANALYSIS_NARRATIVE_CACHE_VERSION,
        "base_url": normalized_base_url,
        "model": model,
        "system_prompt": ANALYSIS_NARRATIVE_SYSTEM_PROMPT,
        "user_prompt": prompt,
    })
    .to_string();

    stable_cache_hex(&fingerprint_payload)
}

async fn request_recommendation_enhancement(
    client: &Client,
    config: &LlmRuntimeConfig,
    request: &AiRecommendationRequest,
    local_response: &AiRecommendationResponse,
) -> Result<RecommendationEnhancement> {
    let api_key = config
        .api_key
        .clone()
        .context("LLM API key is required for recommendation enhancement")?;
    let base_url = config.base_url.trim_end_matches('/').to_string();
    let prompt = build_recommendation_enhancement_prompt(request, local_response);
    let content = request_chat_completion_content(
        client,
        &api_key,
        &base_url,
        &config.model,
        "You polish rule-based Steam game recommendations. Return strict JSON only.",
        prompt,
        0.15,
    )
    .await?;

    Ok(serde_json::from_str(trim_json_content(&content)?)?)
}

fn apply_recommendation_enhancement(
    mut local_response: AiRecommendationResponse,
    enhancement: RecommendationEnhancement,
) -> AiRecommendationResponse {
    if !enhancement.reply.trim().is_empty() {
        local_response.reply = enhancement.reply.trim().to_string();
    }
    local_response.follow_up_question = enhancement
        .follow_up_question
        .filter(|value| !value.trim().is_empty())
        .or(local_response.follow_up_question);

    for item in &mut local_response.items {
        if let Some(enhanced_item) = enhancement
            .items
            .iter()
            .find(|candidate| candidate.appid == item.game.appid)
        {
            if !enhanced_item.reason.trim().is_empty() {
                item.reason = enhanced_item.reason.trim().to_string();
            }
            let caveats = enhanced_item
                .caveats
                .iter()
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
                .take(3)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            if !caveats.is_empty() {
                item.caveats = caveats;
            }
        }
    }

    local_response
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecommendationEnhancement {
    reply: String,
    follow_up_question: Option<String>,
    items: Vec<RecommendationEnhancementItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecommendationEnhancementItem {
    appid: u32,
    reason: String,
    caveats: Vec<String>,
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

    let trimmed = trim_json_content(content)?;
    let raw: Raw = serde_json::from_str(trimmed)?;
    Ok(AiAssessment {
        appid,
        score: raw.score.clamp(0.0, 100.0),
        summary: raw.summary,
        best_for: raw.best_for,
        risks: raw.risks,
    })
}

fn build_analysis_narrative_prompt(game: &GameCard, rule_report: &GameAnalysisReport) -> String {
    let mut normalized_rule_report = rule_report.clone();
    normalized_rule_report.generated_at.clear();
    serde_json::json!({
        "task": "Polish a rule-based multiplayer game analysis in Simplified Chinese without changing factual evidence.",
        "rules": [
            "Return strict JSON only.",
            "Do not invent facts outside the provided game metadata and rule report.",
            "Keep strengths and risks concise.",
            "dimensionReasons must only update reason text for existing dimension keys."
        ],
        "output_schema": {
            "overview": "one concise Chinese paragraph",
            "strengths": [{"title": "short Chinese title", "reason": "short Chinese reason"}],
            "risks": [{"title": "short Chinese title", "reason": "short Chinese reason"}],
            "dimensionReasons": [["dimension_key", "short Chinese reason"]]
        },
        "game": {
            "appid": game.appid,
            "name": game.name,
            "short_description": game.short_description,
            "tags": game.tags,
            "multiplayer_modes": game.multiplayer_modes,
            "positive_review_pct": game.positive_review_pct,
            "total_reviews": game.total_reviews,
            "current_players": game.current_players,
            "review_snippets": game.review_snippets,
        },
        "rule_report": normalized_rule_report,
    })
    .to_string()
}

fn build_recommendation_enhancement_prompt(
    request: &AiRecommendationRequest,
    local_response: &AiRecommendationResponse,
) -> String {
    let items = local_response
        .items
        .iter()
        .map(|item| {
            serde_json::json!({
                "appid": item.game.appid,
                "name": item.game.name,
                "matchScore": item.match_score,
                "reason": item.reason,
                "matchedTraits": item.matched_traits,
                "missingTraits": item.missing_traits,
                "caveats": item.caveats,
                "tags": item.game.tags,
                "multiplayerModes": item.game.multiplayer_modes,
                "positiveReviewPct": item.game.positive_review_pct,
                "totalReviews": item.game.total_reviews,
                "currentPlayers": item.game.current_players,
            })
        })
        .collect::<Vec<_>>();

    serde_json::json!({
        "task": "Polish an already-ranked in-library game recommendation response in Simplified Chinese.",
        "rules": [
            "Do not add, remove, reorder, or replace games.",
            "Use only the provided appids and facts.",
            "Be clear when games are near matches rather than exact matches.",
            "Keep each reason and caveat concise."
        ],
        "output_schema": {
            "reply": "one concise Chinese paragraph",
            "followUpQuestion": "optional one concise Chinese question or null",
            "items": [{"appid": "same existing appid", "reason": "Chinese reason", "caveats": ["1-3 Chinese caveats"]}]
        },
        "user_request": request.prompt,
        "context_messages": request.context_messages,
        "local_response": {
            "reply": local_response.reply,
            "followUpQuestion": local_response.follow_up_question,
            "exactMatchCount": local_response.exact_match_count,
            "items": items,
        }
    })
    .to_string()
}

fn stable_cache_hex(input: &str) -> String {
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET_BASIS;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }

    format!("{hash:016x}")
}

async fn request_chat_completion_content(
    client: &Client,
    api_key: &str,
    base_url: &str,
    model: &str,
    system_prompt: &str,
    user_prompt: String,
    temperature: f32,
) -> Result<String> {
    let endpoint = resolve_llm_endpoint(base_url);

    match endpoint.api_format {
        LlmApiFormat::OpenAiChatCompletions => {
            request_openai_compatible_content(
                client,
                api_key,
                &endpoint.url,
                model,
                system_prompt,
                user_prompt,
                temperature,
            )
            .await
        }
        LlmApiFormat::AnthropicMessages => {
            request_anthropic_compatible_content(
                client,
                api_key,
                &endpoint.url,
                model,
                system_prompt,
                user_prompt,
                temperature,
            )
            .await
        }
    }
}

fn trim_json_content(content: &str) -> Result<&str> {
    let trimmed = content
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    if trimmed.is_empty() {
        anyhow::bail!("empty LLM JSON content");
    }
    Ok(trimmed)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LlmApiFormat {
    OpenAiChatCompletions,
    AnthropicMessages,
}

#[derive(Debug, Clone)]
struct ResolvedLlmEndpoint {
    api_format: LlmApiFormat,
    url: String,
}

fn resolve_llm_endpoint(base_url: &str) -> ResolvedLlmEndpoint {
    let normalized_base_url = base_url.trim().trim_end_matches('/').to_string();
    let lower = normalized_base_url.to_ascii_lowercase();
    let api_format = if lower.contains("api.anthropic.com")
        || lower.contains("/anthropic")
        || lower.ends_with("/messages")
        || lower.ends_with("/v1/messages")
    {
        LlmApiFormat::AnthropicMessages
    } else {
        LlmApiFormat::OpenAiChatCompletions
    };

    let url = match api_format {
        LlmApiFormat::OpenAiChatCompletions => {
            if lower.ends_with("/v1/chat/completions") || lower.ends_with("/chat/completions") {
                normalized_base_url.clone()
            } else if lower.ends_with("/v1") {
                format!("{normalized_base_url}/chat/completions")
            } else {
                format!("{normalized_base_url}/v1/chat/completions")
            }
        }
        LlmApiFormat::AnthropicMessages => {
            if lower.ends_with("/v1/messages") || lower.ends_with("/messages") {
                normalized_base_url.clone()
            } else if lower.ends_with("/v1") {
                format!("{normalized_base_url}/messages")
            } else {
                format!("{normalized_base_url}/v1/messages")
            }
        }
    };

    ResolvedLlmEndpoint { api_format, url }
}

async fn request_openai_compatible_content(
    client: &Client,
    api_key: &str,
    endpoint_url: &str,
    model: &str,
    system_prompt: &str,
    user_prompt: String,
    temperature: f32,
) -> Result<String> {
    let response = client
        .post(endpoint_url)
        .bearer_auth(api_key)
        .json(&ChatRequest {
            model: model.to_string(),
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: system_prompt.to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: user_prompt,
                },
            ],
            temperature,
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
        .map(|choice| choice.message.content.trim().to_string())
        .filter(|content| !content.is_empty())
        .unwrap_or_else(|| "{}".to_string());
    Ok(content)
}

async fn request_anthropic_compatible_content(
    client: &Client,
    api_key: &str,
    endpoint_url: &str,
    model: &str,
    system_prompt: &str,
    user_prompt: String,
    temperature: f32,
) -> Result<String> {
    let response = client
        .post(endpoint_url)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .json(&AnthropicRequest {
            model: model.to_string(),
            system: system_prompt.to_string(),
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: vec![AnthropicMessageContentBlock {
                    kind: "text".to_string(),
                    text: user_prompt,
                }],
            }],
            max_tokens: DEFAULT_ANTHROPIC_MAX_TOKENS,
            temperature,
        })
        .send()
        .await?
        .error_for_status()?
        .json::<AnthropicResponse>()
        .await
        .context("decode LLM response")?;

    let content = response
        .content
        .iter()
        .filter(|block| block.kind == "text")
        .filter_map(|block| block.text.as_deref())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");

    if content.is_empty() {
        return Ok("{}".to_string());
    }

    Ok(content)
}

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    system: String,
    messages: Vec<AnthropicMessage>,
    max_tokens: u32,
    temperature: f32,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    content: Vec<AnthropicMessageContentBlock>,
}

#[derive(Debug, Serialize)]
struct AnthropicMessageContentBlock {
    #[serde(rename = "type")]
    kind: String,
    text: String,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicResponseContentBlock>,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponseContentBlock {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}
