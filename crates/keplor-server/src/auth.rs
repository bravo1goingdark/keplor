//! API key authentication middleware.
//!
//! Supports three key formats in configuration:
//! - `"id:secret"` — explicit key ID and secret (simple, tier = default)
//! - `"secret"` — auto-derives ID as `key_<first8hex_sha256>` (simple, tier = default)
//! - `ApiKeyEntry { id, secret, tier }` — extended format with explicit tier

use std::sync::Arc;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use sha2::{Digest, Sha256};
use smol_str::SmolStr;
use subtle::ConstantTimeEq;

use crate::config::ApiKeyEntry;

/// Authenticated key identity stored in request extensions.
#[derive(Debug, Clone)]
pub struct AuthenticatedKey {
    /// Stable identifier for the matched API key.
    pub key_id: SmolStr,
    /// Retention tier for this key (e.g. `"free"`, `"pro"`, `"team"`).
    pub tier: SmolStr,
}

/// Set of valid API keys for ingestion endpoints.
#[derive(Debug, Clone)]
pub struct ApiKeySet {
    /// Each entry is `(secret_bytes, key_id, tier)`.
    keys: Vec<(Vec<u8>, SmolStr, SmolStr)>,
}

impl ApiKeySet {
    /// Build from simple key strings (backward compatible).
    ///
    /// Each string is either `"id:secret"` or a bare `"secret"` (in which
    /// case the ID is derived as `key_<first8hex_sha256(secret)>`).
    /// All keys are assigned to the given `default_tier`.
    pub fn new(keys: Vec<String>, default_tier: &str) -> Self {
        let tier = SmolStr::new(default_tier);
        Self {
            keys: keys
                .into_iter()
                .map(|raw| {
                    let (id, secret) = parse_key_entry(&raw);
                    (secret.into_bytes(), id, tier.clone())
                })
                .collect(),
        }
    }

    /// Build from extended key entries with explicit tiers.
    pub fn from_entries(entries: Vec<ApiKeyEntry>) -> Self {
        Self {
            keys: entries
                .into_iter()
                .map(|e| (e.secret.into_bytes(), SmolStr::new(&e.id), SmolStr::new(&e.tier)))
                .collect(),
        }
    }

    /// Build from both simple keys and extended entries (merged).
    pub fn from_config(
        simple_keys: Vec<String>,
        entries: Vec<ApiKeyEntry>,
        default_tier: &str,
    ) -> Self {
        let tier = SmolStr::new(default_tier);
        let mut keys: Vec<(Vec<u8>, SmolStr, SmolStr)> = simple_keys
            .into_iter()
            .map(|raw| {
                let (id, secret) = parse_key_entry(&raw);
                (secret.into_bytes(), id, tier.clone())
            })
            .collect();

        keys.extend(
            entries
                .into_iter()
                .map(|e| (e.secret.into_bytes(), SmolStr::new(&e.id), SmolStr::new(&e.tier))),
        );

        Self { keys }
    }

    /// Returns `true` when authentication is disabled (no keys configured).
    pub fn is_open(&self) -> bool {
        self.keys.is_empty()
    }

    /// Constant-time lookup: returns the matched key's ID and tier, or `None`.
    ///
    /// Always scans ALL keys to prevent timing side-channels that would
    /// reveal how many keys exist or which position matched.
    fn matched_key(&self, candidate: &[u8]) -> Option<(SmolStr, SmolStr)> {
        let mut matched: Option<(SmolStr, SmolStr)> = None;
        for (secret, id, tier) in &self.keys {
            if bool::from(secret.ct_eq(candidate)) {
                matched = Some((id.clone(), tier.clone()));
            }
        }
        matched
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

/// Hot-reloadable wrapper around the API key set.
///
/// The middleware holds an `Arc<HotKeys>` and on every request loads
/// the current `ApiKeySet` via `ArcSwap`. SIGHUP-triggered reloads
/// build a new set and call `store(...)` — readers see the swap on
/// their next request, no in-flight requests dropped.
pub type HotKeys = arc_swap::ArcSwap<ApiKeySet>;

/// Axum middleware that validates the `Authorization: Bearer <key>` header.
///
/// On success, inserts an [`AuthenticatedKey`] (with tier) into request
/// extensions so downstream handlers know which key was used.  On
/// failure, emits a metrics counter and returns 401.
///
/// Takes `Arc<HotKeys>` so the set can be hot-swapped via SIGHUP.
pub async fn require_api_key(
    keys: Arc<HotKeys>,
    mut req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let keys_snapshot = keys.load();
    if keys_snapshot.is_open() {
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

    match keys_snapshot.matched_key(token.as_bytes()) {
        Some((key_id, tier)) => {
            metrics::counter!("keplor_auth_successes_total").increment(1);
            req.extensions_mut().insert(AuthenticatedKey { key_id, tier });
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
        let set = ApiKeySet::new(vec![], "free");
        assert!(set.is_open());
    }

    #[test]
    fn matches_correct_key() {
        let set = ApiKeySet::new(vec!["secret123".into()], "free");
        assert!(set.matched_key(b"secret123").is_some());
        assert!(set.matched_key(b"wrong").is_none());
        assert!(set.matched_key(b"").is_none());
    }

    #[test]
    fn matches_multiple_keys() {
        let set = ApiKeySet::new(vec!["key1".into(), "key2".into()], "free");
        assert!(set.matched_key(b"key1").is_some());
        assert!(set.matched_key(b"key2").is_some());
        assert!(set.matched_key(b"key3").is_none());
    }

    #[test]
    fn explicit_id_format() {
        let set = ApiKeySet::new(vec!["myapp:secret123".into()], "pro");
        let (id, tier) = set.matched_key(b"secret123").unwrap();
        assert_eq!(id.as_str(), "myapp");
        assert_eq!(tier.as_str(), "pro");
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
        let set = ApiKeySet::new(vec!["prod-key:abc123".into(), "dev-key:xyz789".into()], "free");
        let (id, _) = set.matched_key(b"abc123").unwrap();
        assert_eq!(id, "prod-key");
        let (id, _) = set.matched_key(b"xyz789").unwrap();
        assert_eq!(id, "dev-key");
        assert!(set.matched_key(b"unknown").is_none());
    }

    #[test]
    fn from_entries_with_tiers() {
        let entries = vec![
            ApiKeyEntry { id: "free-key".into(), secret: "sk-free".into(), tier: "free".into() },
            ApiKeyEntry { id: "pro-key".into(), secret: "sk-pro".into(), tier: "pro".into() },
            ApiKeyEntry { id: "team-key".into(), secret: "sk-team".into(), tier: "team".into() },
        ];
        let set = ApiKeySet::from_entries(entries);
        let (id, tier) = set.matched_key(b"sk-free").unwrap();
        assert_eq!(id.as_str(), "free-key");
        assert_eq!(tier.as_str(), "free");
        let (id, tier) = set.matched_key(b"sk-pro").unwrap();
        assert_eq!(id.as_str(), "pro-key");
        assert_eq!(tier.as_str(), "pro");
        let (id, tier) = set.matched_key(b"sk-team").unwrap();
        assert_eq!(id.as_str(), "team-key");
        assert_eq!(tier.as_str(), "team");
    }

    #[test]
    fn from_config_merges_simple_and_extended() {
        let simple = vec!["simple-key:sk-simple".into()];
        let entries =
            vec![ApiKeyEntry { id: "pro-key".into(), secret: "sk-pro".into(), tier: "pro".into() }];
        let set = ApiKeySet::from_config(simple, entries, "free");
        let (_, tier) = set.matched_key(b"sk-simple").unwrap();
        assert_eq!(tier.as_str(), "free"); // default tier
        let (_, tier) = set.matched_key(b"sk-pro").unwrap();
        assert_eq!(tier.as_str(), "pro"); // explicit tier
    }
}
