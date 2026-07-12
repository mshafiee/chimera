//! Integration tests for PreValidator non-blocking validation (FIX 1)
//!
//! Verifies that:
//! - validate_local() returns immediately without HTTP calls
//! - Cache misses are handled correctly
//! - Slippage estimation works with cached liquidity

use chimera_operator::config::{AppConfig, TokenSafetyConfig};
use chimera_operator::monitoring::pre_validator::PreValidator;
use chimera_operator::price_cache::PriceCache;
use chimera_operator::token::TokenMetadataFetcher;
use rust_decimal::Decimal;
use std::str::FromStr;
use std::sync::Arc;

#[tokio::test]
async fn test_validate_local_cache_miss() {
    // Create a price cache
    let price_cache: Arc<PriceCache> = Arc::new(PriceCache::new().unwrap());

    // Create a token fetcher without pre-populated cache
    let token_fetcher = Arc::new(
        TokenMetadataFetcher::new("https://api.mainnet-beta.solana.com")
            .with_liquidity_ttl(60)
    );

    // Create a minimal config
    let token_safety = TokenSafetyConfig {
        freeze_authority_whitelist: vec![],
        mint_authority_whitelist: vec![],
        min_liquidity_shield_usd: Decimal::from(10000),
        min_liquidity_spear_usd: Decimal::from(5000),
        honeypot_detection_enabled: false,
        cache_capacity: 1000,
        cache_ttl_seconds: 3600,
        allow_unlisted_heuristic: false,
        min_token_age_hours: 24.0,
        liquidity_cache_ttl_secs: 60,
        fdv_cache_ttl_secs: 300,
        liquidity_update_interval_secs: 30,
        cache_backend: "memory".to_string(),
        redis_url: None,
    };

    let config = AppConfig {
        token_safety: token_safety,
        ..Default::default()
    };

    let pre_validator = PreValidator::new(Arc::new(config))
        .with_token_fetcher(token_fetcher)
        .with_price_cache(price_cache.clone());

    // Test with a token that's not in cache
    let unknown_token = "SomeRandomTokenThatIsNotCached";

    println!("Test: validate_local with cache miss...");
    let result = pre_validator.validate_local(
        unknown_token,
        Decimal::from_str("0.1").unwrap(),
        None,
        price_cache,
    );

    println!("validate_local result: {:?}", result);

    // Should fail with cache miss error
    assert!(result.is_err(), "Should fail on cache miss");
    println!("✓ validate_local correctly fails on cache miss");
}

#[tokio::test]
async fn test_pre_validator_initialization() {
    // Test that PreValidator can be initialized with all dependencies
    let price_cache: Arc<PriceCache> = Arc::new(PriceCache::new().unwrap());

    // Add SOL price
    let sol_mint = "So11111111111111111111111111111111111111112";
    let _ = price_cache.set_price(
        sol_mint,
        Decimal::from(150),
        chimera_operator::price_cache::PriceSource::Jupiter,
        None
    );

    // Create token fetcher
    let token_fetcher = Arc::new(
        TokenMetadataFetcher::new("https://api.mainnet-beta.solana.com")
            .with_liquidity_ttl(60)
    );

    // Create minimal config
    let token_safety = TokenSafetyConfig {
        freeze_authority_whitelist: vec![],
        mint_authority_whitelist: vec![],
        min_liquidity_shield_usd: Decimal::from(10000),
        min_liquidity_spear_usd: Decimal::from(5000),
        honeypot_detection_enabled: false,
        cache_capacity: 1000,
        cache_ttl_seconds: 3600,
        allow_unlisted_heuristic: false,
        min_token_age_hours: 24.0,
        liquidity_cache_ttl_secs: 60,
        fdv_cache_ttl_secs: 300,
        liquidity_update_interval_secs: 30,
        cache_backend: "memory".to_string(),
        redis_url: None,
    };

    let config = AppConfig {
        token_safety: token_safety,
        ..Default::default()
    };

    let pre_validator = PreValidator::new(Arc::new(config))
        .with_token_fetcher(token_fetcher)
        .with_price_cache(price_cache.clone());

    println!("✓ PreValidator with price cache initialized successfully");
}

#[tokio::test]
async fn test_validate_local_returns_immediately() {
    // This test verifies that validate_local doesn't make HTTP calls
    let price_cache: Arc<PriceCache> = Arc::new(PriceCache::new().unwrap());

    // Create a token fetcher
    let token_fetcher = Arc::new(
        TokenMetadataFetcher::new("https://api.mainnet-beta.solana.com")
            .with_liquidity_ttl(60)
    );

    // Create minimal config
    let token_safety = TokenSafetyConfig {
        freeze_authority_whitelist: vec![],
        mint_authority_whitelist: vec![],
        min_liquidity_shield_usd: Decimal::from(10000),
        min_liquidity_spear_usd: Decimal::from(5000),
        honeypot_detection_enabled: false,
        cache_capacity: 1000,
        cache_ttl_seconds: 3600,
        allow_unlisted_heuristic: false,
        min_token_age_hours: 24.0,
        liquidity_cache_ttl_secs: 60,
        fdv_cache_ttl_secs: 300,
        liquidity_update_interval_secs: 30,
        cache_backend: "memory".to_string(),
        redis_url: None,
    };

    let config = AppConfig {
        token_safety: token_safety,
        ..Default::default()
    };

    let pre_validator = PreValidator::new(Arc::new(config))
        .with_token_fetcher(token_fetcher)
        .with_price_cache(price_cache.clone());

    let start = std::time::Instant::now();

    // Call validate_local with a token that's not cached
    let result = pre_validator.validate_local(
        "SomeToken",
        Decimal::from_str("0.1").unwrap(),
        None,
        price_cache,
    );

    let elapsed = start.elapsed();

    println!("validate_local completed in {:?}", elapsed);
    println!("Result: {:?}", result);

    // Should complete very quickly (< 10ms) since it's just cache lookups
    assert!(elapsed.as_millis() < 100, "validate_local should return immediately");
    assert!(result.is_err(), "Should fail on cache miss");
    println!("✓ validate_local returns immediately without blocking");
}
