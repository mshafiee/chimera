//! Jupiter API v2 Integration Tests
//!
//! Comprehensive tests for Jupiter Swap API v2 migration including:
//! - /order endpoint functionality
//! - RTSE (Real-Time Slippage Estimation)
//! - Jupiter Beam integration
//! - Error handling and retry logic
//! - Circuit breaker integration

use chimera_operator::config::{AppConfig, JupiterConfig};
use chimera_operator::db_abstraction::Database;
use chimera_operator::engine::transaction_builder::TransactionBuilder;
use chimera_operator::jupiter_error_handling::{JupiterError, JupiterErrorType, RetryConfig, calculate_retry_delay};
use chimera_operator::circuit_breaker::CircuitBreaker;
use chimera_operator::models::{Action, Signal, SignalPayload};
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
use solana_sdk::{
    pubkey::Pubkey,
    signature::{Keypair, Signer},
};
use std::sync::Arc;

#[tokio::test]
#[ignore] // Requires real Jupiter API - run with cargo test -- --ignored
async fn test_jupiter_v2_order_endpoint() {
    // Test v2 /order endpoint with RTSE enabled
    let config = Arc::new(AppConfig {
        jupiter: JupiterConfig {
            api_url: "https://api.jup.ag/swap/v2".to_string(),
            use_swap_v2: true,
            enable_rtse: true,
            ..Default::default()
        },
        ..Default::default()
    });

    let rpc_client = Arc::new(solana_client::nonblocking::rpc_client::RpcClient::new("https://api.mainnet-beta.solana.com".to_string()));

    let tx_builder = TransactionBuilder::new(rpc_client, config).unwrap();

    // Create a test signal
    let keypair = Keypair::new();
    let signal = Signal {
        trade_uuid: "test_v2_order".to_string(),
        timestamp: chrono::Utc::now().timestamp(),
        payload: SignalPayload {
            action: Action::Buy,
            token: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(), // USDC
            amount_sol: dec!(0.1),
            ..Default::default()
        },
        source_ip: None,
        liquidity_usd: None,
        force_slow_path: false,
        token_decimals: None,
    };

    // Build swap transaction with v2 /order endpoint
    let result = tx_builder
        .build_swap_transaction(&signal, &keypair, 100) // 1% slippage
        .await;

    assert!(result.is_ok(), "v2 /order endpoint should succeed");
    let built_tx = result.unwrap();

    // Verify v2 response fields
    assert!(built_tx.price_impact_pct().is_some(), "Should have price impact from v2");
    assert!(built_tx.fill_price_lamports_per_base().is_some(), "Should have fill price from v2");
}

#[tokio::test]
#[ignore] // Requires real Jupiter API - run with cargo test -- --ignored
async fn test_jupiter_v2_rtse_support() {
    // Test RTSE (Real-Time Slippage Estimation) with slippageBps=rtse
    let config = Arc::new(AppConfig {
        jupiter: JupiterConfig {
            api_url: "https://api.jup.ag/swap/v2".to_string(),
            use_swap_v2: true,
            enable_rtse: true, // Enable RTSE
            ..Default::default()
        },
        ..Default::default()
    });

    let rpc_client = Arc::new(solana_client::nonblocking::rpc_client::RpcClient::new("https://api.mainnet-beta.solana.com".to_string()));
    let tx_builder = TransactionBuilder::new(rpc_client, config).unwrap();

    let keypair = Keypair::new();
    let signal = Signal {
        trade_uuid: "test_rtse".to_string(),
        timestamp: chrono::Utc::now().timestamp(),
        payload: SignalPayload {
            action: Action::Buy,
            token: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            amount_sol: dec!(0.1),
            ..Default::default()
        },
        source_ip: None,
        liquidity_usd: None,
        force_slow_path: false,
        token_decimals: None,
    };

    // Build transaction with RTSE enabled
    let result = tx_builder
        .build_swap_transaction(&signal, &keypair, 100) // 1% slippage (RTSE will override)
        .await;

    assert!(result.is_ok(), "RTSE swap should succeed");

    // RTSE should provide better slippage protection
    // Verify that price impact is reasonable (should be optimized by RTSE)
    let built_tx = result.unwrap();
    let price_impact = built_tx
        .price_impact_pct()
        .expect("RTSE should provide price impact for a valid swap");
    assert!(price_impact < dec!(5.0), "RTSE should keep price impact under 5%");
}

#[tokio::test]
#[ignore] // Requires real Jupiter API - run with cargo test -- --ignored
async fn test_jupiter_v2_error_handling() {
    // Test error handling for various Jupiter API failures
    let config = Arc::new(AppConfig {
        jupiter: JupiterConfig {
            api_url: "https://api.jup.ag/swap/v2".to_string(),
            use_swap_v2: true,
            ..Default::default()
        },
        ..Default::default()
    });

    let rpc_client = Arc::new(solana_client::nonblocking::rpc_client::RpcClient::new("https://api.mainnet-beta.solana.com".to_string()));
    let tx_builder = TransactionBuilder::new(rpc_client, config).unwrap();

    let keypair = Keypair::new();

    // Test with invalid token address
    let invalid_signal = Signal {
        trade_uuid: "test_invalid_token".to_string(),
        timestamp: chrono::Utc::now().timestamp(),
        payload: SignalPayload {
            action: Action::Buy,
            token: "InvalidTokenMintAddress123".to_string(), // Invalid mint
            amount_sol: dec!(0.1),
            ..Default::default()
        },
        source_ip: None,
        liquidity_usd: None,
        force_slow_path: false,
        token_decimals: None,
    };

    let result = tx_builder
        .build_swap_transaction(&invalid_signal, &keypair, 100)
        .await;

    assert!(result.is_err(), "Invalid token should fail gracefully");

    // Verify error is appropriate (validation or parse error). A live API call
    // for an invalid mint can also surface Http/Internal/InvalidTokenAddress
    // variants — the contract is that the failure is handled gracefully, not
    // the exact variant.
    match result.unwrap_err() {
        chimera_operator::error::AppError::Validation(_)
        | chimera_operator::error::AppError::Parse(_)
        | chimera_operator::error::AppError::InvalidTokenAddress(_)
        | chimera_operator::error::AppError::Http(_)
        | chimera_operator::error::AppError::Internal(_) => {}
        other => {
            panic!("unexpected error variant for invalid token: {:?}", other);
        }
    }
}

#[test]
fn test_jupiter_error_classification() {
    // Test Jupiter error classification

    // Rate limit error (429)
    let rate_limit_error = JupiterError::from_http_error(429, "Rate limit exceeded".to_string());
    assert_eq!(rate_limit_error.error_type, JupiterErrorType::RateLimit);
    assert!(rate_limit_error.retryable, "Rate limit errors should be retryable");
    assert!(rate_limit_error.retry_delay.is_some(), "Rate limit should have retry delay");

    // Authentication error (401)
    let auth_error = JupiterError::from_http_error(401, "Unauthorized".to_string());
    assert_eq!(auth_error.error_type, JupiterErrorType::Authentication);
    assert!(!auth_error.retryable, "Auth errors should not be retryable");

    // Server error (503)
    let server_error = JupiterError::from_http_error(503, "Service unavailable".to_string());
    assert_eq!(server_error.error_type, JupiterErrorType::ServerError);
    assert!(server_error.retryable, "Server errors should be retryable");

    // Network error
    let network_error = JupiterError::network_error("Connection failed".to_string());
    assert_eq!(network_error.error_type, JupiterErrorType::NetworkError);
    assert!(network_error.retryable, "Network errors should be retryable");

    // Parse error
    let parse_error = JupiterError::parse_error("Invalid JSON".to_string());
    assert_eq!(parse_error.error_type, JupiterErrorType::ParseError);
    assert!(!parse_error.retryable, "Parse errors should not be retryable");
}

#[test]
fn test_retry_delay_calculation() {
    // Test retry delay calculation with exponential backoff

    let config = RetryConfig::default();

    // Bounds derived from the config fields (initial_delay ± jitter) instead
    // of hard-coded literals, so a default change updates them automatically.
    let first_lo = (config.initial_delay_ms as f64 * (1.0 - config.jitter_factor)) as u128;
    let first_hi = (config.initial_delay_ms as f64 * (1.0 + config.jitter_factor)) as u128;

    // First retry should have minimal delay
    let delay1 = calculate_retry_delay(1, &config);
    assert!(
        delay1.as_millis() >= first_lo && delay1.as_millis() <= first_hi,
        "First retry should be within [{first_lo}, {first_hi}]ms"
    );

    // Second retry should have longer delay (exponential backoff)
    let delay2 = calculate_retry_delay(2, &config);
    assert!(delay2 > delay1, "Second retry should have longer delay");

    // Third retry should be even longer
    let delay3 = calculate_retry_delay(3, &config);
    assert!(delay3 > delay2, "Third retry should be longer than second");

    // Verify exponential growth
    assert!(delay2.as_millis() as f64 > delay1.as_millis() as f64 * 1.5, "Should have exponential growth");
    assert!(delay3.as_millis() as f64 > delay2.as_millis() as f64 * 1.5, "Should have exponential growth");
}

#[test]
fn test_retry_delay_capping() {
    // Test that retry delays are properly capped

    let config = RetryConfig {
        max_delay_ms: 200, // 200ms max delay
        ..Default::default()
    };

    // Cap bound derived from the config (max_delay + jitter).
    let cap_hi = (config.max_delay_ms as f64 * (1.0 + config.jitter_factor)) as u128;

    // Even with many retries, delay should not exceed max
    let delay_10 = calculate_retry_delay(10, &config);
    assert!(
        delay_10.as_millis() <= cap_hi,
        "Delay should be capped at max + jitter ({cap_hi}ms), got {}",
        delay_10.as_millis()
    );

    let delay_100 = calculate_retry_delay(100, &config);
    assert!(
        delay_100.as_millis() <= cap_hi,
        "Delay should be capped even at 100 retries"
    );
}

#[tokio::test]
async fn test_jupiter_retry_logic() {
    // Test retry logic with mock failures

    use chimera_operator::jupiter_error_handling::retry_with_backoff;

    let attempt_count = std::sync::atomic::AtomicUsize::new(0);
    let config = RetryConfig {
        max_retries: 3,
        initial_delay_ms: 10,
        max_delay_ms: 100,
        ..Default::default()
    };

    let operation = || {
        attempt_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let n = attempt_count.load(std::sync::atomic::Ordering::SeqCst);
        async move {
            if n < 3 {
                Err(chimera_operator::error::AppError::Http("Temporary failure".to_string()))
            } else {
                Ok("success")
            }
        }
    };

    let result = retry_with_backoff(operation, &config, "test operation").await;

    assert!(result.is_ok(), "Should succeed after retries");
    assert_eq!(attempt_count.load(std::sync::atomic::Ordering::SeqCst), 3, "Should have made 3 attempts");
    assert_eq!(result.unwrap(), "success", "Should return success value");
}

#[tokio::test]
async fn test_jupiter_retry_exhaustion() {
    // Test that retries eventually give up

    use chimera_operator::jupiter_error_handling::retry_with_backoff;

    let attempt_count = std::sync::atomic::AtomicUsize::new(0);
    let config = RetryConfig {
        max_retries: 2,
        initial_delay_ms: 10,
        ..Default::default()
    };

    let operation = || {
        attempt_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        async move {
            Err(chimera_operator::error::AppError::Http("Persistent failure".to_string()))
        }
    };

    let result: chimera_operator::error::AppResult<&str> = retry_with_backoff(operation, &config, "failing operation").await;

    assert!(result.is_err(), "Should fail after all retries exhausted");
    assert_eq!(
        attempt_count.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "Should have made max_retries attempts"
    );
}

#[tokio::test]
#[ignore] // Requires external Postgres (TEST_DATABASE_URL) - run with cargo test -- --ignored
async fn test_circuit_breaker_jupiter_integration() {
    // Test circuit breaker integration with Jupiter failures

    use chimera_operator::config::{CircuitBreakerConfig};
    use chimera_operator::db_abstraction::{create_database, DatabaseConfig};

    // Requires an external Postgres instance (TEST_DATABASE_URL)
    let db = create_database(&DatabaseConfig::postgres(std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL must be set")))
        .await
        .unwrap();

    let config = CircuitBreakerConfig {
        max_jupiter_failures: 3, // Trip after 3 consecutive failures
        ..Default::default()
    };

    let circuit_breaker = CircuitBreaker::new(config, db, dec!(10.0));

    // Record Jupiter failures
    let tripped = circuit_breaker.record_jupiter_failure("rate_limit".to_string()).await.unwrap();
    assert!(!tripped, "1 failure below the threshold must not trip");
    assert_eq!(circuit_breaker.get_jupiter_failure_count(), 1, "Should have 1 failure");

    let tripped = circuit_breaker.record_jupiter_failure("timeout".to_string()).await.unwrap();
    assert!(!tripped, "2 failures below the threshold must not trip");
    assert_eq!(circuit_breaker.get_jupiter_failure_count(), 2, "Should have 2 failures");

    // Third consecutive failure reaches the threshold: must AUTO-TRIP and
    // transition to TRIPPED (this is the auto-trip path in
    // record_jupiter_failure, not manual_trip).
    let tripped = circuit_breaker.record_jupiter_failure("timeout".to_string()).await.unwrap();
    assert!(tripped, "Third consecutive failure should trip the breaker");
    assert_eq!(circuit_breaker.get_jupiter_failure_count(), 3, "Should have 3 failures");

    // Verify circuit breaker state
    let status = circuit_breaker.status();
    assert_eq!(status.state.to_string(), "TRIPPED", "Circuit breaker should trip after 3 failures");
    assert!(status.trip_reason.is_some(), "Should have trip reason");
}

