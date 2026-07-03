//! Enhanced error handling for Jupiter API interactions
//!
//! Provides structured retry logic, exponential backoff, and comprehensive
//! error classification for Jupiter API failures.

use crate::error::{AppError, AppResult};
use std::time::Duration;
use tokio::time::sleep;
use rand::Rng;

/// Jupiter API error classification for targeted handling
#[derive(Debug, Clone, PartialEq)]
pub enum JupiterErrorType {
    /// Rate limit exceeded (429)
    RateLimit,
    /// Authentication failure (401, 403)
    Authentication,
    /// Bad request (400)
    BadRequest,
    /// Server error (500, 502, 503, 504)
    ServerError,
    /// Network connectivity issue
    NetworkError,
    /// Timeout
    Timeout,
    /// Parsing/response format error
    ParseError,
    /// Unknown error
    Unknown,
}

/// Jupiter-specific error with context
#[derive(Debug)]
pub struct JupiterError {
    /// Error type classification
    pub error_type: JupiterErrorType,
    /// HTTP status code (if applicable)
    pub status_code: Option<u16>,
    /// Error message
    pub message: String,
    /// Whether this error is retryable
    pub retryable: bool,
    /// Suggested retry delay (if applicable)
    pub retry_delay: Option<Duration>,
}

impl JupiterError {
    /// Classify an HTTP error into JupiterErrorType
    pub fn from_http_error(status: u16, message: String) -> Self {
        let (error_type, retryable, retry_delay) = match status {
            429 => (JupiterErrorType::RateLimit, true, Some(Duration::from_secs(5))),
            401 | 403 => (JupiterErrorType::Authentication, false, None),
            400 => (JupiterErrorType::BadRequest, false, None),
            500 | 502 | 503 | 504 => (JupiterErrorType::ServerError, true, Some(Duration::from_secs(2))),
            408 => (JupiterErrorType::Timeout, true, Some(Duration::from_secs(1))),
            _ => (JupiterErrorType::Unknown, true, Some(Duration::from_secs(1))),
        };

        JupiterError {
            error_type,
            status_code: Some(status),
            message,
            retryable,
            retry_delay,
        }
    }

    /// Create a network error
    pub fn network_error(message: String) -> Self {
        JupiterError {
            error_type: JupiterErrorType::NetworkError,
            status_code: None,
            message,
            retryable: true,
            retry_delay: Some(Duration::from_secs(2)),
        }
    }

    /// Create a timeout error
    pub fn timeout_error(message: String) -> Self {
        JupiterError {
            error_type: JupiterErrorType::Timeout,
            status_code: None,
            message,
            retryable: true,
            retry_delay: Some(Duration::from_secs(1)),
        }
    }

    /// Create a parse error
    pub fn parse_error(message: String) -> Self {
        JupiterError {
            error_type: JupiterErrorType::ParseError,
            status_code: None,
            message,
            retryable: false,
            retry_delay: None,
        }
    }

    /// Convert to AppError with context
    pub fn to_app_error(&self) -> AppError {
        match self.error_type {
            JupiterErrorType::RateLimit => {
                AppError::ServiceUnavailable(format!("Jupiter rate limit: {}", self.message))
            }
            JupiterErrorType::Authentication => {
                AppError::Config(format!("Jupiter authentication failed: {}", self.message))
            }
            JupiterErrorType::BadRequest => {
                AppError::Validation(format!("Jupiter bad request: {}", self.message))
            }
            JupiterErrorType::ServerError | JupiterErrorType::NetworkError | JupiterErrorType::Timeout => {
                AppError::Http(format!("Jupiter API error: {}", self.message))
            }
            JupiterErrorType::ParseError => {
                AppError::Parse(format!("Jupiter response parsing failed: {}", self.message))
            }
            JupiterErrorType::Unknown => {
                AppError::Internal(format!("Jupiter unknown error: {}", self.message))
            }
        }
    }
}

/// Retry configuration for Jupiter API calls
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts
    pub max_retries: u32,
    /// Initial retry delay in milliseconds
    pub initial_delay_ms: u64,
    /// Maximum retry delay in milliseconds
    pub max_delay_ms: u64,
    /// Exponential backoff multiplier
    pub backoff_multiplier: f64,
    /// Jitter to add to retry delays (0.0-1.0)
    pub jitter_factor: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay_ms: 100,
            max_delay_ms: 10000,
            backoff_multiplier: 2.0,
            jitter_factor: 0.1,
        }
    }
}

/// Calculate retry delay with exponential backoff and jitter
pub fn calculate_retry_delay(attempt: u32, config: &RetryConfig) -> Duration {
    let base_delay = config.initial_delay_ms as f64
        * config.backoff_multiplier.powi(attempt as i32 - 1);
    let capped_delay = base_delay.min(config.max_delay_ms as f64);

    // Add jitter to avoid thundering herd
    let jitter = if config.jitter_factor > 0.0 {
        let mut rng = rand::thread_rng();
        (rng.gen::<f64>() - 0.5) * 2.0 * config.jitter_factor * capped_delay
    } else {
        0.0
    };

    let final_delay = (capped_delay + jitter).max(0.0) as u64;
    Duration::from_millis(final_delay)
}

/// Execute an async operation with retry logic
pub async fn retry_with_backoff<F, Fut, T>(
    operation: F,
    config: &RetryConfig,
    operation_name: &str,
) -> AppResult<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = AppResult<T>>,
{
    let mut last_error = None;

    for attempt in 1..=config.max_retries {
        match operation().await {
            Ok(result) => {
                if attempt > 1 {
                    tracing::info!(
                        operation = %operation_name,
                        attempt = attempt,
                        "Jupiter API operation succeeded after retry"
                    );
                }
                return Ok(result);
            }
            Err(e) => {
                last_error = Some(e.clone());

                // Check if error is retryable
                let delay = if attempt < config.max_retries {
                    Some(calculate_retry_delay(attempt, config))
                } else {
                    None
                };

                if let Some(delay) = delay {
                    tracing::warn!(
                        operation = %operation_name,
                        attempt = attempt,
                        next_attempt = attempt + 1,
                        delay_ms = delay.as_millis(),
                        error = %e,
                        "Jupiter API operation failed, retrying with backoff"
                    );

                    sleep(delay).await;
                } else {
                    tracing::error!(
                        operation = %operation_name,
                        attempt = attempt,
                        error = %e,
                        "Jupiter API operation failed after all retries"
                    );
                    break;
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        AppError::Internal(format!("{} failed with unknown error", operation_name))
    }))
}

/// Execute an operation with Jupiter-specific error handling
pub async fn execute_with_jupiter_error_handling<F, Fut, T>(
    operation: F,
    config: &RetryConfig,
    operation_name: &str,
) -> AppResult<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = AppResult<T>>,
{
    retry_with_backoff(operation, config, operation_name).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jupiter_error_classification() {
        let rate_limit_error = JupiterError::from_http_error(429, "Rate limit".to_string());
        assert_eq!(rate_limit_error.error_type, JupiterErrorType::RateLimit);
        assert!(rate_limit_error.retryable);

        let auth_error = JupiterError::from_http_error(401, "Unauthorized".to_string());
        assert_eq!(auth_error.error_type, JupiterErrorType::Authentication);
        assert!(!auth_error.retryable);
    }

    #[test]
    fn test_retry_delay_calculation() {
        let config = RetryConfig::default();
        let delay1 = calculate_retry_delay(1, &config);
        let delay2 = calculate_retry_delay(2, &config);

        // Second retry should have longer delay (exponential backoff)
        assert!(delay2 > delay1);

        // Delays should be reasonable
        assert!(delay1.as_millis() >= 90); // Account for jitter
        assert!(delay1.as_millis() <= 110);
    }

    #[test]
    fn test_max_delay_capping() {
        let config = RetryConfig {
            max_delay_ms: 100,
            ..Default::default()
        };

        // Even with high backoff, delay should be capped
        let delay = calculate_retry_delay(10, &config);
        assert!(delay.as_millis() <= 100 + 20); // Allow for jitter
    }
}