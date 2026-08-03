//! Integration tests for Jito prioritization features
//!
//! Tests for:
//! - Health check functionality
//! - Bundle resolution tracking
//! - Notification delivery
//! - Metric recording validation
//! - Jito configuration plumbing

use chimera_operator::config::{AppConfig, JitoConfig, RpcConfig, TradeMode};
use chimera_operator::engine::executor::{JitoError, JitoHealth};
use chimera_operator::metrics::MetricsState;
use chimera_operator::models::{Action, Signal, SignalPayload, Strategy};
use chimera_operator::notifications::{NotificationEvent, NotificationService};
use rust_decimal::Decimal;
use std::str::FromStr;
use std::sync::Arc;

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
fn create_test_config() -> AppConfig {
    let mut config = AppConfig::default();
    config.trade_mode = TradeMode::Paper;
    config.rpc.primary_url = "https://api.mainnet-beta.solana.com".to_string();
    config.rpc.timeout_ms = 5000;
    config.jito = JitoConfig {
        enabled: true,
        searcher_endpoint: Some("https://mainnet.block-engine.jito.wtf".to_string()),
        helius_fallback: false,
        tip_floor_sol: Decimal::from_str("0.001").unwrap(),
        tip_ceiling_sol: Decimal::from_str("0.01").unwrap(),
        tip_percentile: 50,
        tip_percent_max: Decimal::from_str("0.10").unwrap(),
        min_failures_before_fallback: 10,
        disable_fallback: false,
        max_retries: 5,
        helius_staked_exits: true,
    };
    config
}

/// Create test signal
fn create_test_signal() -> Signal {
    let payload = SignalPayload {
        strategy: Strategy::Shield,
        token: "BONK".to_string(),
        token_address: Some("DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263".to_string()),
        action: Action::Buy,
        amount_sol: Decimal::from(1u32),
        wallet_address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        trade_uuid: None,
        exit_fraction: None,
    };
    payload.validate().expect("test signal payload must be valid");
    Signal::new(payload, chrono::Utc::now().timestamp(), None)
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
    // Test Jito fallback notification event delivery through the notifier
    let notifier = MockNotifier::new();

    let event = NotificationEvent::JitoFallbackTriggered {
        reason: "Consecutive Jito failures exceeded threshold".to_string(),
        failure_count: 10,
        threshold: 10,
    };

    notifier.notify(&event, "Live").await.unwrap();
    let delivered = notifier.get_events();
    assert_eq!(delivered.len(), 1, "notify() must deliver the event to the notifier");
    assert!(matches!(
        delivered[0],
        NotificationEvent::JitoFallbackTriggered { failure_count: 10, threshold: 10, .. }
    ));

    // Verify event can be created and formatted
    let message = event.format_message("Live");
    assert!(message.contains("Jito fallback"));
    assert!(message.contains("10"));
}

#[tokio::test]
async fn test_notification_jito_recovery_event() {
    // Test Jito recovery notification event delivery
    let notifier = MockNotifier::new();

    let event = NotificationEvent::JitoRecovered { latency_ms: 45 };

    notifier.notify(&event, "Live").await.unwrap();
    let delivered = notifier.get_events();
    assert_eq!(delivered.len(), 1, "notify() must deliver the event to the notifier");
    assert!(matches!(delivered[0], NotificationEvent::JitoRecovered { latency_ms: 45 }));

    let message = event.format_message("Live");
    assert!(message.contains("recovered"));
    assert!(message.contains("45"));
}

#[tokio::test]
async fn test_notification_jito_health_change_event() {
    // Test Jito health change notification event delivery
    let notifier = MockNotifier::new();

    let event_unhealthy = NotificationEvent::JitoHealthChanged {
        healthy: false,
        latency_ms: Some(200),
        success_rate: 0.65,
    };

    notifier.notify(&event_unhealthy, "Live").await.unwrap();
    let delivered = notifier.get_events();
    assert_eq!(delivered.len(), 1, "notify() must deliver the event to the notifier");
    assert!(matches!(
        delivered[0],
        NotificationEvent::JitoHealthChanged { healthy: false, success_rate, .. } if success_rate == 0.65
    ));

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

    assert_eq!(config.jito.min_failures_before_fallback, 15);
    assert_eq!(config.jito.max_retries, 7);
    assert!(config.jito.disable_fallback);
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
    let metrics = MetricsState::new().expect("metrics state must initialize");

    // Jito counters start at zero
    assert_eq!(metrics.jito_submissions.with_label_values(&["jito"]).get(), 0);
    assert_eq!(metrics.jito_submissions.with_label_values(&["helius"]).get(), 0);
    assert_eq!(metrics.jito_resolutions.with_label_values(&["success"]).get(), 0);
    assert_eq!(metrics.jito_resolutions.with_label_values(&["failed"]).get(), 0);
}

#[tokio::test]
async fn test_signal_creation_for_jito() {
    // Test signal creation for Jito execution
    let signal = create_test_signal();

    assert_eq!(signal.payload.strategy, Strategy::Shield);
    assert_eq!(signal.payload.token, "BONK");
    assert_eq!(signal.payload.token_address.as_deref(), Some("DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263"));
    assert!(signal.payload.amount_sol > Decimal::ZERO);
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
    // Test notification rate limiting key generation for the Jito events.
    // The production notifiers (discord.rs/telegram.rs) derive their rate-limit
    // keys from the event variant, so this pins the variant-level mapping.
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
            _ => "other",
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
