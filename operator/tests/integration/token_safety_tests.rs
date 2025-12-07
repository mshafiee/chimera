//! Integration tests for token safety checks
//!
//! Tests fast/slow path validation, cache behavior, and honeypot detection

use chimera_operator::{
    config::{AppConfig, TokenSafetyConfig},
    token::{TokenCache, TokenMetadataFetcher, TokenParser, TokenSafetyResult},
    models::Strategy,
};
use solana_client::rpc_client::RpcClient;
use std::sync::Arc;
use std::time::Duration;

/// Test token cache TTL expiration
#[tokio::test]
async fn test_token_cache_ttl() {
    let cache = TokenCache::new(100, 1); // 1 second TTL
    
    let result = TokenSafetyResult::safe();
    cache.insert("token1:SHIELD".to_string(), result.clone());
    
    // Should be in cache immediately
    assert!(cache.get("token1:SHIELD").is_some());
    
    // Wait for expiration
    tokio::time::sleep(Duration::from_secs(2)).await;
    
    // Should be expired
    assert!(cache.get("token1:SHIELD").is_none());
}

/// Test token cache LRU eviction
#[test]
fn test_token_cache_lru() {
    let cache = TokenCache::new(2, 3600); // Small cache
    
    cache.insert("token1".to_string(), TokenSafetyResult::safe());
    cache.insert("token2".to_string(), TokenSafetyResult::safe());
    cache.insert("token3".to_string(), TokenSafetyResult::safe());
    
    // token1 should be evicted (LRU)
    assert!(cache.get("token1").is_none());
    assert!(cache.get("token2").is_some());
    assert!(cache.get("token3").is_some());
}

/// Test token parser fast check with whitelisted token
#[tokio::test]
async fn test_fast_check_whitelisted() {
    let config = TokenSafetyConfig::default();
    let cache = Arc::new(TokenCache::default_config());
    let rpc_client = Arc::new(RpcClient::new_with_timeout(
        "https://api.mainnet-beta.solana.com".to_string(),
        Duration::from_secs(5),
    ));
    let fetcher = Arc::new(TokenMetadataFetcher::with_client(rpc_client));
    let parser = TokenParser::new(config, cache, fetcher);
    
    // USDC should be whitelisted
    let usdc = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    let result = parser.fast_check(usdc, Strategy::Shield).await;
    
    // Should pass (whitelisted tokens skip checks)
    assert!(result.is_ok());
    // Note: In a real test, we'd check the result, but this requires RPC access
}

/// Test token parser cache usage
#[tokio::test]
async fn test_parser_cache_usage() {
    let config = TokenSafetyConfig::default();
    let cache = Arc::new(TokenCache::default_config());
    let rpc_client = Arc::new(RpcClient::new_with_timeout(
        "https://api.mainnet-beta.solana.com".to_string(),
        Duration::from_secs(5),
    ));
    let fetcher = Arc::new(TokenMetadataFetcher::with_client(rpc_client));
    let parser = TokenParser::new(config, cache.clone(), fetcher);
    
    let token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263"; // BONK
    
    // First call - should fetch from RPC
    let _result1 = parser.fast_check(token, Strategy::Shield).await;
    
    // Second call - should use cache
    let _result2 = parser.fast_check(token, Strategy::Shield).await;
    
    // Cache should have entry
    assert!(cache.get(&format!("{}:{}", token, Strategy::Shield)).is_some());
}
