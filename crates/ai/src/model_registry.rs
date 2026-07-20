//! Live model registry: `/v1/models` discovery + capability canary cache.
//!
//! Models missing from the upstream list are skipped immediately and never
//! retried as if they were temporarily unhealthy.

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use crate::error::AiError;
use crate::types::{ApiProtocol, ModelCapabilities};

const DEFAULT_REGISTRY_TTL: Duration = Duration::from_secs(300);

#[derive(Debug, Clone)]
struct RegistrySnapshot {
    models: HashMap<String, ModelCapabilities>,
    fetched_at: Instant,
    ttl: Duration,
}

impl RegistrySnapshot {
    fn is_fresh(&self) -> bool {
        self.fetched_at.elapsed() < self.ttl
    }
}

/// In-memory model capability cache shared by the task router.
#[derive(Debug, Default)]
pub struct ModelRegistry {
    inner: RwLock<Option<RegistrySnapshot>>,
}

impl ModelRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the registry with a discovered model list.
    pub fn replace_models(&self, models: Vec<ModelCapabilities>, ttl: Duration) {
        let mut map = HashMap::with_capacity(models.len());
        for caps in models {
            map.insert(caps.model.clone(), caps);
        }
        let snapshot = RegistrySnapshot {
            models: map,
            fetched_at: Instant::now(),
            ttl,
        };
        *self.inner.write().expect("model registry lock") = Some(snapshot);
    }

    /// Seed known-good capabilities without an HTTP round-trip (tests / offline).
    pub fn seed(&self, models: impl IntoIterator<Item = ModelCapabilities>) {
        self.replace_models(models.into_iter().collect(), DEFAULT_REGISTRY_TTL);
    }

    pub fn is_fresh(&self) -> bool {
        self.inner
            .read()
            .expect("model registry lock")
            .as_ref()
            .is_some_and(RegistrySnapshot::is_fresh)
    }

    pub fn get(&self, model: &str) -> Option<ModelCapabilities> {
        self.inner
            .read()
            .expect("model registry lock")
            .as_ref()
            .and_then(|snap| snap.models.get(model).cloned())
    }

    /// Models that currently pass discovery and are marked available.
    pub fn available_models(&self) -> Vec<String> {
        self.inner
            .read()
            .expect("model registry lock")
            .as_ref()
            .map(|snap| {
                snap.models
                    .values()
                    .filter(|m| m.available)
                    .map(|m| m.model.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether a model is known-available. When the registry has never been
    /// populated, all models are treated as *unknown* (optimistically allowed)
    /// so single-model M5 deploys keep working before the first discovery run.
    pub fn is_model_allowed(&self, model: &str) -> bool {
        let guard = self.inner.read().expect("model registry lock");
        match guard.as_ref() {
            None => true,
            Some(snap) => snap.models.get(model).is_some_and(|caps| caps.available),
        }
    }

    /// Mark a model unavailable (e.g. repeated 404 model-not-found).
    pub fn mark_unavailable(&self, model: &str) {
        let mut guard = self.inner.write().expect("model registry lock");
        if let Some(snap) = guard.as_mut() {
            if let Some(caps) = snap.models.get_mut(model) {
                caps.available = false;
            } else {
                snap.models
                    .insert(model.to_owned(), ModelCapabilities::unavailable(model));
            }
        } else {
            let mut map = HashMap::new();
            map.insert(model.to_owned(), ModelCapabilities::unavailable(model));
            *guard = Some(RegistrySnapshot {
                models: map,
                fetched_at: Instant::now(),
                ttl: DEFAULT_REGISTRY_TTL,
            });
        }
    }

    pub fn record_protocol_success(&self, model: &str, protocol: ApiProtocol) {
        let mut guard = self.inner.write().expect("model registry lock");
        let snap = guard.get_or_insert_with(|| RegistrySnapshot {
            models: HashMap::new(),
            fetched_at: Instant::now(),
            ttl: DEFAULT_REGISTRY_TTL,
        });
        let caps = snap
            .models
            .entry(model.to_owned())
            .or_insert_with(|| ModelCapabilities {
                model: model.to_owned(),
                chat_completions: false,
                responses: false,
                structured_json: true,
                tool_calling: false,
                streaming: false,
                available: true,
            });
        caps.available = true;
        caps.structured_json = true;
        match protocol {
            ApiProtocol::ChatCompletions => caps.chat_completions = true,
            ApiProtocol::Responses => caps.responses = true,
        }
    }
}

/// Parse an OpenAI-compatible `/v1/models` JSON body into model ids.
pub fn parse_models_list(payload: &serde_json::Value) -> Result<Vec<String>, AiError> {
    let data = payload
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| AiError::InvalidOutput("models response missing data array".into()))?;
    let mut ids = Vec::with_capacity(data.len());
    for item in data {
        let id = item
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AiError::InvalidOutput("models item missing id".into()))?;
        if !id.trim().is_empty() {
            ids.push(id.to_owned());
        }
    }
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// Build optimistic capabilities for discovered model ids.
///
/// Protocol-specific flags stay false until canary or a successful request
/// records them — except we do not assume Chat ≡ Responses (PRD AI-004A).
pub fn capabilities_from_model_ids(
    ids: impl IntoIterator<Item = impl Into<String>>,
) -> Vec<ModelCapabilities> {
    ids.into_iter()
        .map(|id| {
            let model = id.into();
            ModelCapabilities {
                model,
                // Optimistic: most OpenAI-compatible gateways speak chat; Responses
                // is enabled only after canary or successful use.
                chat_completions: true,
                responses: false,
                structured_json: true,
                tool_calling: false,
                streaming: false,
                available: true,
            }
        })
        .collect()
}

/// Apply canary results onto a capability row.
pub fn apply_canary_result(
    caps: &mut ModelCapabilities,
    protocol: ApiProtocol,
    structured_ok: bool,
) {
    if !structured_ok {
        match protocol {
            ApiProtocol::ChatCompletions => caps.chat_completions = false,
            ApiProtocol::Responses => caps.responses = false,
        }
        if !caps.chat_completions && !caps.responses {
            caps.available = false;
            caps.structured_json = false;
        }
        return;
    }
    caps.available = true;
    caps.structured_json = true;
    match protocol {
        ApiProtocol::ChatCompletions => caps.chat_completions = true,
        ApiProtocol::Responses => caps.responses = true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_models_list_extracts_sorted_unique_ids() {
        let payload = json!({
            "data": [
                {"id": "grok-4.5"},
                {"id": "grok-4.3"},
                {"id": "grok-4.5"},
                {"id": "grok-chat-fast"}
            ]
        });
        let ids = parse_models_list(&payload).unwrap();
        assert_eq!(
            ids,
            vec![
                "grok-4.3".to_owned(),
                "grok-4.5".to_owned(),
                "grok-chat-fast".to_owned()
            ]
        );
    }

    #[test]
    fn missing_models_are_skipped_after_discovery() {
        let registry = ModelRegistry::new();
        registry.seed(capabilities_from_model_ids(["grok-4.3", "grok-chat-fast"]));
        assert!(registry.is_model_allowed("grok-4.3"));
        assert!(!registry.is_model_allowed("totally-missing-model"));
    }

    #[test]
    fn empty_registry_allows_models_for_legacy_single_model_deploys() {
        let registry = ModelRegistry::new();
        assert!(registry.is_model_allowed("any-model"));
    }

    #[test]
    fn canary_failure_on_chat_does_not_mark_responses_dead() {
        let mut caps = ModelCapabilities {
            model: "grok-4.5".into(),
            chat_completions: true,
            responses: true,
            structured_json: true,
            tool_calling: false,
            streaming: false,
            available: true,
        };
        apply_canary_result(&mut caps, ApiProtocol::ChatCompletions, false);
        assert!(!caps.chat_completions);
        assert!(caps.responses);
        assert!(caps.available);
    }

    #[test]
    fn canary_failure_on_all_protocols_marks_unavailable() {
        let mut caps = ModelCapabilities {
            model: "broken".into(),
            chat_completions: true,
            responses: false,
            structured_json: true,
            tool_calling: false,
            streaming: false,
            available: true,
        };
        apply_canary_result(&mut caps, ApiProtocol::ChatCompletions, false);
        assert!(!caps.available);
    }

    #[test]
    fn record_protocol_success_enables_responses_for_grok_45_style_models() {
        let registry = ModelRegistry::new();
        registry.record_protocol_success("grok-4.5", ApiProtocol::Responses);
        let caps = registry.get("grok-4.5").unwrap();
        assert!(caps.responses);
        assert!(caps.available);
        assert!(!caps.chat_completions);
    }
}
