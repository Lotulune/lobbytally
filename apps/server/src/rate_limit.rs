use std::collections::HashMap;
use std::env;
use std::hash::{Hash, Hasher};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;

const WINDOW: Duration = Duration::from_secs(60);
const MAX_IDENTITIES: usize = 100_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitConfig {
    pub enabled: bool,
    pub read_per_minute: u32,
    pub search_per_minute: u32,
    pub session_per_minute: u32,
    pub feedback_per_minute: u32,
    pub global_per_minute: u32,
    pub trust_proxy_headers: bool,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            read_per_minute: 120,
            search_per_minute: 30,
            session_per_minute: 20,
            feedback_per_minute: 60,
            global_per_minute: 10_000,
            trust_proxy_headers: false,
        }
    }
}

impl RateLimitConfig {
    pub fn from_env() -> Result<Self, io::Error> {
        let defaults = Self::default();
        Ok(Self {
            enabled: env_bool("MPGS_RATE_LIMIT_ENABLED", defaults.enabled)?,
            read_per_minute: env_positive_u32(
                "MPGS_RATE_LIMIT_READ_PER_MINUTE",
                defaults.read_per_minute,
            )?,
            search_per_minute: env_positive_u32(
                "MPGS_RATE_LIMIT_SEARCH_PER_MINUTE",
                defaults.search_per_minute,
            )?,
            session_per_minute: env_positive_u32(
                "MPGS_RATE_LIMIT_SESSION_PER_MINUTE",
                defaults.session_per_minute,
            )?,
            feedback_per_minute: env_positive_u32(
                "MPGS_RATE_LIMIT_FEEDBACK_PER_MINUTE",
                defaults.feedback_per_minute,
            )?,
            global_per_minute: env_positive_u32(
                "MPGS_RATE_LIMIT_GLOBAL_PER_MINUTE",
                defaults.global_per_minute,
            )?,
            trust_proxy_headers: env_bool(
                "MPGS_TRUST_PROXY_HEADERS",
                defaults.trust_proxy_headers,
            )?,
        })
    }
}

fn env_bool(name: &str, default: bool) -> Result<bool, io::Error> {
    let Ok(value) = env::var(name) else {
        return Ok(default);
    };
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" => Ok(true),
        "0" | "false" | "no" | "" => Ok(false),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} must be true/false or 1/0"),
        )),
    }
}

fn env_positive_u32(name: &str, default: u32) -> Result<u32, io::Error> {
    let Ok(value) = env::var(name) else {
        return Ok(default);
    };
    value
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{name} must be a positive integer"),
            )
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Bucket {
    Read,
    Search,
    Session,
    Feedback,
    Global,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CounterKey {
    bucket: Bucket,
    identity: String,
}

#[derive(Debug, Clone, Copy)]
struct Counter {
    window_started: Instant,
    count: u32,
}

#[derive(Debug, Clone, Copy)]
struct Allowance {
    limit: u32,
    remaining: u32,
}

#[derive(Debug, Clone, Copy)]
struct Limited {
    limit: u32,
    retry_after_seconds: u64,
}

pub struct RateLimiter {
    config: RateLimitConfig,
    counters: Mutex<HashMap<CounterKey, Counter>>,
    requests: AtomicU64,
}

impl RateLimiter {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            counters: Mutex::new(HashMap::new()),
            requests: AtomicU64::new(0),
        }
    }

    fn check(&self, request: &Request<Body>) -> Result<Option<Allowance>, Limited> {
        if !self.config.enabled {
            return Ok(None);
        }
        let Some((bucket, limit)) = self.classify(request.method(), request.uri().path()) else {
            return Ok(None);
        };
        let identities = self.identities(request);
        let now = Instant::now();
        let mut keys: Vec<_> = identities
            .into_iter()
            .map(|identity| (CounterKey { bucket, identity }, limit))
            .collect();
        keys.push((
            CounterKey {
                bucket: Bucket::Global,
                identity: "all".to_owned(),
            },
            self.config.global_per_minute,
        ));

        let mut counters = self
            .counters
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if counters.len() >= MAX_IDENTITIES
            || self
                .requests
                .fetch_add(1, Ordering::Relaxed)
                .is_multiple_of(1_024)
        {
            counters.retain(|_, counter| now.duration_since(counter.window_started) < WINDOW);
        }
        if counters.len() >= MAX_IDENTITIES {
            return Err(Limited {
                limit,
                retry_after_seconds: WINDOW.as_secs(),
            });
        }

        for (key, key_limit) in &keys {
            if let Some(counter) = counters.get(key)
                && now.duration_since(counter.window_started) < WINDOW
                && counter.count >= *key_limit
            {
                return Err(Limited {
                    limit: *key_limit,
                    retry_after_seconds: WINDOW
                        .saturating_sub(now.duration_since(counter.window_started))
                        .as_secs()
                        .max(1),
                });
            }
        }

        let mut remaining = limit;
        for (key, key_limit) in keys {
            let counter = counters.entry(key).or_insert(Counter {
                window_started: now,
                count: 0,
            });
            if now.duration_since(counter.window_started) >= WINDOW {
                *counter = Counter {
                    window_started: now,
                    count: 0,
                };
            }
            counter.count = counter.count.saturating_add(1);
            remaining = remaining.min(key_limit.saturating_sub(counter.count));
        }
        Ok(Some(Allowance { limit, remaining }))
    }

    fn classify(&self, method: &Method, path: &str) -> Option<(Bucket, u32)> {
        if path.starts_with("/v1/session/") || path.starts_with("/v1/auth/") {
            return Some((Bucket::Session, self.config.session_per_minute));
        }
        if matches!(
            path,
            "/v1/search"
                | "/v1/recommendations/natural-language"
                | "/v1/ai/search"
                | "/v1/ai/compare"
                | "/v1/ai/group-advice"
                | "/v1/me/ai-settings/test"
                | "/v1/me/ai-settings/discover"
        ) {
            return Some((Bucket::Search, self.config.search_per_minute));
        }
        if method == Method::POST && path.starts_with("/v1/feedback") {
            return Some((Bucket::Feedback, self.config.feedback_per_minute));
        }
        if method == Method::POST
            && path.starts_with("/v1/games/")
            && path.ends_with("/play-intent")
        {
            return Some((Bucket::Feedback, self.config.feedback_per_minute));
        }
        if path.starts_with("/v1/") || path == "/openapi.json" {
            return Some((Bucket::Read, self.config.read_per_minute));
        }
        None
    }

    fn identities(&self, request: &Request<Body>) -> Vec<String> {
        let mut identities = Vec::with_capacity(2);
        let headers = request.headers();
        if let Some(device) = headers
            .get("x-device-id")
            .and_then(|value| value.to_str().ok())
            .filter(|value| valid_identity(value))
        {
            identities.push(format!("device:{:016x}", stable_hash(device)));
        } else if let Some(token) = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .filter(|value| !value.is_empty())
        {
            identities.push(format!("session:{:016x}", stable_hash(token)));
        }

        if let Some(ip) = self.client_ip(request) {
            identities.push(format!("ip:{ip}"));
        }
        if identities.is_empty() {
            identities.push("unidentified".to_owned());
        }
        identities
    }

    fn client_ip(&self, request: &Request<Body>) -> Option<IpAddr> {
        if self.config.trust_proxy_headers
            && let Some(ip) = proxy_ip(request.headers())
        {
            return Some(ip);
        }
        request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|connect| connect.0.ip())
    }
}

pub async fn middleware(
    State(limiter): State<Arc<RateLimiter>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let decision = limiter.check(&request);
    match decision {
        Err(limited) => rate_limited_response(request.headers(), limited),
        Ok(allowance) => {
            let mut response = next.run(request).await;
            if let Some(allowance) = allowance {
                insert_limit_headers(&mut response, allowance.limit, allowance.remaining);
            }
            response
        }
    }
}

fn rate_limited_response(headers: &HeaderMap, limited: Limited) -> Response {
    let request_id = headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        Json(json!({
            "error": {
                "code": "rate_limited",
                "message": "request rate limit exceeded",
                "request_id": request_id,
            }
        })),
    )
        .into_response();
    insert_limit_headers(&mut response, limited.limit, 0);
    if let Ok(value) = HeaderValue::from_str(&limited.retry_after_seconds.to_string()) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

fn insert_limit_headers(response: &mut Response, limit: u32, remaining: u32) {
    if let Ok(value) = HeaderValue::from_str(&limit.to_string()) {
        response.headers_mut().insert("x-ratelimit-limit", value);
    }
    if let Ok(value) = HeaderValue::from_str(&remaining.to_string()) {
        response
            .headers_mut()
            .insert("x-ratelimit-remaining", value);
    }
}

fn proxy_ip(headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .and_then(|value| value.trim().parse().ok())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse().ok())
        })
}

fn valid_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn stable_hash(value: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_headers_are_only_used_when_explicitly_trusted() {
        let mut request = Request::builder()
            .uri("/v1/meta")
            .header("x-forwarded-for", "203.0.113.10")
            .body(Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1234))));

        let direct = RateLimiter::new(RateLimitConfig::default());
        assert_eq!(
            direct.client_ip(&request),
            Some(IpAddr::from([127, 0, 0, 1]))
        );

        let trusted = RateLimiter::new(RateLimitConfig {
            trust_proxy_headers: true,
            ..RateLimitConfig::default()
        });
        assert_eq!(
            trusted.client_ip(&request),
            Some(IpAddr::from([203, 0, 113, 10]))
        );
    }

    #[test]
    fn account_auth_routes_use_the_strict_session_bucket() {
        let limiter = RateLimiter::new(RateLimitConfig::default());
        assert_eq!(
            limiter.classify(&Method::POST, "/v1/auth/login"),
            Some((
                Bucket::Session,
                RateLimitConfig::default().session_per_minute
            ))
        );
    }

    #[test]
    fn expensive_search_and_ai_routes_use_the_stricter_search_bucket() {
        let limiter = RateLimiter::new(RateLimitConfig::default());
        for path in [
            "/v1/search",
            "/v1/recommendations/natural-language",
            "/v1/ai/search",
            "/v1/ai/compare",
            "/v1/ai/group-advice",
            "/v1/me/ai-settings/test",
            "/v1/me/ai-settings/discover",
        ] {
            assert_eq!(
                limiter.classify(&Method::POST, path),
                Some((Bucket::Search, RateLimitConfig::default().search_per_minute)),
                "{path}"
            );
        }
        assert_eq!(
            limiter.classify(&Method::GET, "/v1/ai/analyses/analysis-id"),
            Some((Bucket::Read, RateLimitConfig::default().read_per_minute))
        );
    }
}
