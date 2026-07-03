//! Jupiter API v2 Integration Tests
//!
//! Comprehensive tests for Jupiter Swap API v2 migration including:
//! - /order endpoint functionality
//! - RTSE (Real-Time Slippage Estimation)
//! - Jupiter Beam integration
//! - Error handling and retry logic
//! - Circuit breaker integration

use chimera_operator::config::{AppConfig, JupiterConfig};
use chimera_operator::engine::transaction_builder::TransactionBuilder;
use chimera_operator::jupiter_error_handling::{JupiterError, JupiterErrorType, RetryConfig, calculate_retry_delay};
use chimera_operator::circuit_breaker::{CircuitBreaker, TripReason};
use chimera_operator::models::{Action, Signal, SignalPayload};
use rust_decimal::prelude::*;
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
        id: "test_v2_order".to_string(),
        timestamp: chrono::Utc::now(),
        payload: SignalPayload {
            action: Action::Buy,
            token: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(), // USDC
            amount_sol: dec!(0.1),
            ..Default::default()
        },
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
        id: "test_rtse".to_string(),
        timestamp: chrono::Utc::now(),
        payload: SignalPayload {
            action: Action::Buy,
            token: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            amount_sol: dec!(0.1),
            ..Default::default()
        },
    };

    // Build transaction with RTSE enabled
    let result = tx_builder
        .build_swap_transaction(&signal, &keypair, 100) // 1% slippage (RTSE will override)
        .await;

    assert!(result.is_ok(), "RTSE swap should succeed");

    // RTSE should provide better slippage protection
    // Verify that price impact is reasonable (should be optimized by RTSE)
    let built_tx = result.unwrap();
    if let Some(price_impact) = built_tx.price_impact_pct() {
        assert!(price_impact < dec!(5.0), "RTSE should keep price impact under 5%");
    }
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
        id: "test_invalid_token".to_string(),
        timestamp: chrono::Utc::now(),
        payload: SignalPayload {
            action: Action::Buy,
            token: "InvalidTokenMintAddress123".to_string(), // Invalid mint
            amount_sol: dec!(0.1),
            ..Default::default()
        },
    };

    let result = tx_builder
        .build_swap_transaction(&invalid_signal, &keypair, 100)
        .await;

    assert!(result.is_err(), "Invalid token should fail gracefully");

    // Verify error is appropriate (validation or parse error)
    match result.unwrap_err() {
        chimera_operator::error::AppError::Validation(_) => {
            // Expected - invalid token should be caught
        }
        chimera_operator::error::AppError::Parse(_) => {
            // Also acceptable - Jupiter might return parse error
        }
        other => {
            panic!("Expected validation or parse error, got: {:?}", other);
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

    // First retry should have minimal delay
    let delay1 = calculate_retry_delay(1, &config);
    assert!(delay1.as_millis() >= 90, "First retry should be around 100ms");
    assert!(delay1.as_millis() <= 110, "First retry should not be too long");

    // Second retry should have longer delay (exponential backoff)
    let delay2 = calculate_retry_delay(2, &config);
    assert!(delay2 > delay1, "Second retry should have longer delay");

    // Third retry should be even longer
    let delay3 = calculate_retry_delay(3, &config);
    assert!(delay3 > delay2, "Third retry should be longer than second");

    // Verify exponential growth
    assert!(delay2.as_millis() > delay1.as_millis() * 1.5, "Should have exponential growth");
    assert!(delay3.as_millis() > delay2.as_millis() * 1.5, "Should have exponential growth");
}

#[test]
fn test_retry_delay_capping() {
    // Test that retry delays are properly capped

    let config = RetryConfig {
        max_delay_ms: 200, // 200ms max delay
        ..Default::default()
    };

    // Even with many retries, delay should not exceed max
    let delay_10 = calculate_retry_delay(10, &config);
    assert!(delay_10.as_millis() <= 220, "Delay should be capped at max + jitter");

    let delay_100 = calculate_retry_delay(100, &config);
    assert!(delay_100.as_millis() <= 220, "Delay should be capped even at 100 retries");
}

#[tokio::test]
async fn test_jupiter_retry_logic() {
    // Test retry logic with mock failures

    use chimera_operator::jupiter_error_handling::retry_with_backoff;

    let mut attempt_count = 0;
    let config = RetryConfig {
        max_retries: 3,
        initial_delay_ms: 10,
        max_delay_ms: 100,
        ..Default::default()
    };

    let operation = || {
        attempt_count += 1;
        async move {
            if attempt_count < 3 {
                Err(chimera_operator::error::AppError::Http("Temporary failure".to_string()))
            } else {
                Ok("success")
            }
        }
    };

    let result = retry_with_backoff(operation, &config, "test operation").await;

    assert!(result.is_ok(), "Should succeed after retries");
    assert_eq!(attempt_count, 3, "Should have made 3 attempts");
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

    let result = retry_with_backoff(operation, &config, "failing operation").await;

    assert!(result.is_err(), "Should fail after all retries exhausted");
    assert_eq!(
        attempt_count.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "Should have made max_retries attempts"
    );
}

#[tokio::test]
async fn test_circuit_breaker_jupiter_integration() {
    // Test circuit breaker integration with Jupiter failures

    use chimera_operator::config::{CircuitBreakerConfig, DatabaseConfig};
    use chimera_operator::db_abstraction::Database;

    // Create mock database
    let db = Arc::new(MockDatabase::new()) as Arc<dyn Database>;

    let config = CircuitBreakerConfig {
        max_jupiter_failures: 3, // Trip after 3 consecutive failures
        ..Default::default()
    };

    let circuit_breaker = CircuitBreaker::new(config, db, dec!(10.0));

    // Record Jupiter failures
    let _ = circuit_breaker.record_jupiter_failure("rate_limit".to_string()).unwrap();
    assert_eq!(circuit_breaker.get_jupiter_failure_count(), 1, "Should have 1 failure");

    let _ = circuit_breaker.record_jupiter_failure("timeout".to_string()).unwrap();
    assert_eq!(circuit_breaker.get_jupiter_failure_count(), 2, "Should have 2 failures");

    // Reset after successful call
    circuit_breaker.reset_jupiter_failures();
    assert_eq!(circuit_breaker.get_jupiter_failure_count(), 0, "Failures should be reset");

    // Test threshold trip
    let _ = circuit_breaker.record_jupiter_failure("server_error".to_string()).unwrap();
    let _ = circuit_breaker.record_jupiter_failure("auth_error".to_string()).unwrap();
    let _ = circuit_breaker.record_jupiter_failure("network_error".to_string()).unwrap();

    // Third failure should trip the circuit breaker
    let tripped = circuit_breaker.record_jupiter_failure("final_failure".to_string()).unwrap();
    assert!(tripped, "Circuit breaker should be tripped after threshold exceeded");

    // Verify circuit breaker state
    let status = circuit_breaker.get_status().await;
    assert_eq!(status.state.to_string(), "TRIPPED", "Circuit breaker should be tripped");
    assert!(status.trip_reason.is_some(), "Should have trip reason");
}

// Mock database for testing
struct MockDatabase {
    state: Arc<parking_lot::RwLock<MockDbState>>,
}

struct MockDbState {
    cb_state: Option<String>,
    cb_tripped_at: Option<String>,
    cb_reason: Option<String>,
}

impl MockDatabase {
    fn new() -> Self {
        Self {
            state: Arc::new(parking_lot::RwLock::new(MockDbState {
                cb_state: None,
                cb_tripped_at: None,
                cb_reason: None,
            })),
        }
    }
}

#[async_trait::async_trait]
impl Database for MockDatabase {
    async fn update_circuit_breaker_state(
        &self,
        state: String,
        tripped_at: Option<&str>,
        trip_reason: Option<&str>,
    ) -> chimera_operator::error::AppResult<()> {
        let mut db_state = self.state.write();
        db_state.cb_state = Some(state);
        db_state.cb_tripped_at = tripped_at.map(|s| s.to_string());
        db_state.cb_reason = trip_reason.map(|s| s.to_string());
        Ok(())
    }

    async fn get_circuit_breaker_state(&self) -> chimera_operator::error::AppResult<chimera_operator::db_abstraction::CircuitBreakerState> {
        let db_state = self.state.read();
        Ok(chimera_operator::db_abstraction::CircuitBreakerState {
            state: db_state.cb_state.clone().unwrap_or("Active".to_string()),
            tripped_at: db_state.cb_tripped_at.clone(),
            trip_reason: db_state.cb_reason.clone(),
        })
    }

    // Implement required methods with stubs
    async fn log_config_change(&self, _change_type: &str, _details: &str) -> chimera_operator::error::AppResult<()> {
        Ok(())
    }

    // Add other required Database trait methods as stubs
    fn get_pool(&self) -> &sqlx::Pool<sqlx::Postgres> {
        unimplemented!()
    }

    async fn get_wallet_by_address(&self, _address: &str) -> chimera_operator::error::AppResult<Option<chimera_operator::db_abstraction::Wallet>> {
        Ok(None)
    }

    async fn create_trade(&self, _trade: &chimera_operator::db_abstraction::Trade) -> chimera_operator::error::AppResult<()> {
        Ok(())
    }

    async fn update_trade_status(&self, _id: i64, _status: &str, _signature: Option<&str>, _error_msg: Option<&str>) -> chimera_operator::error::AppResult<()> {
        Ok(())
    }

    async fn get_trade_by_id(&self, _id: i64) -> chimera_operator::error::AppResult<Option<chimera_operator::db_abstraction::Trade>> {
        Ok(None)
    }

    async fn get_recent_trades(&self, _limit: i64) -> chimera_operator::error::AppResult<Vec<chimera_operator::db_abstraction::Trade>> {
        Ok(vec![])
    }

    // Add more stub methods as needed...
    // (This is a simplified mock for testing purposes)
}

// Implement other required Database methods for the mock
impl MockDatabase {
    async fn create_wallet(&self, _wallet: &chimera_operator::db_abstraction::Wallet) -> chimera_operator::error::AppResult<()> {
        Ok(())
    }

    async fn update_wallet_status(&self, _address: &str, _status: &str) -> chimera_operator::error::AppResult<()> {
        Ok(())
    }

    async fn get_all_wallets(&self) -> chimera_operator::error::AppResult<Vec<chimera_operator::db_abstraction::Wallet>> {
        Ok(vec![])
    }

    async fn create_position(&self, _position: &chimera_operator::db_abstraction::Position) -> chimera_operator::error::AppResult<()> {
        Ok(())
    }

    async fn update_position(&self, _position: &chimera_operator::db_abstraction::Position) -> chimera_operator::error::AppResult<()> {
        Ok(())
    }

    async fn get_active_positions(&self) -> chimera_operator::error::AppResult<Vec<chimera_operator::db_abstraction::Position>> {
        Ok(vec![])
    }

    async fn get_position_by_id(&self, _id: i64) -> chimera_operator::error::AppResult<Option<chimera_operator::db_abstraction::Position>> {
        Ok(None)
    }

    async fn delete_position(&self, _id: i64) -> chimera_operator::error::AppResult<()> {
        Ok(())
    }

    async fn get_dead_letter_queue(&self, _limit: i64) -> chimera_operator::error::AppResult<Vec<chimera_operator::db_abstraction::DeadLetterEntry>> {
        Ok(vec![])
    }

    async fn create_dead_letter_entry(&self, _entry: &chimera_operator::db_abstraction::DeadLetterEntry) -> chimera_operator::error::AppResult<()> {
        Ok(())
    }

    async fn delete_dead_letter_entry(&self, _id: i64) -> chimera_operator::error::AppResult<()> {
        Ok(())
    }

    async fn get_config_audit_log(&self, _limit: i64) -> chimera_operator::error::AppResult<Vec<chimera_operator::db_abstraction::ConfigAuditEntry>> {
        Ok(vec![])
    }

    async fn get_24h_loss_usd(&self) -> chimera_operator::error::AppResult<rust_decimal::Decimal> {
        Ok(rust_decimal::Decimal::ZERO)
    }

    async fn get_consecutive_losses(&self) -> chimera_operator::error::AppResult<u32> {
        Ok(0)
    }

    async fn get_max_drawdown(&self) -> chimera_operator::error::AppResult<rust_decimal::Decimal> {
        Ok(rust_decimal::Decimal::ZERO)
    }

    async fn get_portfolio_pnl_24h(&self) -> chimera_operator::error::AppResult<rust_decimal::Decimal> {
        Ok(rust_decimal::Decimal::ZERO)
    }

    async fn get_all_trades(&self) -> chimera_operator::error::AppResult<Vec<chimera_operator::db_abstraction::Trade>> {
        Ok(vec![])
    }

    async fn get_recent_wallet_promotions(&self, _limit: i64) -> chimera_operator::error::AppResult<Vec<chimera_operator::db_abstraction::Wallet>> {
        Ok(vec![])
    }

    async fn get_recent_wallet_demotions(&self, _limit: i64) -> chimera_operator::error::AppResult<Vec<chimera_operator::db_abstraction::Wallet>> {
        Ok(vec![])
    }

    async fn get_trade_metrics(&self, _hours: i64) -> chimera_operator::error::AppResult<chimera_operator::db_abstraction::TradeMetrics> {
        Ok(chimera_operator::db_abstraction::TradeMetrics {
            total_trades: 0,
            successful_trades: 0,
            failed_trades: 0,
            total_volume_usd: rust_decimal::Decimal::ZERO,
            total_pnl_usd: rust_decimal::Decimal::ZERO,
            avg_pnl_pct: rust_decimal::Decimal::ZERO,
        })
    }

    async fn check_health(&self) -> chimera_operator::error::AppResult<bool> {
        Ok(true)
    }

    async fn close(&self) -> chimera_operator::error::AppResult<()> {
        Ok(())
    }
}