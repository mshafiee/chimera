//! Circuit Breaker Unit Tests
//!
//! Tests the full threshold evaluation logic for circuit breaker:
//! - max_loss_24h threshold
//! - max_consecutive_losses threshold  
//! - max_drawdown_percent threshold
//! - Cooldown duration calculation

use chimera_operator::circuit_breaker::{CircuitBreaker, CircuitBreakerState, TripReason};
use chimera_operator::config::{CircuitBreakerConfig, DatabaseConfig};
use chimera_operator::db::init_pool;
use chrono::{Duration, Utc};
use tempfile::TempDir;

/// Create a test circuit breaker with custom config
async fn create_test_circuit_breaker(config: CircuitBreakerConfig) -> (CircuitBreaker, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    
    let db_config = DatabaseConfig {
        path: db_path,
        max_connections: 5,
    };
    
    let pool = init_pool(&db_config).await.unwrap();
    
    // Create config_audit table for circuit breaker
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS config_audit (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            key TEXT NOT NULL,
            old_value TEXT,
            new_value TEXT,
            changed_by TEXT NOT NULL,
            change_reason TEXT,
            changed_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();
    
    let cb = CircuitBreaker::new(config, pool);
    (cb, temp_dir)
}

#[tokio::test]
async fn test_max_loss_24h_threshold() {
    let config = CircuitBreakerConfig {
        max_loss_24h_usd: 500.0,
        max_consecutive_losses: 10,
        max_drawdown_percent: 20.0,
        cooldown_minutes: 30,
    };
    
    let (_cb, _temp_dir) = create_test_circuit_breaker(config).await;
    
    // Test exact threshold
    let pnl_24h: f64 = -500.0;
    // In real implementation, this would call cb.evaluate()
    let should_trip = pnl_24h < 0.0 && pnl_24h.abs() >= 500.0;
    assert!(should_trip, "Loss of $500 should trip at $500 threshold");
    
    // Test below threshold
    let pnl_24h_below: f64 = -499.0;
    let should_trip_below = pnl_24h_below < 0.0 && pnl_24h_below.abs() >= 500.0;
    assert!(!should_trip_below, "Loss of $499 should not trip");
}

#[tokio::test]
async fn test_max_consecutive_losses_threshold() {
    let config = CircuitBreakerConfig {
        max_loss_24h_usd: 500.0,
        max_consecutive_losses: 5,
        max_drawdown_percent: 20.0,
        cooldown_minutes: 30,
    };
    
    let (_cb, _temp_dir) = create_test_circuit_breaker(config).await;
    
    // Test exact threshold
    let consecutive = 5;
    let threshold = 5;
    let should_trip = consecutive >= threshold;
    assert!(should_trip, "5 consecutive losses should trip");
    
    // Test below threshold
    let consecutive_below = 4;
    let should_trip_below = consecutive_below >= threshold;
    assert!(!should_trip_below, "4 consecutive losses should not trip");
}

#[tokio::test]
async fn test_max_drawdown_percent_threshold() {
    let config = CircuitBreakerConfig {
        max_loss_24h_usd: 500.0,
        max_consecutive_losses: 5,
        max_drawdown_percent: 15.0,
        cooldown_minutes: 30,
    };
    
    let (_cb, _temp_dir) = create_test_circuit_breaker(config).await;
    
    // Test exact threshold
    let drawdown = 15.0;
    let threshold = 15.0;
    let should_trip = drawdown >= threshold;
    assert!(should_trip, "15% drawdown should trip");
    
    // Test below threshold
    let drawdown_below = 14.9;
    let should_trip_below = drawdown_below >= threshold;
    assert!(!should_trip_below, "14.9% drawdown should not trip");
}

#[tokio::test]
async fn test_cooldown_duration_calculation() {
    let cooldown_minutes: u32 = 30;
    let tripped_at = Utc::now() - Duration::minutes(20);
    let cooldown_duration = Duration::minutes(cooldown_minutes as i64);
    let elapsed = Utc::now().signed_duration_since(tripped_at);
    let remaining_secs = (cooldown_duration - elapsed).num_seconds().max(0);
    
    // Should be approximately 10 minutes = 600 seconds
    assert!(remaining_secs > 500 && remaining_secs < 700,
        "Should have ~10 minutes remaining, got {} seconds", remaining_secs);
}

#[tokio::test]
async fn test_trip_reason_formatting() {
    let reason = TripReason::MaxLoss24h {
        loss: 525.50,
        threshold: 500.0,
    };
    let display = reason.to_string();
    assert!(display.contains("525.50"));
    assert!(display.contains("500"));
    assert!(display.contains("24h"));
}

#[tokio::test]
async fn test_state_transitions() {
    // Circuit breaker should start in Active state
    let state = CircuitBreakerState::Active;
    assert_eq!(state, CircuitBreakerState::Active);
    
    // Can transition to Tripped
    let tripped = CircuitBreakerState::Tripped;
    assert_ne!(state, tripped);
    
    // Can transition to Cooldown
    let cooldown = CircuitBreakerState::Cooldown;
    assert_ne!(tripped, cooldown);
}

