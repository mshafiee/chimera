//! RPC Response Caching
//!
//! Implements FIFO cache for RPC responses with TTL-based expiration (the
//! eviction order tracks insertion order, so the "oldest" entry is removed
//! first). Reduces RPC costs by caching non-critical queries.

use parking_lot::RwLock;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// Cached RPC response
#[derive(Debug, Clone)]
struct CachedResponse {
    data: Arc<Vec<u8>>,
    cached_at: SystemTime,
    ttl: Duration,
}

/// Internal cache state: entries plus a FIFO insertion-order key queue.
struct CacheState {
    entries: HashMap<String, CachedResponse>,
    order: VecDeque<String>,
}

/// RPC response cache
pub struct RpcCache {
    /// Cache storage (key -> response) + insertion order for FIFO eviction
    cache: Arc<RwLock<CacheState>>,
    /// Maximum cache size
    max_size: usize,
}

impl RpcCache {
    /// Create a new RPC cache
    pub fn new(max_size: usize) -> Self {
        Self {
            cache: Arc::new(RwLock::new(CacheState {
                entries: HashMap::new(),
                order: VecDeque::new(),
            })),
            max_size,
        }
    }

    /// Get cached response
    ///
    /// # Arguments
    /// * `key` - Cache key (e.g., "getBalance:wallet_address")
    ///
    /// # Returns
    /// Some(response) if cached and not expired, None otherwise. Expiry uses
    /// the TTL captured at `set()` time — the caller cannot extend a stored
    /// entry's lifetime by asking with a larger TTL.
    pub fn get(&self, key: &str) -> Option<Arc<Vec<u8>>> {
        let cache = self.cache.read();
        if let Some(cached) = cache.entries.get(key) {
            if cached.cached_at.elapsed().unwrap_or_default() < cached.ttl {
                return Some(cached.data.clone());
            }
        }
        None
    }

    /// Store response in cache
    ///
    /// # Arguments
    /// * `key` - Cache key
    /// * `data` - Response data
    /// * `ttl` - Time to live
    pub fn set(&self, key: String, data: Vec<u8>, ttl: Duration) {
        let mut cache = self.cache.write();

        // A max_size of 0 means caching is disabled entirely.
        if self.max_size == 0 {
            return;
        }

        // Opportunistic self-clean: purge expired entries while we hold the
        // write lock, so stale data cannot accumulate or evict live entries
        // even if callers never invoke clear_expired().
        cache
            .entries
            .retain(|k, cached| cached.cached_at.elapsed().unwrap_or_default() < cached.ttl || k == &key);
        let order = std::mem::take(&mut cache.order);
        cache.order = order
            .into_iter()
            .filter(|k| cache.entries.contains_key(k))
            .collect();

        // If cache is full, evict the oldest inserted entry (FIFO).
        if cache.entries.len() >= self.max_size {
            while let Some(oldest) = cache.order.pop_front() {
                if cache.entries.remove(&oldest).is_some() {
                    break;
                }
            }
        }

        let is_new = !cache.entries.contains_key(&key);
        cache.entries.insert(
            key.clone(),
            CachedResponse {
                data: Arc::new(data),
                cached_at: SystemTime::now(),
                ttl,
            },
        );
        if is_new {
            cache.order.push_back(key);
        }
    }

    /// Clear expired entries
    pub fn clear_expired(&self) {
        let mut cache = self.cache.write();
        cache
            .entries
            .retain(|_, cached| cached.cached_at.elapsed().unwrap_or_default() < cached.ttl);
        let order = std::mem::take(&mut cache.order);
        cache.order = order
            .into_iter()
            .filter(|k| cache.entries.contains_key(k))
            .collect();
    }

    /// Clear all cache entries
    pub fn clear_all(&self) {
        let mut cache = self.cache.write();
        cache.entries.clear();
        cache.order.clear();
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        let cache = self.cache.read();
        CacheStats {
            size: cache.entries.len(),
            max_size: self.max_size,
        }
    }
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub size: usize,
    pub max_size: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rpc_cache() {
        let cache = RpcCache::new(100);

        // Store and retrieve
        cache.set(
            "test_key".to_string(),
            b"test_data".to_vec(),
            Duration::from_secs(10),
        );

        let data = cache.get("test_key");
        assert!(data.is_some());
        assert_eq!(data.unwrap().as_slice(), b"test_data");
    }

    #[test]
    fn test_rpc_cache_expiration() {
        let cache = RpcCache::new(100);

        // Store with short TTL
        cache.set(
            "test_key".to_string(),
            b"test_data".to_vec(),
            Duration::from_millis(50),
        );

        // Should be available immediately
        assert!(cache.get("test_key").is_some());

        // After expiration, should return None
        std::thread::sleep(Duration::from_millis(80));
        assert!(cache.get("test_key").is_none());
    }

    #[test]
    fn test_rpc_cache_evicts_oldest_first() {
        let cache = RpcCache::new(2);

        cache.set("key1".to_string(), b"one".to_vec(), Duration::from_secs(10));
        cache.set("key2".to_string(), b"two".to_vec(), Duration::from_secs(10));
        cache.set("key3".to_string(), b"three".to_vec(), Duration::from_secs(10));

        // key1 was inserted first and must be evicted first (FIFO).
        assert!(cache.get("key1").is_none());
        assert!(cache.get("key2").is_some());
        assert!(cache.get("key3").is_some());
    }

    #[test]
    fn test_rpc_cache_zero_max_size_stores_nothing() {
        let cache = RpcCache::new(0);

        cache.set("key1".to_string(), b"one".to_vec(), Duration::from_secs(10));
        assert!(cache.get("key1").is_none());
        assert_eq!(cache.stats().size, 0);
    }
}
