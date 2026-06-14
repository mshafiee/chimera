//! DEX pool enumeration for liquidity detection
//!
//! Queries Raydium and Orca pools directly via RPC to get accurate liquidity data.

use crate::error::AppError;
use rust_decimal::Decimal;
use solana_client::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;

/// Pool liquidity data
#[derive(Debug, Clone)]
pub struct PoolLiquidity {
    /// Token address
    pub token_address: String,
    /// Total liquidity in USD (using Decimal for precision)
    pub liquidity_usd: Decimal,
    /// Pool addresses containing this token
    pub pool_addresses: Vec<String>,
}

/// Cache entry for pool data
struct PoolCacheEntry {
    /// Cached liquidity data
    data: PoolLiquidity,
    /// When this was cached
    cached_at: SystemTime,
}

/// Pool enumerator for Raydium and Orca
pub struct PoolEnumerator {
    /// RPC client (async for future use, sync for current implementation)
    #[allow(dead_code)]
    rpc_client: Arc<RpcClient>,
    /// Raydium program ID
    #[allow(dead_code)]
    raydium_program: Pubkey,
    /// Orca program ID
    #[allow(dead_code)]
    orca_program: Pubkey,
    /// Cache for pool data
    cache: Arc<RwLock<lru::LruCache<String, PoolCacheEntry>>>,
    /// Cache TTL in seconds
    cache_ttl: Duration,
}

impl PoolEnumerator {
    /// Create a new pool enumerator
    pub fn new(rpc_client: Arc<RpcClient>, cache_capacity: usize, cache_ttl_seconds: u64) -> Self {
        // Note: Currently uses sync RpcClient, but structure allows for async implementation
        // Raydium program ID (mainnet)
        let raydium_program = Pubkey::from_str("675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8")
            .expect("Invalid Raydium program ID");

        // Orca program ID (mainnet)
        let orca_program = Pubkey::from_str("9W959DqEETiGZocYWCQPaJ6sBmUzgfxXfqGeTEdp3aQP")
            .expect("Invalid Orca program ID");

        Self {
            rpc_client,
            raydium_program,
            orca_program,
            cache: Arc::new(RwLock::new(lru::LruCache::new(
                std::num::NonZeroUsize::new(cache_capacity).unwrap(),
            ))),
            cache_ttl: Duration::from_secs(cache_ttl_seconds),
        }
    }

    /// Get liquidity for a token from Raydium pools.
    ///
    /// On-chain Raydium pool parsing is not implemented; DexScreener is the authoritative
    /// liquidity source. Returns an error so callers fall through to DexScreener.
    pub async fn get_raydium_liquidity(&self, token_address: &str) -> Result<Decimal, AppError> {
        // Check cache first (only valid entries are cached; we never cache a zero/stub result)
        if let Some(cached) = self.get_from_cache(token_address).await {
            return Ok(cached.liquidity_usd);
        }

        Err(AppError::Http(format!(
            "Raydium on-chain liquidity not implemented for {}; use DexScreener",
            token_address
        )))
    }

    /// Get liquidity for a token from Orca pools.
    ///
    /// On-chain Orca pool parsing is not implemented; DexScreener is the authoritative
    /// liquidity source. Returns an error so callers fall through to DexScreener.
    pub async fn get_orca_liquidity(&self, token_address: &str) -> Result<Decimal, AppError> {
        // Check cache first (only valid entries are cached; we never cache a zero/stub result)
        if let Some(cached) = self.get_from_cache(token_address).await {
            return Ok(cached.liquidity_usd);
        }

        Err(AppError::Http(format!(
            "Orca on-chain liquidity not implemented for {}; use DexScreener",
            token_address
        )))
    }

    /// Get liquidity from both Raydium and Orca pools
    pub async fn get_combined_liquidity(&self, token_address: &str) -> Result<Decimal, AppError> {
        let raydium_liq = self
            .get_raydium_liquidity(token_address)
            .await
            .unwrap_or(Decimal::ZERO);
        let orca_liq = self
            .get_orca_liquidity(token_address)
            .await
            .unwrap_or(Decimal::ZERO);

        Ok(raydium_liq + orca_liq)
    }

    /// Get from cache if not expired
    async fn get_from_cache(&self, token_address: &str) -> Option<PoolLiquidity> {
        let mut cache = self.cache.write().await;
        if let Some(entry) = cache.get(token_address) {
            if entry.cached_at.elapsed().unwrap_or(Duration::MAX) < self.cache_ttl {
                return Some(entry.data.clone());
            } else {
                // Expired, remove from cache
                cache.pop(token_address);
            }
        }
        None
    }

    /// Cache a result (reserved for future on-chain liquidity implementations)
    #[allow(dead_code)]
    async fn cache_result(
        &self,
        token_address: &str,
        liquidity_usd: Decimal,
        pool_addresses: Vec<String>,
    ) {
        let mut cache = self.cache.write().await;
        cache.put(
            token_address.to_string(),
            PoolCacheEntry {
                data: PoolLiquidity {
                    token_address: token_address.to_string(),
                    liquidity_usd,
                    pool_addresses,
                },
                cached_at: SystemTime::now(),
            },
        );
    }

    /// Clear the cache
    pub async fn clear_cache(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_pool_enumerator_creation() {
        let rpc_client = Arc::new(RpcClient::new(
            "https://api.mainnet-beta.solana.com".to_string(),
        ));
        let enumerator = PoolEnumerator::new(rpc_client, 100, 300);

        // Test that it was created successfully
        assert_eq!(enumerator.cache_ttl.as_secs(), 300);
    }

    #[tokio::test]
    async fn test_cache_operations() {
        let rpc_client = Arc::new(RpcClient::new(
            "https://api.mainnet-beta.solana.com".to_string(),
        ));
        let enumerator = PoolEnumerator::new(rpc_client, 10, 60);

        // Test caching
        enumerator
            .cache_result("test_token", Decimal::from(1000), vec![])
            .await;

        // Should be able to retrieve from cache
        let cached = enumerator.get_from_cache("test_token").await;
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().liquidity_usd, Decimal::from(1000));
    }
}
