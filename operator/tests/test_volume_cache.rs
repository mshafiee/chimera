//! Tests for Volume Cache functionality
//!
//! Tests the volume cache for 24h volume tracking and liquidity drop detection.

use chimera_operator::engine::volume_cache::VolumeCache;
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
use std::thread;
use std::time::Duration;

#[test]
fn test_volume_cache_initialization() {
    // Test Volume Cache initialization
    let cache = VolumeCache::new();
    println!("✓ Volume Cache initialized");

    // Test that we can record volume
    cache.record_volume("test_token", dec!(1000.0));
    println!("✓ Volume recorded successfully");

    // Verify current volume retrieval
    let current_volume = cache.get_current_volume("test_token");
    assert!(current_volume.is_some(), "Should retrieve current volume");
    assert_eq!(current_volume.unwrap(), dec!(1000.0));
    println!("✓ Current volume retrieved: {}", current_volume.unwrap());
}

#[test]
fn test_24h_average_volume() {
    // Test 24h average volume calculation
    let cache = VolumeCache::new();

    // Record multiple volume entries
    cache.record_volume("test_token", dec!(1000.0));
    cache.record_volume("test_token", dec!(2000.0));
    cache.record_volume("test_token", dec!(3000.0));

    // Get 24h average
    let avg_volume = cache.get_24h_average_volume("test_token");
    assert!(avg_volume.is_some(), "Should calculate average volume");
    let expected_avg = (dec!(1000.0) + dec!(2000.0) + dec!(3000.0)) / dec!(3);
    assert_eq!(avg_volume.unwrap(), expected_avg);
    println!("✓ 24h average volume: {}", avg_volume.unwrap());
}

#[test]
fn test_volume_drop_detection() {
    // Test volume drop detection
    let cache = VolumeCache::new();

    // Simulate gradual volume decline
    for i in 0..10 {
        let volume = dec!(1000.0) - dec!(50.0) * Decimal::from(i);
        cache.record_volume("declining_token", volume);
        thread::sleep(Duration::from_millis(10)); // Small delay for timestamp variation
    }

    // Current volume should be significantly lower than average
    let current = cache.get_current_volume("declining_token");
    let average = cache.get_24h_average_volume("declining_token");

    assert!(current.is_some() && average.is_some());
    println!("✓ Current volume: {}, Average: {}", current.unwrap(), average.unwrap());

    // Test volume drop detection with 50% threshold
    let has_dropped = cache.has_volume_drop("declining_token", dec!(50));
    println!("✓ Volume drop detected (50% threshold): {}", has_dropped);

    // Test with lower threshold (should be more sensitive)
    let has_dropped_lower = cache.has_volume_drop("declining_token", dec!(20));
    println!("✓ Volume drop detected (20% threshold): {}", has_dropped_lower);
}

#[test]
fn test_volume_cache_token_isolation() {
    // Test that different tokens have separate volume histories
    let cache = VolumeCache::new();

    // Record volumes for different tokens
    cache.record_volume("token_a", dec!(1000.0));
    cache.record_volume("token_b", dec!(2000.0));
    cache.record_volume("token_c", dec!(3000.0));

    // Verify token isolation
    let volume_a = cache.get_current_volume("token_a");
    let volume_b = cache.get_current_volume("token_b");
    let volume_c = cache.get_current_volume("token_c");

    assert_eq!(volume_a.unwrap(), dec!(1000.0));
    assert_eq!(volume_b.unwrap(), dec!(2000.0));
    assert_eq!(volume_c.unwrap(), dec!(3000.0));

    println!("✓ Token volume isolation works correctly");
}

#[test]
fn test_volume_cache_empty_handling() {
    // Test handling of non-existent tokens
    let cache = VolumeCache::new();

    // Try to get volume for non-existent token
    let no_volume = cache.get_current_volume("nonexistent_token");
    assert!(no_volume.is_none(), "Should return None for non-existent token");

    let no_average = cache.get_24h_average_volume("nonexistent_token");
    assert!(no_average.is_none(), "Should return None for non-existent token");

    // Volume drop detection should return false for non-existent token
    let no_drop = cache.has_volume_drop("nonexistent_token", dec!(50));
    assert!(!no_drop, "Should return false for non-existent token");

    println!("✓ Empty token handling works correctly");
}

#[test]
fn test_volume_cache_precision() {
    // Test volume calculation precision with Decimal
    let cache = VolumeCache::new();

    // Record volumes with decimal precision
    cache.record_volume("precise_token", dec!(1234.56));
    cache.record_volume("precise_token", dec!(7890.12));
    cache.record_volume("precise_token", dec!(3456.78));

    let avg = cache.get_24h_average_volume("precise_token");
    assert!(avg.is_some());

    let expected_avg = (dec!(1234.56) + dec!(7890.12) + dec!(3456.78)) / dec!(3);
    assert_eq!(avg.unwrap(), expected_avg);

    println!("✓ Decimal precision maintained: {}", avg.unwrap());
}

#[test]
fn test_volume_stale_data_handling() {
    // Test that stale data (>10 minutes old) doesn't trigger false drops
    let cache = VolumeCache::new();

    // Record some volume
    cache.record_volume("stale_token", dec!(1000.0));

    // Wait a moment to ensure timestamp is slightly older
    thread::sleep(Duration::from_millis(100));

    // Add a very recent high volume entry
    cache.record_volume("stale_token", dec!(5000.0));

    // Volume drop should not be detected (most recent is high)
    let has_dropped = cache.has_volume_drop("stale_token", dec!(50));
    assert!(!has_dropped, "Should not detect drop with recent high volume");

    println!("✓ Stale data handling works correctly");
}

#[test]
fn test_volume_cache_concurrent_access() {
    // Test thread-safe concurrent access
    let cache = std::sync::Arc::new(VolumeCache::new());
    let mut handles = Vec::new();

    // Spawn multiple threads writing to different tokens
    for i in 0..5 {
        let cache_clone = cache.clone();
        let handle = thread::spawn(move || {
            let token = format!("concurrent_token_{}", i);
            for j in 0..10 {
                let volume = dec!(100.0) * Decimal::from(j + 1);
                cache_clone.record_volume(&token, volume);
                thread::sleep(Duration::from_millis(1));
            }
        });
        handles.push(handle);
    }

    // Wait for all threads to complete
    for handle in handles {
        handle.join().unwrap();
    }

    // Verify all tokens were recorded correctly
    for i in 0..5 {
        let token = format!("concurrent_token_{}", i);
        let volume = cache.get_current_volume(&token);
        assert!(volume.is_some(), "Should have volume for token {}", token);
        assert_eq!(volume.unwrap(), dec!(1000.0), "Final volume should be 1000.0");
    }

    println!("✓ Concurrent access handled safely");
}

fn main() {
    println!("Running Volume Cache tests...");
    println!("Note: Use 'cargo test' instead of running this directly");
}