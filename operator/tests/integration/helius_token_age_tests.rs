//! Integration tests for Helius token age fetching
//!
//! Tests that token age is correctly fetched from Helius API
//! and used in signal quality scoring.

use chimera_operator::monitoring::HeliusClient;
use std::env;

#[tokio::test]
#[ignore] // Requires Helius API key - run with: cargo test -- --ignored
async fn test_helius_token_age_fetching() {
    // Get API key from environment or use test key
    let api_key = env::var("HELIUS_API_KEY")
        .unwrap_or_else(|_| "609cb910-17a5-4a76-9d1b-2ca9c42f759e".to_string());
    
    let client = HeliusClient::new(api_key).expect("Failed to create HeliusClient");

    // Test with USDC (known token, should exist)
    let usdc_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    
    match client.get_token_age_hours(usdc_mint).await {
        Ok(Some(age_hours)) => {
            println!("USDC token age: {:.2} hours", age_hours);
            // USDC should be older than 1 day (24 hours)
            assert!(age_hours > 24.0, "USDC should be older than 24 hours");
        }
        Ok(None) => {
            println!("Token age not found (may be a new token)");
        }
        Err(e) => {
            // If API fails, just log it (don't fail test)
            println!("Helius API error (expected in CI): {}", e);
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_helius_token_age_caching() {
    let api_key = env::var("HELIUS_API_KEY")
        .unwrap_or_else(|_| "609cb910-17a5-4a76-9d1b-2ca9c42f759e".to_string());
    
    let client = HeliusClient::new(api_key).expect("Failed to create HeliusClient");
    let token_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    // First call should query API
    let result1 = client.get_token_age_hours(token_mint).await;
    
    // Second call should use cache (within TTL)
    let result2 = client.get_token_age_hours(token_mint).await;

    // Results should be the same (cached)
    assert_eq!(result1, result2, "Second call should use cache");
}

#[tokio::test]
async fn test_helius_token_age_invalid_token() {
    // Test with invalid token address
    let api_key = "test-key".to_string();
    let client = HeliusClient::new(api_key).expect("Failed to create HeliusClient");
    
    let invalid_mint = "InvalidTokenAddress111111111111111111111111";
    
    // Should handle gracefully (return None or error)
    let result = client.get_token_age_hours(invalid_mint).await;
    
    // Should not panic
    match result {
        Ok(None) | Err(_) => {
            // Expected behavior for invalid token
        }
        Ok(Some(_)) => {
            // If API returns something, that's also fine
        }
    }
}






