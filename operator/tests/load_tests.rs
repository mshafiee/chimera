//! Load tests for Jito prioritization performance
//!
//! Tests for:
//! - Metric recording overhead
//! - Notification performance
//! - Health check latency
//! - Atomic counter performance
//! - Concurrent metric updates
//!
//! Timing budgets are deliberately generous: these are smoke-level
//! performance checks, and tight wall-clock thresholds are flaky under CI load.

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
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

    // Performance assertion: 1 million increments in under a second
    assert!(duration.as_millis() < 1000, "Atomic operations too slow: {:?}", duration);
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

    // Performance assertion: 10k metric updates should be fast
    assert!(
        duration.as_millis() < 200,
        "Metrics recording too slow: {:?} for {} iterations",
        duration,
        iterations
    );

    // The counters must have recorded every update.
    assert_eq!(
        metrics.jito_submissions.with_label_values(&["jito"]).get(),
        10_000
    );
    assert_eq!(
        metrics.jito_resolutions.with_label_values(&["success"]).get(),
        10_000
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
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

    // Data integrity: 25 tasks × 1000 submissions per mode; per task 900
    // successes + 100 failures. No update may be lost or mislabeled.
    assert_eq!(metrics.jito_submissions.with_label_values(&["jito"]).get(), 25_000);
    assert_eq!(metrics.jito_submissions.with_label_values(&["helius"]).get(), 25_000);
    assert_eq!(metrics.jito_resolutions.with_label_values(&["success"]).get(), 45_000);
    assert_eq!(metrics.jito_resolutions.with_label_values(&["failed"]).get(), 5_000);

    // Performance assertion: 50k concurrent updates should be fast
    assert!(
        duration.as_millis() < 1000,
        "Concurrent metric updates too slow: {:?}",
        duration
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
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
    assert!(
        duration.as_millis() < 2000,
        "Notification throughput too low: {:?} for {} notifications",
        duration,
        num_events
    );

    // Check average processing time is reasonable
    let avg_ns = notifier.get_avg_processing_time_ns();
    assert!(
        avg_ns < 1_000_000, // < 1ms average
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
        // Simulate health check operations (black_box prevents the optimizer
        // from eliminating the loop in release builds)
        std::hint::black_box(health.healthy);
        std::hint::black_box(health.latency_ms);
        std::hint::black_box(health.resolution_success_rate);
        std::hint::black_box(health.total_submissions);
        std::hint::black_box(health.successful_resolutions);

        // Simulate calculation
        std::hint::black_box(health.successful_resolutions as f64 / health.total_submissions as f64);
    }

    let duration = start.elapsed();

    // Performance assertion: 10k health-check reads in under 200ms
    assert!(
        duration.as_millis() < 200,
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
        std::hint::black_box(health.clone());
    }

    let duration = start.elapsed();

    // Performance assertion: cloning should be fast
    assert!(
        duration.as_millis() < 500,
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

        // Simulate error classification (black_box forces evaluation)
        let retryable = std::hint::black_box(match error {
            JitoError::Retryable(_) => true,
            JitoError::Fatal(_) => false,
            JitoError::Network(_) => false,
        });
        std::hint::black_box(retryable);
    }

    let duration = start.elapsed();

    // Performance assertion: 10k classifications in under 200ms
    assert!(
        duration.as_millis() < 200,
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
    // 10k allocations should be < 2s
    assert!(
        duration.as_millis() < 2000,
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
        // Simulate configuration reads (black_box forces evaluation)
        std::hint::black_box(config.jito.enabled);
        std::hint::black_box(config.jito.min_failures_before_fallback);
        std::hint::black_box(config.jito.max_retries);
        std::hint::black_box(config.jito.disable_fallback);
        std::hint::black_box(config.rpc.primary_provider.as_str());
    }

    let duration = start.elapsed();

    // Performance assertion: config reads should be fast
    assert!(
        duration.as_millis() < 200,
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
    assert!(
        duration.as_millis() < 500,
        "Labeled metrics too slow: {:?}",
        duration
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
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
                    // Simulate health check operations (black_box forces evaluation)
                    std::hint::black_box(health.healthy);
                    std::hint::black_box(health.latency_ms);
                    std::hint::black_box(health.resolution_success_rate);
                }
            })
        })
        .collect();

    // Wait for all tasks
    for handle in handles {
        handle.await.unwrap();
    }

    let duration = start.elapsed();

    // Performance assertion: 10k concurrent checks in under a second
    assert!(
        duration.as_millis() < 1000,
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
    assert!(
        duration.as_millis() < 2000,
        "Notification event creation too slow: {:?}",
        duration
    );
}