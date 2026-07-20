//! Per-host token bucket, concurrency cap, and 429 circuit breaker (AI-016).
//!
//! Dual-channel collectors must isolate Steam hosts from web hosts so one
//! source's 429 never clears another channel's valid snapshots.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::error::AiError;

#[derive(Debug, Clone)]
pub struct HostLimitConfig {
    pub max_tokens: f64,
    pub refill_per_sec: f64,
    pub max_concurrency: u32,
    pub circuit_open: Duration,
    pub consecutive_429_threshold: u32,
}

impl Default for HostLimitConfig {
    fn default() -> Self {
        Self {
            max_tokens: 10.0,
            refill_per_sec: 2.0,
            max_concurrency: 2,
            circuit_open: Duration::from_secs(30),
            consecutive_429_threshold: 2,
        }
    }
}

#[derive(Debug)]
struct HostState {
    tokens: f64,
    in_flight: u32,
    last_refill: Instant,
    consecutive_429: u32,
    circuit_open_until: Option<Instant>,
    config: HostLimitConfig,
}

impl HostState {
    fn new(config: HostLimitConfig) -> Self {
        Self {
            tokens: config.max_tokens,
            in_flight: 0,
            last_refill: Instant::now(),
            consecutive_429: 0,
            circuit_open_until: None,
            config,
        }
    }

    fn refill(&mut self) {
        let elapsed = self.last_refill.elapsed().as_secs_f64();
        if elapsed <= 0.0 {
            return;
        }
        self.tokens =
            (self.tokens + elapsed * self.config.refill_per_sec).min(self.config.max_tokens);
        self.last_refill = Instant::now();
    }

    fn try_acquire(&mut self) -> Result<(), AiError> {
        if let Some(until) = self.circuit_open_until {
            if Instant::now() < until {
                return Err(AiError::CircuitOpen);
            }
            self.circuit_open_until = None;
        }
        self.refill();
        if self.in_flight >= self.config.max_concurrency {
            return Err(AiError::RateLimited);
        }
        if self.tokens < 1.0 {
            return Err(AiError::RateLimited);
        }
        self.tokens -= 1.0;
        self.in_flight = self.in_flight.saturating_add(1);
        Ok(())
    }

    fn release_ok(&mut self) {
        self.in_flight = self.in_flight.saturating_sub(1);
        self.consecutive_429 = 0;
    }

    fn release_429(&mut self, retry_after: Option<Duration>) {
        self.in_flight = self.in_flight.saturating_sub(1);
        self.consecutive_429 = self.consecutive_429.saturating_add(1);
        if self.consecutive_429 >= self.config.consecutive_429_threshold {
            let open_for = retry_after.unwrap_or(self.config.circuit_open);
            self.circuit_open_until = Some(Instant::now() + open_for);
        }
    }

    fn release_error(&mut self) {
        self.in_flight = self.in_flight.saturating_sub(1);
    }
}

/// Process-wide per-host limiter. Never increases concurrency after a 429.
#[derive(Debug, Default)]
pub struct HostLimiter {
    hosts: Mutex<HashMap<String, HostState>>,
    default_config: HostLimitConfig,
}

impl HostLimiter {
    pub fn new(default_config: HostLimitConfig) -> Self {
        Self {
            hosts: Mutex::new(HashMap::new()),
            default_config,
        }
    }

    pub fn try_acquire(&self, host: &str) -> Result<HostPermit<'_>, AiError> {
        let host = host.trim().to_ascii_lowercase();
        if host.is_empty() {
            return Err(AiError::Config("host is required for rate limiting".into()));
        }
        let mut guard = self.hosts.lock().expect("host limiter lock");
        let state = guard
            .entry(host.clone())
            .or_insert_with(|| HostState::new(self.default_config.clone()));
        state.try_acquire()?;
        Ok(HostPermit {
            limiter: self,
            host,
            released: false,
        })
    }

    fn release(&self, host: &str, outcome: HostRelease) {
        let mut guard = self.hosts.lock().expect("host limiter lock");
        if let Some(state) = guard.get_mut(host) {
            match outcome {
                HostRelease::Ok => state.release_ok(),
                HostRelease::RateLimited { retry_after } => state.release_429(retry_after),
                HostRelease::Error => state.release_error(),
            }
        }
    }
}

#[derive(Debug)]
enum HostRelease {
    Ok,
    RateLimited { retry_after: Option<Duration> },
    Error,
}

/// RAII permit. Drop without calling success/rate_limited counts as a soft error.
pub struct HostPermit<'a> {
    limiter: &'a HostLimiter,
    host: String,
    released: bool,
}

impl std::fmt::Debug for HostPermit<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostPermit")
            .field("host", &self.host)
            .field("released", &self.released)
            .finish_non_exhaustive()
    }
}

impl HostPermit<'_> {
    pub fn success(mut self) {
        self.limiter.release(&self.host, HostRelease::Ok);
        self.released = true;
    }

    pub fn rate_limited(mut self, retry_after: Option<Duration>) {
        self.limiter
            .release(&self.host, HostRelease::RateLimited { retry_after });
        self.released = true;
    }

    pub fn failed(mut self) {
        self.limiter.release(&self.host, HostRelease::Error);
        self.released = true;
    }
}

impl Drop for HostPermit<'_> {
    fn drop(&mut self) {
        if !self.released {
            self.limiter.release(&self.host, HostRelease::Error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrency_cap_blocks_extra_inflight() {
        let limiter = HostLimiter::new(HostLimitConfig {
            max_tokens: 100.0,
            refill_per_sec: 100.0,
            max_concurrency: 1,
            ..HostLimitConfig::default()
        });
        let first = limiter.try_acquire("api.steampowered.com").unwrap();
        let second = limiter.try_acquire("api.steampowered.com").unwrap_err();
        assert_eq!(second, AiError::RateLimited);
        first.success();
        assert!(limiter.try_acquire("api.steampowered.com").is_ok());
    }

    #[test]
    fn hosts_are_isolated() {
        let limiter = HostLimiter::new(HostLimitConfig {
            max_tokens: 1.0,
            refill_per_sec: 0.0,
            max_concurrency: 1,
            ..HostLimitConfig::default()
        });
        let steam = limiter.try_acquire("api.steampowered.com").unwrap();
        // Same token budget is per-host, so web host still works.
        let web = limiter.try_acquire("example.com").unwrap();
        steam.success();
        web.success();
    }

    #[test]
    fn repeated_429_opens_circuit_without_raising_concurrency() {
        let limiter = HostLimiter::new(HostLimitConfig {
            max_tokens: 100.0,
            refill_per_sec: 100.0,
            max_concurrency: 2,
            consecutive_429_threshold: 2,
            circuit_open: Duration::from_secs(60),
        });
        let p1 = limiter.try_acquire("store.steampowered.com").unwrap();
        p1.rate_limited(None);
        let p2 = limiter.try_acquire("store.steampowered.com").unwrap();
        p2.rate_limited(Some(Duration::from_secs(10)));
        let err = limiter.try_acquire("store.steampowered.com").unwrap_err();
        assert_eq!(err, AiError::CircuitOpen);
    }
}
