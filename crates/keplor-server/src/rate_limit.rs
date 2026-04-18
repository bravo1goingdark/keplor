//! Per-API-key rate limiting using a token-bucket algorithm.
//!
//! Each authenticated API key gets an independent token bucket that refills
//! at `requests_per_second` and allows bursts up to `burst` tokens.
//! When a key's bucket is exhausted, requests are rejected with `429 Too Many
//! Requests` and a `Retry-After` header.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use axum::http::{HeaderValue, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use smol_str::SmolStr;

use crate::auth::AuthenticatedKey;

/// Per-key rate limiter configuration.
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Refill rate in requests per second.
    pub requests_per_second: f64,
    /// Maximum burst size (bucket capacity).
    pub burst: usize,
}

/// In-process rate limiter that tracks token buckets per API key.
pub struct RateLimiter {
    buckets: Mutex<HashMap<SmolStr, TokenBucket>>,
    config: RateLimitConfig,
}

struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
}

impl RateLimiter {
    /// Create a new rate limiter.
    pub fn new(config: RateLimitConfig) -> Self {
        Self { buckets: Mutex::new(HashMap::new()), config }
    }

    /// Try to acquire a token for the given key.
    ///
    /// Returns `Ok(())` if allowed, or `Err(seconds_until_next_token)` if
    /// the bucket is exhausted.
    pub fn try_acquire(&self, key: &str) -> Result<(), f64> {
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let burst = self.config.burst as f64;
        let rps = self.config.requests_per_second;

        let bucket = buckets.entry(SmolStr::new(key)).or_insert(TokenBucket {
            tokens: burst,
            last_refill: now,
        });

        // Refill tokens based on elapsed time.
        let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * rps).min(burst);
        bucket.last_refill = now;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            Ok(())
        } else {
            // Seconds until next token is available.
            let wait = (1.0 - bucket.tokens) / rps;
            Err(wait)
        }
    }
}

/// Build an axum middleware function that enforces per-key rate limits.
///
/// Must be placed after the auth middleware so that `AuthenticatedKey` is
/// available in request extensions. When auth is disabled (no keys), rate
/// limiting is skipped.
pub fn make_rate_limit_middleware(
    limiter: std::sync::Arc<RateLimiter>,
) -> impl Fn(
    Request<axum::body::Body>,
    Next,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>>
       + Clone
       + Send {
    move |request: Request<axum::body::Body>, next: Next| {
        let limiter = limiter.clone();
        Box::pin(async move {
            if let Some(auth) = request.extensions().get::<AuthenticatedKey>() {
                if let Err(retry_after) = limiter.try_acquire(&auth.key_id) {
                    let secs = retry_after.ceil() as u64;
                    let mut resp = StatusCode::TOO_MANY_REQUESTS.into_response();
                    if let Ok(val) = HeaderValue::from_str(&secs.to_string()) {
                        resp.headers_mut().insert("retry-after", val);
                    }
                    return resp;
                }
            }
            next.run(request).await
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn default_limiter() -> RateLimiter {
        RateLimiter::new(RateLimitConfig { requests_per_second: 10.0, burst: 5 })
    }

    #[test]
    fn allows_burst_up_to_limit() {
        let limiter = default_limiter();
        for _ in 0..5 {
            assert!(limiter.try_acquire("key-1").is_ok());
        }
    }

    #[test]
    fn rejects_after_burst_exhausted() {
        let limiter = default_limiter();
        for _ in 0..5 {
            limiter.try_acquire("key-1").ok();
        }
        let result = limiter.try_acquire("key-1");
        assert!(result.is_err());
    }

    #[test]
    fn returns_retry_after_seconds() {
        let limiter = default_limiter();
        for _ in 0..5 {
            limiter.try_acquire("key-1").ok();
        }
        if let Err(wait) = limiter.try_acquire("key-1") {
            assert!(wait > 0.0);
            assert!(wait <= 1.0);
        }
    }

    #[test]
    fn different_keys_are_independent() {
        let limiter = default_limiter();
        for _ in 0..5 {
            limiter.try_acquire("key-1").ok();
        }
        assert!(limiter.try_acquire("key-1").is_err());
        assert!(limiter.try_acquire("key-2").is_ok());
    }

    #[test]
    fn refills_over_time() {
        let limiter = default_limiter();
        for _ in 0..5 {
            limiter.try_acquire("key-1").ok();
        }
        assert!(limiter.try_acquire("key-1").is_err());
        // Simulate passage of time by sleeping.
        std::thread::sleep(Duration::from_millis(150));
        assert!(limiter.try_acquire("key-1").is_ok());
    }
}
