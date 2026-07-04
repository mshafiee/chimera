//! Integration test for decimals caching functionality
//!
//! This test verifies that:
//! 1. PriceCache stores decimals from Jupiter API responses
//! 2. TokenMetadataFetcher uses the fast path via PriceCache
//! 3. Fallback to RPC works when decimals not in cache

use chimera_operator::price_cache::{PriceCache, PriceSource};
use chimera_operator::token::TokenMetadataFetcher;
use rust_decimal::Decimal;
use std::sync::Arc;

#[tokio::test]
async fn test_decimals_cache_storage() {
    let cache = PriceCache::new().expect("Failed to create PriceCache");

    // Simulate storing decimals from Jupiter
    cache.set_price(
        "So11111111111111111111111111111111111111112", // SOL
        Decimal::from(150),
        PriceSource::Jupiter,
        Some(9),
    );

    // Verify decimals are retrievable
    let decimals = cache.get_decimals("So11111111111111111111111111111111111111112");
    assert_eq!(decimals, Some(9), "SOL should have 9 decimals");
}

#[tokio::test]
async fn test_decimals_cache_miss() {
    let cache = PriceCache::new().expect("Failed to create PriceCache");

    // Try to get decimals for a token that's not cached
    let decimals = cache.get_decimals("UnknownToken11111111111111111111111111111");
    assert_eq!(decimals, None, "Unknown token should return None");
}

#[tokio::test]
async fn test_decimals_in_price_entry() {
    let cache = PriceCache::new().expect("Failed to create PriceCache");

    // Set price with decimals
    cache.set_price(
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", // USDC
        Decimal::from(1),
        PriceSource::Jupiter,
        Some(6),
    );

    // Decimals should be accessible via get_decimals
    let decimals = cache.get_decimals("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
    assert_eq!(decimals, Some(6), "USDC should have 6 decimals");
}

#[tokio::test]
async fn test_metadata_fetcher_with_price_cache() {
    // Create a PriceCache with pre-populated decimals
    let cache = PriceCache::new().expect("Failed to create PriceCache");
    cache.set_price(
        "So11111111111111111111111111111111111111112",
        Decimal::from(150),
        PriceSource::Jupiter,
        Some(9),
    );

    // Create TokenMetadataFetcher with PriceCache reference
    let fetcher = TokenMetadataFetcher::new("mock_rpc_url")
        .with_price_cache(Arc::new(cache));

    // Try to get decimals (should use fast path from PriceCache)
    let decimals = fetcher.get_decimals_only("So11111111111111111111111111111111111111112").await;

    assert_eq!(decimals, Some(9), "Should get decimals from PriceCache");
}

#[tokio::test]
async fn test_metadata_fetcher_fallback() {
    // Create an empty PriceCache (no decimals)
    let cache = PriceCache::new().expect("Failed to create PriceCache");

    // Create TokenMetadataFetcher with PriceCache reference
    let fetcher = TokenMetadataFetcher::new("mock_rpc_url")
        .with_price_cache(Arc::new(cache));

    // Try to get decimals for an unknown token
    // Should fall back to RPC (which will fail in this test, returning None)
    let decimals = fetcher.get_decimals_only("UnknownToken11111111111111111111111111111111").await;

    // None is expected since we don't have a real RPC connection
    assert_eq!(decimals, None, "Should return None when not in cache and RPC fails");
}
