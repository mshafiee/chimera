//! LRU Token Cache with TTL
//!
//! Caches token safety check results to avoid repeated RPC calls.
//! - Key: `{token_address}:{strategy}` or `{token_address}:{strategy}:full`
//! - TTL: 1 hour by default
//! - Capacity: 1000 tokens by default

use super::TokenSafetyResult;
use chrono::{DateTime, Duration, Utc};
use lru::LruCache;
use parking_lot::Mutex;
use std::num::NonZeroUsize;

/// Cache entry with timestamp for TTL checking
#[derive(Clone)]
struct CacheEntry {
    result: TokenSafetyResult,
    cached_at: DateTime<Utc>,
}

/// LRU cache for token safety results
pub struct TokenCache {
    /// The underlying LRU cache
    cache: Mutex<LruCache<String, CacheEntry>>,
    /// Time-to-live for cache entries
    ttl: Duration,
}

impl TokenCache {
    /// Create a new token cache
    ///
    /// # Arguments
    /// * `capacity` - Maximum number of tokens to cache
    /// * `ttl_seconds` - Time-to-live in seconds for each entry
    pub fn new(capacity: usize, ttl_seconds: i64) -> Self {
        let cap = NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::new(1000).unwrap());
        Self {
            cache: Mutex::new(LruCache::new(cap)),
            ttl: Duration::seconds(ttl_seconds),
        }
    }

    /// Create a new token cache with default settings
    /// - Capacity: 1000 tokens
    /// - TTL: 1 hour (3600 seconds)
    pub fn default_config() -> Self {
        Self::new(1000, 3600)
    }

    /// Get a cached result if it exists and hasn't expired
    pub fn get(&self, key: &str) -> Option<TokenSafetyResult> {
        let mut cache = self.cache.lock();

        if let Some(entry) = cache.get(key) {
            // Check if entry has expired
            let age = Utc::now() - entry.cached_at;
            if age < self.ttl {
                tracing::trace!(key = key, age_secs = age.num_seconds(), "Cache hit");
                return Some(entry.result.clone());
            } else {
                // Entry expired, remove it
                tracing::trace!(key = key, "Cache entry expired");
                cache.pop(key);
            }
        }

        None
    }

    /// Insert a result into the cache
    pub fn insert(&self, key: String, result: TokenSafetyResult) {
        let entry = CacheEntry {
            result,
            cached_at: Utc::now(),
        };

        let mut cache = self.cache.lock();
        cache.put(key.clone(), entry);

        tracing::trace!(key = key, "Cache insert");
    }

    /// Remove an entry from the cache
    pub fn invalidate(&self, key: &str) {
        let mut cache = self.cache.lock();
        cache.pop(key);
        tracing::trace!(key = key, "Cache invalidate");
    }

    /// Clear all entries from the cache
    pub fn clear(&self) {
        let mut cache = self.cache.lock();
        cache.clear();
        tracing::debug!("Cache cleared");
    }

    /// Get the current number of entries in the cache
    pub fn len(&self) -> usize {
        self.cache.lock().len()
    }

    /// Check if the cache is empty
    pub fn is_empty(&self) -> bool {
        self.cache.lock().is_empty()
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        let cache = self.cache.lock();
        CacheStats {
            entries: cache.len(),
            capacity: cache.cap().get(),
        }
    }
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    /// Current number of entries
    pub entries: usize,
    /// Maximum capacity
    pub capacity: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_insert_and_get() {
        let cache = TokenCache::new(10, 3600);

        let result = TokenSafetyResult::safe();
        cache.insert("token1:SHIELD".to_string(), result.clone());

        let cached = cache.get("token1:SHIELD");
        assert!(cached.is_some());
        assert!(cached.unwrap().safe);
    }

    #[test]
    fn test_cache_miss() {
        let cache = TokenCache::new(10, 3600);

        let cached = cache.get("nonexistent");
        assert!(cached.is_none());
    }

    #[test]
    fn test_cache_expiry() {
        // Create cache with 0 second TTL (immediate expiry)
        let cache = TokenCache::new(10, 0);

        let result = TokenSafetyResult::safe();
        cache.insert("token1:SHIELD".to_string(), result);

        // Should be expired immediately
        std::thread::sleep(std::time::Duration::from_millis(10));
        let cached = cache.get("token1:SHIELD");
        assert!(cached.is_none());
    }

    #[test]
    fn test_cache_invalidate() {
        let cache = TokenCache::new(10, 3600);

        let result = TokenSafetyResult::safe();
        cache.insert("token1:SHIELD".to_string(), result);

        cache.invalidate("token1:SHIELD");

        let cached = cache.get("token1:SHIELD");
        assert!(cached.is_none());
    }

    #[test]
    fn test_cache_clear() {
        let cache = TokenCache::new(10, 3600);

        cache.insert("token1:SHIELD".to_string(), TokenSafetyResult::safe());
        cache.insert("token2:SPEAR".to_string(), TokenSafetyResult::safe());

        assert_eq!(cache.len(), 2);

        cache.clear();

        assert!(cache.is_empty());
    }

    #[test]
    fn test_cache_lru_eviction() {
        // Small cache to test eviction
        let cache = TokenCache::new(2, 3600);

        cache.insert("token1".to_string(), TokenSafetyResult::safe());
        cache.insert("token2".to_string(), TokenSafetyResult::safe());
        cache.insert("token3".to_string(), TokenSafetyResult::safe());

        // token1 should be evicted (LRU)
        assert!(cache.get("token1").is_none());
        assert!(cache.get("token2").is_some());
        assert!(cache.get("token3").is_some());
    }
}
