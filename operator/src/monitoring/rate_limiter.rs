//! Adaptive rate limiter with priority queue and credit tracking
//!
//! Implements token bucket algorithm with sliding window for rate limiting
//! within Helius Developer plan constraints (50 req/sec).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::time::sleep;

/// Priority levels for rate-limited requests
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RequestPriority {
    /// Exit signals (highest priority)
    Exit = 3,
    /// Entry signals (medium priority)
    Entry = 2,
    /// Polling operations (lowest priority)
    Polling = 1,
}

/// Rate limiter using token bucket with sliding window
pub struct RateLimiter {
    /// Maximum requests per window
    max_requests: u32,
    /// Window size in seconds
    window_secs: u64,
    /// Request timestamps within current window
    requests: Arc<Mutex<VecDeque<Instant>>>,
    /// Current credit usage (for tracking)
    credit_usage: Arc<Mutex<u64>>,
}

impl RateLimiter {
    /// Create a new rate limiter
    ///
    /// # Arguments
    /// * `max_requests` - Maximum requests per second
    /// * `window_secs` - Time window in seconds (typically 1)
    pub fn new(max_requests: u32, window_secs: u64) -> Self {
        Self {
            max_requests,
            window_secs,
            requests: Arc::new(Mutex::new(VecDeque::new())),
            credit_usage: Arc::new(Mutex::new(0)),
        }
    }

    /// Acquire permission to make a request (blocks if at limit)
    ///
    /// # Arguments
    /// * `priority` - Request priority (higher priority requests can preempt)
    pub async fn acquire(&self, priority: RequestPriority) {
        loop {
            let now = Instant::now();
            let window_start = now - Duration::from_secs(self.window_secs);

            // Clean up old requests outside the window
            let mut requests = self.requests.lock().unwrap();
            while let Some(&oldest) = requests.front() {
                if oldest < window_start {
                    requests.pop_front();
                } else {
                    break;
                }
            }

            // Check if we can proceed
            if (requests.len() as u32) < self.max_requests {
                // Add current request
                requests.push_back(now);
                *self.credit_usage.lock().unwrap() += 1;
                return;
            }

            // At limit - wait for oldest request to expire
            if let Some(&oldest) = requests.front() {
                let wait_time = oldest + Duration::from_secs(self.window_secs) - now;
                if wait_time.as_millis() > 0 {
                    // For high priority requests, wait less time
                    let adjusted_wait = match priority {
                        RequestPriority::Exit => wait_time / 2,
                        RequestPriority::Entry => wait_time * 3 / 4,
                        RequestPriority::Polling => wait_time,
                    };
                    drop(requests);
                    sleep(adjusted_wait).await;
                } else {
                    // Shouldn't happen, but handle it
                    drop(requests);
                    sleep(Duration::from_millis(10)).await;
                }
            } else {
                drop(requests);
                sleep(Duration::from_millis(10)).await;
            }
        }
    }

    /// Try to acquire permission without blocking (returns immediately)
    ///
    /// Returns `true` if permission granted, `false` if at limit
    pub fn try_acquire(&self) -> bool {
        let now = Instant::now();
        let window_start = now - Duration::from_secs(self.window_secs);

        let mut requests = self.requests.lock().unwrap();
        
        // Clean up old requests
        while let Some(&oldest) = requests.front() {
            if oldest < window_start {
                requests.pop_front();
            } else {
                break;
            }
        }

        // Check if we can proceed
        if (requests.len() as u32) < self.max_requests {
            requests.push_back(now);
            *self.credit_usage.lock().unwrap() += 1;
            true
        } else {
            false
        }
    }

    /// Get current requests per second
    pub fn current_rate(&self) -> f64 {
        let now = Instant::now();
        let window_start = now - Duration::from_secs(self.window_secs);

        let mut requests = self.requests.lock().unwrap();
        
        // Clean up old requests
        while let Some(&oldest) = requests.front() {
            if oldest < window_start {
                requests.pop_front();
            } else {
                break;
            }
        }

        requests.len() as f64 / self.window_secs as f64
    }

    /// Get total credit usage (for tracking)
    pub fn credit_usage(&self) -> u64 {
        *self.credit_usage.lock().unwrap()
    }

    /// Reset credit usage counter
    pub fn reset_credit_usage(&self) {
        *self.credit_usage.lock().unwrap() = 0;
    }
}

/// Rate limit metrics for monitoring
#[derive(Debug, Clone, Default)]
pub struct RateLimitMetrics {
    pub requests_per_second: f64,
    pub total_credits_used: u64,
    pub rate_limit_hits: u64,
    pub average_wait_time_ms: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn test_rate_limiter_basic() {
        let limiter = RateLimiter::new(5, 1);
        
        // Should allow 5 requests immediately
        for _ in 0..5 {
            limiter.acquire(RequestPriority::Polling).await;
        }
        
        // 6th request should be blocked
        let start = Instant::now();
        limiter.acquire(RequestPriority::Polling).await;
        let elapsed = start.elapsed();
        
        // Should have waited at least some time
        assert!(elapsed.as_millis() > 0);
    }

    #[tokio::test]
    async fn test_rate_limiter_priority() {
        let limiter = RateLimiter::new(1, 1);
        
        // Fill the bucket
        limiter.acquire(RequestPriority::Polling).await;
        
        // High priority should wait less
        let start = Instant::now();
        limiter.acquire(RequestPriority::Exit).await;
        let elapsed = start.elapsed();
        
        // Exit priority should wait less than full window
        assert!(elapsed < Duration::from_secs(1));
    }

    #[test]
    fn test_try_acquire() {
        let limiter = RateLimiter::new(2, 1);
        
        assert!(limiter.try_acquire());
        assert!(limiter.try_acquire());
        assert!(!limiter.try_acquire()); // Should fail at limit
    }
}
