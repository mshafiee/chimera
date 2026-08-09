//! Circuit Breaker Unit Tests
//!
//! Tests the real circuit-breaker transitions:
//! - Jupiter failure threshold auto-trip (record_jupiter_failure)
//! - Jupiter failure counter reset
//! - Manual trip state transition
//! - Trip reason formatting
//!
//! NOTE: the full threshold EVALUATION (max_loss_24h / consecutive losses /
//! drawdown, driven by DB loss records) is covered in
//! `circuit_breaker_real_tests.rs`; this file covers the state-machine and
//! failure-counter surface.

use chimera_operator::circuit_breaker::{CircuitBreaker, CircuitBreakerState, TripReason};
use chimera_operator::config::CircuitBreakerConfig;
use chimera_operator::db_abstraction::Database;
use rust_decimal_macros::dec;
use std::sync::Arc;

#[path = "../common/mod.rs"]
mod common;

async fn create_test_breaker(
    config: CircuitBreakerConfig,
) -> (CircuitBreaker, Arc<dyn Database>, common::TestDbGuard) {
    let (db, guard) = common::create_test_pg_db().await;
    let breaker = CircuitBreaker::new(config, db.clone(), dec!(100));
    (breaker, db, guard)
}

fn test_config() -> CircuitBreakerConfig {
    CircuitBreakerConfig {
        max_loss_24h_usd: dec!(500.0),
        max_consecutive_losses: 5,
        max_drawdown_percent: dec!(15.0),
        portfolio_stop_loss_percent: dec!(5.0),
        cooldown_minutes: 30,
        max_jupiter_failures: 5,
    }
}

#[tokio::test]
async fn test_jupiter_failure_threshold_auto_trip() {
    // record_jupiter_failure must return Ok(true) and transition to TRIPPED
    // once the consecutive-failure count reaches max_jupiter_failures.
    let (breaker, _db, _guard) = create_test_breaker(test_config()).await;

    assert!(
        breaker.is_trading_allowed(),
        "breaker must start un-tripped"
    );
    assert_eq!(breaker.current_state(), CircuitBreakerState::Active);

    for i in 1..5u32 {
        let tripped = breaker
            .record_jupiter_failure(format!("timeout-{i}"))
            .await
            .unwrap();
        assert!(!tripped, "failure {i} of 5 must not trip yet");
        assert_eq!(breaker.get_jupiter_failure_count(), i);
    }

    let tripped = breaker
        .record_jupiter_failure("timeout-5".to_string())
        .await
        .unwrap();
    assert!(tripped, "5th consecutive failure must auto-trip");
    assert_eq!(breaker.get_jupiter_failure_count(), 5);
    assert!(
        !breaker.is_trading_allowed(),
        "breaker must block trading after trip"
    );
    assert_eq!(breaker.current_state(), CircuitBreakerState::Tripped);

    let status = breaker.status();
    assert!(
        status
            .trip_reason
            .as_deref()
            .unwrap_or("")
            .contains("Jupiter"),
        "trip reason must explain the Jupiter failures, got: {:?}",
        status.trip_reason
    );
}

#[tokio::test]
async fn test_jupiter_failure_counter_resets() {
    let (breaker, _db, _guard) = create_test_breaker(test_config()).await;

    for i in 0..3u32 {
        let _ = breaker
            .record_jupiter_failure(format!("failure-{i}"))
            .await
            .unwrap();
    }
    assert_eq!(breaker.get_jupiter_failure_count(), 3);

    breaker.reset_jupiter_failures();
    assert_eq!(
        breaker.get_jupiter_failure_count(),
        0,
        "successful calls must reset the failure counter"
    );
}

#[tokio::test]
async fn test_manual_trip_state_transition() {
    let (breaker, _db, _guard) = create_test_breaker(test_config()).await;

    breaker
        .manual_trip("test-admin", "manual trip for testing".to_string())
        .await
        .unwrap();

    assert!(!breaker.is_trading_allowed());
    assert_eq!(breaker.current_state(), CircuitBreakerState::Tripped);
    assert!(breaker.status().trip_reason.is_some());
}

#[tokio::test]
async fn test_trip_reason_formatting() {
    let reason = TripReason::MaxLoss24h {
        loss: dec!(525.50),
        threshold: dec!(500),
    };
    let display = reason.to_string();
    assert!(display.contains("525.50"));
    assert!(display.contains("500"));
    assert!(display.contains("24h"));
}
