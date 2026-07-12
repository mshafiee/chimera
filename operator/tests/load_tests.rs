//! Load tests for Jito prioritization performance
//!
//! Tests for:
//! - Metric recording overhead
//! - Notification performance
//! - Health check latency
//! - Atomic counter performance
//! - Concurrent metric updates

use chimera_operator::config::{AppConfig, JitoConfig, RpcConfig};
use chimera_operator::engine::executor::{JitoError, JitoHealth};
use chimera_operator::metrics::MetricsState;
use chimera_operator::notifications::{NotificationEvent, NotificationService};
use rust_decimal::Decimal;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Mock notification service for load testing
struct LoadNotifier {
    events_sent: Arc<AtomicU64>,
    processing_time_ns: Arc<AtomicU64>,
}

impl LoadNotifier {
    fn new() -> Self {
        Self {
            events_sent: Arc::new(AtomicU64::new(0)),
            processing_time_ns: Arc::new(AtomicU64::new(0)),
        }
    }

    fn get_events_sent(&self) -> u64 {
        self.events_sent.load(Ordering::Relaxed)
    }

    fn get_avg_processing_time_ns(&self) -> u64 {
        let events = self.events_sent.load(Ordering::Relaxed);
        if events == 0 {
            return 0;
        }
        self.processing_time_ns.load(Ordering::Relaxed) / events
    }
}

#[async_trait::async_trait]
impl NotificationService for LoadNotifier {
    async fn notify(&self, event: &NotificationEvent, _trade_mode: &str) -> anyhow::Result<()> {
        let start = Instant::now();

        // Simulate minimal notification processing
        let _ = event.level();
        let _ = event.format_message("Live");

        let elapsed = start.elapsed();
        self.events_sent.fetch_add(1, Ordering::Relaxed);
        self.processing_time_ns.fetch_add(elapsed.as_nanos() as u64, Ordering::Relaxed);

        Ok(())
    }

    fn is_enabled(&self) -> bool {
        true
    }
}

/// Create load test configuration
fn create_load_config() -> AppConfig {
    use rust_decimal::Decimal;

    AppConfig {
        rpc: RpcConfig {
            primary_provider: "helius".to_string(),
            primary_url: "https://api.mainnet-beta.solana.com".to_string(),
            fallback_url: Some("https://solana-api.projectserum.com".to_string()),
            rate_limit_per_second: 40,
            max_consecutive_failures: 10,
            functional_health_check: true,
            timeout_ms: 5000,
            rate_limit_config: None,
        },

        jito: JitoConfig {
            enabled: true,
            searcher_endpoint: Some("https://mainnet.block-engine.jito.wtf".to_string()),
            helius_fallback: true,
            tip_floor_sol: Decimal::from_str("0.001").unwrap(),
            tip_ceiling_sol: Decimal::from_str("0.01").unwrap(),
            tip_percentile: 50,
            tip_percent_max: Decimal::from_str("0.1").unwrap(),
            min_failures_before_fallback: 10,
            disable_fallback: false,
            max_retries: 5,
            helius_staked_exits: true,
        },

        ..Default::default()
    }
}

#[tokio::test]
async fn test_atomic_counter_performance() {
    // Test atomic counter performance under high load
    let counter = Arc::new(AtomicU64::new(0));

    let num_threads = 10;
    let increments_per_thread = 100_000;

    let start = Instant::now();

    let handles: Vec<_> = (0..num_threads)
        .map(|_| {
            let counter = counter.clone();
            tokio::spawn(async move {
                for _ in 0..increments_per_thread {
                    counter.fetch_add(1, Ordering::Relaxed);
                }
            })
        })
        .collect();

    // Wait for all threads to complete
    for handle in handles {
        handle.await.unwrap();
    }

    let duration = start.elapsed();

    // Verify correctness
    let expected = num_threads * increments_per_thread;
    assert_eq!(counter.load(Ordering::Relaxed), expected);

    // Performance assertion: should complete in reasonable time
    // 1 million increments should take < 100ms (highly conservative)
    assert!(duration.as_millis() < 100, "Atomic operations too slow: {:?}", duration);
}

#[tokio::test]
async fn test_metrics_recording_overhead() {
    // Test that metrics recording adds minimal overhead
    let metrics_result = MetricsState::new();
    let metrics = Arc::new(metrics_result.unwrap());

    let iterations = 10_000;
    let start = Instant::now();

    for i in 0..iterations {
        // Simulate metrics recording
        metrics
            .jito_submissions
            .with_label_values(&["jito"])
            .inc();
        metrics
            .jito_resolutions
            .with_label_values(&["success"])
            .inc();

        if i % 100 == 0 {
            // Simulate occasional health update
            metrics.jito_health.set(1);
        }
    }

    let duration = start.elapsed();

    // Performance assertion: 10k metric updates should be very fast
    // Should be < 10ms total (averaging < 1μs per update)
    assert!(
        duration.as_millis() < 10,
        "Metrics recording too slow: {:?} for {} iterations",
        duration,
        iterations
    );

    let avg_time_ns = duration.as_nanos() / iterations as u128;
    assert!(
        avg_time_ns < 1000, // < 1μs average
        "Average metric recording time too high: {}ns",
        avg_time_ns
    );
}

#[tokio::test]
async fn test_concurrent_metric_updates() {
    // Test concurrent metric update performance
    let metrics_result = MetricsState::new();
    let metrics = Arc::new(metrics_result.unwrap());

    let num_tasks = 50;
    let updates_per_task = 1000;

    let start = Instant::now();

    let handles: Vec<_> = (0..num_tasks)
        .map(|i| {
            let metrics = metrics.clone();
            tokio::spawn(async move {
                for j in 0..updates_per_task {
                    let mode = if i % 2 == 0 { "jito" } else { "helius" };
                    let status = if j % 10 == 0 { "failed" } else { "success" };

                    metrics.jito_submissions.with_label_values(&[mode]).inc();
                    metrics.jito_resolutions.with_label_values(&[status]).inc();
                }
            })
        })
        .collect();

    // Wait for all tasks
    for handle in handles {
        handle.await.unwrap();
    }

    let duration = start.elapsed();

    // Performance assertion: 50k concurrent updates should be fast
    // Should be < 50ms total
    assert!(
        duration.as_millis() < 50,
        "Concurrent metric updates too slow: {:?}",
        duration
    );
}

#[tokio::test]
async fn test_notification_throughput() {
    // Test notification throughput under load
    let notifier = Arc::new(LoadNotifier::new());

    let num_events = 1000;
    let events: Vec<NotificationEvent> = (0..num_events)
        .map(|i| {
            if i % 3 == 0 {
                NotificationEvent::JitoFallbackTriggered {
                    reason: "load test".to_string(),
                    failure_count: 10,
                    threshold: 10,
                }
            } else if i % 3 == 1 {
                NotificationEvent::JitoRecovered { latency_ms: 45 }
            } else {
                NotificationEvent::JitoHealthChanged {
                    healthy: i % 2 == 0,
                    latency_ms: Some(30),
                    success_rate: 0.9,
                }
            }
        })
        .collect();

    let start = Instant::now();

    // Send all notifications concurrently
    let handles: Vec<_> = events
        .into_iter()
        .map(|event| {
            let notifier = notifier.clone();
            tokio::spawn(async move {
                notifier.notify(&event, "Live").await.unwrap();
            })
        })
        .collect();

    // Wait for all notifications
    for handle in handles {
        handle.await.unwrap();
    }

    let duration = start.elapsed();

    // Verify all notifications were sent
    assert_eq!(notifier.get_events_sent(), num_events as u64);

    // Performance assertion: 1000 notifications should complete quickly
    // Should be < 100ms total (averaging < 0.1ms per notification)
    assert!(
        duration.as_millis() < 100,
        "Notification throughput too low: {:?} for {} notifications",
        duration,
        num_events
    );

    // Check average processing time is reasonable
    let avg_ns = notifier.get_avg_processing_time_ns();
    assert!(
        avg_ns < 100_000, // < 0.1ms average
        "Average notification processing time too high: {}ns",
        avg_ns
    );
}

#[tokio::test]
async fn test_health_check_latency() {
    // Test health check operation latency
    let health = JitoHealth {
        healthy: true,
        last_check: chrono::Utc::now(),
        latency_ms: Some(45),
        resolution_success_rate: 0.92,
        total_submissions: 1000,
        successful_resolutions: 920,
    };

    let iterations = 10_000;
    let start = Instant::now();

    for _ in 0..iterations {
        // Simulate health check operations
        let _ = health.healthy;
        let _ = health.latency_ms;
        let _ = health.resolution_success_rate;
        let _ = health.total_submissions;
        let _ = health.successful_resolutions;

        // Simulate calculation
        let _ = health.successful_resolutions as f64 / health.total_submissions as f64;
    }

    let duration = start.elapsed();

    // Performance assertion: health checks should be very fast
    // 10k iterations should be < 5ms
    assert!(
        duration.as_millis() < 5,
        "Health check operations too slow: {:?}",
        duration
    );
}

#[tokio::test]
async fn test_jito_health_clone_performance() {
    // Test JitoHealth clone performance
    let health = JitoHealth {
        healthy: true,
        last_check: chrono::Utc::now(),
        latency_ms: Some(30),
        resolution_success_rate: 0.95,
        total_submissions: 10000,
        successful_resolutions: 9500,
    };

    let iterations = 1000;
    let start = Instant::now();

    for _ in 0..iterations {
        let _ = health.clone();
    }

    let duration = start.elapsed();

    // Performance assertion: cloning should be fast
    // 1000 clones should be < 10ms
    assert!(
        duration.as_millis() < 10,
        "JitoHealth cloning too slow: {:?}",
        duration
    );
}

#[tokio::test]
async fn test_error_classification_performance() {
    // Test error classification performance
    let retryable = JitoError::Retryable("insufficient tip".to_string());
    let fatal = JitoError::Fatal("insufficient balance".to_string());
    let network = JitoError::Network("endpoint unavailable".to_string());

    let iterations = 10_000;
    let start = Instant::now();

    for i in 0..iterations {
        let error = match i % 3 {
            0 => &retryable,
            1 => &fatal,
            _ => &network,
        };

        // Simulate error classification
        match error {
            JitoError::Retryable(_) => true,
            JitoError::Fatal(_) => false,
            JitoError::Network(_) => false,
        };
    }

    let duration = start.elapsed();

    // Performance assertion: error classification should be very fast
    // 10k classifications should be < 5ms
    assert!(
        duration.as_millis() < 5,
        "Error classification too slow: {:?}",
        duration
    );
}

#[tokio::test]
async fn test_memory_allocation_pressure() {
    // Test system behavior under memory allocation pressure
    let mut health_states: Vec<JitoHealth> = Vec::new();

    let iterations = 10_000;
    let start = Instant::now();

    for i in 0..iterations {
        health_states.push(JitoHealth {
            healthy: i % 2 == 0,
            last_check: chrono::Utc::now(),
            latency_ms: Some((i * 10) as u64),
            resolution_success_rate: 0.9,
            total_submissions: 100 + i as u64,
            successful_resolutions: 90 + i as u64,
        });
    }

    let duration = start.elapsed();

    // Verify all allocations completed
    assert_eq!(health_states.len(), iterations);

    // Performance assertion: allocations should be reasonable
    // 10k allocations should be < 100ms
    assert!(
        duration.as_millis() < 100,
        "Memory allocation too slow: {:?}",
        duration
    );
}

#[tokio::test]
async fn test_configuration_reading_overhead() {
    // Test configuration read overhead
    let config = create_load_config();

    let iterations = 100_000;
    let start = Instant::now();

    for _ in 0..iterations {
        // Simulate configuration reads
        let _ = config.jito.enabled;
        let _ = config.jito.min_failures_before_fallback;
        let _ = config.jito.max_retries;
        let _ = config.jito.disable_fallback;
        let _ = config.rpc.primary_provider;
    }

    let duration = start.elapsed();

    // Performance assertion: config reads should be very fast
    // 100k reads should be < 10ms
    assert!(
        duration.as_millis() < 10,
        "Configuration reads too slow: {:?}",
        duration
    );
}

#[tokio::test]
async fn test_prometheus_metric_label_overhead() {
    // Test Prometheus metric with label overhead
    let metrics_result = MetricsState::new();
    let metrics = Arc::new(metrics_result.unwrap());

    let modes = vec!["jito", "helius", "standard"];
    let statuses = vec!["success", "failed"];

    let iterations = 5000;
    let start = Instant::now();

    for i in 0..iterations {
        let mode = modes[i % modes.len()];
        let status = statuses[i % statuses.len()];

        metrics.jito_submissions.with_label_values(&[mode]).inc();
        metrics.jito_resolutions.with_label_values(&[status]).inc();
    }

    let duration = start.elapsed();

    // Performance assertion: labeled metrics should still be fast
    // 10k labeled updates should be < 20ms
    assert!(
        duration.as_millis() < 20,
        "Labeled metrics too slow: {:?}",
        duration
    );
}

#[tokio::test]
async fn test_concurrent_health_checks() {
    // Test concurrent health check operations
    let health = Arc::new(JitoHealth {
        healthy: true,
        last_check: chrono::Utc::now(),
        latency_ms: Some(40),
        resolution_success_rate: 0.92,
        total_submissions: 500,
        successful_resolutions: 460,
    });

    let num_tasks = 100;
    let checks_per_task = 100;

    let start = Instant::now();

    let handles: Vec<_> = (0..num_tasks)
        .map(|_| {
            let health = health.clone();
            tokio::spawn(async move {
                for _ in 0..checks_per_task {
                    // Simulate health check operations
                    let _ = health.healthy;
                    let _ = health.latency_ms;
                    let _ = health.resolution_success_rate;
                }
            })
        })
        .collect();

    // Wait for all tasks
    for handle in handles {
        handle.await.unwrap();
    }

    let duration = start.elapsed();

    // Performance assertion: concurrent health checks should be fast
    // 10k concurrent checks should be < 50ms
    assert!(
        duration.as_millis() < 50,
        "Concurrent health checks too slow: {:?}",
        duration
    );
}

#[tokio::test]
async fn test_notification_event_creation_overhead() {
    // Test notification event creation overhead
    let iterations = 10_000;
    let start = Instant::now();

    for i in 0..iterations {
        let event = if i % 3 == 0 {
            NotificationEvent::JitoFallbackTriggered {
                reason: "test".to_string(),
                failure_count: 10,
                threshold: 10,
            }
        } else if i % 3 == 1 {
            NotificationEvent::JitoRecovered { latency_ms: 45 }
        } else {
            NotificationEvent::JitoHealthChanged {
                healthy: true,
                latency_ms: Some(30),
                success_rate: 0.9,
            }
        };

        // Simulate event usage
        let _ = event.level();
        let _ = event.format_message("Live");
    }

    let duration = start.elapsed();

    // Performance assertion: event creation should be fast
    // 10k events should be < 100ms
    assert!(
        duration.as_millis() < 100,
        "Notification event creation too slow: {:?}",
        duration
    );
}