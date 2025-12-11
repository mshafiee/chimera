//! Integration tests for volatility calculation
//!
//! Tests that SOL price volatility is correctly calculated
//! and used in market condition filtering.

use chimera_operator::price_cache::{PriceCache, PriceSource};
use chrono::Utc;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

#[tokio::test]
async fn test_volatility_calculation() {
    let cache = Arc::new(PriceCache::new());
    let sol_mint = "So11111111111111111111111111111111111111112";

    // Add price history with some volatility
    let _base_price = 100.0;
    let prices = vec![
        100.0, 101.0, 99.0, 102.0, 98.0, 103.0, 97.0, 104.0, 96.0, 105.0,
        95.0, 104.0, 96.0, 103.0, 97.0, 102.0, 98.0, 101.0, 99.0, 100.0,
    ];

    for price in prices {
        cache.set_price(sol_mint, price, PriceSource::Jupiter);
        // Small delay to ensure different timestamps
        sleep(Duration::from_millis(10)).await;
    }

    // Calculate volatility
    let volatility = cache.calculate_volatility(sol_mint);
    
    assert!(volatility.is_some(), "Should calculate volatility with sufficient data");
    let vol = volatility.unwrap();
    assert!(vol > 0.0, "Volatility should be positive");
    println!("Calculated volatility: {:.2}%", vol);
}

#[tokio::test]
async fn test_volatility_insufficient_data() {
    let cache = Arc::new(PriceCache::new());
    let sol_mint = "So11111111111111111111111111111111111111112";

    // Add only one price point
    cache.set_price(sol_mint, 100.0, PriceSource::Jupiter);

    // Should return None (insufficient data)
    let volatility = cache.calculate_volatility(sol_mint);
    assert!(volatility.is_none(), "Should return None with insufficient data");
}

#[tokio::test]
async fn test_volatility_24h_window() {
    let cache = Arc::new(PriceCache::new());
    let sol_mint = "So11111111111111111111111111111111111111112";

    // Add prices within 24h window
    for i in 0..10 {
        let price = 100.0 + (i as f64 * 0.1);
        cache.set_price(sol_mint, price, PriceSource::Jupiter);
        sleep(Duration::from_millis(10)).await;
    }

    let volatility = cache.calculate_volatility(sol_mint);
    assert!(volatility.is_some(), "Should calculate volatility within 24h window");
}

#[tokio::test]
async fn test_get_sol_volatility() {
    let cache = Arc::new(PriceCache::new());
    let sol_mint = "So11111111111111111111111111111111111111112";

    // Add some price history
    let prices = vec![100.0, 105.0, 95.0, 110.0, 90.0];
    for price in prices {
        cache.set_price(sol_mint, price, PriceSource::Jupiter);
        sleep(Duration::from_millis(10)).await;
    }

    let sol_volatility = cache.get_sol_volatility();
    assert!(sol_volatility.is_some(), "Should return SOL volatility");
    let vol = sol_volatility.unwrap();
    assert!(vol > 0.0, "SOL volatility should be positive");
}

#[tokio::test]
async fn test_volatility_high_volatility_detection() {
    let cache = Arc::new(PriceCache::new());
    let sol_mint = "So11111111111111111111111111111111111111112";

    // Simulate high volatility (large price swings)
    let prices = vec![
        100.0, 130.0, 70.0, 120.0, 80.0, 140.0, 60.0, 150.0, 50.0, 160.0,
    ];

    for price in prices {
        cache.set_price(sol_mint, price, PriceSource::Jupiter);
        sleep(Duration::from_millis(10)).await;
    }

    let volatility = cache.calculate_volatility(sol_mint).unwrap();
    println!("High volatility scenario: {:.2}%", volatility);
    
    // Should detect high volatility (>30%)
    assert!(volatility > 30.0, "Should detect high volatility scenario");
}


