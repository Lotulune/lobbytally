//! Multi-model task router with per-model circuit breakers and protocol fallback.
//!
//! AI is still an enhancement layer: when every model in a task chain fails, the
//! router surfaces a normal [`AiError`] so callers can return deterministic
//! results. Failures are never disguised as successful AI output.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::time::{Instant, timeout};

use crate::error::AiError;
use crate::model_registry::ModelRegistry;
use crate::provider::AiProvider;
use crate::types::{
    AiTaskType, ApiProtocol, ModelCapabilities, RoutedCompletion, StructuredRequest,
    TaskRouteConfig,
};
use serde::{Deserialize, Serialize};

/// Serializable task route for admin/settings UIs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRouteSnapshot {
    pub task: String,
    pub primary_model: String,
    pub fallback_models: Vec<String>,
    pub protocol_preference: Vec<String>,
    pub timeout_ms: u64,
    pub max_output_tokens: u32,
    pub enabled: bool,
    pub route_version: String,
    pub primary_available: bool,
}

/// Runtime budget + breaker policy applied around each model attempt.
#[derive(Debug, Clone)]
pub struct RouterPolicy {
    pub minute_budget: u32,
    pub daily_budget: u32,
    pub circuit_failure_threshold: u32,
    pub circuit_open_ms: u64,
}

impl Default for RouterPolicy {
    fn default() -> Self {
        Self {
            minute_budget: 60,
            daily_budget: 2_000,
            circuit_failure_threshold: 5,
            circuit_open_ms: 30_000,
        }
    }
}

#[derive(Debug, Default)]
struct ModelRuntimeState {
    consecutive_failures: AtomicU32,
    circuit_open_until_ms: AtomicU64,
}

#[derive(Debug, Default)]
struct GlobalBudget {
    minute_window: AtomicU64,
    minute_count: AtomicU32,
    day_window: AtomicU64,
    day_count: AtomicU32,
}

/// Routes structured completions by task: primary model → fallbacks → error.
pub struct TaskRouter {
    provider: Arc<dyn AiProvider>,
    routes: HashMap<AiTaskType, TaskRouteConfig>,
    registry: Arc<ModelRegistry>,
    policy: RouterPolicy,
    model_state: RwLockMap,
    budget: GlobalBudget,
}

/// Tiny interior mutability map without pulling parking_lot.
struct RwLockMap {
    inner: std::sync::Mutex<HashMap<String, Arc<ModelRuntimeState>>>,
}

impl RwLockMap {
    fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(HashMap::new()),
        }
    }

    fn get(&self, model: &str) -> Arc<ModelRuntimeState> {
        let mut guard = self.inner.lock().expect("model state lock");
        guard
            .entry(model.to_owned())
            .or_insert_with(|| Arc::new(ModelRuntimeState::default()))
            .clone()
    }
}

impl TaskRouter {
    pub fn new(
        provider: Arc<dyn AiProvider>,
        routes: HashMap<AiTaskType, TaskRouteConfig>,
        registry: Arc<ModelRegistry>,
        policy: RouterPolicy,
    ) -> Self {
        Self {
            provider,
            routes,
            registry,
            policy,
            model_state: RwLockMap::new(),
            budget: GlobalBudget::default(),
        }
    }

    pub fn from_provider(provider: Arc<dyn AiProvider>) -> Self {
        Self::new(
            provider,
            crate::route::default_task_routes(),
            Arc::new(ModelRegistry::new()),
            RouterPolicy::default(),
        )
    }

    pub fn registry(&self) -> &ModelRegistry {
        &self.registry
    }

    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }

    pub fn is_available(&self) -> bool {
        self.provider.is_available()
    }

    pub fn route_for(&self, task: AiTaskType) -> Option<&TaskRouteConfig> {
        self.routes.get(&task)
    }

    pub fn routes(&self) -> &HashMap<AiTaskType, TaskRouteConfig> {
        &self.routes
    }

    pub fn route_version(&self, task: AiTaskType) -> &str {
        self.routes
            .get(&task)
            .map(|r| r.route_version.as_str())
            .unwrap_or(crate::route::DEFAULT_ROUTE_VERSION)
    }

    /// Public snapshot for settings/meta UIs (no secrets).
    pub fn route_snapshot(&self) -> Vec<TaskRouteSnapshot> {
        let mut rows: Vec<_> = self
            .routes
            .values()
            .map(|route| {
                let allowed = self.registry.is_model_allowed(&route.primary_model);
                TaskRouteSnapshot {
                    task: route.task.as_str().to_owned(),
                    primary_model: route.primary_model.clone(),
                    fallback_models: route.fallback_models.clone(),
                    protocol_preference: route
                        .protocol_preference
                        .iter()
                        .map(|p| p.as_str().to_owned())
                        .collect(),
                    timeout_ms: route.timeout.as_millis() as u64,
                    max_output_tokens: route.max_output_tokens,
                    enabled: route.enabled,
                    route_version: route.route_version.clone(),
                    primary_available: allowed,
                }
            })
            .collect();
        rows.sort_by(|a, b| a.task.cmp(&b.task));
        rows
    }

    pub fn multi_model_active(&self) -> bool {
        let online: Vec<_> = self
            .routes
            .values()
            .filter(|r| {
                matches!(
                    r.task,
                    AiTaskType::IntentParse
                        | AiTaskType::RankExplain
                        | AiTaskType::CompareGames
                        | AiTaskType::GroupAdvice
                ) && r.enabled
            })
            .collect();
        if online.is_empty() {
            return false;
        }
        let first = online[0].primary_model.as_str();
        online.iter().any(|r| r.primary_model != first)
            || online.iter().any(|r| !r.fallback_models.is_empty())
    }

    /// Refresh model availability from the upstream provider (`/v1/models`).
    pub async fn refresh_model_registry(&self) -> Result<usize, AiError> {
        let models = self.provider.list_models().await?;
        let count = models.len();
        self.registry
            .replace_models(models, std::time::Duration::from_secs(300));
        Ok(count)
    }

    /// Execute a structured completion for `request.task`, walking the model chain.
    pub async fn structured_completion(
        &self,
        mut request: StructuredRequest,
    ) -> Result<RoutedCompletion, AiError> {
        if !self.provider.is_available() {
            return Err(AiError::Disabled);
        }

        let route = self.routes.get(&request.task).cloned().ok_or_else(|| {
            AiError::Config(format!(
                "no route configured for task {}",
                request.task.as_str()
            ))
        })?;

        if !route.enabled {
            return Err(AiError::Disabled);
        }
        let deadline = Instant::now() + route.timeout;

        // Allow explicit model override on the request to short-circuit routing
        // (tests / admin tools). Otherwise walk the configured chain.
        let chain: Vec<String> = if let Some(model) = request.model.clone() {
            vec![model]
        } else {
            route.model_chain().into_iter().map(str::to_owned).collect()
        };

        self.consume_budget()?;

        // Capture once: per-attempt `request.protocol = Some(...)` must not
        // collapse the next model's protocol preference list.
        let protocol_override = request.protocol;

        let mut attempted = Vec::new();
        let mut last_error = AiError::ProviderRejected("no models attempted".into());

        for (index, model) in chain.iter().enumerate() {
            if Instant::now() >= deadline {
                return Err(AiError::Timeout);
            }
            if !self.registry.is_model_allowed(model) {
                // PRD: missing models are skipped immediately, not retried.
                continue;
            }
            if self.model_circuit_open(model) {
                last_error = AiError::CircuitOpen;
                continue;
            }

            attempted.push(model.clone());
            let protocols = self.protocols_for_model(model, &route, protocol_override);
            if protocols.is_empty() {
                last_error = AiError::ProviderRejected(format!(
                    "model {model} has no usable protocol for task {}",
                    request.task.as_str()
                ));
                continue;
            }

            let mut protocol_error = None;
            for protocol in protocols {
                request.model = Some(model.clone());
                request.protocol = Some(protocol);
                // Cap tokens by route policy.
                if request.max_output_tokens > route.max_output_tokens {
                    request.max_output_tokens = route.max_output_tokens;
                }

                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(AiError::Timeout);
                }
                // Cap each attempt so a single hang cannot burn the whole chain
                // budget (still bounded by remaining shared deadline).
                let attempt_budget = per_attempt_budget(route.timeout, remaining);
                let result = timeout(
                    attempt_budget,
                    self.provider.structured_completion(request.clone()),
                )
                .await;

                match result {
                    Ok(Ok(mut response)) => {
                        response.protocol = Some(protocol);
                        if response.model.is_empty() {
                            response.model = model.clone();
                        }
                        self.note_model_success(model);
                        self.registry.record_protocol_success(model, protocol);
                        return Ok(RoutedCompletion {
                            response,
                            task: request.task,
                            route_version: route.route_version.clone(),
                            attempted_models: attempted,
                            used_fallback: index > 0,
                        });
                    }
                    Ok(Err(error)) => {
                        // Protocol unsupported: try next protocol before next model.
                        if is_protocol_rejection(&error) {
                            tracing::warn!(
                                model = %model,
                                task = %request.task.as_str(),
                                protocol = %protocol.as_str(),
                                error = %error,
                                "AI protocol attempt rejected; trying next protocol"
                            );
                            protocol_error = Some(error);
                            continue;
                        }
                        if is_model_missing(&error) {
                            self.registry.mark_unavailable(model);
                        }
                        tracing::warn!(
                            model = %model,
                            task = %request.task.as_str(),
                            protocol = %protocol.as_str(),
                            error = %error,
                            "AI model attempt failed; trying next model"
                        );
                        self.note_model_failure(model, &error);
                        last_error = error;
                        protocol_error = None;
                        break;
                    }
                    Err(_) => {
                        // Per-attempt budget hit. If shared deadline remains,
                        // soft-fail and try the next protocol/model; do not
                        // open the per-model circuit (may only have been slow).
                        last_error = AiError::Timeout;
                        protocol_error = None;
                        if Instant::now() >= deadline {
                            tracing::warn!(
                                model = %model,
                                task = %request.task.as_str(),
                                protocol = %protocol.as_str(),
                                "AI route deadline exhausted"
                            );
                            return Err(AiError::Timeout);
                        }
                        tracing::warn!(
                            model = %model,
                            task = %request.task.as_str(),
                            protocol = %protocol.as_str(),
                            "AI attempt timed out within route budget; trying next"
                        );
                        continue;
                    }
                }
            }
            if let Some(error) = protocol_error {
                self.note_model_failure(model, &error);
                last_error = error;
            }
        }

        if attempted.is_empty() {
            return Err(AiError::ProviderRejected(
                "no available models for task after registry filter".into(),
            ));
        }
        Err(last_error)
    }

    fn protocols_for_model(
        &self,
        model: &str,
        route: &TaskRouteConfig,
        request_override: Option<ApiProtocol>,
    ) -> Vec<ApiProtocol> {
        if let Some(protocol) = request_override {
            return vec![protocol];
        }

        let caps = self.registry.get(model).unwrap_or(ModelCapabilities {
            model: model.to_owned(),
            chat_completions: true,
            responses: true,
            structured_json: true,
            tool_calling: false,
            streaming: false,
            available: true,
        });

        let preferred = if route.protocol_preference.is_empty() {
            caps.preferred_protocols()
        } else {
            route.protocol_preference.clone()
        };

        preferred
            .into_iter()
            .filter(|p| match p {
                ApiProtocol::ChatCompletions => caps.chat_completions || !self.registry.is_fresh(),
                ApiProtocol::Responses => {
                    // Only attempt Responses when known-good or registry empty
                    // (unknown). After discovery, require the flag.
                    caps.responses || !self.registry.is_fresh()
                }
            })
            .collect()
    }

    fn model_circuit_open(&self, model: &str) -> bool {
        let state = self.model_state.get(model);
        let open_until = state.circuit_open_until_ms.load(Ordering::Relaxed);
        open_until > now_ms()
    }

    fn note_model_success(&self, model: &str) {
        let state = self.model_state.get(model);
        state.consecutive_failures.store(0, Ordering::Relaxed);
        state.circuit_open_until_ms.store(0, Ordering::Relaxed);
    }

    fn note_model_failure(&self, model: &str, error: &AiError) {
        if !matches!(
            error,
            AiError::Timeout
                | AiError::Transport(_)
                | AiError::RateLimited
                | AiError::ProviderRejected(_)
                | AiError::InvalidOutput(_)
        ) {
            return;
        }
        let state = self.model_state.get(model);
        let failures = state.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
        if failures >= self.policy.circuit_failure_threshold {
            let until = now_ms().saturating_add(self.policy.circuit_open_ms);
            state.circuit_open_until_ms.store(until, Ordering::Relaxed);
        }
    }

    fn consume_budget(&self) -> Result<(), AiError> {
        let wall = now_ms() / 1000;
        let minute = wall / 60;
        let day = wall / 86_400;

        let prev_minute = self.budget.minute_window.load(Ordering::Relaxed);
        if prev_minute != minute {
            self.budget.minute_window.store(minute, Ordering::Relaxed);
            self.budget.minute_count.store(0, Ordering::Relaxed);
        }
        let minute_count = self.budget.minute_count.fetch_add(1, Ordering::Relaxed) + 1;
        if minute_count > self.policy.minute_budget {
            return Err(AiError::BudgetExhausted);
        }

        let prev_day = self.budget.day_window.load(Ordering::Relaxed);
        if prev_day != day {
            self.budget.day_window.store(day, Ordering::Relaxed);
            self.budget.day_count.store(0, Ordering::Relaxed);
        }
        let day_count = self.budget.day_count.fetch_add(1, Ordering::Relaxed) + 1;
        if day_count > self.policy.daily_budget {
            return Err(AiError::BudgetExhausted);
        }
        Ok(())
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Per-attempt budget: at most half the route timeout, never more than remaining.
/// Keeps room for at least one more protocol/model try when the primary hangs.
fn per_attempt_budget(route_timeout: Duration, remaining: Duration) -> Duration {
    let half = (route_timeout / 2).max(Duration::from_millis(1));
    remaining.min(half)
}

fn is_protocol_rejection(error: &AiError) -> bool {
    match error {
        AiError::ProviderRejected(msg) => {
            let lower = msg.to_ascii_lowercase();
            lower.contains("protocol")
                || lower.contains("responses")
                || lower.contains("chat/completions")
                || lower.contains("not supported")
        }
        _ => false,
    }
}

fn is_model_missing(error: &AiError) -> bool {
    match error {
        AiError::ProviderRejected(msg) => {
            let lower = msg.to_ascii_lowercase();
            lower.contains("model")
                && (lower.contains("not found")
                    || lower.contains("does not exist")
                    || lower.contains("unknown")
                    || lower.contains("http 404"))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_registry::capabilities_from_model_ids;
    use crate::provider::FakeProvider;
    use crate::route::default_task_routes;
    use crate::types::AiTaskType;
    use serde_json::json;
    use std::sync::Mutex;
    use std::time::Duration;

    fn base_request(task: AiTaskType) -> StructuredRequest {
        StructuredRequest {
            task,
            system_prompt: "sys".into(),
            data_prompt: "data".into(),
            json_schema_name: "test".into(),
            json_schema: json!({"type": "object"}),
            max_output_tokens: 100,
            temperature: 0.0,
            model: None,
            protocol: None,
        }
    }

    #[derive(Debug, Default)]
    struct DeadlineProvider {
        attempted_models: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl AiProvider for DeadlineProvider {
        fn name(&self) -> &str {
            "deadline-test"
        }

        fn capabilities(&self) -> crate::types::ProviderCapabilities {
            crate::types::ProviderCapabilities {
                name: self.name().into(),
                structured_output: true,
                embeddings: false,
                max_context_tokens: 8_192,
                embedding_dimensions: None,
            }
        }

        fn is_available(&self) -> bool {
            true
        }

        async fn structured_completion(
            &self,
            request: StructuredRequest,
        ) -> Result<crate::types::StructuredResponse, AiError> {
            let model = request.model.expect("router supplies a model");
            self.attempted_models
                .lock()
                .expect("lock")
                .push(model.clone());
            if model == "primary" {
                return Err(AiError::Transport("upstream unavailable".into()));
            }
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn router_uses_primary_model_when_available() {
        let provider = FakeProvider {
            response: json!({"ok": true}),
            ..FakeProvider::default()
        };
        let registry = Arc::new(ModelRegistry::new());
        registry.seed(capabilities_from_model_ids([
            "grok-4.5",
            "grok-4.3",
            "grok-4.20-0309-non-reasoning",
        ]));
        let router = TaskRouter::new(
            Arc::new(provider),
            default_task_routes(),
            registry,
            RouterPolicy::default(),
        );
        let result = router
            .structured_completion(base_request(AiTaskType::RankExplain))
            .await
            .unwrap();
        assert_eq!(result.response.model, "grok-4.5");
        assert!(!result.used_fallback);
        assert_eq!(result.attempted_models, vec!["grok-4.5".to_owned()]);
    }

    #[tokio::test]
    async fn router_skips_missing_primary_and_uses_fallback() {
        let provider = FakeProvider {
            response: json!({"ok": true}),
            ..FakeProvider::default()
        };
        let registry = Arc::new(ModelRegistry::new());
        // Primary grok-4.5 is absent from discovery.
        registry.seed(capabilities_from_model_ids([
            "grok-4.20-0309-non-reasoning",
            "grok-4.3",
        ]));
        let router = TaskRouter::new(
            Arc::new(provider),
            default_task_routes(),
            registry,
            RouterPolicy::default(),
        );
        let result = router
            .structured_completion(base_request(AiTaskType::RankExplain))
            .await
            .unwrap();
        assert_eq!(result.response.model, "grok-4.20-0309-non-reasoning");
        assert!(result.used_fallback);
    }

    #[tokio::test]
    async fn router_falls_back_when_primary_errors() {
        let provider = FakeProvider {
            response: json!({"ok": true}),
            fail_models: Mutex::new(
                [("grok-4.5".into(), AiError::RateLimited)]
                    .into_iter()
                    .collect(),
            ),
            ..FakeProvider::default()
        };
        let registry = Arc::new(ModelRegistry::new());
        registry.seed(capabilities_from_model_ids([
            "grok-4.5",
            "grok-4.3",
            "grok-4.20-0309-non-reasoning",
        ]));
        let router = TaskRouter::new(
            Arc::new(provider),
            default_task_routes(),
            registry,
            RouterPolicy::default(),
        );
        let result = router
            .structured_completion(base_request(AiTaskType::RankExplain))
            .await
            .unwrap();
        assert_eq!(result.response.model, "grok-4.20-0309-non-reasoning");
        assert!(result.used_fallback);
        assert_eq!(
            result.attempted_models,
            vec![
                "grok-4.5".to_owned(),
                "grok-4.20-0309-non-reasoning".to_owned()
            ]
        );
    }

    #[tokio::test]
    async fn inferred_primary_success_does_not_block_unobserved_fallback() {
        let provider = Arc::new(FakeProvider {
            response: json!({"ok": true}),
            ..FakeProvider::default()
        });
        let registry = Arc::new(ModelRegistry::new());
        let router = TaskRouter::new(
            provider.clone(),
            default_task_routes(),
            registry,
            RouterPolicy::default(),
        );

        // Models discovery failed or was unavailable: the first successful
        // protocol observation must remain a partial, open registry snapshot.
        let first = router
            .structured_completion(base_request(AiTaskType::RankExplain))
            .await
            .unwrap();
        assert_eq!(first.response.model, "grok-4.5");

        provider
            .fail_models
            .lock()
            .expect("lock")
            .insert("grok-4.5".into(), AiError::RateLimited);
        let second = router
            .structured_completion(base_request(AiTaskType::RankExplain))
            .await
            .unwrap();
        assert_eq!(second.response.model, "grok-4.20-0309-non-reasoning");
        assert!(second.used_fallback);
    }

    #[tokio::test]
    async fn router_does_not_pretend_success_when_all_models_fail() {
        let provider = FakeProvider {
            fail_with: Some(AiError::Timeout),
            ..FakeProvider::default()
        };
        let registry = Arc::new(ModelRegistry::new());
        registry.seed(capabilities_from_model_ids([
            "grok-4.5",
            "grok-4.3",
            "grok-4.20-0309-non-reasoning",
        ]));
        let router = TaskRouter::new(
            Arc::new(provider),
            default_task_routes(),
            registry,
            RouterPolicy::default(),
        );
        let err = router
            .structured_completion(base_request(AiTaskType::RankExplain))
            .await
            .unwrap_err();
        assert_eq!(err, AiError::Timeout);
    }

    #[tokio::test]
    async fn per_model_circuit_breaker_skips_open_model() {
        let provider = FakeProvider {
            response: json!({"ok": true}),
            fail_models: Mutex::new(
                [("grok-4.5".into(), AiError::Timeout)]
                    .into_iter()
                    .collect(),
            ),
            ..FakeProvider::default()
        };
        let registry = Arc::new(ModelRegistry::new());
        registry.seed(capabilities_from_model_ids([
            "grok-4.5",
            "grok-4.20-0309-non-reasoning",
            "grok-4.3",
        ]));
        let policy = RouterPolicy {
            circuit_failure_threshold: 1,
            circuit_open_ms: 60_000,
            ..RouterPolicy::default()
        };
        let router = TaskRouter::new(Arc::new(provider), default_task_routes(), registry, policy);
        // First call trips breaker on primary, succeeds on fallback.
        let first = router
            .structured_completion(base_request(AiTaskType::RankExplain))
            .await
            .unwrap();
        assert!(first.used_fallback);
        // Second call should skip primary entirely due to open circuit.
        let second = router
            .structured_completion(base_request(AiTaskType::RankExplain))
            .await
            .unwrap();
        assert_eq!(
            second.attempted_models,
            vec!["grok-4.20-0309-non-reasoning".to_owned()]
        );
    }

    #[tokio::test]
    async fn chat_protocol_failure_retries_responses_before_next_model() {
        let provider = FakeProvider {
            response: json!({"ok": true}),
            protocol_failures: Mutex::new(
                [(
                    ("grok-4.5".into(), ApiProtocol::ChatCompletions),
                    AiError::ProviderRejected("protocol chat/completions not supported".into()),
                )]
                .into_iter()
                .collect(),
            ),
            default_model: "grok-4.5".into(),
            ..FakeProvider::default()
        };
        let mut routes = default_task_routes();
        routes.insert(
            AiTaskType::RankExplain,
            TaskRouteConfig {
                task: AiTaskType::RankExplain,
                primary_model: "grok-4.5".into(),
                fallback_models: vec!["should-not-be-used".into()],
                protocol_preference: vec![ApiProtocol::ChatCompletions, ApiProtocol::Responses],
                timeout: Duration::from_secs(5),
                max_output_tokens: 100,
                enabled: true,
                route_version: "test".into(),
            },
        );
        let registry = Arc::new(ModelRegistry::new());
        // Unknown registry → both protocols attempted.
        let router = TaskRouter::new(
            Arc::new(provider),
            routes,
            registry,
            RouterPolicy::default(),
        );
        let result = router
            .structured_completion(base_request(AiTaskType::RankExplain))
            .await
            .unwrap();
        assert_eq!(result.response.model, "grok-4.5");
        assert_eq!(result.response.protocol, Some(ApiProtocol::Responses));
        assert!(!result.used_fallback);
    }

    #[tokio::test]
    async fn route_timeout_is_shared_across_the_model_chain() {
        let provider = Arc::new(DeadlineProvider::default());
        let mut routes = default_task_routes();
        routes.insert(
            AiTaskType::RankExplain,
            TaskRouteConfig {
                task: AiTaskType::RankExplain,
                primary_model: "primary".into(),
                fallback_models: vec!["fallback-1".into(), "fallback-2".into()],
                protocol_preference: vec![ApiProtocol::Responses],
                // Per-attempt cap is half of this; hung models still leave
                // budget for the next model in the chain.
                timeout: Duration::from_millis(80),
                max_output_tokens: 100,
                enabled: true,
                route_version: "test".into(),
            },
        );
        let registry = Arc::new(ModelRegistry::new());
        registry.seed(capabilities_from_model_ids([
            "primary",
            "fallback-1",
            "fallback-2",
        ]));
        let router = TaskRouter::new(provider.clone(), routes, registry, RouterPolicy::default());

        let error = router
            .structured_completion(base_request(AiTaskType::RankExplain))
            .await
            .unwrap_err();

        assert_eq!(error, AiError::Timeout);
        // primary fails fast; each hang uses at most half remaining, so both
        // fallbacks get attempted before the shared deadline ends.
        assert_eq!(
            *provider.attempted_models.lock().expect("lock"),
            vec![
                "primary".to_owned(),
                "fallback-1".to_owned(),
                "fallback-2".to_owned()
            ]
        );
    }

    #[tokio::test]
    async fn per_attempt_cap_lets_fallback_run_after_primary_hang() {
        #[derive(Debug, Default)]
        struct HangPrimaryProvider {
            attempted_models: Mutex<Vec<String>>,
        }

        #[async_trait::async_trait]
        impl AiProvider for HangPrimaryProvider {
            fn name(&self) -> &str {
                "hang-primary"
            }

            fn capabilities(&self) -> crate::types::ProviderCapabilities {
                crate::types::ProviderCapabilities {
                    name: self.name().into(),
                    structured_output: true,
                    embeddings: false,
                    max_context_tokens: 8_192,
                    embedding_dimensions: None,
                }
            }

            fn is_available(&self) -> bool {
                true
            }

            async fn structured_completion(
                &self,
                request: StructuredRequest,
            ) -> Result<crate::types::StructuredResponse, AiError> {
                let model = request.model.expect("router supplies a model");
                self.attempted_models
                    .lock()
                    .expect("lock")
                    .push(model.clone());
                if model == "primary" {
                    std::future::pending().await
                }
                Ok(crate::types::StructuredResponse {
                    provider: self.name().into(),
                    model,
                    content: json!({"ok": true}),
                    usage_input: 1,
                    usage_output: 1,
                    prompt_cache_hit_tokens: None,
                    prompt_cache_miss_tokens: None,
                    latency_ms: 1,
                    protocol: request.protocol,
                })
            }
        }

        let provider = Arc::new(HangPrimaryProvider::default());
        let mut routes = default_task_routes();
        routes.insert(
            AiTaskType::RankExplain,
            TaskRouteConfig {
                task: AiTaskType::RankExplain,
                primary_model: "primary".into(),
                fallback_models: vec!["fallback".into()],
                protocol_preference: vec![ApiProtocol::Responses],
                timeout: Duration::from_millis(60),
                max_output_tokens: 100,
                enabled: true,
                route_version: "test".into(),
            },
        );
        let registry = Arc::new(ModelRegistry::new());
        registry.seed(capabilities_from_model_ids(["primary", "fallback"]));
        let router = TaskRouter::new(provider.clone(), routes, registry, RouterPolicy::default());

        let result = router
            .structured_completion(base_request(AiTaskType::RankExplain))
            .await
            .unwrap();

        assert_eq!(result.response.model, "fallback");
        assert!(result.used_fallback);
        assert_eq!(
            *provider.attempted_models.lock().expect("lock"),
            vec!["primary".to_owned(), "fallback".to_owned()]
        );
    }

    #[tokio::test]
    async fn protocol_preference_is_not_stuck_across_fallback_models() {
        // Primary walks Chat → Responses, then fails non-protocol on Responses.
        // Sticky mutation would force fallback to only try Responses; the
        // fallback's ChatCompletions path must still be attempted.
        let provider = FakeProvider {
            response: json!({"ok": true}),
            protocol_failures: Mutex::new(
                [
                    (
                        ("primary".into(), ApiProtocol::ChatCompletions),
                        AiError::ProviderRejected("protocol chat/completions not supported".into()),
                    ),
                    (
                        ("fallback".into(), ApiProtocol::Responses),
                        AiError::ProviderRejected("protocol responses not supported".into()),
                    ),
                ]
                .into_iter()
                .collect(),
            ),
            fail_models: Mutex::new(
                [("primary".into(), AiError::RateLimited)]
                    .into_iter()
                    .collect(),
            ),
            default_model: "fallback".into(),
            ..FakeProvider::default()
        };
        let mut routes = default_task_routes();
        routes.insert(
            AiTaskType::RankExplain,
            TaskRouteConfig {
                task: AiTaskType::RankExplain,
                primary_model: "primary".into(),
                fallback_models: vec!["fallback".into()],
                protocol_preference: vec![ApiProtocol::ChatCompletions, ApiProtocol::Responses],
                timeout: Duration::from_secs(5),
                max_output_tokens: 100,
                enabled: true,
                route_version: "test".into(),
            },
        );
        let registry = Arc::new(ModelRegistry::new());
        registry.seed(capabilities_from_model_ids(["primary", "fallback"]));
        let router = TaskRouter::new(
            Arc::new(provider),
            routes,
            registry,
            RouterPolicy::default(),
        );

        let result = router
            .structured_completion(base_request(AiTaskType::RankExplain))
            .await
            .unwrap();

        assert_eq!(result.response.model, "fallback");
        assert_eq!(result.response.protocol, Some(ApiProtocol::ChatCompletions));
        assert!(result.used_fallback);
        assert_eq!(
            result.attempted_models,
            vec!["primary".to_owned(), "fallback".to_owned()]
        );
    }

    #[tokio::test]
    async fn shared_deadline_exhaustion_does_not_open_model_circuit() {
        // Hang forever after recording the attempt: timeout is shared budget
        // exhaustion, not a provider fault that should trip the circuit.
        #[derive(Debug, Default)]
        struct HangProvider {
            attempted_models: Mutex<Vec<String>>,
        }

        #[async_trait::async_trait]
        impl AiProvider for HangProvider {
            fn name(&self) -> &str {
                "hang-test"
            }

            fn capabilities(&self) -> crate::types::ProviderCapabilities {
                crate::types::ProviderCapabilities {
                    name: self.name().into(),
                    structured_output: true,
                    embeddings: false,
                    max_context_tokens: 8_192,
                    embedding_dimensions: None,
                }
            }

            fn is_available(&self) -> bool {
                true
            }

            async fn structured_completion(
                &self,
                request: StructuredRequest,
            ) -> Result<crate::types::StructuredResponse, AiError> {
                let model = request.model.expect("router supplies a model");
                self.attempted_models.lock().expect("lock").push(model);
                std::future::pending().await
            }
        }

        let provider = Arc::new(HangProvider::default());
        let mut routes = default_task_routes();
        routes.insert(
            AiTaskType::RankExplain,
            TaskRouteConfig {
                task: AiTaskType::RankExplain,
                primary_model: "slow-model".into(),
                fallback_models: vec![],
                protocol_preference: vec![ApiProtocol::Responses],
                timeout: Duration::from_millis(40),
                max_output_tokens: 100,
                enabled: true,
                route_version: "test".into(),
            },
        );
        let registry = Arc::new(ModelRegistry::new());
        registry.seed(capabilities_from_model_ids(["slow-model"]));
        let policy = RouterPolicy {
            circuit_failure_threshold: 1,
            circuit_open_ms: 60_000,
            ..RouterPolicy::default()
        };
        let router = TaskRouter::new(provider.clone(), routes, registry, policy);

        let first = router
            .structured_completion(base_request(AiTaskType::RankExplain))
            .await
            .unwrap_err();
        assert_eq!(first, AiError::Timeout);

        // Model must remain eligible after pure shared-deadline timeouts.
        let second = router
            .structured_completion(base_request(AiTaskType::RankExplain))
            .await
            .unwrap_err();
        assert_eq!(second, AiError::Timeout);
        assert_eq!(
            *provider.attempted_models.lock().expect("lock"),
            vec!["slow-model".to_owned(), "slow-model".to_owned()]
        );
    }
}
