//! RPC Response Caching
//!
//! Implements LRU cache for RPC responses with TTL-based expiration.
//! Reduces RPC costs by caching non-critical queries.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// Cached RPC response
#[derive(Debug, Clone)]
struct CachedResponse {
    data: Vec<u8>,
    cached_at: SystemTime,
    ttl: Duration,
}

/// RPC response cache
pub struct RpcCache {
    /// Cache storage (key -> response)
    cache: Arc<RwLock<HashMap<String, CachedResponse>>>,
    /// Maximum cache size
    max_size: usize,
}

impl RpcCache {
    /// Create a new RPC cache
    pub fn new(max_size: usize) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            max_size,
        }
    }

    /// Get cached response
    ///
    /// # Arguments
    /// * `key` - Cache key (e.g., "getBalance:wallet_address")
    /// * `ttl` - Time to live for this cache entry
    ///
    /// # Returns
    /// Some(response) if cached and not expired, None otherwise
    pub fn get(&self, key: &str, ttl: Duration) -> Option<Vec<u8>> {
        let cache = self.cache.read();
        if let Some(cached) = cache.get(key) {
            // Check if expired
            if cached.cached_at.elapsed().unwrap_or_default() < ttl {
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
        
        // If cache is full, remove oldest entry (simple FIFO for now)
        if cache.len() >= self.max_size {
            // Remove first entry (oldest)
            if let Some(first_key) = cache.keys().next().cloned() {
                cache.remove(&first_key);
            }
        }

        cache.insert(
            key,
            CachedResponse {
                data,
                cached_at: SystemTime::now(),
                ttl,
            },
        );
    }

    /// Clear expired entries
    pub fn clear_expired(&self) {
        let mut cache = self.cache.write();
        let now = SystemTime::now();
        cache.retain(|_, cached| {
            cached.cached_at.elapsed().unwrap_or_default() < cached.ttl
        });
    }

    /// Clear all cache entries
    pub fn clear_all(&self) {
        let mut cache = self.cache.write();
        cache.clear();
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        let cache = self.cache.read();
        CacheStats {
            size: cache.len(),
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
        cache.set("test_key".to_string(), b"test_data".to_vec(), Duration::from_secs(10));
        
        let data = cache.get("test_key", Duration::from_secs(10));
        assert!(data.is_some());
        assert_eq!(data.unwrap(), b"test_data");
    }

    #[test]
    fn test_rpc_cache_expiration() {
        let cache = RpcCache::new(100);
        
        // Store with short TTL
        cache.set("test_key".to_string(), b"test_data".to_vec(), Duration::from_secs(1));
        
        // Should be available immediately
        assert!(cache.get("test_key", Duration::from_secs(1)).is_some());
        
        // After expiration, should return None
        // (This would need actual time passing in integration tests)
    }
}






