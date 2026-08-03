//! Integration tests for Helius token age fetching
//!
//! Tests that token age is correctly fetched from Helius API
//! and used in signal quality scoring.

use chimera_operator::monitoring::HeliusClient;
use chimera_operator::token::TokenMetadata;
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use parking_lot::RwLock;

#[tokio::test]
#[ignore] // Requires Helius API key - run with: cargo test -- --ignored
async fn test_helius_token_age_fetching() {
    // Get API key from environment or use test key
    let api_key = env::var("HELIUS_API_KEY").expect("HELIUS_API_KEY must be set for this test");

    let cache = Arc::new(RwLock::new(HashMap::<String, TokenMetadata>::new()));
    let client = HeliusClient::new(api_key, cache).expect("Failed to create HeliusClient");

    // Test with USDC (known token, should exist)
    let usdc_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    // This test is deliberately run with a real API key; a broken integration
    // or auth failure must FAIL the test, not pass silently.
    let age_hours = client
        .get_token_age_hours(usdc_mint)
        .await
        .expect("Helius API must succeed for a known token like USDC")
        .expect("USDC is a long-lived token and must have a token age");
    println!("USDC token age: {:.2} hours", age_hours);
    assert!(age_hours > 24.0, "USDC should be older than 24 hours");
}

#[tokio::test]
#[ignore] // Requires Helius API key - run with: cargo test -- --ignored
async fn test_helius_token_age_caching() {
    let api_key = env::var("HELIUS_API_KEY").expect("HELIUS_API_KEY must be set for this test");

    let cache = Arc::new(RwLock::new(HashMap::<String, TokenMetadata>::new()));
    let client = HeliusClient::new(api_key, cache).expect("Failed to create HeliusClient");
    let token_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    // Fail loudly on API errors so a broken integration is caught.
    let result1 = client
        .get_token_age_hours(token_mint)
        .await
        .expect("first Helius call must succeed")
        .expect("USDC must have a token age");
    let result2 = client
        .get_token_age_hours(token_mint)
        .await
        .expect("second Helius call must succeed")
        .expect("USDC must have a token age");

    // NOTE: equal results are consistent with a cache hit, but do not prove one
    // (two live calls could return the same age). A strict cache-hit proof
    // requires a mock client with a request counter.
    assert_eq!(result1, result2, "Second call should return the same age");
}

#[tokio::test]
#[ignore] // Avoid live network call to Helius with a fake key in default test runs
async fn test_helius_token_age_invalid_token() {
    // Test with invalid token address
    let api_key = "test-key".to_string();
    let cache = Arc::new(RwLock::new(HashMap::<String, TokenMetadata>::new()));
    let client = HeliusClient::new(api_key, cache).expect("Failed to create HeliusClient");

    let invalid_mint = "InvalidTokenAddress111111111111111111111111";

    // Should handle gracefully (return None or error) — but must NOT report a
    // token age for an invalid mint.
    let result = client.get_token_age_hours(invalid_mint).await;

    match result {
        Ok(None) | Err(_) => {
            // Expected behavior for invalid token / unreachable API
        }
        Ok(Some(_)) => {
            panic!("invalid token address must not return a token age");
        }
    }
}
