//! In-memory idempotency cache with TTL.
//!
//! Prevents duplicate event creation when clients retry requests with the
//! same `Idempotency-Key` header within a configurable time window.

use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use lru::LruCache;
use smol_str::SmolStr;

use crate::schema::IngestResponse;

/// Cached response for a completed idempotent request.
struct CachedEntry {
    response: IngestResponse,
    expires_at: Instant,
}

/// Thread-safe idempotency cache backed by an LRU map with TTL expiry.
pub struct IdempotencyCache {
    cache: Mutex<LruCache<SmolStr, CachedEntry>>,
    ttl: Duration,
}

impl IdempotencyCache {
    /// Create a new cache with the given capacity and TTL.
    pub fn new(max_entries: usize, ttl: Duration) -> Self {
        // SAFETY: 1 is always non-zero.
        let one = NonZeroUsize::MIN;
        let cap = NonZeroUsize::new(max_entries).unwrap_or(one);
        Self { cache: Mutex::new(LruCache::new(cap)), ttl }
    }

    /// Look up a cached response by idempotency key.
    ///
    /// Returns `Some` if the key exists and has not expired, `None` otherwise.
    /// Expired entries are removed on access.
    pub fn get(&self, key: &str) -> Option<IngestResponse> {
        let key = SmolStr::new(key);
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = cache.get(&key) {
            if Instant::now() < entry.expires_at {
                return Some(entry.response.clone());
            }
            // Expired — remove it.
            cache.pop(&key);
        }
        None
    }

    /// Insert a response for the given idempotency key.
    pub fn insert(&self, key: &str, response: IngestResponse) {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.put(
            SmolStr::new(key),
            CachedEntry { response, expires_at: Instant::now() + self.ttl },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_response() -> IngestResponse {
        IngestResponse {
            id: "01ABC".into(),
            cost_nanodollars: 100,
            model: "gpt-4o".into(),
            provider: "openai".into(),
        }
    }

    #[test]
    fn cache_hit_returns_response() {
        let cache = IdempotencyCache::new(100, Duration::from_secs(300));
        cache.insert("key-1", sample_response());
        let hit = cache.get("key-1");
        assert!(hit.is_some());
        assert_eq!(hit.as_ref().map(|r| r.id.as_str()), Some("01ABC"));
    }

    #[test]
    fn cache_miss_returns_none() {
        let cache = IdempotencyCache::new(100, Duration::from_secs(300));
        assert!(cache.get("nonexistent").is_none());
    }

    #[test]
    fn expired_entry_returns_none() {
        let cache = IdempotencyCache::new(100, Duration::from_millis(1));
        cache.insert("key-1", sample_response());
        std::thread::sleep(Duration::from_millis(10));
        assert!(cache.get("key-1").is_none());
    }

    #[test]
    fn different_keys_are_independent() {
        let cache = IdempotencyCache::new(100, Duration::from_secs(300));
        cache.insert("key-1", sample_response());
        assert!(cache.get("key-2").is_none());
    }

    #[test]
    fn lru_eviction_works() {
        let cache = IdempotencyCache::new(2, Duration::from_secs(300));
        cache.insert("key-1", sample_response());
        cache.insert("key-2", sample_response());
        cache.insert("key-3", sample_response()); // evicts key-1
        assert!(cache.get("key-1").is_none());
        assert!(cache.get("key-2").is_some());
        assert!(cache.get("key-3").is_some());
    }
}
