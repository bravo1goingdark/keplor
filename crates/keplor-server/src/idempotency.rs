//! In-memory idempotency cache with TTL.
//!
//! Prevents duplicate event creation when clients retry requests with the
//! same `Idempotency-Key` header within a configurable time window.
//!
//! The cache is sharded 16-way to reduce mutex contention under high
//! concurrency.

use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use lru::LruCache;
use smol_str::SmolStr;

use crate::schema::IngestResponse;

/// Number of independent LRU shards.
const NUM_SHARDS: usize = 16;

/// Cached response for a completed idempotent request.
struct CachedEntry {
    response: IngestResponse,
    expires_at: Instant,
}

/// Thread-safe idempotency cache backed by sharded LRU maps with TTL expiry.
pub struct IdempotencyCache {
    shards: [Mutex<LruCache<SmolStr, CachedEntry>>; NUM_SHARDS],
    ttl: Duration,
}

impl IdempotencyCache {
    /// Create a new cache with the given total capacity and TTL.
    ///
    /// Capacity is divided evenly across shards.
    pub fn new(max_entries: usize, ttl: Duration) -> Self {
        let per_shard = (max_entries / NUM_SHARDS).max(1);
        let cap = NonZeroUsize::new(per_shard).unwrap_or(NonZeroUsize::MIN);
        let shards = std::array::from_fn(|_| Mutex::new(LruCache::new(cap)));
        Self { shards, ttl }
    }

    /// Look up a cached response by idempotency key.
    ///
    /// Returns `Some` if the key exists and has not expired, `None` otherwise.
    /// Expired entries are removed on access.
    pub fn get(&self, key: &str) -> Option<IngestResponse> {
        let key = SmolStr::new(key);
        let shard = &self.shards[shard_index(&key)];
        let mut cache = shard.lock().unwrap_or_else(|e| e.into_inner());
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
        let smol = SmolStr::new(key);
        let shard = &self.shards[shard_index(&smol)];
        let mut cache = shard.lock().unwrap_or_else(|e| e.into_inner());
        cache.put(smol, CachedEntry { response, expires_at: Instant::now() + self.ttl });
    }
}

/// Map a key to a shard index using a fast hash.
#[inline]
fn shard_index(key: &SmolStr) -> usize {
    let mut hasher = std::hash::DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish() as usize % NUM_SHARDS
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
        // 2 total entries → 1 per shard (floor division). Keys that hash
        // to the same shard will evict each other.
        let cache = IdempotencyCache::new(NUM_SHARDS * 2, Duration::from_secs(300));
        cache.insert("key-1", sample_response());
        cache.insert("key-2", sample_response());
        cache.insert("key-3", sample_response());
        // At least 2 of the 3 should be retrievable (they land in
        // different shards unless they collide).
        let found = ["key-1", "key-2", "key-3"].iter().filter(|k| cache.get(k).is_some()).count();
        assert!(found >= 2, "expected at least 2 of 3 keys to survive, got {found}");
    }

    #[test]
    fn shards_are_independent() {
        // Over-provision capacity so hash collisions don't evict.
        let cache = IdempotencyCache::new(NUM_SHARDS * 32, Duration::from_secs(300));
        for i in 0..64 {
            cache.insert(&format!("key-{i}"), sample_response());
        }
        let found = (0..64).filter(|i| cache.get(&format!("key-{i}")).is_some()).count();
        assert_eq!(found, 64, "all 64 keys should be retrievable with sufficient capacity");
    }
}
