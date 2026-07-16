use thiserror::Error;

/// Vendor-agnostic AI errors. Never embed raw provider secrets.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AiError {
    #[error("AI provider is disabled")]
    Disabled,
    #[error("AI provider timed out")]
    Timeout,
    #[error("AI provider is rate limited")]
    RateLimited,
    #[error("AI daily or minute budget exhausted")]
    BudgetExhausted,
    #[error("AI circuit breaker is open")]
    CircuitOpen,
    #[error("AI provider rejected the request: {0}")]
    ProviderRejected(String),
    #[error("AI provider returned invalid structured output: {0}")]
    InvalidOutput(String),
    #[error("AI transport error: {0}")]
    Transport(String),
    #[error("AI configuration error: {0}")]
    Config(String),
}

impl AiError {
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Timeout | Self::RateLimited | Self::Transport(_)
        )
    }

    pub fn fallback_reason(&self) -> &'static str {
        match self {
            Self::Disabled => "AI provider is not configured; deterministic recommendations are shown",
            Self::Timeout => "AI provider timed out; deterministic recommendations are shown",
            Self::RateLimited => "AI provider rate limited; deterministic recommendations are shown",
            Self::BudgetExhausted => {
                "AI budget exhausted; deterministic recommendations are shown"
            }
            Self::CircuitOpen => {
                "AI provider temporarily unavailable; deterministic recommendations are shown"
            }
            Self::ProviderRejected(_) => {
                "AI provider rejected the request; deterministic recommendations are shown"
            }
            Self::InvalidOutput(_) => {
                "AI output failed validation; deterministic recommendations are shown"
            }
            Self::Transport(_) => {
                "AI provider transport failed; deterministic recommendations are shown"
            }
            Self::Config(_) => "AI provider misconfigured; deterministic recommendations are shown",
        }
    }
}
