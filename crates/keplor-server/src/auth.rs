//! API key authentication middleware.
//!
//! Supports two key formats in configuration:
//! - `"id:secret"` — explicit key ID and secret
//! - `"secret"` — auto-derives ID as `key_<first8hex_sha256>`

use std::sync::Arc;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use sha2::{Digest, Sha256};
use smol_str::SmolStr;
use subtle::ConstantTimeEq;

/// Authenticated key identity stored in request extensions.
#[derive(Debug, Clone)]
pub struct AuthenticatedKey {
    /// Stable identifier for the matched API key.
    pub key_id: SmolStr,
}

/// Set of valid API keys for ingestion endpoints.
#[derive(Debug, Clone)]
pub struct ApiKeySet {
    /// Each entry is `(secret_bytes, key_id)`.
    keys: Vec<(Vec<u8>, SmolStr)>,
}

impl ApiKeySet {
    /// Build from a list of key strings.
    ///
    /// Each string is either `"id:secret"` or a bare `"secret"` (in which
    /// case the ID is derived as `key_<first8hex_sha256(secret)>`).
    pub fn new(keys: Vec<String>) -> Self {
        Self {
            keys: keys
                .into_iter()
                .map(|raw| {
                    let (id, secret) = parse_key_entry(&raw);
                    (secret.into_bytes(), id)
                })
                .collect(),
        }
    }

    /// Returns `true` when authentication is disabled (no keys configured).
    pub fn is_open(&self) -> bool {
        self.keys.is_empty()
    }

    /// Constant-time lookup: returns the matched key's ID, or `None`.
    ///
    /// Always scans ALL keys to prevent timing side-channels that would
    /// reveal how many keys exist or which position matched.
    fn matched_key_id(&self, candidate: &[u8]) -> Option<SmolStr> {
        let mut matched_id: Option<SmolStr> = None;
        for (secret, id) in &self.keys {
            if bool::from(secret.ct_eq(candidate)) {
                matched_id = Some(id.clone());
            }
        }
        matched_id
    }
}

/// Parse a key config entry into `(id, secret)`.
///
/// Format: `"my-key-id:the-actual-secret"` or just `"the-actual-secret"`.
fn parse_key_entry(raw: &str) -> (SmolStr, String) {
    if let Some((id, secret)) = raw.split_once(':') {
        if !id.is_empty() && !secret.is_empty() {
            return (SmolStr::new(id), secret.to_owned());
        }
    }
    // Bare secret — derive a stable ID from its SHA-256 hash prefix.
    let hash = Sha256::digest(raw.as_bytes());
    let id = format!("key_{:x}{:x}{:x}{:x}", hash[0], hash[1], hash[2], hash[3]);
    (SmolStr::new(&id), raw.to_owned())
}

/// Axum middleware that validates the `Authorization: Bearer <key>` header.
///
/// On success, inserts an [`AuthenticatedKey`] into request extensions so
/// downstream handlers know which key was used.  On failure, emits a
/// metrics counter and returns 401.
pub async fn require_api_key(
    keys: Arc<ApiKeySet>,
    mut req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if keys.is_open() {
        return Ok(next.run(req).await);
    }

    let header =
        req.headers().get(http::header::AUTHORIZATION).and_then(|v| v.to_str().ok()).unwrap_or("");

    let token = header.strip_prefix("Bearer ").unwrap_or("");

    if token.is_empty() {
        metrics::counter!("keplor_auth_failures_total", "reason" => "missing").increment(1);
        tracing::warn!("auth rejected: missing or empty bearer token");
        return Err(StatusCode::UNAUTHORIZED);
    }

    match keys.matched_key_id(token.as_bytes()) {
        Some(key_id) => {
            metrics::counter!("keplor_auth_successes_total").increment(1);
            req.extensions_mut().insert(AuthenticatedKey { key_id });
            Ok(next.run(req).await)
        },
        None => {
            metrics::counter!("keplor_auth_failures_total", "reason" => "invalid").increment(1);
            tracing::warn!("auth rejected: invalid bearer token");
            Err(StatusCode::UNAUTHORIZED)
        },
    }
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
        assert!(set.matched_key_id(b"secret123").is_some());
        assert!(set.matched_key_id(b"wrong").is_none());
        assert!(set.matched_key_id(b"").is_none());
    }

    #[test]
    fn matches_multiple_keys() {
        let set = ApiKeySet::new(vec!["key1".into(), "key2".into()]);
        assert!(set.matched_key_id(b"key1").is_some());
        assert!(set.matched_key_id(b"key2").is_some());
        assert!(set.matched_key_id(b"key3").is_none());
    }

    #[test]
    fn explicit_id_format() {
        let set = ApiKeySet::new(vec!["myapp:secret123".into()]);
        let id = set.matched_key_id(b"secret123").unwrap();
        assert_eq!(id.as_str(), "myapp");
    }

    #[test]
    fn auto_derived_id_is_stable() {
        let (id1, _) = parse_key_entry("secret123");
        let (id2, _) = parse_key_entry("secret123");
        assert_eq!(id1, id2);
        assert!(id1.starts_with("key_"));
    }

    #[test]
    fn different_secrets_get_different_ids() {
        let (id1, _) = parse_key_entry("secret1");
        let (id2, _) = parse_key_entry("secret2");
        assert_ne!(id1, id2);
    }

    #[test]
    fn explicit_ids_returned_on_match() {
        let set = ApiKeySet::new(vec!["prod-key:abc123".into(), "dev-key:xyz789".into()]);
        assert_eq!(set.matched_key_id(b"abc123").unwrap(), "prod-key");
        assert_eq!(set.matched_key_id(b"xyz789").unwrap(), "dev-key");
        assert!(set.matched_key_id(b"unknown").is_none());
    }
}
