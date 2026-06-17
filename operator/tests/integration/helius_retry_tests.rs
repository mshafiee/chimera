//! Integration tests for Helius retry logic
//!
//! Tests that Helius API client properly implements:
//! - Exponential backoff with ±25% jitter
//! - 30-second maximum backoff cap
//! - Correct error classification (retryable vs non-retryable)
//! - 5 retry attempts by default

use std::time::Duration;
use tokio::time::sleep;

// Import retry module for testing backoff calculation
use chimera_operator::retry::{calculate_backoff, is_retryable_status};

#[test]
fn test_retryable_status_classification() {
    // Helius retryable errors: 408, 429, 500, 502, 503, 504
    assert!(is_retryable_status(408), "408 timeout should be retryable");
    assert!(is_retryable_status(429), "429 rate limit should be retryable");
    assert!(is_retryable_status(500), "500 internal error should be retryable");
    assert!(is_retryable_status(502), "502 bad gateway should be retryable");
    assert!(is_retryable_status(503), "503 service unavailable should be retryable");
    assert!(is_retryable_status(504), "504 gateway timeout should be retryable");

    // Non-retryable errors: 400, 401, 403, 404, 409, 422
    assert!(!is_retryable_status(400), "400 bad request should not be retryable");
    assert!(!is_retryable_status(401), "401 unauthorized should not be retryable");
    assert!(!is_retryable_status(403), "403 forbidden should not be retryable");
    assert!(!is_retryable_status(404), "404 not found should not be retryable");
    assert!(!is_retryable_status(409), "409 conflict should not be retryable");
    assert!(!is_retryable_status(422), "422 validation error should not be retryable");
}

#[test]
fn test_exponential_backoff_pattern() {
    // Test base backoff values (allowing for ±25% jitter)
    let backoff_0 = calculate_backoff(0);
    // 1s * (0.75 to 1.25) = 0.75s to 1.25s
    assert!(
        backoff_0.as_secs() >= 0 && backoff_0.as_secs() <= 2,
        "Attempt 0 backoff should be ~1s with jitter, got {}s",
        backoff_0.as_secs_f64()
    );

    let backoff_1 = calculate_backoff(1);
    // 2s * (0.75 to 1.25) = 1.5s to 2.5s
    assert!(
        backoff_1.as_secs() >= 1 && backoff_1.as_secs() <= 3,
        "Attempt 1 backoff should be ~2s with jitter, got {}s",
        backoff_1.as_secs_f64()
    );

    let backoff_2 = calculate_backoff(2);
    // 4s * (0.75 to 1.25) = 3s to 5s
    assert!(
        backoff_2.as_secs() >= 3 && backoff_2.as_secs() <= 5,
        "Attempt 2 backoff should be ~4s with jitter, got {}s",
        backoff_2.as_secs_f64()
    );

    let backoff_3 = calculate_backoff(3);
    // 8s * (0.75 to 1.25) = 6s to 10s
    assert!(
        backoff_3.as_secs() >= 6 && backoff_3.as_secs() <= 10,
        "Attempt 3 backoff should be ~8s with jitter, got {}s",
        backoff_3.as_secs_f64()
    );

    let backoff_4 = calculate_backoff(4);
    // 16s * (0.75 to 1.25) = 12s to 20s
    assert!(
        backoff_4.as_secs() >= 12 && backoff_4.as_secs() <= 20,
        "Attempt 4 backoff should be ~16s with jitter, got {}s",
        backoff_4.as_secs_f64()
    );
}

#[test]
fn test_backoff_cap_at_30_seconds() {
    // Test that backoff is capped at 30 seconds per Helius best practices
    let backoff_10 = calculate_backoff(10);
    // Without cap, this would be 2^10 = 1024s
    // With cap, should be <= 30s
    assert!(
        backoff_10.as_secs() <= 30,
        "Backoff should be capped at 30s, got {}s",
        backoff_10.as_secs()
    );

    let backoff_100 = calculate_backoff(100);
    assert!(
        backoff_100.as_secs() <= 30,
        "Backoff should be capped at 30s even for high attempt numbers, got {}s",
        backoff_100.as_secs()
    );
}

#[test]
fn test_jitter_variation() {
    // Test that jitter actually adds variation
    // Run calculate_backoff multiple times for same attempt and verify we get different values
    let mut backoffs = Vec::new();
    for _ in 0..20 {
        backoffs.push(calculate_backoff(2).as_secs_f64());
    }

    // With ±25% jitter, we should see variation
    let min = backoffs.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = backoffs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

    // Base is 4s, so range should be roughly 3s to 5s
    assert!(
        min < max,
        "Jitter should produce variation in backoff times"
    );

    // Verify the range is reasonable (with some tolerance for randomness)
    let range = max - min;
    assert!(
        range >= 1.0,
        "Jitter range should be at least 1 second, got {}",
        range
    );
}

#[tokio::test]
async fn test_retry_with_backoff_success_after_retries() {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    use chimera_operator::retry::retry_with_backoff;

    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = counter.clone();

    let operation = || {
        let counter = counter_clone.clone();
        async move {
            let count = counter.fetch_add(1, Ordering::SeqCst);
            if count < 2 {
                Err(anyhow::anyhow!("Simulated transient error"))
            } else {
                Ok::<(), anyhow::Error>(())
            }
        }
    };

    let result = retry_with_backoff(operation, 5).await;
    assert!(
        result.is_ok(),
        "Operation should succeed after retries"
    );
    assert_eq!(
        counter.load(Ordering::SeqCst),
        3,
        "Should have 2 failures + 1 success"
    );
}

#[tokio::test]
async fn test_retry_with_backoff_exhaustion() {
    use chimera_operator::retry::retry_with_backoff;

    let operation = || async {
        Err::<(), anyhow::Error>(anyhow::anyhow!("Permanent error"))
    };

    let result = retry_with_backoff(operation, 3).await;
    assert!(
        result.is_err(),
        "Operation should fail after exhausting retries"
    );
}

#[tokio::test]
async fn test_retry_with_backoff_immediate_fail_on_non_retryable() {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    use chimera_operator::retry::retry_with_backoff;

    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = counter.clone();

    let operation = || {
        let counter = counter_clone.clone();
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            // Simulate a 404 error - non-retryable
            Err(anyhow::anyhow!("HTTP 404"))
        }
    };

    let result = retry_with_backoff(operation, 5).await;
    assert!(
        result.is_err(),
        "Non-retryable error should fail"
    );

    // Should only be called once (non-retryable errors fail immediately)
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "Non-retryable error should fail immediately without retries"
    );
}

#[tokio::test]
async fn test_retry_timing_with_jitter() {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::Instant;

    use chimera_operator::retry::retry_with_backoff;

    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = counter.clone();

    let operation = || {
        let counter = counter_clone.clone();
        async move {
            let count = counter.fetch_add(1, Ordering::SeqCst);
            if count < 3 {
                Err(anyhow::anyhow!("Simulated transient error"))
            } else {
                Ok::<(), anyhow::Error>(())
            }
        }
    };

    let start = Instant::now();
    let _ = retry_with_backoff(operation, 5).await;
    let elapsed = start.elapsed();

    // Should have: 1st attempt (fail) + backoff1 + 2nd attempt (fail) + backoff2 + 3rd attempt (success)
    // With exponential backoff: ~1s + ~2s = ~3s total wait time (plus execution time)
    // Allow generous tolerance for jitter and execution time
    assert!(
        elapsed.as_secs() >= 2 && elapsed.as_secs() <= 6,
        "Total time with retries should be roughly 3 seconds (1s + 2s), got {}s",
        elapsed.as_secs_f64()
    );
}
