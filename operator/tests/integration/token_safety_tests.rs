//! Integration tests for token safety checks
//!
//! Tests fast-path validation and cache behavior. Whitelisted tokens (USDC)
//! short-circuit BEFORE any RPC fetch, so these tests are deterministic and
//! hermetic — no live network access is required or exercised.

use chimera_operator::{
    models::Strategy,
    token::{TokenCache, TokenMetadataFetcher, TokenParser, TokenSafetyResult},
    TokenSafetyConfig,
};
use solana_client::rpc_client::RpcClient;
use std::sync::Arc;
use std::time::Duration;

const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

/// Test token cache TTL expiration (0-second TTL = immediate expiry, so no
/// wall-clock sleep is needed and the test is deterministic).
#[tokio::test]
async fn test_token_cache_ttl() {
    let cache = TokenCache::new(100, 0); // 0 second TTL — expires immediately

    let result = TokenSafetyResult::safe();
    cache.insert("token1:SHIELD".to_string(), result.clone());

    // TTL 0: the entry must already be expired — get() must return None.
    assert!(cache.get("token1:SHIELD").is_none());

    // Sanity: a long TTL keeps the entry.
    let persistent = TokenCache::new(100, 3600);
    persistent.insert("token2:SHIELD".to_string(), result);
    assert!(persistent.get("token2:SHIELD").is_some());
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
    let parser = TokenParser::new(config, cache.clone(), fetcher);

    // USDC is whitelisted: fast_check returns safe WITHOUT any RPC fetch
    // (the whitelist branch runs before metadata lookup).
    let result = parser.fast_check(USDC_MINT, Strategy::Shield).await;
    let result = result.expect("whitelisted fast check must not error");
    assert!(
        result.safe,
        "whitelisted token must be reported safe: {:?}",
        result.rejection_reason
    );

    // The whitelisted branch also populates the cache (key = "mint:STRATEGY").
    assert!(
        cache.get(&format!("{USDC_MINT}:SHIELD")).is_some(),
        "whitelisted fast check must populate the cache"
    );
}

/// Test token parser cache usage via the whitelisted fast path
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

    // Two identical calls on a whitelisted token: both must return the same
    // safe verdict, and the cache must hold the entry afterwards (the
    // whitelisted branch inserts without any network I/O).
    let result1 = parser
        .fast_check(USDC_MINT, Strategy::Shield)
        .await
        .expect("first fast check must not error");
    let result2 = parser
        .fast_check(USDC_MINT, Strategy::Shield)
        .await
        .expect("second fast check must not error");

    assert_eq!(
        result1.safe, result2.safe,
        "both calls must agree on the safety verdict"
    );
    assert!(result1.safe, "whitelisted token must be safe");
    assert!(
        cache
            .get(&format!("{USDC_MINT}:SHIELD"))
            .is_some(),
        "fast check must leave the result in the cache"
    );
}
