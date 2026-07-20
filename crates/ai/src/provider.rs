use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use crate::error::AiError;
use crate::types::{
    ApiProtocol, Embedding, EmbeddingInput, ProviderCapabilities, StructuredRequest,
    StructuredResponse,
};

#[async_trait]
pub trait AiProvider: Send + Sync {
    fn name(&self) -> &str;
    fn cache_identity(&self) -> String {
        self.name().to_owned()
    }
    fn capabilities(&self) -> ProviderCapabilities;
    fn is_available(&self) -> bool;
    async fn structured_completion(
        &self,
        request: StructuredRequest,
    ) -> Result<StructuredResponse, AiError>;
}

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn name(&self) -> &str;
    fn dimensions(&self) -> usize;
    fn is_available(&self) -> bool;
    async fn embed(&self, inputs: &[EmbeddingInput]) -> Result<Vec<Embedding>, AiError>;
}

/// Always-off provider used when no key is configured or AI is force-disabled.
#[derive(Debug, Default, Clone)]
pub struct DisabledProvider;

#[async_trait]
impl AiProvider for DisabledProvider {
    fn name(&self) -> &str {
        "disabled"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            name: "disabled".into(),
            structured_output: false,
            embeddings: false,
            max_context_tokens: 0,
            embedding_dimensions: None,
        }
    }

    fn is_available(&self) -> bool {
        false
    }

    async fn structured_completion(
        &self,
        _request: StructuredRequest,
    ) -> Result<StructuredResponse, AiError> {
        Err(AiError::Disabled)
    }
}

#[async_trait]
impl EmbeddingProvider for DisabledProvider {
    fn name(&self) -> &str {
        "disabled"
    }

    fn dimensions(&self) -> usize {
        0
    }

    fn is_available(&self) -> bool {
        false
    }

    async fn embed(&self, _inputs: &[EmbeddingInput]) -> Result<Vec<Embedding>, AiError> {
        Err(AiError::Disabled)
    }
}

/// Deterministic fake provider for unit/integration tests.
#[derive(Debug)]
pub struct FakeProvider {
    pub response: serde_json::Value,
    pub fail_with: Option<AiError>,
    pub available: bool,
    pub delay: Duration,
    /// Default model name reported on success when request.model is unset.
    pub default_model: String,
    /// Fail specific models before the global `fail_with`.
    pub fail_models: Mutex<HashMap<String, AiError>>,
    /// Fail specific (model, protocol) pairs — used to test protocol fallback.
    pub protocol_failures: Mutex<HashMap<(String, ApiProtocol), AiError>>,
    /// Record of models that were attempted (for assertions).
    pub attempted_models: Mutex<Vec<String>>,
}

impl Default for FakeProvider {
    fn default() -> Self {
        Self {
            response: serde_json::json!({
                "recommendations": [],
                "summary": "",
                "summary_evidence_ids": []
            }),
            fail_with: None,
            available: true,
            delay: Duration::ZERO,
            default_model: "fake-model".into(),
            fail_models: Mutex::new(HashMap::new()),
            protocol_failures: Mutex::new(HashMap::new()),
            attempted_models: Mutex::new(Vec::new()),
        }
    }
}

impl Clone for FakeProvider {
    fn clone(&self) -> Self {
        Self {
            response: self.response.clone(),
            fail_with: self.fail_with.clone(),
            available: self.available,
            delay: self.delay,
            default_model: self.default_model.clone(),
            fail_models: Mutex::new(self.fail_models.lock().expect("lock").clone()),
            protocol_failures: Mutex::new(self.protocol_failures.lock().expect("lock").clone()),
            attempted_models: Mutex::new(self.attempted_models.lock().expect("lock").clone()),
        }
    }
}

#[async_trait]
impl AiProvider for FakeProvider {
    fn name(&self) -> &str {
        "fake"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            name: "fake".into(),
            structured_output: true,
            embeddings: false,
            max_context_tokens: 8_192,
            embedding_dimensions: None,
        }
    }

    fn cache_identity(&self) -> String {
        format!("fake:{}", self.default_model)
    }

    fn is_available(&self) -> bool {
        self.available
    }

    async fn structured_completion(
        &self,
        request: StructuredRequest,
    ) -> Result<StructuredResponse, AiError> {
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        if !self.available {
            return Err(AiError::Disabled);
        }

        let model = request
            .model
            .clone()
            .unwrap_or_else(|| self.default_model.clone());
        let protocol = request.protocol.unwrap_or(ApiProtocol::ChatCompletions);

        self.attempted_models
            .lock()
            .expect("lock")
            .push(model.clone());

        if let Some(error) = self
            .protocol_failures
            .lock()
            .expect("lock")
            .get(&(model.clone(), protocol))
        {
            return Err(error.clone());
        }
        if let Some(error) = self.fail_models.lock().expect("lock").get(&model) {
            return Err(error.clone());
        }
        if let Some(error) = &self.fail_with {
            return Err(error.clone());
        }

        Ok(StructuredResponse {
            provider: "fake".into(),
            model,
            content: self.response.clone(),
            usage_input: request.system_prompt.len() as u32,
            usage_output: 32,
            prompt_cache_hit_tokens: None,
            prompt_cache_miss_tokens: None,
            latency_ms: 1,
            protocol: Some(protocol),
        })
    }
}

/// Deterministic local embedding for tests and offline smoke (hash bag-of-chars).
#[derive(Debug, Clone)]
pub struct HashEmbeddingProvider {
    pub dimensions: usize,
}

impl Default for HashEmbeddingProvider {
    fn default() -> Self {
        Self { dimensions: 32 }
    }
}

#[async_trait]
impl EmbeddingProvider for HashEmbeddingProvider {
    fn name(&self) -> &str {
        "hash-embed"
    }

    fn dimensions(&self) -> usize {
        self.dimensions.max(1)
    }

    fn is_available(&self) -> bool {
        true
    }

    async fn embed(&self, inputs: &[EmbeddingInput]) -> Result<Vec<Embedding>, AiError> {
        let dims = self.dimensions();
        let mut out = Vec::with_capacity(inputs.len());
        for input in inputs {
            let mut vector = vec![0.0f32; dims];
            for (i, ch) in input.text.chars().enumerate() {
                let idx = (ch as usize).wrapping_add(i).wrapping_mul(2_654_435_761) % dims;
                vector[idx] += 1.0;
            }
            crate::vector::l2_normalize(&mut vector);
            out.push(Embedding {
                id: input.id.clone(),
                model: "hash-embed-v2".into(),
                dimensions: dims,
                vector,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AiTaskType;
    use serde_json::json;

    #[tokio::test]
    async fn hash_embedding_uses_the_shared_v2_mapping() {
        let provider = HashEmbeddingProvider { dimensions: 64 };
        let result = provider
            .embed(&[EmbeddingInput {
                id: "one".into(),
                text: "a".into(),
            }])
            .await
            .unwrap();
        assert_eq!(result[0].vector[17], 1.0);
        assert_eq!(
            result[0]
                .vector
                .iter()
                .filter(|value| **value != 0.0)
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn fake_provider_honors_model_and_protocol_override() {
        let provider = FakeProvider::default();
        let response = provider
            .structured_completion(StructuredRequest {
                task: AiTaskType::RankExplain,
                system_prompt: "s".into(),
                data_prompt: "d".into(),
                json_schema_name: "t".into(),
                json_schema: json!({}),
                max_output_tokens: 10,
                temperature: 0.0,
                model: Some("grok-4.5".into()),
                protocol: Some(ApiProtocol::Responses),
            })
            .await
            .unwrap();
        assert_eq!(response.model, "grok-4.5");
        assert_eq!(response.protocol, Some(ApiProtocol::Responses));
    }
}
