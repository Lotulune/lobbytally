use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::time::timeout;

use crate::error::AiError;
use crate::provider::AiProvider;
use crate::types::{AiStatus, StructuredRequest, StructuredResponse};

/// Runtime policy around a concrete provider: timeout, budget, circuit breaker.
#[derive(Debug, Clone)]
pub struct AiPolicy {
    pub online_timeout: Duration,
    pub minute_budget: u32,
    pub daily_budget: u32,
    pub circuit_failure_threshold: u32,
    pub circuit_open_ms: u64,
}

impl Default for AiPolicy {
    fn default() -> Self {
        Self {
            online_timeout: Duration::from_secs(12),
            minute_budget: 60,
            daily_budget: 2_000,
            circuit_failure_threshold: 5,
            circuit_open_ms: 30_000,
        }
    }
}

#[derive(Debug)]
struct BudgetState {
    minute_window: AtomicU64,
    minute_count: AtomicU32,
    day_window: AtomicU64,
    day_count: AtomicU32,
    consecutive_failures: AtomicU32,
    circuit_open_until_ms: AtomicU64,
}

impl Default for BudgetState {
    fn default() -> Self {
        Self {
            minute_window: AtomicU64::new(0),
            minute_count: AtomicU32::new(0),
            day_window: AtomicU64::new(0),
            day_count: AtomicU32::new(0),
            consecutive_failures: AtomicU32::new(0),
            circuit_open_until_ms: AtomicU64::new(0),
        }
    }
}

#[derive(Clone)]
pub struct AiGateway {
    provider: Arc<dyn AiProvider>,
    policy: AiPolicy,
    state: Arc<BudgetState>,
}

impl AiGateway {
    pub fn new(provider: Arc<dyn AiProvider>, policy: AiPolicy) -> Self {
        Self {
            provider,
            policy,
            state: Arc::new(BudgetState::default()),
        }
    }

    pub fn disabled() -> Self {
        Self::new(
            Arc::new(crate::provider::DisabledProvider),
            AiPolicy::default(),
        )
    }

    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }

    pub fn provider_cache_identity(&self) -> String {
        self.provider.cache_identity()
    }

    pub fn is_available(&self) -> bool {
        self.provider.is_available()
    }

    pub fn client_status_when_idle(&self) -> AiStatus {
        if self.provider.is_available() {
            AiStatus::Used
        } else {
            AiStatus::Disabled
        }
    }

    pub async fn structured_completion(
        &self,
        request: StructuredRequest,
    ) -> Result<StructuredResponse, AiError> {
        if !self.provider.is_available() {
            return Err(AiError::Disabled);
        }
        self.check_circuit()?;
        self.consume_budget()?;

        let result = timeout(
            self.policy.online_timeout,
            self.provider.structured_completion(request),
        )
        .await;

        match result {
            Ok(Ok(response)) => {
                self.state.consecutive_failures.store(0, Ordering::Relaxed);
                Ok(response)
            }
            Ok(Err(error)) => {
                self.note_failure(&error);
                Err(error)
            }
            Err(_) => {
                let error = AiError::Timeout;
                self.note_failure(&error);
                Err(error)
            }
        }
    }

    fn check_circuit(&self) -> Result<(), AiError> {
        let open_until = self.state.circuit_open_until_ms.load(Ordering::Relaxed);
        let now = now_ms();
        if open_until > now {
            return Err(AiError::CircuitOpen);
        }
        Ok(())
    }

    fn consume_budget(&self) -> Result<(), AiError> {
        let now = Instant::now().elapsed().as_secs(); // monotonic-ish bucket seed; wall below
        let wall = now_ms() / 1000;
        let minute = wall / 60;
        let day = wall / 86_400;

        // Minute window
        let prev_minute = self.state.minute_window.load(Ordering::Relaxed);
        if prev_minute != minute {
            self.state.minute_window.store(minute, Ordering::Relaxed);
            self.state.minute_count.store(0, Ordering::Relaxed);
        }
        let minute_count = self.state.minute_count.fetch_add(1, Ordering::Relaxed) + 1;
        if minute_count > self.policy.minute_budget {
            return Err(AiError::BudgetExhausted);
        }

        // Day window
        let prev_day = self.state.day_window.load(Ordering::Relaxed);
        if prev_day != day {
            self.state.day_window.store(day, Ordering::Relaxed);
            self.state.day_count.store(0, Ordering::Relaxed);
        }
        let day_count = self.state.day_count.fetch_add(1, Ordering::Relaxed) + 1;
        if day_count > self.policy.daily_budget {
            return Err(AiError::BudgetExhausted);
        }

        let _ = now;
        Ok(())
    }

    fn note_failure(&self, error: &AiError) {
        if matches!(
            error,
            AiError::Timeout
                | AiError::Transport(_)
                | AiError::RateLimited
                | AiError::ProviderRejected(_)
        ) {
            let failures = self
                .state
                .consecutive_failures
                .fetch_add(1, Ordering::Relaxed)
                + 1;
            if failures >= self.policy.circuit_failure_threshold {
                let until = now_ms().saturating_add(self.policy.circuit_open_ms);
                self.state
                    .circuit_open_until_ms
                    .store(until, Ordering::Relaxed);
            }
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::FakeProvider;
    use crate::types::AiTaskType;
    use serde_json::json;

    fn request() -> StructuredRequest {
        StructuredRequest {
            task: AiTaskType::RankExplain,
            system_prompt: "sys".into(),
            data_prompt: "data".into(),
            json_schema_name: "rank".into(),
            json_schema: json!({"type":"object"}),
            max_output_tokens: 100,
            temperature: 0.0,
            model: None,
            protocol: None,
        }
    }

    #[tokio::test]
    async fn disabled_gateway_returns_disabled() {
        let gw = AiGateway::disabled();
        let err = gw.structured_completion(request()).await.unwrap_err();
        assert_eq!(err, AiError::Disabled);
        assert!(!gw.is_available());
    }

    #[tokio::test]
    async fn fake_gateway_succeeds() {
        let gw = AiGateway::new(Arc::new(FakeProvider::default()), AiPolicy::default());
        let response = gw.structured_completion(request()).await.unwrap();
        assert_eq!(response.provider, "fake");
    }

    #[tokio::test]
    async fn budget_exhaustion() {
        let policy = AiPolicy {
            minute_budget: 1,
            daily_budget: 100,
            ..AiPolicy::default()
        };
        let gw = AiGateway::new(Arc::new(FakeProvider::default()), policy);
        assert!(gw.structured_completion(request()).await.is_ok());
        let err = gw.structured_completion(request()).await.unwrap_err();
        assert_eq!(err, AiError::BudgetExhausted);
    }

    #[tokio::test]
    async fn provider_timeout_opens_fallback_path() {
        let policy = AiPolicy {
            online_timeout: Duration::from_millis(5),
            ..AiPolicy::default()
        };
        let provider = FakeProvider {
            delay: Duration::from_millis(50),
            ..FakeProvider::default()
        };
        let gw = AiGateway::new(Arc::new(provider), policy);
        assert_eq!(
            gw.structured_completion(request()).await.unwrap_err(),
            AiError::Timeout
        );
    }
}
