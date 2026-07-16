use serde::{Deserialize, Serialize};
use serde_json::Value;

/// High-level online/offline task kinds used in logs and cache keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiTaskType {
    IntentParse,
    RankAnalysis,
    FeatureExtract,
    Embed,
}

/// Status returned to clients for AI-enhanced endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiStatus {
    Used,
    Cached,
    Fallback,
    Disabled,
}

impl AiStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuredResponse {
    pub provider: String,
    pub model: String,
    pub content: Value,
    pub usage_input: u32,
    pub usage_output: u32,
    pub latency_ms: u64,
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
