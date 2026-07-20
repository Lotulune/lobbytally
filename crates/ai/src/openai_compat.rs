use std::{
    fmt,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::error::AiError;
use crate::model_registry::{capabilities_from_model_ids, parse_models_list};
use crate::provider::{AiProvider, EmbeddingProvider};
use crate::types::{
    ApiProtocol, Embedding, EmbeddingInput, ModelCapabilities, ProviderCapabilities,
    StructuredRequest, StructuredResponse,
};
use crate::vector::l2_normalize;

const MAX_COMPLETION_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_EMBEDDING_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_PROBE_RESPONSE_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomBaseUrlResolution {
    pub base_url: String,
    pub host: String,
    pub address: SocketAddr,
}

fn validated_base_url(raw: impl Into<String>, label: &str) -> Result<String, AiError> {
    let raw = raw.into();
    let parsed = reqwest::Url::parse(raw.trim())
        .map_err(|error| AiError::Config(format!("{label} is invalid: {error}")))?;
    let local_http = parsed.scheme() == "http"
        && matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if parsed.scheme() != "https" && !local_http {
        return Err(AiError::Config(format!(
            "{label} must be https:// or exact localhost/127.0.0.1/[::1] HTTP"
        )));
    }
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(AiError::Config(format!(
            "{label} must not contain credentials, query, or fragment"
        )));
    }
    Ok(parsed.as_str().trim_end_matches('/').to_owned())
}

/// Validate a user-configured OpenAI-compatible endpoint. Environment-owned
/// providers retain the looser local-development policy above; user settings
/// are always HTTPS and must resolve exclusively to public addresses.
pub async fn validate_custom_base_url(raw: impl Into<String>) -> Result<String, AiError> {
    Ok(resolve_custom_base_url(raw).await?.base_url)
}

/// Validate a user endpoint and retain the exact public address that passed
/// validation. The caller must use this resolution for the subsequent request
/// so a DNS rebinding cannot swap the destination between validation and use.
pub async fn resolve_custom_base_url(
    raw: impl Into<String>,
) -> Result<CustomBaseUrlResolution, AiError> {
    let validated = validated_base_url(raw, "custom AI base URL")?;
    let parsed = reqwest::Url::parse(&validated)
        .map_err(|error| AiError::Config(format!("custom AI base URL is invalid: {error}")))?;
    if parsed.scheme() != "https" {
        return Err(AiError::Config(
            "custom AI base URL must use HTTPS".to_owned(),
        ));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| AiError::Config("custom AI base URL must have a host".to_owned()))?;
    let port = parsed.port_or_known_default().unwrap_or(443);
    if let Ok(ip) = host.parse::<IpAddr>() {
        if !is_public_ip(ip) {
            return Err(AiError::Config(
                "custom AI base URL must not target a private or local address".to_owned(),
            ));
        }
        return Ok(CustomBaseUrlResolution {
            base_url: validated,
            host: host.to_owned(),
            address: SocketAddr::new(ip, port),
        });
    }
    let addresses = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| AiError::Config("custom AI host could not be resolved".to_owned()))?;
    let mut selected = None;
    for address in addresses {
        if !is_public_ip(address.ip()) {
            return Err(AiError::Config(
                "custom AI host resolves to a private or local address".to_owned(),
            ));
        }
        selected.get_or_insert(SocketAddr::new(address.ip(), port));
    }
    let Some(address) = selected else {
        return Err(AiError::Config(
            "custom AI host did not resolve to an address".to_owned(),
        ));
    };
    Ok(CustomBaseUrlResolution {
        base_url: validated,
        host: host.to_owned(),
        address,
    })
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_ipv4(ip),
        IpAddr::V6(ip) => is_public_ipv6(ip),
    }
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    if a == 0
        || a == 10
        || (a == 100 && (64..=127).contains(&b))
        || a == 127
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && (b == 0 || b == 168))
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224
    {
        return false;
    }
    true
}

fn is_public_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(mapped) = ip.to_ipv4() {
        return is_public_ipv4(mapped);
    }
    if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
        return false;
    }
    let segments = ip.segments();
    let first = segments[0];
    let unique_local = (first & 0xfe00) == 0xfc00;
    let link_local = (first & 0xffc0) == 0xfe80;
    let documentation = segments[0] == 0x2001 && segments[1] == 0x0db8;
    !(unique_local || link_local || documentation)
}

async fn read_limited_body(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<(reqwest::StatusCode, Vec<u8>), AiError> {
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(AiError::InvalidOutput(format!(
            "provider response exceeds {max_bytes} bytes"
        )));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| AiError::Transport(error.to_string()))?
    {
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err(AiError::InvalidOutput(format!(
                "provider response exceeds {max_bytes} bytes"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok((status, body))
}

/// OpenAI-compatible chat completions adapter (official OpenAI or compatible gateways).
#[derive(Clone)]
pub struct OpenAiCompatProvider {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub timeout: Duration,
    client: reqwest::Client,
}

impl fmt::Debug for OpenAiCompatProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenAiCompatProvider")
            .field("name", &self.name)
            .field("base_url", &self.base_url)
            .field("api_key", &"[redacted]")
            .field("model", &self.model)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl OpenAiCompatProvider {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, AiError> {
        let base_url = validated_base_url(base_url, "AI base URL")?;
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(AiError::Config("AI API key is empty".into()));
        }
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
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

    pub fn new_with_custom_resolution(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        timeout: Duration,
        resolution: &CustomBaseUrlResolution,
    ) -> Result<Self, AiError> {
        let mut provider = Self::new(base_url, api_key, model, timeout)?;
        provider.client = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .resolve(&resolution.host, resolution.address)
            .build()
            .map_err(|error| AiError::Config(error.to_string()))?;
        Ok(provider)
    }
}

/// Send the smallest supported authenticated probe before a custom setting is
/// stored. No upstream body is returned or logged, and redirects are disabled.
pub async fn test_custom_openai_connection(
    base_url: impl Into<String>,
    api_key: impl AsRef<str>,
) -> Result<(), AiError> {
    let resolution = resolve_custom_base_url(base_url).await?;
    let base_url = resolution.base_url;
    let api_key = api_key.as_ref();
    if api_key.trim().is_empty() {
        return Err(AiError::Config("AI API key is empty".to_owned()));
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none())
        .resolve(&resolution.host, resolution.address)
        .build()
        .map_err(|error| AiError::Config(error.to_string()))?;
    let response = client
        .get(format!("{base_url}/models"))
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                AiError::Timeout
            } else {
                AiError::Transport("custom AI connection test failed".to_owned())
            }
        })?;
    let (status, _body) = read_limited_body(response, MAX_PROBE_RESPONSE_BYTES).await?;
    if !status.is_success() {
        return Err(AiError::ProviderRejected(format!(
            "connection test returned HTTP {}",
            status.as_u16()
        )));
    }
    Ok(())
}

#[async_trait]
impl AiProvider for OpenAiCompatProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn cache_identity(&self) -> String {
        format!("{}:{}:{}", self.name, self.base_url, self.model)
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
        let model = request
            .model
            .as_deref()
            .unwrap_or(self.model.as_str())
            .to_owned();
        let protocol = request.protocol.unwrap_or(ApiProtocol::ChatCompletions);

        let (url, body) = match protocol {
            ApiProtocol::ChatCompletions => (
                format!("{}/chat/completions", self.base_url),
                build_chat_completions_body(&model, &request)?,
            ),
            ApiProtocol::Responses => (
                format!("{}/responses", self.base_url),
                build_responses_body(&model, &request)?,
            ),
        };

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

        let (status, bytes) = read_limited_body(response, MAX_COMPLETION_RESPONSE_BYTES).await?;
        if status.as_u16() == 429 {
            return Err(AiError::RateLimited);
        }
        if !status.is_success() {
            // Surface protocol vs generic HTTP failures so the router can try the
            // alternate protocol instead of treating the model as dead (AI-004A).
            let code = status.as_u16();
            let hint = if protocol == ApiProtocol::Responses && matches!(code, 404 | 405 | 501) {
                format!("HTTP {code} responses protocol not supported")
            } else if protocol == ApiProtocol::ChatCompletions && matches!(code, 404 | 405 | 501) {
                format!("HTTP {code} chat/completions protocol not supported")
            } else {
                format!("HTTP {code}")
            };
            return Err(AiError::ProviderRejected(hint));
        }

        let payload: Value = serde_json::from_slice(&bytes)
            .map_err(|error| AiError::InvalidOutput(error.to_string()))?;
        let content = match protocol {
            ApiProtocol::ChatCompletions => parse_chat_completions_content(&payload)?,
            ApiProtocol::Responses => parse_responses_content(&payload)?,
        };
        let (usage_input, usage_output, prompt_cache_hit_tokens, prompt_cache_miss_tokens) =
            parse_usage(&payload);

        Ok(StructuredResponse {
            provider: self.name.clone(),
            model,
            content,
            usage_input,
            usage_output,
            prompt_cache_hit_tokens,
            prompt_cache_miss_tokens,
            latency_ms: started.elapsed().as_millis() as u64,
            protocol: Some(protocol),
        })
    }
}

impl OpenAiCompatProvider {
    /// Discover models from `GET /v1/models` without logging secrets or bodies.
    pub async fn list_models(&self) -> Result<Vec<ModelCapabilities>, AiError> {
        let url = format!("{}/models", self.base_url);
        let response = self
            .client
            .get(url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    AiError::Timeout
                } else {
                    AiError::Transport(error.to_string())
                }
            })?;
        let (status, bytes) = read_limited_body(response, MAX_PROBE_RESPONSE_BYTES * 4).await?;
        if !status.is_success() {
            return Err(AiError::ProviderRejected(format!(
                "HTTP {}",
                status.as_u16()
            )));
        }
        let payload: Value = serde_json::from_slice(&bytes)
            .map_err(|error| AiError::InvalidOutput(error.to_string()))?;
        let ids = parse_models_list(&payload)?;
        Ok(capabilities_from_model_ids(ids))
    }
}

/// Build Chat Completions request body (OpenAI-compatible).
pub fn build_chat_completions_body(
    model: &str,
    request: &StructuredRequest,
) -> Result<Value, AiError> {
    let schema_text = serde_json::to_string(&request.json_schema)
        .map_err(|error| AiError::Config(error.to_string()))?;
    let system_prompt = format!(
        "{} Required JSON schema named {}: {}",
        request.system_prompt, request.json_schema_name, schema_text
    );
    Ok(json!({
        "model": model,
        "temperature": request.temperature,
        "max_tokens": request.max_output_tokens,
        // `json_object` is the interoperable OpenAI-compatible contract.
        // The gateway validates the returned object against the task schema
        // before it can influence ranking.
        "response_format": { "type": "json_object" },
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user", "content": request.data_prompt }
        ]
    }))
}

/// Build Responses API request body (OpenAI-compatible / Grok2API).
pub fn build_responses_body(model: &str, request: &StructuredRequest) -> Result<Value, AiError> {
    let schema_text = serde_json::to_string(&request.json_schema)
        .map_err(|error| AiError::Config(error.to_string()))?;
    let instructions = format!(
        "{} Required JSON schema named {}: {}. Output a single JSON object only.",
        request.system_prompt, request.json_schema_name, schema_text
    );
    Ok(json!({
        "model": model,
        "temperature": request.temperature,
        "max_output_tokens": request.max_output_tokens,
        "instructions": instructions,
        "input": [
            { "role": "user", "content": request.data_prompt }
        ],
        "text": {
            "format": { "type": "json_object" }
        }
    }))
}

/// Parse Chat Completions content into a JSON value.
pub fn parse_chat_completions_content(payload: &Value) -> Result<Value, AiError> {
    let content_text = payload
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .ok_or_else(|| AiError::InvalidOutput("missing choices[0].message.content".into()))?;
    serde_json::from_str(content_text)
        .map_err(|error| AiError::InvalidOutput(format!("content is not JSON: {error}")))
}

/// Parse Responses API content into a JSON value.
///
/// Supports common OpenAI / gateway shapes:
/// - `output_text` top-level string
/// - `output[].content[].text` / `output_text`
/// - `output[].content[].type == output_text`
pub fn parse_responses_content(payload: &Value) -> Result<Value, AiError> {
    if let Some(text) = payload.get("output_text").and_then(Value::as_str) {
        return serde_json::from_str(text)
            .map_err(|error| AiError::InvalidOutput(format!("output_text is not JSON: {error}")));
    }

    if let Some(output) = payload.get("output").and_then(Value::as_array) {
        for item in output {
            if let Some(content) = item.get("content").and_then(Value::as_array) {
                for part in content {
                    let text = part
                        .get("text")
                        .and_then(Value::as_str)
                        .or_else(|| part.get("output_text").and_then(Value::as_str));
                    if let Some(text) = text
                        && let Ok(value) = serde_json::from_str::<Value>(text)
                    {
                        return Ok(value);
                    }
                }
            }
            // Some gateways return a single message string on the item.
            if let Some(text) = item.get("text").and_then(Value::as_str)
                && let Ok(value) = serde_json::from_str::<Value>(text)
            {
                return Ok(value);
            }
        }
    }

    // Fallback: some proxies still wrap Responses like chat completions.
    if payload.pointer("/choices/0/message/content").is_some() {
        return parse_chat_completions_content(payload);
    }

    Err(AiError::InvalidOutput(
        "responses payload missing structured text output".into(),
    ))
}

fn parse_usage(payload: &Value) -> (u32, u32, Option<u32>, Option<u32>) {
    let usage_input = payload
        .pointer("/usage/prompt_tokens")
        .or_else(|| payload.pointer("/usage/input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    let usage_output = payload
        .pointer("/usage/completion_tokens")
        .or_else(|| payload.pointer("/usage/output_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    let prompt_cache_hit_tokens = payload
        .pointer("/usage/prompt_cache_hit_tokens")
        .or_else(|| payload.pointer("/usage/prompt_tokens_details/cached_tokens"))
        .or_else(|| payload.pointer("/usage/input_tokens_details/cached_tokens"))
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());
    let prompt_cache_miss_tokens = payload
        .pointer("/usage/prompt_cache_miss_tokens")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .or_else(|| prompt_cache_hit_tokens.map(|hit| usage_input.saturating_sub(hit)));
    (
        usage_input,
        usage_output,
        prompt_cache_hit_tokens,
        prompt_cache_miss_tokens,
    )
}

/// OpenAI-compatible `/embeddings` adapter.
#[derive(Clone)]
pub struct OpenAiCompatEmbeddingProvider {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub dimensions: usize,
    pub timeout: Duration,
    client: reqwest::Client,
}

impl fmt::Debug for OpenAiCompatEmbeddingProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenAiCompatEmbeddingProvider")
            .field("name", &self.name)
            .field("base_url", &self.base_url)
            .field("api_key", &"[redacted]")
            .field("model", &self.model)
            .field("dimensions", &self.dimensions)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl OpenAiCompatEmbeddingProvider {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        dimensions: usize,
        timeout: Duration,
    ) -> Result<Self, AiError> {
        let base_url = validated_base_url(base_url, "AI embedding base URL")?;
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
            .redirect(reqwest::redirect::Policy::none())
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
            "dimensions": self.dimensions,
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
        let (status, bytes) = read_limited_body(response, MAX_EMBEDDING_RESPONSE_BYTES).await?;
        if status.as_u16() == 429 {
            return Err(AiError::RateLimited);
        }
        if !status.is_success() {
            return Err(AiError::ProviderRejected(format!(
                "HTTP {}",
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
        let mut out: Vec<Option<Embedding>> = vec![None; inputs.len()];
        for (position, item) in data.iter().enumerate() {
            let idx = item
                .get("index")
                .and_then(Value::as_u64)
                .map(usize::try_from)
                .transpose()
                .map_err(|_| AiError::InvalidOutput("embedding index is too large".into()))?
                .unwrap_or(position);
            let input = inputs.get(idx).ok_or_else(|| {
                AiError::InvalidOutput(format!("embedding index {idx} is out of range"))
            })?;
            if out[idx].is_some() {
                return Err(AiError::InvalidOutput(format!(
                    "duplicate embedding index {idx}"
                )));
            }
            let values = item
                .get("embedding")
                .and_then(Value::as_array)
                .ok_or_else(|| AiError::InvalidOutput(format!("data[{idx}].embedding missing")))?;
            let mut vector = Vec::with_capacity(values.len());
            for value in values {
                let f = value.as_f64().ok_or_else(|| {
                    AiError::InvalidOutput(format!("data[{idx}].embedding non-float"))
                })? as f32;
                vector.push(f);
            }
            if vector.len() != self.dimensions {
                return Err(AiError::InvalidOutput(format!(
                    "data[{idx}].embedding dimensions {} != configured {}",
                    vector.len(),
                    self.dimensions
                )));
            }
            l2_normalize(&mut vector);
            out[idx] = Some(Embedding {
                id: input.id.clone(),
                model: self.model.clone(),
                dimensions: vector.len(),
                vector,
            });
        }
        out.into_iter()
            .enumerate()
            .map(|(idx, item)| {
                item.ok_or_else(|| AiError::InvalidOutput(format!("missing embedding index {idx}")))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AiTaskType;

    fn sample_request() -> StructuredRequest {
        StructuredRequest {
            task: AiTaskType::RankExplain,
            system_prompt: "sys".into(),
            data_prompt: "data".into(),
            json_schema_name: "rank".into(),
            json_schema: json!({"type": "object"}),
            max_output_tokens: 100,
            temperature: 0.0,
            model: None,
            protocol: None,
        }
    }

    #[test]
    fn base_url_requires_exact_local_http_host() {
        for invalid in [
            "http://localhost.evil.example/v1",
            "http://localhost@evil.example/v1",
            "http://127.0.0.1.evil.example/v1",
        ] {
            assert!(
                OpenAiCompatProvider::new(invalid, "key", "model", Duration::from_secs(1)).is_err()
            );
        }
        assert!(
            OpenAiCompatProvider::new(
                "http://localhost:1234/v1",
                "key",
                "model",
                Duration::from_secs(1)
            )
            .is_ok()
        );
    }

    #[test]
    fn cache_identity_changes_with_model_or_endpoint() {
        let first = OpenAiCompatProvider::new(
            "https://provider.example/v1",
            "key",
            "model-a",
            Duration::from_secs(1),
        )
        .unwrap();
        let second = OpenAiCompatProvider::new(
            "https://provider.example/v1",
            "key",
            "model-b",
            Duration::from_secs(1),
        )
        .unwrap();
        assert_ne!(first.cache_identity(), second.cache_identity());
    }

    #[tokio::test]
    async fn custom_endpoints_reject_local_private_and_non_https_addresses() {
        for endpoint in [
            "http://example.com/v1",
            "https://127.0.0.1/v1",
            "https://10.0.0.1/v1",
            "https://192.168.1.10/v1",
            "https://[::1]/v1",
        ] {
            assert!(
                validate_custom_base_url(endpoint).await.is_err(),
                "endpoint should be rejected: {endpoint}"
            );
        }
    }

    #[test]
    fn chat_body_uses_requested_model_override() {
        let mut request = sample_request();
        request.model = Some("grok-4.3".into());
        let body = build_chat_completions_body("default-model", &request).unwrap();
        // When building for an explicit model argument (router-selected), that wins.
        assert_eq!(body["model"], "default-model");
        assert_eq!(body["response_format"]["type"], "json_object");
        assert!(body["messages"].as_array().unwrap().len() >= 2);
    }

    #[test]
    fn responses_body_declares_json_object_format() {
        let body = build_responses_body("grok-4.5", &sample_request()).unwrap();
        assert_eq!(body["model"], "grok-4.5");
        assert_eq!(body["text"]["format"]["type"], "json_object");
        assert!(body.get("instructions").and_then(Value::as_str).is_some());
        assert!(body.get("input").and_then(Value::as_array).is_some());
    }

    #[test]
    fn parse_responses_output_text() {
        let payload = json!({
            "output_text": "{\"summary\":\"ok\",\"recommendations\":[]}",
            "usage": { "input_tokens": 10, "output_tokens": 4 }
        });
        let content = parse_responses_content(&payload).unwrap();
        assert_eq!(content["summary"], "ok");
    }

    #[test]
    fn parse_responses_nested_output_content() {
        let payload = json!({
            "output": [{
                "type": "message",
                "content": [{
                    "type": "output_text",
                    "text": "{\"ok\":true}"
                }]
            }]
        });
        let content = parse_responses_content(&payload).unwrap();
        assert_eq!(content["ok"], true);
    }

    #[test]
    fn parse_chat_completions_content_roundtrip() {
        let payload = json!({
            "choices": [{
                "message": { "content": "{\"a\":1}" }
            }]
        });
        let content = parse_chat_completions_content(&payload).unwrap();
        assert_eq!(content["a"], 1);
    }
}
