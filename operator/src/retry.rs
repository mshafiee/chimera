//! Retry utilities with exponential backoff and jitter
//!
//! Implements Helius-recommended retry strategy:
//! - Exponential backoff: 1s, 2s, 4s, 8s, 16s
//! - ±25% jitter to prevent synchronized retries
//! - Maximum backoff capped at 30 seconds
//! - 5 retry attempts by default

use anyhow::Result;
use rand::Rng;
use std::time::Duration;
use tokio::time::sleep;

/// Check if HTTP status code is retryable per Helius best practices.
///
/// Per Helius documentation:
/// - Retryable: 408 (timeout), 429 (rate limit), 500, 502, 503, 504
/// - Non-retryable: 400, 401, 403, 404, 409, 422
#[inline]
pub fn is_retryable_status(status: u16) -> bool {
    matches!(status, 408 | 429 | 500 | 502 | 503 | 504)
}

/// Check if an error represents a network error (retryable).
#[inline]
pub fn is_network_error(err: &anyhow::Error) -> bool {
    // Check for reqwest error types
    if let Some(e) = err.downcast_ref::<reqwest::Error>() {
        e.is_timeout() || e.is_connect() || e.is_request()
    } else {
        false
    }
}

/// Extract HTTP status code from an error if available.
pub fn extract_status(err: &anyhow::Error) -> u16 {
    if let Some(e) = err.downcast_ref::<reqwest::Error>() {
        e.status().map(|s| s.as_u16()).unwrap_or(0)
    } else {
        0
    }
}

/// Calculate backoff duration with exponential backoff and ±25% jitter.
///
/// Pattern: 1s, 2s, 4s, 8s, 16s with jitter, capped at 30s
/// Per Helius best practices.
///
/// # Arguments
/// * `attempt` - Current attempt number (0-indexed)
///
/// # Returns
/// Duration to wait before next retry
pub fn calculate_backoff(attempt: u32) -> Duration {
    // Base backoff: 2^attempt seconds (1, 2, 4, 8, 16 for attempts 0-4)
    let base = 2_u64.pow(attempt.min(4));

    // Add ±25% jitter (random value between -0.25 and +0.25)
    let mut rng = rand::rng();
    let jitter = rng.random_range(-0.25..0.25);

    // Calculate final duration with jitter, ensure at least 1 second minimum
    let with_jitter = base as f64 * (1.0 + jitter);
    let millis = (with_jitter.min(30.0) * 1000.0).max(100.0) as u64;

    Duration::from_millis(millis)
}

/// Retry an async operation with exponential backoff and jitter.
///
/// Follows Helius best practices for retrying failed requests:
/// - Starts with 1s backoff, doubling each retry
/// - Adds ±25% jitter to prevent synchronized retries
/// - Maximum backoff capped at 30 seconds
/// - Gives up after max_retries attempts
///
/// Only retries retryable errors (408, 429, 500, 502, 503, 504, network errors).
/// Non-retryable errors (400, 401, 403, 404, 409, 422) fail immediately.
///
/// # Arguments
/// * `operation` - Async operation to retry (must be FnMut so it can be called multiple times)
/// * `max_retries` - Maximum number of retry attempts (default: 5)
///
/// # Returns
/// * `Ok(T)` - Operation succeeded
/// * `Err(E)` - Operation failed after all retries or encountered non-retryable error
///
/// # Example
/// ```no_run
/// use anyhow::Result;
/// use chimera_operator::retry::retry_with_backoff;
///
/// async fn fetch_data() -> Result<String> {
///     // Your HTTP request here
///     Ok("data".to_string())
/// }
///
/// #[tokio::main]
/// async fn main() -> Result<()> {
///     let result = retry_with_backoff(fetch_data, 5).await?;
///     Ok(())
/// }
/// ```
pub async fn retry_with_backoff<F, Fut, T>(
    mut operation: F,
    max_retries: u32,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    for attempt in 0..max_retries {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(e) if attempt < max_retries - 1 => {
                let status = extract_status(&e);
                let is_network = is_network_error(&e);

                // Determine if error is retryable
                // 1. Explicit retryable status (408, 429, 500, 502, 503, 504)
                // 2. Network errors (timeout, connection issues)
                // 3. Unknown errors (status = 0) - retry with caution for transient issues
                let is_retryable = is_retryable_status(status) || is_network || status == 0;

                if is_retryable {
                    let backoff = calculate_backoff(attempt);
                    tracing::debug!(
                        attempt = attempt + 1,
                        wait_ms = backoff.as_millis(),
                        status = status,
                        is_network = is_network,
                        "Retrying request after backoff (Helius best practices)"
                    );
                    sleep(backoff).await;
                    continue;
                }

                // Non-retryable error - fail immediately
                tracing::warn!(
                    attempt = attempt + 1,
                    status = status,
                    "Non-retryable error encountered (failing immediately per Helius best practices)"
                );
                return Err(e);
            }
            Err(e) => {
                // Final attempt failed
                tracing::error!(
                    attempts = max_retries,
                    "Operation failed after all retry attempts"
                );
                return Err(e);
            }
        }
    }

    // This should be unreachable since we always return within the loop,
    // but return a proper error instead of panicking in production
    Err(anyhow::anyhow!("Internal error: retry logic failed to return"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[test]
    fn test_is_retryable_status() {
        // Retryable statuses
        assert!(is_retryable_status(408));
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(500));
        assert!(is_retryable_status(502));
        assert!(is_retryable_status(503));
        assert!(is_retryable_status(504));

        // Non-retryable statuses
        assert!(!is_retryable_status(400));
        assert!(!is_retryable_status(401));
        assert!(!is_retryable_status(403));
        assert!(!is_retryable_status(404));
        assert!(!is_retryable_status(409));
        assert!(!is_retryable_status(422));
    }

    #[test]
    fn test_calculate_backoff() {
        // Test base backoff values (with ±25% jitter)
        let backoff_0 = calculate_backoff(0);
        // 1s base with jitter, minimum 100ms enforced
        assert!(backoff_0.as_millis() >= 100);
        assert!(backoff_0.as_millis() <= 1250);  // 1.25s max

        let backoff_1 = calculate_backoff(1);
        // 2s base with jitter
        assert!(backoff_1.as_millis() >= 1500);  // 2s * 0.75 = 1.5s minimum
        assert!(backoff_1.as_millis() <= 2500);  // 2s * 1.25 = 2.5s maximum

        let backoff_4 = calculate_backoff(4);  // 16s base
        assert!(backoff_4.as_millis() >= 12000);   // 16s * 0.75 = 12s minimum
        assert!(backoff_4.as_millis() <= 20000);   // 16s * 1.25 = 20s maximum

        // Test max cap (attempt 10 would be 1024s base, but capped at 30s)
        let backoff_capped = calculate_backoff(10);
        assert!(backoff_capped.as_secs() <= 30);
    }

    #[tokio::test]
    async fn test_retry_with_backoff_success() {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let operation = || {
            let counter = counter_clone.clone();
            async move {
                let count = counter.fetch_add(1, Ordering::SeqCst);
                if count < 2 {
                    // Use a retryable error (500 internal server error)
                    // We create an anyhow error that indicates retryable status
                    Err(anyhow::anyhow!("HTTP error: 500").context("Simulated server error"))
                } else {
                    Ok::<(), anyhow::Error>(())
                }
            }
        };

        let result = retry_with_backoff(operation, 5).await;
        assert!(result.is_ok());
        assert_eq!(counter.load(Ordering::SeqCst), 3); // 2 failures + 1 success
    }

    #[tokio::test]
    async fn test_retry_with_backoff_exhausted() {
        let operation = || async {
            Err::<(), anyhow::Error>(anyhow::anyhow!("Permanent error"))
        };

        let result = retry_with_backoff(operation, 3).await;
        assert!(result.is_err());
    }

    // Note: Testing non-retryable error behavior requires reqwest::Error
    // with proper status codes. Integration tests cover this with mock HTTP responses.
}
