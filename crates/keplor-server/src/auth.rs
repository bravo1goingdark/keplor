//! API key authentication middleware.

use std::sync::Arc;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use subtle::ConstantTimeEq;

/// Set of valid API keys for ingestion endpoints.
#[derive(Debug, Clone)]
pub struct ApiKeySet {
    keys: Vec<Vec<u8>>,
}

impl ApiKeySet {
    /// Build from a list of raw key strings.
    pub fn new(keys: Vec<String>) -> Self {
        Self { keys: keys.into_iter().map(|k| k.into_bytes()).collect() }
    }

    /// Returns `true` when authentication is disabled (no keys configured).
    pub fn is_open(&self) -> bool {
        self.keys.is_empty()
    }

    /// Constant-time check whether `candidate` matches any configured key.
    /// Always scans ALL keys to prevent timing side-channels.
    fn contains(&self, candidate: &[u8]) -> bool {
        let mut found = false;
        for k in &self.keys {
            found |= bool::from(k.ct_eq(candidate));
        }
        found
    }
}

/// Axum middleware that validates the `Authorization: Bearer <key>` header.
pub async fn require_api_key(
    keys: Arc<ApiKeySet>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if keys.is_open() {
        return Ok(next.run(req).await);
    }

    let header =
        req.headers().get(http::header::AUTHORIZATION).and_then(|v| v.to_str().ok()).unwrap_or("");

    let token = header.strip_prefix("Bearer ").unwrap_or("");

    if token.is_empty() || !keys.contains(token.as_bytes()) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(next.run(req).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_when_empty() {
        let set = ApiKeySet::new(vec![]);
        assert!(set.is_open());
    }

    #[test]
    fn matches_correct_key() {
        let set = ApiKeySet::new(vec!["secret123".into()]);
        assert!(set.contains(b"secret123"));
        assert!(!set.contains(b"wrong"));
        assert!(!set.contains(b""));
    }

    #[test]
    fn matches_multiple_keys() {
        let set = ApiKeySet::new(vec!["key1".into(), "key2".into()]);
        assert!(set.contains(b"key1"));
        assert!(set.contains(b"key2"));
        assert!(!set.contains(b"key3"));
    }
}
