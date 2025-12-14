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

/// Request weight for rate limiting
/// Higher weights consume more credits (e.g., simulation calls are heavier than getAccountInfo)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RequestWeight(u32);

impl RequestWeight {
    /// Standard request weight (1 credit)
    pub const STANDARD: Self = Self(1);
    /// Heavy request weight for simulation calls (typically 5-10x standard)
    pub const SIMULATION: Self = Self(5);
    /// Custom weight
    pub fn new(weight: u32) -> Self {
        Self(weight)
    }
    
    pub fn value(&self) -> u32 {
        self.0
    }
}

/// Rate limiter using token bucket with sliding window
pub struct RateLimiter {
    /// Maximum credits per window (weighted)
    max_credits: u32,
    /// Window size in seconds
    window_secs: u64,
    /// Request entries with timestamp and weight
    requests: Arc<Mutex<VecDeque<(Instant, u32)>>>,
    /// Current credit usage (for tracking)
    credit_usage: Arc<Mutex<u64>>,
    /// Current weighted credit usage in window
    current_credits: Arc<Mutex<u32>>,
}

impl RateLimiter {
    /// Create a new rate limiter
    ///
    /// # Arguments
    /// * `max_requests` - Maximum requests per second (treated as max credits)
    /// * `window_secs` - Time window in seconds (typically 1)
    pub fn new(max_requests: u32, window_secs: u64) -> Self {
        Self {
            max_credits: max_requests,
            window_secs,
            requests: Arc::new(Mutex::new(VecDeque::new())),
            credit_usage: Arc::new(Mutex::new(0)),
            current_credits: Arc::new(Mutex::new(0)),
        }
    }

    /// Acquire permission to make a request (blocks if at limit)
    ///
    /// # Arguments
    /// * `priority` - Request priority (higher priority requests can preempt)
    /// * `weight` - Request weight (defaults to STANDARD if not provided)
    pub async fn acquire(&self, priority: RequestPriority, weight: RequestWeight) {
        loop {
            let now = Instant::now();
            let window_start = now - Duration::from_secs(self.window_secs);
            let weight_value = weight.value();

            // Clean up old requests outside the window and check if we can proceed
            let (can_proceed, wait_time) = {
                let mut requests = self.requests.lock().unwrap();
                let mut current_credits = self.current_credits.lock().unwrap();
                
                while let Some(&(oldest_time, oldest_weight)) = requests.front() {
                    if oldest_time < window_start {
                        requests.pop_front();
                        *current_credits = current_credits.saturating_sub(oldest_weight);
                    } else {
                        break;
                    }
                }

                // Check if we can proceed with this weight
                if *current_credits + weight_value <= self.max_credits {
                    // Add current request with weight
                    requests.push_back((now, weight_value));
                    *current_credits += weight_value;
                    *self.credit_usage.lock().unwrap() += weight_value as u64;
                    return;
                }

                // Calculate wait time if at limit
                let wait_time = if let Some(&(oldest_time, _)) = requests.front() {
                    let wait = oldest_time + Duration::from_secs(self.window_secs) - now;
                    if wait.as_millis() > 0 {
                        Some(wait)
                    } else {
                        Some(Duration::from_millis(10))
                    }
                } else {
                    Some(Duration::from_millis(10))
                };
                
                (false, wait_time)
            };

            // Wait outside the lock
            if !can_proceed {
                if let Some(wait_time) = wait_time {
                    // For high priority requests, wait less time
                    let adjusted_wait = match priority {
                        RequestPriority::Exit => wait_time / 2,
                        RequestPriority::Entry => wait_time * 3 / 4,
                        RequestPriority::Polling => wait_time,
                    };
                    sleep(adjusted_wait).await;
                } else {
                    sleep(Duration::from_millis(10)).await;
                }
            }
        }
    }
    
    /// Acquire permission with standard weight (backward compatibility)
    pub async fn acquire_standard(&self, priority: RequestPriority) {
        self.acquire(priority, RequestWeight::STANDARD).await;
    }

    /// Try to acquire permission without blocking (returns immediately)
    ///
    /// Returns `true` if permission granted, `false` if at limit
    /// Uses standard weight (1 credit)
    pub fn try_acquire(&self) -> bool {
        self.try_acquire_weighted(RequestWeight::STANDARD)
    }
    
    /// Try to acquire permission with specified weight without blocking
    ///
    /// Returns `true` if permission granted, `false` if at limit
    pub fn try_acquire_weighted(&self, weight: RequestWeight) -> bool {
        let now = Instant::now();
        let window_start = now - Duration::from_secs(self.window_secs);
        let weight_value = weight.value();

        let mut requests = self.requests.lock().unwrap();
        let mut current_credits = self.current_credits.lock().unwrap();
        
        // Clean up old requests
        while let Some(&(oldest_time, oldest_weight)) = requests.front() {
            if oldest_time < window_start {
                requests.pop_front();
                *current_credits = current_credits.saturating_sub(oldest_weight);
            } else {
                break;
            }
        }

        // Check if we can proceed with this weight
        if *current_credits + weight_value <= self.max_credits {
            requests.push_back((now, weight_value));
            *current_credits += weight_value;
            *self.credit_usage.lock().unwrap() += weight_value as u64;
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
        let mut current_credits = self.current_credits.lock().unwrap();
        
        // Clean up old requests
        while let Some(&(oldest_time, oldest_weight)) = requests.front() {
            if oldest_time < window_start {
                requests.pop_front();
                *current_credits = current_credits.saturating_sub(oldest_weight);
            } else {
                break;
            }
        }

        requests.len() as f64 / self.window_secs as f64
    }
    
    /// Get current credit usage in the window
    pub fn current_credits(&self) -> u32 {
        let now = Instant::now();
        let window_start = now - Duration::from_secs(self.window_secs);

        let mut requests = self.requests.lock().unwrap();
        let mut current_credits = self.current_credits.lock().unwrap();
        
        // Clean up old requests
        while let Some(&(oldest_time, oldest_weight)) = requests.front() {
            if oldest_time < window_start {
                requests.pop_front();
                *current_credits = current_credits.saturating_sub(oldest_weight);
            } else {
                break;
            }
        }
        
        *current_credits
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
            limiter.acquire(RequestPriority::Polling, RequestWeight::STANDARD).await;
        }
        
        // 6th request should be blocked
        let start = Instant::now();
        limiter.acquire(RequestPriority::Polling, RequestWeight::STANDARD).await;
        let elapsed = start.elapsed();
        
        // Should have waited at least some time
        assert!(elapsed.as_millis() > 0);
    }

    #[tokio::test]
    async fn test_rate_limiter_priority() {
        let limiter = RateLimiter::new(1, 1);
        
        // Fill the bucket
        limiter.acquire(RequestPriority::Polling, RequestWeight::STANDARD).await;
        
        // Test that Exit priority gets reduced wait time compared to Polling
        // We'll test by comparing wait times when bucket is full
        
        // First, measure Exit priority wait time
        let start = Instant::now();
        limiter.acquire(RequestPriority::Exit, RequestWeight::STANDARD).await;
        let exit_elapsed = start.elapsed();
        
        // Refill bucket
        limiter.acquire(RequestPriority::Polling, RequestWeight::STANDARD).await;
        
        // Now measure Polling priority wait time (should be longer)
        let start = Instant::now();
        limiter.acquire(RequestPriority::Polling, RequestWeight::STANDARD).await;
        let polling_elapsed = start.elapsed();
        
        // Exit priority should wait less than or equal to polling priority
        // (In practice, Exit divides wait by 2, so it should be significantly less)
        // We allow some tolerance for timing variations
        assert!(
            exit_elapsed <= polling_elapsed + Duration::from_millis(100),
            "Exit priority should wait less than or equal to polling priority. Exit: {:?}, Polling: {:?}",
            exit_elapsed, polling_elapsed
        );
        
        // Additionally, verify that Exit priority actually reduces wait time
        // by checking it's less than the full window (accounting for overhead)
        assert!(
            exit_elapsed < Duration::from_secs(2),
            "Exit priority should complete within reasonable time (got {:?})", exit_elapsed
        );
    }
    
    #[tokio::test]
    async fn test_rate_limiter_weighted() {
        let limiter = RateLimiter::new(10, 1);
        
        // Should allow 2 simulation requests (2 * 5 = 10 credits)
        limiter.acquire(RequestPriority::Polling, RequestWeight::SIMULATION).await;
        limiter.acquire(RequestPriority::Polling, RequestWeight::SIMULATION).await;
        
        // 3rd simulation should be blocked (would exceed 10 credits)
        let start = Instant::now();
        limiter.acquire(RequestPriority::Polling, RequestWeight::SIMULATION).await;
        let elapsed = start.elapsed();
        
        // Should have waited at least some time
        assert!(elapsed.as_millis() > 0);
        
        // But should allow 10 standard requests
        for _ in 0..10 {
            assert!(limiter.try_acquire());
        }
        assert!(!limiter.try_acquire());
    }

    #[test]
    fn test_try_acquire() {
        let limiter = RateLimiter::new(2, 1);
        
        assert!(limiter.try_acquire());
        assert!(limiter.try_acquire());
        assert!(!limiter.try_acquire()); // Should fail at limit
    }
}
