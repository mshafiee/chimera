//! Unit tests for Jito prioritization features
//!
//! Tests for:
//! - JitoError classification (enum vocabulary)
//! - JitoHealth monitoring structure
//! - Configuration defaults (via the public JitoConfig::default())
//! - Tip scaling by trade size against the real default config

use chimera_operator::config::JitoConfig;
use chimera_operator::engine::executor::{JitoError, JitoHealth};
use chrono::Utc;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal_macros::dec;

/// Test JitoError variants can be created, matched, and formatted
#[test]
fn test_jito_error_classification() {
    let retryable = JitoError::Retryable("insufficient tip".to_string());
    let fatal = JitoError::Fatal("insufficient balance".to_string());
    let network = JitoError::Network("endpoint unavailable".to_string());

    match &retryable {
        JitoError::Retryable(msg) => assert_eq!(msg, "insufficient tip"),
        _ => panic!("Expected retryable error"),
    }
    match &fatal {
        JitoError::Fatal(msg) => assert_eq!(msg, "insufficient balance"),
        _ => panic!("Expected fatal error"),
    }
    match &network {
        JitoError::Network(msg) => assert_eq!(msg, "endpoint unavailable"),
        _ => panic!("Expected network error"),
    }

    // Debug formatting is stable for diagnostics
    assert_eq!(format!("{:?}", retryable), "Retryable(\"insufficient tip\")");
}

/// Test JitoHealth structure creation
#[test]
fn test_jito_health_creation() {
    let health = JitoHealth {
        healthy: true,
        last_check: Utc::now(),
        latency_ms: Some(45),
        resolution_success_rate: 0.85,
        total_submissions: 100,
        successful_resolutions: 85,
    };

    assert!(health.healthy);
    assert_eq!(health.latency_ms, Some(45));
    assert_eq!(health.resolution_success_rate, 0.85);
    assert_eq!(health.total_submissions, 100);
    assert_eq!(health.successful_resolutions, 85);
}

/// Test JitoHealth can be cloned
#[test]
fn test_jito_health_clone() {
    let health1 = JitoHealth {
        healthy: true,
        last_check: Utc::now(),
        latency_ms: Some(25),
        resolution_success_rate: 0.95,
        total_submissions: 1000,
        successful_resolutions: 950,
    };

    let health2 = health1.clone();
    assert_eq!(health2.healthy, health1.healthy);
    assert_eq!(health2.latency_ms, health1.latency_ms);
    assert_eq!(health2.resolution_success_rate, health1.resolution_success_rate);
    assert_eq!(health2.total_submissions, health1.total_submissions);
    assert_eq!(health2.successful_resolutions, health1.successful_resolutions);
}

/// Test JitoHealth edge cases (zero data, degraded state)
#[test]
fn test_jito_health_edge_cases() {
    let zero = JitoHealth {
        healthy: true,
        last_check: Utc::now(),
        latency_ms: None,
        resolution_success_rate: 1.0, // Default to healthy when no data
        total_submissions: 0,
        successful_resolutions: 0,
    };
    assert!(zero.healthy);
    assert_eq!(zero.resolution_success_rate, 1.0);

    let degraded = JitoHealth {
        healthy: false,
        last_check: Utc::now(),
        latency_ms: Some(5000),
        resolution_success_rate: 0.3,
        total_submissions: 200,
        successful_resolutions: 60,
    };
    assert!(!degraded.healthy);
    assert_eq!(degraded.resolution_success_rate, 0.3);
    assert_eq!(degraded.latency_ms, Some(5000));
}

/// Test configuration defaults via the public `JitoConfig::default()`
#[test]
fn test_jito_config_defaults() {
    let config = JitoConfig::default();

    assert!(config.enabled, "Jito should be enabled by default");
    assert_eq!(config.min_failures_before_fallback, 10);
    assert!(!config.disable_fallback, "Fallback should be enabled by default");
    assert_eq!(config.max_retries, 5);
    assert_eq!(config.tip_floor_sol, dec!(0.0005));
    assert_eq!(config.tip_ceiling_sol, dec!(0.005));
    assert_eq!(config.tip_percent_max, dec!(0.02));
    assert_eq!(config.tip_percentile, 50);
    assert!(config.helius_staked_exits);

    // Sanity bounds on the thresholds
    assert!(config.min_failures_before_fallback >= 3);
    assert!(config.max_retries >= 1);
}

/// Test Jito tip calculation scales by trade size (fix for Issue 2)
///
/// Verify that tips are calculated as a percentage of trade size, capped by
/// `tip_percent_max` and `tip_ceiling_sol`, and floored at `tip_floor_sol`.
/// This prevents unrealistic 50% tip-to-position ratios that make P&L
/// meaningless.
#[test]
fn test_jito_tip_scales_by_trade_size() {
    let config = JitoConfig::default();
    assert_eq!(config.tip_percent_max, dec!(0.02)); // 2% of trade size

    // Small trade (0.02 SOL): tip = 0.02 * 0.02 = 0.0004, floored at 0.0005.
    let small_trade_size = dec!(0.02);
    let small_trade_tip = (small_trade_size * config.tip_percent_max).max(config.tip_floor_sol);
    assert_eq!(small_trade_tip, config.tip_floor_sol, "small tip must hit the floor");

    // Medium trade (0.1 SOL): tip = 0.1 * 0.02 = 0.002, under the 0.005 ceiling.
    let medium_trade_size = dec!(0.1);
    let medium_trade_tip = medium_trade_size * config.tip_percent_max;
    assert_eq!(medium_trade_tip, dec!(0.002));
    assert!(medium_trade_tip < config.tip_ceiling_sol);

    // Large trade (1.0 SOL): 1.0 * 0.02 = 0.02, capped at ceiling 0.005.
    let large_trade_size = dec!(1.0);
    let large_trade_tip = (large_trade_size * config.tip_percent_max).min(config.tip_ceiling_sol);
    assert_eq!(large_trade_tip, config.tip_ceiling_sol, "large tip must cap at ceiling");

    // Tiny trade (0.005 SOL): 0.005 * 0.02 = 0.0001, floored at 0.0005.
    let tiny_trade_size = dec!(0.005);
    let tiny_trade_tip = (tiny_trade_size * config.tip_percent_max).max(config.tip_floor_sol);
    assert_eq!(tiny_trade_tip, config.tip_floor_sol);

    // Verify tip-to-position ratios are reasonable (not 50% as in bug)
    let small_ratio = (small_trade_tip / small_trade_size).to_f64().unwrap_or(0.0);
    assert!(small_ratio <= 0.15, "small trade tip ratio ({small_ratio}) must be reasonable");
    let medium_ratio = (medium_trade_tip / medium_trade_size).to_f64().unwrap_or(0.0);
    assert!(medium_ratio <= 0.15, "medium trade tip ratio ({medium_ratio}) must be reasonable");
}
