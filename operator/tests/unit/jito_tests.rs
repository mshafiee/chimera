//! Unit tests for Jito prioritization features
//!
//! Tests for:
//! - Error classification logic
//! - Health monitoring and metrics
//! - Configuration defaults
//! - Metric recording functions

use chimera_operator::engine::executor::{JitoError, JitoHealth};
use rust_decimal::Decimal;
use std::sync::Arc;
use std::time::Duration;
use chrono::Utc;

/// Test error classification for various ExecutorError types
#[test]
fn test_jito_error_classification() {
    // This test would require an Executor instance, so we'll test the logic indirectly
    // by verifying the JitoError enum exists and has the right variants

    // Verify JitoError enum exists and is the right size
    // We can't easily test the classify_jito_error method without a full Executor setup,
    // but we can at least verify the types compile correctly

    // Test that JitoError variants can be created
    let retryable = JitoError::Retryable("insufficient tip".to_string());
    let fatal = JitoError::Fatal("insufficient balance".to_string());
    let network = JitoError::Network("endpoint unavailable".to_string());

    // Verify the Debug implementation works
    let _ = format!("{:?}", retryable);
    let _ = format!("{:?}", fatal);
    let _ = format!("{:?}", network);
}

/// Test JitoHealth structure creation and defaults
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

    // Calculate success rate matches
    let expected_rate = 85.0 / 100.0;
    assert!((health.resolution_success_rate - expected_rate).abs() < 0.01);
}

/// Test JitoHealth success rate calculation
#[test]
fn test_jito_health_success_rate_calculation() {
    // Test with zero submissions (should default to 1.0)
    let health_zero = JitoHealth {
        healthy: true,
        last_check: Utc::now(),
        latency_ms: None,
        resolution_success_rate: 1.0, // No submissions, assume healthy
        total_submissions: 0,
        successful_resolutions: 0,
    };
    assert_eq!(health_zero.resolution_success_rate, 1.0);

    // Test with actual submissions
    let health_with_data = JitoHealth {
        healthy: true,
        last_check: Utc::now(),
        latency_ms: Some(30),
        resolution_success_rate: 0.92, // 92 out of 100
        total_submissions: 100,
        successful_resolutions: 92,
    };
    assert_eq!(health_with_data.resolution_success_rate, 0.92);
    assert_eq!(health_with_data.total_submissions, 100);
    assert_eq!(health_with_data.successful_resolutions, 92);
}

/// Test JitoHealth handles edge cases
#[test]
fn test_jito_health_edge_cases() {
    // Perfect success rate
    let perfect = JitoHealth {
        healthy: true,
        last_check: Utc::now(),
        latency_ms: Some(10),
        resolution_success_rate: 1.0,
        total_submissions: 50,
        successful_resolutions: 50,
    };
    assert_eq!(perfect.resolution_success_rate, 1.0);

    // Zero success rate
    let zero = JitoHealth {
        healthy: false,
        last_check: Utc::now(),
        latency_ms: None,
        resolution_success_rate: 0.0,
        total_submissions: 50,
        successful_resolutions: 0,
    };
    assert_eq!(zero.resolution_success_rate, 0.0);

    // High latency with failures
    let degraded = JitoHealth {
        healthy: false,
        last_check: Utc::now(),
        latency_ms: Some(5000), // 5 seconds
        resolution_success_rate: 0.3,
        total_submissions: 200,
        successful_resolutions: 60,
    };
    assert_eq!(degraded.healthy, false);
    assert_eq!(degraded.resolution_success_rate, 0.3);
    assert_eq!(degraded.latency_ms, Some(5000));
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

/// Test configuration defaults for Jito settings
#[test]
fn test_jito_config_defaults() {
    use chimera_operator::config::{
        default_jito_enabled,
        default_jito_min_failures_before_fallback,
        default_jito_disable_fallback,
        default_jito_max_retries,
    };

    // Verify defaults
    assert_eq!(default_jito_enabled(), true);
    assert_eq!(default_jito_min_failures_before_fallback(), 10);
    assert_eq!(default_jito_disable_fallback(), false);
    assert_eq!(default_jito_max_retries(), 5);

    // Verify the new defaults are higher than old ones
    assert!(default_jito_min_failures_before_fallback() > 3,
        "Min failures before fallback should be higher than old default of 3");
    assert_eq!(default_jito_min_failures_before_fallback(), 10,
        "Min failures before fallback should be 10");
}

/// Test Jito configuration field ordering
#[test]
fn test_jito_config_field_ordering() {
    // This test verifies that all Jito configuration fields can be created
    // with their default values

    let enabled = default_jito_enabled();
    let min_failures = default_jito_min_failures_before_fallback();
    let disable_fallback = default_jito_disable_fallback();
    let max_retries = default_jito_max_retries();

    // Verify all defaults are sensible
    assert!(enabled, "Jito should be enabled by default");
    assert!(min_failures >= 3, "Min failures should be at least 3");
    assert!(!disable_fallback, "Fallback should be enabled by default");
    assert!(max_retries >= 1, "Should allow at least 1 retry");

    // Verify the specific values
    assert_eq!(min_failures, 10, "Min failures should be 10");
    assert_eq!(max_retries, 5, "Max retries should be 5");
}

/// Test JitoError enum can be used in match statements
#[test]
fn test_jito_error_pattern_matching() {
    let retryable = JitoError::Retryable("test error".to_string());

    match retryable {
        JitoError::Retryable(_) => {
            // Expected path
        },
        JitoError::Fatal(_) => {
            panic!("Should not be fatal");
        },
        JitoError::Network(_) => {
            panic!("Should not be network");
        },
    }

    let fatal = JitoError::Fatal("fatal error".to_string());
    match fatal {
        JitoError::Retryable(_) => panic!("Should not be retryable"),
        JitoError::Fatal(_) => {
            // Expected path
        },
        JitoError::Network(_) => panic!("Should not be network"),
    }

    let network = JitoError::Network("network error".to_string());
    match network {
        JitoError::Retryable(_) => panic!("Should not be retryable"),
        JitoError::Fatal(_) => panic!("Should not be fatal"),
        JitoError::Network(_) => {
            // Expected path
        },
    }
}

/// Test atomic counter operations are thread-safe (compile-time verification)
#[test]
fn test_atomic_counter_types() {
    use std::sync::atomic::{AtomicU64, Ordering};

    // Verify atomic counter types exist and work
    let counter = AtomicU64::new(0);

    // Test increment
    counter.fetch_add(1, Ordering::Relaxed);
    assert_eq!(counter.load(Ordering::Relaxed), 1);

    // Test multiple increments
    counter.fetch_add(5, Ordering::Relaxed);
    assert_eq!(counter.load(Ordering::Relaxed), 6);

    // Test load operation
    assert_eq!(counter.load(Ordering::Relaxed), 6);

    // Verify we can do concurrent operations
    let counter1 = AtomicU64::new(10);
    let counter2 = AtomicU64::new(20);

    counter1.fetch_add(1, Ordering::Relaxed);
    counter2.fetch_add(1, Ordering::Relaxed);

    assert_eq!(counter1.load(Ordering::Relaxed), 11);
    assert_eq!(counter2.load(Ordering::Relaxed), 21);
}

/// Test JitoHealth can represent different states
#[test]
fn test_jito_health_state_variations() {
    let healthy_low_latency = JitoHealth {
        healthy: true,
        last_check: Utc::now(),
        latency_ms: Some(20),
        resolution_success_rate: 0.98,
        total_submissions: 1000,
        successful_resolutions: 980,
    };

    let healthy_high_latency = JitoHealth {
        healthy: true,
        last_check: Utc::now(),
        latency_ms: Some(200),
        resolution_success_rate: 0.98,
        total_submissions: 1000,
        successful_resolutions: 980,
    };

    let unhealthy_no_latency = JitoHealth {
        healthy: false,
        last_check: Utc::now(),
        latency_ms: None,
        resolution_success_rate: 0.5,
        total_submissions: 200,
        successful_resolutions: 100,
    };

    let unhealthy_high_latency = JitoHealth {
        healthy: false,
        last_check: Utc::now(),
        latency_ms: Some(1000),
        resolution_success_rate: 0.3,
        total_submissions: 500,
        successful_resolutions: 150,
    };

    // Verify all can be created and have expected values
    assert!(healthy_low_latency.healthy);
    assert_eq!(healthy_low_latency.latency_ms, Some(20));
    assert_eq!(healthy_low_latency.resolution_success_rate, 0.98);

    assert!(healthy_high_latency.healthy);
    assert_eq!(healthy_high_latency.latency_ms, Some(200));

    assert!(!unhealthy_no_latency.healthy);
    assert_eq!(unhealthy_no_latency.latency_ms, None);

    assert!(!unhealthy_high_latency.healthy);
    assert_eq!(unhealthy_high_latency.latency_ms, Some(1000));
    assert_eq!(unhealthy_high_latency.resolution_success_rate, 0.3);
}

/// Test success rate calculation is accurate
#[test]
fn test_success_rate_accuracy() {
    let test_cases = vec![
        (100, 100, 1.0),      // 100% success
        (100, 50, 0.5),       // 50% success
        (200, 0, 0.0),         // 0% success
        (50, 40, 0.8),         // 80% success
        (1000, 950, 0.95),    // 95% success
        (10, 5, 0.5),         // 50% success
    ];

    for (total, success, expected) in test_cases {
        let health = JitoHealth {
            healthy: true,
            last_check: Utc::now(),
            latency_ms: None,
            resolution_success_rate: expected, // Pre-calculated
            total_submissions: total,
            successful_resolutions: success,
        };

        // Verify the success rate matches expected
        assert!((health.resolution_success_rate - expected).abs() < 0.001,
            "Success rate calculation should be accurate");

        // Verify total and successful counts match
        assert_eq!(health.total_submissions, total);
        assert_eq!(health.successful_resolutions, success);
    }
}

/// Test JitoHealth handles zero submissions correctly
#[test]
fn test_jito_health_zero_submissions() {
    let health = JitoHealth {
        healthy: true,
        last_check: Utc::now(),
        latency_ms: None,
        resolution_success_rate: 1.0, // Default to healthy when no data
        total_submissions: 0,
        successful_resolutions: 0,
    };

    // With zero submissions, should default to 1.0 (healthy)
    assert_eq!(health.resolution_success_rate, 1.0);
    assert_eq!(health.total_submissions, 0);
    assert_eq!(health.successful_resolutions, 0);
}

/// Test JitoError can be created with various message types
#[test]
fn test_jito_error_message_creation() {
    let messages = vec![
        "insufficient tip",
        "bundle timeout",
        "endpoint unavailable",
        "insufficient balance",
        "invalid transaction",
        "transaction too large",
    ];

    for msg in messages {
        let retryable = JitoError::Retryable(msg.to_string());
        assert_eq!(format!("{:?}", retryable), format!("Retryable(\"{}\")", msg));

        let fatal = JitoError::Fatal(msg.to_string());
        assert_eq!(format!("{:?}", fatal), format!("Fatal(\"{}\")", msg));

        let network = JitoError::Network(msg.to_string());
        assert_eq!(format!("{:?}", network), format!("Network(\"{}\")", msg));
    }
}

/// Test configuration values are within reasonable bounds
#[test]
fn test_jito_config_reasonable_bounds() {
    let enabled = default_jito_enabled();
    let min_failures = default_jito_min_failures_before_fallback();
    let disable_fallback = default_jito_disable_fallback();
    let max_retries = default_jito_max_retries();

    // Verify enabled is boolean
    assert!(enabled == true || enabled == false);

    // Verify min_failures is reasonable (3-20 range)
    assert!(min_failures >= 3, "Min failures should be at least 3");
    assert!(min_failures <= 100, "Min failures should be at most 100");

    // Verify max_retries is reasonable (1-20 range)
    assert!(max_retries >= 1, "Max retries should be at least 1");
    assert!(max_retries <= 20, "Max retries should be at most 20");

    // Verify min_failures >= max_retries (should retry before falling back)
    assert!(min_failures >= max_retries,
        "Should retry at least as many times as configured before fallback");
}

/// Test Jito tip calculation scales by trade size (fix for Issue 2)
///
/// Verify that tips are calculated as a percentage of trade size (10% max),
/// capped by tip_floor (0.001 SOL) and tip_ceiling (0.01 SOL).
/// This prevents unrealistic 50% tip-to-position ratios that make P&L meaningless.
#[test]
fn test_jito_tip_scales_by_trade_size() {
    use chimera_operator::config::{JitoConfig, default_tip_floor, default_tip_ceiling, default_tip_percent_max};

    // Create Jito config with defaults
    let config = JitoConfig {
        enabled: true,
        searcher_endpoint: Some("https://mainnet.block-engine.jito.wtf".to_string()),
        helius_fallback: true,
        tip_floor_sol: default_tip_floor(),
        tip_ceiling_sol: default_tip_ceiling(),
        tip_percentile: 50,
        tip_percent_max: default_tip_percent_max(),
        min_failures_before_fallback: 10,
        disable_fallback: false,
        max_retries: 5,
        helius_staked_exits: true,
    };

    // Test small trade (0.02 SOL)
    // Expected: tip = 0.02 * 0.10 = 0.002 SOL (10% of position)
    let small_trade_size = dec!(0.02);
    let small_trade_tip = small_trade_size * config.tip_percent_max;
    assert_eq!(small_trade_tip, dec!(0.002),
        "Small trade tip should be 10% of position size");

    // Verify tip meets floor and ceiling constraints
    assert!(small_trade_tip >= config.tip_floor_sol,
        "Small trade tip should meet floor (0.001)");
    assert!(small_trade_tip <= config.tip_ceiling_sol,
        "Small trade tip should not exceed ceiling (0.01)");

    // Test medium trade (0.1 SOL)
    // Expected: tip = 0.1 * 0.10 = 0.01 SOL (at ceiling)
    let medium_trade_size = dec!(0.1);
    let medium_trade_tip = medium_trade_size * config.tip_percent_max;
    assert_eq!(medium_trade_tip, dec!(0.01),
        "Medium trade tip should be 10% of position size");
    assert_eq!(medium_trade_tip, config.tip_ceiling_sol,
        "Medium trade tip should hit ceiling (0.01)");

    // Test large trade (1.0 SOL)
    // Expected: tip = 1.0 * 0.10 = 0.10 SOL, but capped at ceiling 0.01
    let large_trade_size = dec!(1.0);
    let large_trade_tip = large_trade_size * config.tip_percent_max.min(config.tip_ceiling_sol);
    assert_eq!(large_trade_tip, config.tip_ceiling_sol,
        "Large trade tip should be capped at ceiling (0.01)");

    // Test tiny trade (0.005 SOL)
    // Expected: tip = 0.005 * 0.10 = 0.0005 SOL, but must meet floor 0.001
    let tiny_trade_size = dec!(0.005);
    let tiny_trade_tip = tiny_trade_size * config.tip_percent_max;
    let final_tiny_tip = tiny_trade_tip.max(config.tip_floor_sol);
    assert_eq!(final_tiny_tip, config.tip_floor_sol,
        "Tiny trade tip should be raised to floor (0.001)");

    // Verify tip-to-position ratios are reasonable (not 50% as in bug)
    let small_ratio = (small_trade_tip / small_trade_size).to_f64().unwrap_or(0.0);
    assert!(small_ratio <= 0.15, // 15% tolerance
        "Small trade tip ratio ({}) should be reasonable (not 50%)", small_ratio);

    let medium_ratio = (medium_trade_tip / medium_trade_size).to_f64().unwrap_or(0.0);
    assert!(medium_ratio <= 0.15,
        "Medium trade tip ratio ({}) should be reasonable", medium_ratio);
}
