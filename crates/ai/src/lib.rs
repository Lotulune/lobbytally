//! AI provider abstraction, validation, embeddings helpers, and runtime gateway.
//!
//! AI is an enhancement layer. Callers must always be able to fall back to
//! deterministic ranking when the provider is disabled, timed out, or invalid.

#![forbid(unsafe_code)]

pub mod error;
pub mod gateway;
pub mod openai_compat;
pub mod provider;
pub mod sanitize;
pub mod types;
pub mod validate;
pub mod vector;

pub use error::AiError;
pub use gateway::{AiGateway, AiPolicy};
pub use openai_compat::{OpenAiCompatEmbeddingProvider, OpenAiCompatProvider};
pub use provider::{
    AiProvider, DisabledProvider, EmbeddingProvider, FakeProvider, HashEmbeddingProvider,
};
pub use sanitize::{sanitize_untrusted_text, wrap_untrusted_data_block};
pub use types::*;
pub use validate::{CandidateEvidence, validate_rank_result};
pub use vector::{
    cosine_similarity, decode_f32_le, encode_f32_le, l2_normalize, reciprocal_rank_fusion,
};

use std::env;
use std::sync::Arc;
use std::time::Duration;

/// Build a gateway from environment variables.
///
/// - `MPGS_AI_PROVIDER=disabled|openai_compat` (default disabled)
/// - `MPGS_AI_API_KEY` required for openai_compat
/// - `MPGS_AI_BASE_URL` default `https://api.openai.com/v1`
/// - `MPGS_AI_MODEL` default `gpt-4o-mini`
/// - `MPGS_AI_TIMEOUT_SECS` default `12`
pub fn gateway_from_env() -> Result<AiGateway, AiError> {
    let kind = env::var("MPGS_AI_PROVIDER")
        .unwrap_or_else(|_| "disabled".into())
        .to_ascii_lowercase();
    match kind.as_str() {
        "" | "disabled" | "off" | "none" => Ok(AiGateway::disabled()),
        "openai_compat" | "openai" => {
            let api_key = env::var("MPGS_AI_API_KEY").map_err(|_| {
                AiError::Config("MPGS_AI_API_KEY is required for openai_compat".into())
            })?;
            let base_url = env::var("MPGS_AI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1".into());
            let model = env::var("MPGS_AI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into());
            let timeout_secs: u64 = env::var("MPGS_AI_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(12)
                .clamp(1, 60);
            let provider = OpenAiCompatProvider::new(
                base_url,
                api_key,
                model,
                Duration::from_secs(timeout_secs),
            )?;
            let policy = AiPolicy {
                online_timeout: Duration::from_secs(timeout_secs),
                ..AiPolicy::default()
            };
            Ok(AiGateway::new(Arc::new(provider), policy))
        }
        other => Err(AiError::Config(format!(
            "unknown MPGS_AI_PROVIDER '{other}' (expected disabled|openai_compat)"
        ))),
    }
}

/// Build an embedding provider from environment variables.
///
/// - `MPGS_AI_EMBED_PROVIDER=disabled|hash|openai_compat` (default `hash`)
/// - For `openai_compat`: reuses `MPGS_AI_API_KEY` / `MPGS_AI_BASE_URL`,
///   model from `MPGS_AI_EMBED_MODEL` (default `text-embedding-3-small`),
///   dimensions from `MPGS_AI_EMBED_DIMENSIONS` (default `1536`).
pub fn embedding_provider_from_env() -> Result<Arc<dyn EmbeddingProvider>, AiError> {
    let kind = env::var("MPGS_AI_EMBED_PROVIDER")
        .unwrap_or_else(|_| "hash".into())
        .to_ascii_lowercase();
    match kind.as_str() {
        "" | "disabled" | "off" | "none" => Ok(Arc::new(DisabledProvider)),
        "hash" | "hash-embed" | "local" => {
            let dimensions = env::var("MPGS_AI_EMBED_DIMENSIONS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(64)
                .clamp(8, 1024);
            Ok(Arc::new(HashEmbeddingProvider { dimensions }))
        }
        "openai_compat" | "openai" => {
            let api_key = env::var("MPGS_AI_API_KEY").map_err(|_| {
                AiError::Config("MPGS_AI_API_KEY is required for openai_compat embeddings".into())
            })?;
            let base_url = env::var("MPGS_AI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1".into());
            let model = env::var("MPGS_AI_EMBED_MODEL")
                .unwrap_or_else(|_| "text-embedding-3-small".into());
            let dimensions = env::var("MPGS_AI_EMBED_DIMENSIONS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1536)
                .clamp(8, 4096);
            let timeout_secs: u64 = env::var("MPGS_AI_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30)
                .clamp(1, 120);
            Ok(Arc::new(OpenAiCompatEmbeddingProvider::new(
                base_url,
                api_key,
                model,
                dimensions,
                Duration::from_secs(timeout_secs),
            )?))
        }
        other => Err(AiError::Config(format!(
            "unknown MPGS_AI_EMBED_PROVIDER '{other}' (expected disabled|hash|openai_compat)"
        ))),
    }
}

/// System prompt for online ranking analysis (not user-visible).
pub fn rank_analysis_system_prompt() -> &'static str {
    "You are MPGS ranking assistant. Only use provided candidate facts. \
     Never invent AppIDs, scores outside [0,1], URLs, HTML, or evidence IDs. \
     Return a single JSON object matching the schema. \
     If unsure, lower confidence and avoid concrete unsupported claims."
}

pub fn rank_analysis_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["recommendations", "summary"],
        "properties": {
            "summary": { "type": "string" },
            "recommendations": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": [
                        "app_id",
                        "fit_score",
                        "confidence",
                        "reason_evidence_ids",
                        "reasons",
                        "cautions"
                    ],
                    "properties": {
                        "app_id": { "type": "integer", "minimum": 1 },
                        "fit_score": { "type": "number", "minimum": 0, "maximum": 1 },
                        "confidence": { "type": "number", "minimum": 0, "maximum": 1 },
                        "reason_evidence_ids": {
                            "type": "array",
                            "items": { "type": "string" }
                        },
                        "reasons": {
                            "type": "array",
                            "items": { "type": "string" }
                        },
                        "cautions": {
                            "type": "array",
                            "items": { "type": "string" }
                        }
                    }
                }
            }
        }
    })
}
