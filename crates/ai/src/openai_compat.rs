use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::error::AiError;
use crate::provider::{AiProvider, EmbeddingProvider};
use crate::types::{
    Embedding, EmbeddingInput, ProviderCapabilities, StructuredRequest, StructuredResponse,
};
use crate::vector::l2_normalize;

/// OpenAI-compatible chat completions adapter (official OpenAI or compatible gateways).
#[derive(Debug, Clone)]
pub struct OpenAiCompatProvider {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub timeout: Duration,
    client: reqwest::Client,
}

impl OpenAiCompatProvider {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, AiError> {
        let base_url = base_url.into().trim_end_matches('/').to_owned();
        if base_url.is_empty() {
            return Err(AiError::Config("AI base URL is empty".into()));
        }
        if !(base_url.starts_with("https://") || base_url.starts_with("http://127.0.0.1") || base_url.starts_with("http://localhost")) {
            return Err(AiError::Config(
                "AI base URL must be https:// or localhost/127.0.0.1 for MVP".into(),
            ));
        }
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(AiError::Config("AI API key is empty".into()));
        }
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|error| AiError::Config(error.to_string()))?;
        Ok(Self {
            name: "openai_compat".into(),
            base_url,
            api_key,
            model: model.into(),
            timeout,
            client,
        })
    }
}

#[async_trait]
impl AiProvider for OpenAiCompatProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            name: self.name.clone(),
            structured_output: true,
            embeddings: false,
            max_context_tokens: 128_000,
            embedding_dimensions: None,
        }
    }

    fn is_available(&self) -> bool {
        true
    }

    async fn structured_completion(
        &self,
        request: StructuredRequest,
    ) -> Result<StructuredResponse, AiError> {
        let started = Instant::now();
        let url = format!("{}/chat/completions", self.base_url);
        let body = json!({
            "model": self.model,
            "temperature": request.temperature,
            "max_tokens": request.max_output_tokens,
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": request.json_schema_name,
                    "schema": request.json_schema,
                    "strict": true
                }
            },
            "messages": [
                { "role": "system", "content": request.system_prompt },
                { "role": "user", "content": request.data_prompt }
            ]
        });

        let response = self
            .client
            .post(url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    AiError::Timeout
                } else {
                    AiError::Transport(error.to_string())
                }
            })?;

        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| AiError::Transport(error.to_string()))?;
        if status.as_u16() == 429 {
            return Err(AiError::RateLimited);
        }
        if !status.is_success() {
            let snippet = String::from_utf8_lossy(&bytes);
            let safe = snippet.chars().take(200).collect::<String>();
            return Err(AiError::ProviderRejected(format!(
                "HTTP {} body={safe}",
                status.as_u16()
            )));
        }

        let payload: Value = serde_json::from_slice(&bytes)
            .map_err(|error| AiError::InvalidOutput(error.to_string()))?;
        let content_text = payload
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .ok_or_else(|| AiError::InvalidOutput("missing choices[0].message.content".into()))?;
        let content: Value = serde_json::from_str(content_text)
            .map_err(|error| AiError::InvalidOutput(format!("content is not JSON: {error}")))?;
        let usage_input = payload
            .pointer("/usage/prompt_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;
        let usage_output = payload
            .pointer("/usage/completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;

        Ok(StructuredResponse {
            provider: self.name.clone(),
            model: self.model.clone(),
            content,
            usage_input,
            usage_output,
            latency_ms: started.elapsed().as_millis() as u64,
        })
    }
}

/// OpenAI-compatible `/embeddings` adapter.
#[derive(Debug, Clone)]
pub struct OpenAiCompatEmbeddingProvider {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub dimensions: usize,
    pub timeout: Duration,
    client: reqwest::Client,
}

impl OpenAiCompatEmbeddingProvider {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        dimensions: usize,
        timeout: Duration,
    ) -> Result<Self, AiError> {
        let base_url = base_url.into().trim_end_matches('/').to_owned();
        if base_url.is_empty() {
            return Err(AiError::Config("AI embedding base URL is empty".into()));
        }
        if !(base_url.starts_with("https://")
            || base_url.starts_with("http://127.0.0.1")
            || base_url.starts_with("http://localhost"))
        {
            return Err(AiError::Config(
                "AI embedding base URL must be https:// or localhost/127.0.0.1 for MVP".into(),
            ));
        }
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(AiError::Config("AI embedding API key is empty".into()));
        }
        if dimensions == 0 {
            return Err(AiError::Config(
                "AI embedding dimensions must be > 0".into(),
            ));
        }
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|error| AiError::Config(error.to_string()))?;
        Ok(Self {
            name: "openai_compat_embed".into(),
            base_url,
            api_key,
            model: model.into(),
            dimensions,
            timeout,
            client,
        })
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAiCompatEmbeddingProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn is_available(&self) -> bool {
        true
    }

    async fn embed(&self, inputs: &[EmbeddingInput]) -> Result<Vec<Embedding>, AiError> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        if inputs.len() > 64 {
            return Err(AiError::ProviderRejected(
                "embedding batch exceeds 64 inputs".into(),
            ));
        }
        let url = format!("{}/embeddings", self.base_url);
        let body = json!({
            "model": self.model,
            "input": inputs.iter().map(|i| &i.text).collect::<Vec<_>>(),
        });
        let response = self
            .client
            .post(url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    AiError::Timeout
                } else {
                    AiError::Transport(error.to_string())
                }
            })?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| AiError::Transport(error.to_string()))?;
        if status.as_u16() == 429 {
            return Err(AiError::RateLimited);
        }
        if !status.is_success() {
            let snippet = String::from_utf8_lossy(&bytes);
            let safe = snippet.chars().take(200).collect::<String>();
            return Err(AiError::ProviderRejected(format!(
                "HTTP {} body={safe}",
                status.as_u16()
            )));
        }
        let payload: Value = serde_json::from_slice(&bytes)
            .map_err(|error| AiError::InvalidOutput(error.to_string()))?;
        let data = payload
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| AiError::InvalidOutput("missing embeddings data array".into()))?;
        if data.len() != inputs.len() {
            return Err(AiError::InvalidOutput(format!(
                "embedding count {} != input count {}",
                data.len(),
                inputs.len()
            )));
        }
        let mut out = Vec::with_capacity(inputs.len());
        for (idx, (input, item)) in inputs.iter().zip(data.iter()).enumerate() {
            let values = item
                .get("embedding")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    AiError::InvalidOutput(format!("data[{idx}].embedding missing"))
                })?;
            let mut vector = Vec::with_capacity(values.len());
            for value in values {
                let f = value.as_f64().ok_or_else(|| {
                    AiError::InvalidOutput(format!("data[{idx}].embedding non-float"))
                })? as f32;
                vector.push(f);
            }
            if vector.is_empty() {
                return Err(AiError::InvalidOutput(format!(
                    "data[{idx}].embedding empty"
                )));
            }
            l2_normalize(&mut vector);
            out.push(Embedding {
                id: input.id.clone(),
                model: self.model.clone(),
                dimensions: vector.len(),
                vector,
            });
        }
        Ok(out)
    }
}
