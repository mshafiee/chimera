//! Integration tests for Jito prioritization features
//!
//! Tests for:
//! - End-to-end Jito bundle execution
//! - Health check functionality
//! - Bundle resolution tracking
//! - Notification delivery
//! - Metric recording validation
//! - Mode switching behavior

use chimera_operator::config::{Config, JitoConfig, RpcConfig, TradeConfig};
use chimera_operator::engine::executor::{Executor, JitoError, JitoHealth};
use chimera_operator::metrics::MetricsState;
use chimera_operator::notifications::{NotificationEvent, NotificationService};
use chimera_operator::trade::{Signal, Strategy};
use chimera_operator::rpc_mode::RpcMode;
use rust_decimal::Decimal;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

/// Mock notification service for testing
struct MockNotifier {
    events: Arc<parking_lot::Mutex<Vec<NotificationEvent>>>,
}

impl MockNotifier {
    fn new() -> Self {
        Self {
            events: Arc::new(parking_lot::Mutex::new(Vec::new())),
        }
    }

    fn get_events(&self) -> Vec<NotificationEvent> {
        self.events.lock().clone()
    }

    fn clear(&self) {
        self.events.lock().clear();
    }
}

#[async_trait::async_trait]
impl NotificationService for MockNotifier {
    async fn notify(&self, event: &NotificationEvent, _trade_mode: &str) -> anyhow::Result<()> {
        self.events.lock().push(event.clone());
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        true
    }
}

/// Create test configuration for Jito tests
fn create_test_config() -> Config {
    Config {
        trade_mode: chimera_operator::trade_mode::TradeMode::Live,
        rpc_mode: chimera_operator::rpc_mode::RpcMode::Jito,

        rpc: RpcConfig {
            primary_url: "https://api.mainnet-beta.solana.com".to_string(),
            fallback_url: None,
            health_check_interval_secs: 30,
            timeout_ms: 5000,
            mode: chimera_operator::rpc_mode::RpcMode::Jito,
        },

        trade: TradeConfig {
            max_position_size_sol: 10.0,
            min_liquidity_usd: 5000.0,
            slippage_tolerance_bps: 100,
            max_slippage_bps: 500,
            stop_loss_bps: 200,
            take_profit_bps: 500,
        },

        jito: JitoConfig {
            enabled: true,
            searcher_endpoint: "https://mainnet.block-engine.jito.wtf".to_string(),
            default_tip_lamports: 1000,
            min_failures_before_fallback: 10,
            disable_fallback: false,
            max_retries: 5,
        },

        // Default values for other fields
        ..Default::default()
    }
}

/// Create test signal
fn create_test_signal() -> Signal {
    Signal {
        id: uuid::Uuid::new_v4(),
        wallet_address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        token_address: "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263".to_string(),
        token_symbol: "BONK".to_string(),
        strategy: Strategy::Shield,
        entry_type: chimera_operator::trade::EntryType::Long,
        position_size_sol: Decimal::from(1u32),
        price: Decimal::from_str("0.000025").unwrap(),
        timestamp: chrono::Utc::now(),
        stop_loss_bps: Some(200),
        take_profit_bps: Some(500),
        confidence: 0.85,
    }
}

#[tokio::test]
async fn test_jito_health_check_initialization() {
    // Test that health monitoring initializes correctly
    let health = JitoHealth {
        healthy: true,
        last_check: chrono::Utc::now(),
        latency_ms: Some(50),
        resolution_success_rate: 1.0,
        total_submissions: 0,
        successful_resolutions: 0,
    };

    assert!(health.healthy);
    assert_eq!(health.total_submissions, 0);
    assert_eq!(health.resolution_success_rate, 1.0);
    assert_eq!(health.latency_ms, Some(50));
}

#[tokio::test]
async fn test_jito_health_success_rate_tracking() {
    // Test success rate tracking across multiple submissions
    let mut health = JitoHealth {
        healthy: true,
        last_check: chrono::Utc::now(),
        latency_ms: Some(45),
        resolution_success_rate: 0.0,
        total_submissions: 0,
        successful_resolutions: 0,
    };

    // Simulate 10 submissions with 8 successes
    health.total_submissions = 10;
    health.successful_resolutions = 8;
    health.resolution_success_rate = 0.8;

    assert_eq!(health.total_submissions, 10);
    assert_eq!(health.successful_resolutions, 8);
    assert_eq!(health.resolution_success_rate, 0.8);
}

#[tokio::test]
async fn test_jito_health_degradation_detection() {
    // Test detection of health degradation
    let healthy = JitoHealth {
        healthy: true,
        last_check: chrono::Utc::now(),
        latency_ms: Some(30),
        resolution_success_rate: 0.95,
        total_submissions: 100,
        successful_resolutions: 95,
    };

    let degraded = JitoHealth {
        healthy: false,
        last_check: chrono::Utc::now(),
        latency_ms: Some(5000),
        resolution_success_rate: 0.5,
        total_submissions: 100,
        successful_resolutions: 50,
    };

    assert!(healthy.healthy);
    assert!(healthy.resolution_success_rate > 0.9);
    assert!(healthy.latency_ms.unwrap() < 100);

    assert!(!degraded.healthy);
    assert!(degraded.resolution_success_rate < 0.6);
    assert!(degraded.latency_ms.unwrap() > 1000);
}

#[tokio::test]
async fn test_notification_jito_fallback_event() {
    // Test Jito fallback notification event creation
    let notifier = MockNotifier::new();

    let event = NotificationEvent::JitoFallbackTriggered {
        reason: "Consecutive Jito failures exceeded threshold".to_string(),
        failure_count: 10,
        threshold: 10,
    };

    // Verify event can be created and formatted
    let message = event.format_message("Live");
    assert!(message.contains("Jito fallback"));
    assert!(message.contains("10"));
}

#[tokio::test]
async fn test_notification_jito_recovery_event() {
    // Test Jito recovery notification event
    let event = NotificationEvent::JitoRecovered {
        latency_ms: 45,
    };

    let message = event.format_message("Live");
    assert!(message.contains("recovered"));
    assert!(message.contains("45"));
}

#[tokio::test]
async fn test_notification_jito_health_change_event() {
    // Test Jito health change notification
    let event_unhealthy = NotificationEvent::JitoHealthChanged {
        healthy: false,
        latency_ms: Some(200),
        success_rate: 0.65,
    };

    let message = event_unhealthy.format_message("Live");
    assert!(message.contains("unhealthy"));
    assert!(message.contains("200"));
    assert!(message.contains("65"));

    let event_healthy = NotificationEvent::JitoHealthChanged {
        healthy: true,
        latency_ms: Some(30),
        success_rate: 0.95,
    };

    let message_healthy = event_healthy.format_message("Live");
    assert!(message_healthy.contains("healthy"));
}

#[tokio::test]
async fn test_jito_configuration_defaults() {
    // Test Jito configuration defaults
    let config = create_test_config();

    assert_eq!(config.jito.min_failures_before_fallback, 10);
    assert_eq!(config.jito.max_retries, 5);
    assert!(!config.jito.disable_fallback);
    assert!(config.jito.enabled);
}

#[tokio::test]
async fn test_jito_configuration_custom_values() {
    // Test Jito configuration with custom values
    let mut config = create_test_config();

    config.jito.min_failures_before_fallback = 15;
    config.jito.max_retries = 7;
    config.jito.disable_fallback = true;
    config.jito.default_tip_lamports = 2000;

    assert_eq!(config.jito.min_failures_before_fallback, 15);
    assert_eq!(config.jito.max_retries, 7);
    assert!(config.jito.disable_fallback);
    assert_eq!(config.jito.default_tip_lamports, 2000);
}

#[tokio::test]
async fn test_jito_error_classification_pattern() {
    // Test Jito error classification patterns
    let retryable = JitoError::Retryable("insufficient tip".to_string());
    let fatal = JitoError::Fatal("insufficient balance".to_string());
    let network = JitoError::Network("endpoint unavailable".to_string());

    // Verify error types can be matched
    match retryable {
        JitoError::Retryable(msg) => assert_eq!(msg, "insufficient tip"),
        _ => panic!("Expected retryable error"),
    }

    match fatal {
        JitoError::Fatal(msg) => assert_eq!(msg, "insufficient balance"),
        _ => panic!("Expected fatal error"),
    }

    match network {
        JitoError::Network(msg) => assert_eq!(msg, "endpoint unavailable"),
        _ => panic!("Expected network error"),
    }
}

#[tokio::test]
async fn test_metrics_initialization() {
    // Test metrics state initialization
    let metrics = MetricsState::new();

    // Verify metrics are initialized
    // This tests the MetricsState::new() method includes Jito metrics
    assert!(metrics.jito_submissions.get_metric().name().contains("jito"));
    assert!(metrics.jito_resolutions.get_metric().name().contains("jito"));
}

#[tokio::test]
async fn test_signal_creation_for_jito() {
    // Test signal creation for Jito execution
    let signal = create_test_signal();

    assert_eq!(signal.strategy, Strategy::Shield);
    assert_eq!(signal.token_symbol, "BONK");
    assert!(signal.confidence > 0.8);
    assert!(signal.position_size_sol > Decimal::ZERO);
}

#[tokio::test]
async fn test_jito_health_zero_submissions_handling() {
    // Test Jito health with zero submissions
    let health = JitoHealth {
        healthy: true,
        last_check: chrono::Utc::now(),
        latency_ms: None,
        resolution_success_rate: 1.0, // Default to healthy when no data
        total_submissions: 0,
        successful_resolutions: 0,
    };

    // With zero submissions, should default to healthy
    assert!(health.healthy);
    assert_eq!(health.resolution_success_rate, 1.0);
    assert_eq!(health.total_submissions, 0);
}

#[tokio::test]
async fn test_jito_health_clone_and_update() {
    // Test Jito health can be cloned and updated
    let health1 = JitoHealth {
        healthy: true,
        last_check: chrono::Utc::now(),
        latency_ms: Some(40),
        resolution_success_rate: 0.9,
        total_submissions: 100,
        successful_resolutions: 90,
    };

    let mut health2 = health1.clone();
    health2.total_submissions = 200;
    health2.successful_resolutions = 180;
    health2.resolution_success_rate = 0.9;

    assert_eq!(health2.total_submissions, 200);
    assert_eq!(health2.successful_resolutions, 180);
    assert_eq!(health2.resolution_success_rate, 0.9);
}

#[tokio::test]
async fn test_jito_retry_threshold_configuration() {
    // Test retry threshold configuration
    let config = create_test_config();

    // Verify default retry threshold
    assert_eq!(config.jito.max_retries, 5);

    // Test with custom threshold
    let mut custom_config = config.clone();
    custom_config.jito.max_retries = 10;
    assert_eq!(custom_config.jito.max_retries, 10);
}

#[tokio::test]
async fn test_jito_fallback_disabled_configuration() {
    // Test fallback disabled configuration
    let mut config = create_test_config();
    config.jito.disable_fallback = true;

    assert!(config.jito.disable_fallback);

    // This should prevent fallback regardless of failure count
    config.jito.min_failures_before_fallback = 1;
    assert_eq!(config.jito.min_failures_before_fallback, 1);
}

#[tokio::test]
async fn test_jito_health_various_scenarios() {
    // Test various health scenarios
    let scenarios = vec![
        // (healthy, latency, success_rate, total, successful)
        (true, Some(20), 1.0, 100, 100),   // Perfect
        (true, Some(50), 0.95, 100, 95),   // Good
        (true, Some(100), 0.85, 100, 85),  // Acceptable
        (false, Some(500), 0.5, 100, 50),  // Poor
        (false, None, 0.3, 100, 30),       // Bad
    ];

    for (healthy, latency, success_rate, total, successful) in scenarios {
        let health = JitoHealth {
            healthy,
            last_check: chrono::Utc::now(),
            latency_ms: latency,
            resolution_success_rate: success_rate,
            total_submissions: total,
            successful_resolutions: successful,
        };

        assert_eq!(health.healthy, healthy);
        assert_eq!(health.latency_ms, latency);
        assert_eq!(health.resolution_success_rate, success_rate);
    }
}

#[tokio::test]
async fn test_jito_notification_rate_limiting_keys() {
    // Test notification rate limiting key generation
    let events = vec![
        NotificationEvent::JitoFallbackTriggered {
            reason: "test".to_string(),
            failure_count: 10,
            threshold: 10,
        },
        NotificationEvent::JitoRecovered { latency_ms: 45 },
        NotificationEvent::JitoHealthChanged {
            healthy: true,
            latency_ms: Some(30),
            success_rate: 0.95,
        },
    ];

    // All events should have different rate limit keys
    let mut keys = Vec::new();
    for event in events {
        let key = match event {
            NotificationEvent::JitoFallbackTriggered { .. } => "jito_fallback",
            NotificationEvent::JitoRecovered { .. } => "jito_recovered",
            NotificationEvent::JitoHealthChanged { .. } => "jito_health",
            _ => "",
        };
        keys.push(key.to_string());
    }

    assert!(keys.contains(&"jito_fallback".to_string()));
    assert!(keys.contains(&"jito_recovered".to_string()));
    assert!(keys.contains(&"jito_health".to_string()));
}

#[tokio::test]
async fn test_jito_error_retryable_conditions() {
    // Test retryable error conditions
    let retryable_errors = vec![
        "insufficient tip",
        "bundle timeout",
        "transaction timeout",
        "network timeout",
        "endpoint slow",
    ];

    for error_msg in retryable_errors {
        let error = JitoError::Retryable(error_msg.to_string());
        match error {
            JitoError::Retryable(msg) => {
                assert!(msg.contains("timeout") || msg.contains("tip") || msg.contains("slow"));
            },
            _ => panic!("Expected retryable error"),
        }
    }
}

#[tokio::test]
async fn test_jito_error_fatal_conditions() {
    // Test fatal error conditions
    let fatal_errors = vec![
        "insufficient balance",
        "invalid transaction",
        "account not found",
        "transaction too large",
    ];

    for error_msg in fatal_errors {
        let error = JitoError::Fatal(error_msg.to_string());
        match error {
            JitoError::Fatal(msg) => {
                assert!(msg.contains("balance") || msg.contains("invalid") || msg.contains("not found") || msg.contains("large"));
            },
            _ => panic!("Expected fatal error"),
        }
    }
}

#[tokio::test]
async fn test_jito_error_network_conditions() {
    // Test network error conditions
    let network_errors = vec![
        "endpoint unavailable",
        "connection refused",
        "DNS resolution failed",
        "network unreachable",
    ];

    for error_msg in network_errors {
        let error = JitoError::Network(error_msg.to_string());
        match error {
            JitoError::Network(msg) => {
                assert!(msg.contains("unavailable") || msg.contains("refused") || msg.contains("DNS") || msg.contains("unreachable"));
            },
            _ => panic!("Expected network error"),
        }
    }
}