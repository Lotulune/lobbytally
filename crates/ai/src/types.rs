use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

/// High-level online/offline task kinds used in logs, routing, and cache keys.
///
/// Wire names follow PRD task ids (`intent_parse`, `rank_explain`, …). Legacy
/// `rank_analysis` is accepted as an alias of `rank_explain`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiTaskType {
    IntentParse,
    #[serde(alias = "rank_analysis")]
    RankExplain,
    FeatureExtract,
    Embed,
    GameSummary,
    CompareGames,
    GroupAdvice,
    DataQuality,
}

impl AiTaskType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IntentParse => "intent_parse",
            Self::RankExplain => "rank_explain",
            Self::FeatureExtract => "feature_extract",
            Self::Embed => "embed",
            Self::GameSummary => "game_summary",
            Self::CompareGames => "compare_games",
            Self::GroupAdvice => "group_advice",
            Self::DataQuality => "data_quality",
        }
    }

    /// Online ranking analysis task (PRD `rank_explain`).
    pub const RANK_ANALYSIS: Self = Self::RankExplain;
}

/// Upstream protocol used for structured completions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiProtocol {
    ChatCompletions,
    Responses,
}

impl ApiProtocol {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat_completions",
            Self::Responses => "responses",
        }
    }
}

/// Status returned to clients for AI-enhanced endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiStatus {
    /// Base results returned; AI enhancement still running.
    Pending,
    Used,
    Cached,
    Fallback,
    Disabled,
}

impl AiStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Used => "used",
            Self::Cached => "cached",
            Self::Fallback => "fallback",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuredRequest {
    pub task: AiTaskType,
    pub system_prompt: String,
    /// Untrusted game/user-adjacent materials; never treated as instructions.
    pub data_prompt: String,
    pub json_schema_name: String,
    pub json_schema: Value,
    pub max_output_tokens: u32,
    pub temperature: f32,
    /// Optional multi-model override. When set, the provider must use this model
    /// instead of its construction-time default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Preferred upstream protocol. Providers that lack the protocol should
    /// return a protocol-specific rejection rather than a generic model failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<ApiProtocol>,
}

impl StructuredRequest {
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_protocol(mut self, protocol: ApiProtocol) -> Self {
        self.protocol = Some(protocol);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuredResponse {
    pub provider: String,
    pub model: String,
    pub content: Value,
    pub usage_input: u32,
    pub usage_output: u32,
    pub prompt_cache_hit_tokens: Option<u32>,
    pub prompt_cache_miss_tokens: Option<u32>,
    pub latency_ms: u64,
    /// Protocol that actually produced the response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<ApiProtocol>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbeddingInput {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Embedding {
    pub id: String,
    pub vector: Vec<f32>,
    pub model: String,
    pub dimensions: usize,
}

/// Validated AI ranking item after server-side schema checks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiRankItem {
    pub app_id: u32,
    pub fit_score: f64,
    pub confidence: f64,
    pub reason_evidence_ids: Vec<String>,
    pub reasons: Vec<String>,
    pub cautions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiRankResult {
    pub recommendations: Vec<AiRankItem>,
    pub summary: String,
    pub summary_evidence_ids: Vec<String>,
}

/// Capability snapshot for meta/runtime decisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub name: String,
    pub structured_output: bool,
    pub embeddings: bool,
    pub max_context_tokens: u32,
    pub embedding_dimensions: Option<usize>,
}

/// Per-model capability flags discovered via `/v1/models` and canary probes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub model: String,
    pub chat_completions: bool,
    pub responses: bool,
    pub structured_json: bool,
    pub tool_calling: bool,
    pub streaming: bool,
    pub available: bool,
}

impl ModelCapabilities {
    pub fn unavailable(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            chat_completions: false,
            responses: false,
            structured_json: false,
            tool_calling: false,
            streaming: false,
            available: false,
        }
    }

    /// Preferred protocols for this model, Responses first when proven available.
    pub fn preferred_protocols(&self) -> Vec<ApiProtocol> {
        let mut protocols = Vec::new();
        if self.responses {
            protocols.push(ApiProtocol::Responses);
        }
        if self.chat_completions {
            protocols.push(ApiProtocol::ChatCompletions);
        }
        protocols
    }
}

/// Configured route for one AI task: primary model, fallbacks, limits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskRouteConfig {
    pub task: AiTaskType,
    pub primary_model: String,
    pub fallback_models: Vec<String>,
    /// Protocol preference order for this task. Empty means "use model capability order".
    pub protocol_preference: Vec<ApiProtocol>,
    pub timeout: Duration,
    pub max_output_tokens: u32,
    pub enabled: bool,
    /// Bumped when route policy changes so caches invalidate.
    pub route_version: String,
}

impl TaskRouteConfig {
    pub fn model_chain(&self) -> Vec<&str> {
        let mut chain = Vec::with_capacity(1 + self.fallback_models.len());
        chain.push(self.primary_model.as_str());
        for model in &self.fallback_models {
            if !chain.contains(&model.as_str()) {
                chain.push(model.as_str());
            }
        }
        chain
    }
}

/// Outcome of a routed structured completion, including which model/protocol won.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutedCompletion {
    pub response: StructuredResponse,
    pub task: AiTaskType,
    pub route_version: String,
    pub attempted_models: Vec<String>,
    pub used_fallback: bool,
}

/// Build a stable AI cache key. Includes every PRD-required dimension.
pub fn build_ai_cache_key(
    task: AiTaskType,
    model: &str,
    prompt_version: &str,
    route_version: &str,
    data_snapshot_hash: &str,
    input_hash: &str,
    preference_hash: &str,
) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    task.as_str().hash(&mut hasher);
    model.hash(&mut hasher);
    prompt_version.hash(&mut hasher);
    route_version.hash(&mut hasher);
    data_snapshot_hash.hash(&mut hasher);
    input_hash.hash(&mut hasher);
    preference_hash.hash(&mut hasher);
    format!(
        "ai:{}:{}:{}:{}:{:016x}",
        task.as_str(),
        sanitize_key_part(model),
        sanitize_key_part(prompt_version),
        sanitize_key_part(route_version),
        hasher.finish()
    )
}

fn sanitize_key_part(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_wire_names_match_prd() {
        assert_eq!(AiTaskType::IntentParse.as_str(), "intent_parse");
        assert_eq!(AiTaskType::RankExplain.as_str(), "rank_explain");
        assert_eq!(AiTaskType::GameSummary.as_str(), "game_summary");
        assert_eq!(AiTaskType::CompareGames.as_str(), "compare_games");
        assert_eq!(AiTaskType::GroupAdvice.as_str(), "group_advice");
        assert_eq!(AiTaskType::DataQuality.as_str(), "data_quality");
    }

    #[test]
    fn rank_analysis_alias_deserializes_to_rank_explain() {
        let task: AiTaskType = serde_json::from_str("\"rank_analysis\"").unwrap();
        assert_eq!(task, AiTaskType::RankExplain);
        let task: AiTaskType = serde_json::from_str("\"rank_explain\"").unwrap();
        assert_eq!(task, AiTaskType::RankExplain);
        assert_eq!(
            serde_json::to_string(&AiTaskType::RankExplain).unwrap(),
            "\"rank_explain\""
        );
    }

    #[test]
    fn cache_key_changes_when_route_or_model_changes() {
        let base = build_ai_cache_key(
            AiTaskType::RankExplain,
            "grok-4.3",
            "rank-v5",
            "route-v1",
            "snap-a",
            "input-a",
            "pref-a",
        );
        let model_changed = build_ai_cache_key(
            AiTaskType::RankExplain,
            "grok-chat-fast",
            "rank-v5",
            "route-v1",
            "snap-a",
            "input-a",
            "pref-a",
        );
        let route_changed = build_ai_cache_key(
            AiTaskType::RankExplain,
            "grok-4.3",
            "rank-v5",
            "route-v2",
            "snap-a",
            "input-a",
            "pref-a",
        );
        assert_ne!(base, model_changed);
        assert_ne!(base, route_changed);
        assert_eq!(
            base,
            build_ai_cache_key(
                AiTaskType::RankExplain,
                "grok-4.3",
                "rank-v5",
                "route-v1",
                "snap-a",
                "input-a",
                "pref-a",
            )
        );
    }

    #[test]
    fn pending_status_is_wire_compatible() {
        assert_eq!(AiStatus::Pending.as_str(), "pending");
        assert_eq!(
            serde_json::to_string(&AiStatus::Pending).unwrap(),
            "\"pending\""
        );
    }

    #[test]
    fn model_capabilities_prefer_responses_when_available() {
        let caps = ModelCapabilities {
            model: "grok-4.5".into(),
            chat_completions: true,
            responses: true,
            structured_json: true,
            tool_calling: false,
            streaming: false,
            available: true,
        };
        assert_eq!(
            caps.preferred_protocols(),
            vec![ApiProtocol::Responses, ApiProtocol::ChatCompletions]
        );
    }

    #[test]
    fn task_route_model_chain_dedupes_primary() {
        let route = TaskRouteConfig {
            task: AiTaskType::IntentParse,
            primary_model: "a".into(),
            fallback_models: vec!["a".into(), "b".into()],
            protocol_preference: vec![],
            timeout: Duration::from_secs(5),
            max_output_tokens: 256,
            enabled: true,
            route_version: "v1".into(),
        };
        assert_eq!(route.model_chain(), vec!["a", "b"]);
    }
}
