//! Jupiter Error Handling Unit Tests
//!
//! Unit tests for Jupiter error classification, retry logic, and
//! error handling utilities.

use chimera_operator::jupiter_error_handling::{
    calculate_retry_delay, JupiterError, JupiterErrorType, RetryConfig,
};
use std::time::Duration;

#[test]
fn test_jupiter_error_rate_limit_classification() {
    let error = JupiterError::from_http_error(429, "Too many requests".to_string());

    assert_eq!(error.error_type, JupiterErrorType::RateLimit);
    assert!(error.retryable, "Rate limit errors should be retryable");
    assert_eq!(
        error.status_code,
        Some(429),
        "Should preserve status code"
    );
    assert_eq!(
        error.retry_delay,
        Some(Duration::from_secs(5)),
        "Rate limit should have 5s retry delay"
    );
}

#[test]
fn test_jupiter_error_authentication_classification() {
    let error = JupiterError::from_http_error(401, "Unauthorized".to_string());

    assert_eq!(error.error_type, JupiterErrorType::Authentication);
    assert!(!error.retryable, "Auth errors should not be retryable");
    assert!(error.retry_delay.is_none(), "Auth errors should not have retry delay");
}

#[test]
fn test_jupiter_error_bad_request_classification() {
    let error = JupiterError::from_http_error(400, "Invalid parameters".to_string());

    assert_eq!(error.error_type, JupiterErrorType::BadRequest);
    assert!(!error.retryable, "Bad request errors should not be retryable");
}

#[test]
fn test_jupiter_error_server_error_classification() {
    let error = JupiterError::from_http_error(503, "Service unavailable".to_string());

    assert_eq!(error.error_type, JupiterErrorType::ServerError);
    assert!(error.retryable, "Server errors should be retryable");
    assert_eq!(
        error.retry_delay,
        Some(Duration::from_secs(2)),
        "Server errors should have 2s retry delay"
    );
}

#[test]
fn test_jupiter_error_timeout_classification() {
    let error = JupiterError::from_http_error(408, "Request timeout".to_string());

    assert_eq!(error.error_type, JupiterErrorType::Timeout);
    assert!(error.retryable, "Timeout errors should be retryable");
}

#[test]
fn test_jupiter_network_error_creation() {
    let error = JupiterError::network_error("Connection refused".to_string());

    assert_eq!(error.error_type, JupiterErrorType::NetworkError);
    assert!(error.retryable, "Network errors should be retryable");
    assert!(error.retry_delay.is_some(), "Network errors should have retry delay");
    assert!(error.status_code.is_none(), "Network errors have no status code");
}

#[test]
fn test_jupiter_timeout_error_creation() {
    let error = JupiterError::timeout_error("Request timed out".to_string());

    assert_eq!(error.error_type, JupiterErrorType::Timeout);
    assert!(error.retryable, "Timeout errors should be retryable");
}

#[test]
fn test_jupiter_parse_error_creation() {
    let error = JupiterError::parse_error("Invalid JSON response".to_string());

    assert_eq!(error.error_type, JupiterErrorType::ParseError);
    assert!(!error.retryable, "Parse errors should not be retryable");
    assert!(error.retry_delay.is_none(), "Parse errors should not have retry delay");
}

#[test]
fn test_jupiter_unknown_4xx_classification() {
    let error = JupiterError::from_http_error(418, "I'm a teapot".to_string());

    // Unknown 4xx client errors are permanent: retrying them with backoff
    // only masks the root cause. Only 5xx/network-level unknowns default to
    // retryable (see from_http_error).
    assert_eq!(error.error_type, JupiterErrorType::Unknown);
    assert!(!error.retryable, "Unknown 4xx errors must not be retryable");
    assert!(
        error.retry_delay.is_none(),
        "Unknown 4xx errors must not have a retry delay"
    );
}

#[test]
fn test_jupiter_unknown_5xx_classification() {
    let error = JupiterError::from_http_error(599, "Unusual server error".to_string());

    // Unknown 5xx errors remain retryable (transient server-side failures).
    assert_eq!(error.error_type, JupiterErrorType::Unknown);
    assert!(error.retryable, "Unknown 5xx errors should be retryable");
    assert!(
        error.retry_delay.is_some(),
        "Unknown 5xx errors should have a retry delay"
    );
}

#[test]
fn test_retry_delay_exponential_backoff() {
    let config = RetryConfig::default();

    let delay1 = calculate_retry_delay(1, &config);
    let delay2 = calculate_retry_delay(2, &config);
    let delay3 = calculate_retry_delay(3, &config);
    let delay4 = calculate_retry_delay(4, &config);

    // Verify exponential growth
    assert!(delay2 > delay1, "Delay 2 > Delay 1");
    assert!(delay3 > delay2, "Delay 3 > Delay 2");
    assert!(delay4 > delay3, "Delay 4 > Delay 3");

    // Bounds derived from the config (multiplier ± jitter envelope):
    // min ratio = mult * (1-j)/(1+j), max ratio = mult * (1+j)/(1-j).
    let j = config.jitter_factor;
    let min_ratio = config.backoff_multiplier * (1.0 - j) / (1.0 + j);
    let max_ratio = config.backoff_multiplier * (1.0 + j) / (1.0 - j);

    let ratio2_1 = delay2.as_millis() as f64 / delay1.as_millis() as f64;
    let ratio3_2 = delay3.as_millis() as f64 / delay2.as_millis() as f64;
    let ratio4_3 = delay4.as_millis() as f64 / delay3.as_millis() as f64;

    assert!(
        ratio2_1 >= min_ratio && ratio2_1 <= max_ratio,
        "Second retry ratio {} outside derived bounds [{}, {}]",
        ratio2_1,
        min_ratio,
        max_ratio
    );
    assert!(
        ratio3_2 >= min_ratio && ratio3_2 <= max_ratio,
        "Third retry ratio {} outside derived bounds [{}, {}]",
        ratio3_2,
        min_ratio,
        max_ratio
    );
    assert!(
        ratio4_3 >= min_ratio && ratio4_3 <= max_ratio,
        "Fourth retry ratio {} outside derived bounds [{}, {}]",
        ratio4_3,
        min_ratio,
        max_ratio
    );
}

#[test]
fn test_retry_delay_maximum_cap() {
    let config = RetryConfig {
        max_delay_ms: 200, // Cap at 200ms
        ..Default::default()
    };

    // Even at high attempt numbers, delay should be capped
    let delay_5 = calculate_retry_delay(5, &config);
    let delay_10 = calculate_retry_delay(10, &config);
    let delay_20 = calculate_retry_delay(20, &config);

    assert!(
        delay_5.as_millis() <= 220,
        "Delay 5 should be capped (allowing for jitter)"
    );
    assert!(
        delay_10.as_millis() <= 220,
        "Delay 10 should be capped (allowing for jitter)"
    );
    assert!(
        delay_20.as_millis() <= 220,
        "Delay 20 should be capped (allowing for jitter)"
    );
}

#[test]
fn test_retry_delay_jitter_variation() {
    let config = RetryConfig {
        jitter_factor: 0.2, // 20% jitter
        ..Default::default()
    };

    // Calculate multiple delays at the same attempt number
    let mut delays = Vec::new();
    for _ in 0..10 {
        let delay = calculate_retry_delay(2, &config);
        delays.push(delay);
    }

    // Verify there's variation due to jitter
    let min_delay = *delays.iter().min().unwrap();
    let max_delay = *delays.iter().max().unwrap();

    assert!(
        max_delay.as_millis() > min_delay.as_millis(),
        "Jitter should create variation in delays"
    );

    // Variation should be reasonable: max-min ≤ 2 * jitter_factor * base.
    // Normalize by the DETERMINISTIC theoretical base delay for attempt 2
    // (initial_delay * backoff^1, capped at max_delay), NOT by a random
    // sample — normalizing by a sample is flaky.
    let base_delay = (config.initial_delay_ms as f64 * config.backoff_multiplier.powi(1))
        .min(config.max_delay_ms as f64);
    let variation = (max_delay.as_millis() - min_delay.as_millis()) as f64 / base_delay;

    assert!(
        variation <= 2.0 * config.jitter_factor,
        "Jitter variation {} should be within 2*jitter_factor of the base delay",
        variation
    );
}

#[test]
fn test_retry_delay_no_jitter() {
    let config = RetryConfig {
        jitter_factor: 0.0, // No jitter
        ..Default::default()
    };

    let delay1 = calculate_retry_delay(1, &config);
    let delay2 = calculate_retry_delay(1, &config);

    // Without jitter, delays should be consistent
    assert_eq!(
        delay1.as_millis(),
        delay2.as_millis(),
        "Without jitter, delays should be identical"
    );
}

#[test]
fn test_retry_config_defaults() {
    let config = RetryConfig::default();

    assert_eq!(config.max_retries, 3, "Default max retries should be 3");
    assert_eq!(
        config.initial_delay_ms,
        100,
        "Default initial delay should be 100ms"
    );
    assert_eq!(
        config.max_delay_ms,
        10000,
        "Default max delay should be 10s"
    );
    assert_eq!(
        config.backoff_multiplier,
        2.0,
        "Default backoff multiplier should be 2.0"
    );
    assert_eq!(
        config.jitter_factor,
        0.1,
        "Default jitter factor should be 0.1"
    );
}

#[test]
fn test_jupiter_error_to_app_error_conversion() {
    let rate_limit_error = JupiterError::from_http_error(429, "Rate limit".to_string());
    let app_error = rate_limit_error.to_app_error();

    match app_error {
        chimera_operator::error::AppError::ServiceUnavailable(_) => {
            // Expected - rate limit should convert to service unavailable
        }
        other => {
            panic!("Rate limit error should convert to ServiceUnavailable, got: {:?}", other);
        }
    }

    let auth_error = JupiterError::from_http_error(401, "Unauthorized".to_string());
    let app_error = auth_error.to_app_error();

    match app_error {
        chimera_operator::error::AppError::Auth(_) => {
            // Expected - an upstream 401/403 is a runtime API-credential
            // problem, not a configuration problem.
        }
        other => {
            panic!("Auth error should convert to Auth error, got: {:?}", other);
        }
    }

    let bad_request_error = JupiterError::from_http_error(400, "Bad request".to_string());
    let app_error = bad_request_error.to_app_error();

    match app_error {
        chimera_operator::error::AppError::Validation(_) => {
            // Expected - bad request should convert to validation error
        }
        other => {
            panic!(
                "Bad request error should convert to Validation error, got: {:?}",
                other
            );
        }
    }

    let server_error = JupiterError::from_http_error(503, "Service unavailable".to_string());
    let app_error = server_error.to_app_error();

    match app_error {
        chimera_operator::error::AppError::Http(_) => {
            // Expected - server errors should convert to HTTP errors
        }
        other => {
            panic!("Server error should convert to Http error, got: {:?}", other);
        }
    }

    let parse_error = JupiterError::parse_error("Invalid JSON".to_string());
    let app_error = parse_error.to_app_error();

    match app_error {
        chimera_operator::error::AppError::Parse(_) => {
            // Expected - parse errors should convert to parse errors
        }
        other => {
            panic!("Parse error should convert to Parse error, got: {:?}", other);
        }
    }
}

#[test]
fn test_retry_delay_first_attempt() {
    let config = RetryConfig::default();
    let delay = calculate_retry_delay(1, &config);

    // Bounds derived from the config fields (initial ± jitter) instead of
    // hard-coded literals.
    let lo = (config.initial_delay_ms as f64 * (1.0 - config.jitter_factor)) as u128;
    let hi = (config.initial_delay_ms as f64 * (1.0 + config.jitter_factor)) as u128 + 1;
    assert!(
        delay.as_millis() >= lo && delay.as_millis() <= hi,
        "First retry should be within [{}, {}]ms (initial_delay ± jitter), got {:?}",
        lo,
        hi,
        delay
    );
}

#[test]
fn test_retry_delay_boundary_conditions() {
    let config = RetryConfig {
        max_retries: 3,
        initial_delay_ms: 50,
        max_delay_ms: 200,
        backoff_multiplier: 2.0,
        jitter_factor: 0.0, // No jitter for predictable testing
    };

    // Test exponential progression without jitter
    let delay1 = calculate_retry_delay(1, &config);
    let delay2 = calculate_retry_delay(2, &config);
    let delay3 = calculate_retry_delay(3, &config);
    let delay4 = calculate_retry_delay(4, &config);

    assert_eq!(delay1.as_millis(), 50, "First delay should be initial_delay_ms");
    assert_eq!(delay2.as_millis(), 100, "Second delay should be 2x initial");
    assert_eq!(delay3.as_millis(), 200, "Third delay should be 4x initial");
    assert_eq!(delay4.as_millis(), 200, "Fourth delay should be capped at max");

    // Verify cap is maintained for higher attempts
    let delay10 = calculate_retry_delay(10, &config);
    assert_eq!(delay10.as_millis(), 200, "High attempts should stay at max delay");
}