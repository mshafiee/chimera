//! Prometheus metrics for Chimera Operator
//!
//! Exposes metrics endpoint for monitoring:
//! - Queue depth gauge
//! - Trade latency histogram
//! - RPC health metrics
//! - Circuit breaker state gauge
//! - Position count gauge

use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get, Router};
use prometheus::{
    Encoder, Gauge, Histogram, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge, Opts, Registry,
    TextEncoder,
};
use std::sync::Arc;

/// Metrics state
pub struct MetricsState {
    /// Prometheus registry
    registry: Registry,
    /// Queue depth gauge
    pub queue_depth: IntGauge,
    /// Trade latency histogram (in milliseconds)
    pub trade_latency: Histogram,
    /// RPC health gauge (1 = healthy, 0 = unhealthy)
    pub rpc_health: IntGauge,
    /// Circuit breaker state gauge (2 = active, 1 = cooldown, 0 = tripped)
    pub circuit_breaker_state: IntGauge,
    /// Circuit breaker trips counter
    pub circuit_breaker_trips: prometheus::IntCounter,
    /// Active positions count
    pub active_positions: IntGauge,
    /// Total trades count
    pub total_trades: IntGauge,
    /// RPC latency histogram (in milliseconds)
    pub rpc_latency: HistogramVec,
    /// Reconciliation checked total (counter)
    pub reconciliation_checked: prometheus::IntCounter,
    /// Reconciliation discrepancies total (counter)
    pub reconciliation_discrepancies: prometheus::IntCounter,
    /// Reconciliation unresolved total (gauge)
    pub reconciliation_unresolved: IntGauge,
    /// Secret rotation last success timestamp (gauge, Unix timestamp)
    pub secret_rotation_last_success: IntGauge,
    /// Secret rotation days until due (gauge)
    pub secret_rotation_days_until_due: IntGauge,
    /// Average cost per trade (SOL) - histogram
    pub cost_per_trade: HistogramVec,
    /// Signal quality distribution (histogram)
    pub signal_quality_score: Histogram,
    /// Portfolio heat percentage (gauge)
    pub portfolio_heat_percent: prometheus::Gauge,
    /// Rate limiter health for webhook processing (1 = healthy, 0 = degraded)
    pub webhook_rate_limiter_health: IntGauge,
    /// Rate limiter usage ratio (0-1, current credits / max credits)
    pub webhook_rate_limiter_usage: prometheus::Gauge,
    /// Database query latency histogram (in milliseconds)
    pub db_query_latency: Histogram,
    /// Jito bundle submissions total counter (by mode: jito, helius, standard)
    pub jito_submissions: IntCounterVec,
    /// Jito bundle resolutions total counter (by status: success, failed)
    pub jito_resolutions: IntCounterVec,
    /// Jito endpoint health gauge (1 = healthy, 0 = unhealthy)
    pub jito_health: IntGauge,
    /// Jito fallback trigger counter (by reason)
    pub jito_fallback_total: IntCounterVec,
    /// Jito retry counter (by attempt number)
    pub jito_retry_total: IntCounterVec,
}

/// Execution lock metrics for monitoring lock operations
pub struct ExecutionLockMetrics {
    /// Lock acquisition success counter
    pub acquire_success: prometheus::IntCounter,
    /// Lock acquisition failure counter
    pub acquire_failed: prometheus::IntCounter,
    /// Lock acquisition disabled counter
    pub acquire_disabled: prometheus::IntCounter,
    /// Lock release counter
    pub released: prometheus::IntCounter,
    /// Lock force release counter
    pub force_released: prometheus::IntCounter,
    /// Lock expired and reclaimed counter
    pub expired_reclaimed: prometheus::IntCounter,
    /// Lock expired and cleaned up counter
    pub expired_cleaned: prometheus::IntCounter,
    /// Lock held duration histogram (in seconds)
    pub held_duration: Histogram,
}

/// Rent scavenger metrics for monitoring rent reclamation operations
pub struct RentScavengerMetrics {
    /// Total rent reclaimed (in lamports)
    pub rent_reclaimed_total: prometheus::IntCounter,
    /// Total accounts closed
    pub accounts_closed_total: prometheus::IntCounter,
    /// Rent scavenger errors total
    pub errors_total: prometheus::IntCounter,
    /// Rent scavenger run duration histogram (in seconds)
    pub run_duration: Histogram,
}

impl RentScavengerMetrics {
    /// Create new rent scavenger metrics
    pub fn new() -> Self {
        Self {
            rent_reclaimed_total: IntCounter::with_opts(
                Opts::new("chimera_rent_scavenger_reclaimed_lamports", "Total rent reclaimed in lamports")
            ).unwrap_or_else(|_| IntCounter::with_opts(Opts::new("dummy", "dummy")).unwrap()),
            accounts_closed_total: IntCounter::with_opts(
                Opts::new("chimera_rent_scavenger_accounts_closed", "Total token accounts closed")
            ).unwrap_or_else(|_| IntCounter::with_opts(Opts::new("dummy", "dummy")).unwrap()),
            errors_total: IntCounter::with_opts(
                Opts::new("chimera_rent_scavenger_errors", "Total rent scavenger errors")
            ).unwrap_or_else(|_| IntCounter::with_opts(Opts::new("dummy", "dummy")).unwrap()),
            run_duration: Histogram::with_opts(HistogramOpts::new(
                "chimera_rent_scavenger_run_duration_seconds",
                "Rent scavenger run duration in seconds"
            )).unwrap_or_else(|_| Histogram::with_opts(HistogramOpts::new("dummy", "dummy")).unwrap()),
        }
    }

    /// Increment rent reclaimed counter
    pub fn increment_rent_reclaimed(&self, lamports: u64) {
        self.rent_reclaimed_total.inc_by(lamports);
    }

    /// Increment accounts closed counter
    pub fn increment_accounts_closed(&self, count: u64) {
        self.accounts_closed_total.inc_by(count);
    }

    /// Increment errors counter
    pub fn increment_errors(&self) {
        self.errors_total.inc();
    }

    /// Record run duration
    pub fn record_run_duration(&self, duration: std::time::Duration) {
        self.run_duration.observe(duration.as_secs_f64());
    }
}

impl ExecutionLockMetrics {
    /// Create new execution lock metrics (minimal stub for now)
    pub fn new() -> Self {
        Self {
            acquire_success: IntCounter::with_opts(
                Opts::new("chimera_execution_lock_acquire_success", "Successful lock acquisitions")
            ).unwrap_or_else(|_| IntCounter::with_opts(Opts::new("dummy", "dummy")).unwrap()),
            acquire_failed: IntCounter::with_opts(
                Opts::new("chimera_execution_lock_acquire_failed", "Failed lock acquisitions")
            ).unwrap_or_else(|_| IntCounter::with_opts(Opts::new("dummy", "dummy")).unwrap()),
            acquire_disabled: IntCounter::with_opts(
                Opts::new("chimera_execution_lock_acquire_disabled", "Disabled lock acquisitions")
            ).unwrap_or_else(|_| IntCounter::with_opts(Opts::new("dummy", "dummy")).unwrap()),
            released: IntCounter::with_opts(
                Opts::new("chimera_execution_lock_released", "Lock releases")
            ).unwrap_or_else(|_| IntCounter::with_opts(Opts::new("dummy", "dummy")).unwrap()),
            force_released: IntCounter::with_opts(
                Opts::new("chimera_execution_lock_force_released", "Force lock releases")
            ).unwrap_or_else(|_| IntCounter::with_opts(Opts::new("dummy", "dummy")).unwrap()),
            expired_reclaimed: IntCounter::with_opts(
                Opts::new("chimera_execution_lock_expired_reclaimed", "Expired locks reclaimed")
            ).unwrap_or_else(|_| IntCounter::with_opts(Opts::new("dummy", "dummy")).unwrap()),
            expired_cleaned: IntCounter::with_opts(
                Opts::new("chimera_execution_lock_expired_cleaned", "Expired locks cleaned up")
            ).unwrap_or_else(|_| IntCounter::with_opts(Opts::new("dummy", "dummy")).unwrap()),
            held_duration: Histogram::with_opts(HistogramOpts::new(
                "chimera_execution_lock_held_duration_seconds",
                "Duration in seconds that locks are held",
            ))
            .unwrap_or_else(|_| Histogram::with_opts(HistogramOpts::new("dummy", "dummy")).unwrap()),
        }
    }

    /// Increment successful lock acquisition counter
    pub fn increment_lock_acquire_success(&self) {
        self.acquire_success.inc();
    }

    /// Increment failed lock acquisition counter
    pub fn increment_lock_acquire_failed(&self) {
        self.acquire_failed.inc();
    }

    /// Increment disabled lock acquisition counter
    pub fn increment_lock_acquire_disabled(&self) {
        self.acquire_disabled.inc();
    }

    /// Increment lock release counter
    pub fn increment_lock_released(&self) {
        self.released.inc();
    }

    /// Increment force release counter
    pub fn increment_lock_force_released(&self) {
        self.force_released.inc();
    }

    /// Increment expired lock reclaimed counter
    pub fn increment_lock_expired_reclaimed(&self) {
        self.expired_reclaimed.inc();
    }

    /// Increment expired lock cleaned up counter
    pub fn increment_lock_expired_cleaned(&self) {
        self.expired_cleaned.inc();
    }

    /// Record lock held duration
    pub fn record_lock_held_duration(&self, duration: std::time::Duration) {
        self.held_duration.observe(duration.as_secs_f64());
    }
}

impl MetricsState {
    /// Create a new metrics state with all metrics registered.
    ///
    /// Returns an error if any metric fails to create or register, allowing the
    /// caller to decide how to handle metrics initialization failure (e.g., log
    /// and continue with degraded functionality rather than crashing the service).
    pub fn new() -> Result<Self, String> {
        let registry = Registry::new();

        // Queue depth gauge
        let queue_depth = IntGauge::with_opts(Opts::new(
            "chimera_queue_depth",
            "Current depth of the priority queue",
        ))
        .map_err(|e| format!("Failed to create queue_depth gauge: {}", e))?;
        registry
            .register(Box::new(queue_depth.clone()))
            .map_err(|e| format!("Failed to register queue_depth: {}", e))?;

        // Trade latency histogram
        let trade_latency = Histogram::with_opts(HistogramOpts::new(
            "chimera_trade_latency_ms",
            "Trade execution latency in milliseconds",
        ))
        .map_err(|e| format!("Failed to create trade_latency histogram: {}", e))?;
        registry
            .register(Box::new(trade_latency.clone()))
            .map_err(|e| format!("Failed to register trade_latency: {}", e))?;

        // RPC health gauge
        let rpc_health = IntGauge::with_opts(Opts::new(
            "chimera_rpc_health",
            "RPC endpoint health (1 = healthy, 0 = unhealthy)",
        ))
        .map_err(|e| format!("Failed to create rpc_health gauge: {}", e))?;
        registry
            .register(Box::new(rpc_health.clone()))
            .map_err(|e| format!("Failed to register rpc_health: {}", e))?;

        // Circuit breaker state gauge
        let circuit_breaker_state = IntGauge::with_opts(Opts::new(
            "chimera_circuit_breaker_state",
            "Circuit breaker state (1 = active, 0 = tripped)",
        ))
        .map_err(|e| format!("Failed to create circuit_breaker_state gauge: {}", e))?;
        registry
            .register(Box::new(circuit_breaker_state.clone()))
            .map_err(|e| format!("Failed to register circuit_breaker_state: {}", e))?;

        // Circuit breaker trips counter
        let circuit_breaker_trips = prometheus::IntCounter::with_opts(Opts::new(
            "chimera_circuit_breaker_trips_total",
            "Total number of circuit breaker trips",
        ))
        .map_err(|e| format!("Failed to create circuit_breaker_trips counter: {}", e))?;
        registry
            .register(Box::new(circuit_breaker_trips.clone()))
            .map_err(|e| format!("Failed to register circuit_breaker_trips: {}", e))?;

        // Active positions count
        let active_positions = IntGauge::with_opts(Opts::new(
            "chimera_active_positions",
            "Number of active positions",
        ))
        .map_err(|e| format!("Failed to create active_positions gauge: {}", e))?;
        registry
            .register(Box::new(active_positions.clone()))
            .map_err(|e| format!("Failed to register active_positions: {}", e))?;

        // Total trades count
        let total_trades = IntGauge::with_opts(Opts::new(
            "chimera_total_trades",
            "Total number of trades processed",
        ))
        .map_err(|e| format!("Failed to create total_trades gauge: {}", e))?;
        registry
            .register(Box::new(total_trades.clone()))
            .map_err(|e| format!("Failed to register total_trades: {}", e))?;

        // RPC latency histogram by endpoint/method. Backed by a process-global so that
        // `timed_rpc` can observe at call sites that do not hold a MetricsState handle;
        // registering the (shared) clone here exposes it for /metrics scraping.
        let rpc_latency = rpc_latency_metric();
        registry
            .register(Box::new(rpc_latency.clone()))
            .map_err(|e| format!("Failed to register rpc_latency: {}", e))?;

        // RPC error counter by endpoint/method (companion to rpc_latency for error rate).
        let rpc_errors = rpc_errors_metric();
        registry
            .register(Box::new(rpc_errors.clone()))
            .map_err(|e| format!("Failed to register rpc_errors: {}", e))?;

        // Reconciliation checked counter
        let reconciliation_checked = prometheus::IntCounter::with_opts(Opts::new(
            "chimera_reconciliation_checked_total",
            "Total number of positions checked during reconciliation",
        ))
        .map_err(|e| format!("Failed to create reconciliation_checked counter: {}", e))?;
        registry
            .register(Box::new(reconciliation_checked.clone()))
            .map_err(|e| format!("Failed to register reconciliation_checked: {}", e))?;

        // Reconciliation discrepancies counter
        let reconciliation_discrepancies = prometheus::IntCounter::with_opts(Opts::new(
            "chimera_reconciliation_discrepancies_total",
            "Total number of reconciliation discrepancies found",
        ))
        .map_err(|e| {
            format!(
                "Failed to create reconciliation_discrepancies counter: {}",
                e
            )
        })?;
        registry
            .register(Box::new(reconciliation_discrepancies.clone()))
            .map_err(|e| format!("Failed to register reconciliation_discrepancies: {}", e))?;

        // Reconciliation unresolved gauge
        let reconciliation_unresolved = IntGauge::with_opts(Opts::new(
            "chimera_reconciliation_unresolved_total",
            "Number of unresolved reconciliation discrepancies",
        ))
        .map_err(|e| format!("Failed to create reconciliation_unresolved gauge: {}", e))?;
        registry
            .register(Box::new(reconciliation_unresolved.clone()))
            .map_err(|e| format!("Failed to register reconciliation_unresolved: {}", e))?;

        // Secret rotation last success timestamp
        let secret_rotation_last_success = IntGauge::with_opts(Opts::new(
            "chimera_secret_rotation_last_success_timestamp",
            "Unix timestamp of last successful secret rotation",
        ))
        .map_err(|e| format!("Failed to create secret_rotation_last_success gauge: {}", e))?;
        registry
            .register(Box::new(secret_rotation_last_success.clone()))
            .map_err(|e| format!("Failed to register secret_rotation_last_success: {}", e))?;

        // Secret rotation days until due
        let secret_rotation_days_until_due = IntGauge::with_opts(Opts::new(
            "chimera_secret_rotation_days_until_due",
            "Number of days until next secret rotation is due",
        ))
        .map_err(|e| {
            format!(
                "Failed to create secret_rotation_days_until_due gauge: {}",
                e
            )
        })?;
        registry
            .register(Box::new(secret_rotation_days_until_due.clone()))
            .map_err(|e| format!("Failed to register secret_rotation_days_until_due: {}", e))?;

        // Cost per trade histogram (by cost type)
        let cost_per_trade = HistogramVec::new(
            HistogramOpts::new("chimera_cost_per_trade_sol", "Cost per trade in SOL"),
            &["cost_type"], // "jito_tip", "dex_fee", "slippage", "total"
        )
        .map_err(|e| format!("Failed to create cost_per_trade histogram: {}", e))?;
        registry
            .register(Box::new(cost_per_trade.clone()))
            .map_err(|e| format!("Failed to register cost_per_trade: {}", e))?;

        // Signal quality score histogram
        let signal_quality_score = Histogram::with_opts(HistogramOpts::new(
            "chimera_signal_quality_score",
            "Signal quality score distribution (0.0-1.0)",
        ))
        .map_err(|e| format!("Failed to create signal_quality_score histogram: {}", e))?;
        registry
            .register(Box::new(signal_quality_score.clone()))
            .map_err(|e| format!("Failed to register signal_quality_score: {}", e))?;

        // Portfolio heat percentage gauge
        let portfolio_heat_percent = Gauge::with_opts(Opts::new(
            "chimera_portfolio_heat_percent",
            "Current portfolio heat as percentage of capital",
        ))
        .map_err(|e| format!("Failed to create portfolio_heat_percent gauge: {}", e))?;
        registry
            .register(Box::new(portfolio_heat_percent.clone()))
            .map_err(|e| format!("Failed to register portfolio_heat_percent: {}", e))?;

        // Rate limiter health gauge for webhook processing
        let webhook_rate_limiter_health = IntGauge::with_opts(Opts::new(
            "chimera_webhook_rate_limiter_health",
            "Webhook rate limiter health (1 = healthy, 0 = degraded)",
        ))
        .map_err(|e| format!("Failed to create webhook_rate_limiter_health gauge: {}", e))?;
        registry
            .register(Box::new(webhook_rate_limiter_health.clone()))
            .map_err(|e| format!("Failed to register webhook_rate_limiter_health: {}", e))?;

        // Rate limiter usage ratio gauge
        let webhook_rate_limiter_usage = Gauge::with_opts(Opts::new(
            "chimera_webhook_rate_limiter_usage",
            "Webhook rate limiter usage ratio (current credits / max credits, 0-1)",
        ))
        .map_err(|e| format!("Failed to create webhook_rate_limiter_usage gauge: {}", e))?;
        registry
            .register(Box::new(webhook_rate_limiter_usage.clone()))
            .map_err(|e| format!("Failed to register webhook_rate_limiter_usage: {}", e))?;

        // Database query latency histogram
        let db_query_latency = Histogram::with_opts(HistogramOpts::new(
            "chimera_db_query_latency_ms",
            "Database query execution latency in milliseconds",
        ))
        .map_err(|e| format!("Failed to create db_query_latency histogram: {}", e))?;
        registry
            .register(Box::new(db_query_latency.clone()))
            .map_err(|e| format!("Failed to register db_query_latency: {}", e))?;

        // Jito bundle submissions counter (by mode: jito, helius, standard)
        let jito_submissions = IntCounterVec::new(
            Opts::new(
                "chimera_jito_submissions_total",
                "Total Jito bundle submissions by mode",
            ),
            &["mode"],
        )
        .map_err(|e| format!("Failed to create jito_submissions counter: {}", e))?;
        registry
            .register(Box::new(jito_submissions.clone()))
            .map_err(|e| format!("Failed to register jito_submissions: {}", e))?;

        // Jito bundle resolutions counter (by status: success, failed)
        let jito_resolutions = IntCounterVec::new(
            Opts::new(
                "chimera_jito_resolutions_total",
                "Total Jito bundle resolutions by status",
            ),
            &["status"],
        )
        .map_err(|e| format!("Failed to create jito_resolutions counter: {}", e))?;
        registry
            .register(Box::new(jito_resolutions.clone()))
            .map_err(|e| format!("Failed to register jito_resolutions: {}", e))?;

        // Jito endpoint health gauge
        let jito_health = IntGauge::with_opts(Opts::new(
            "chimera_jito_health",
            "Jito endpoint health (1 = healthy, 0 = unhealthy)",
        ))
        .map_err(|e| format!("Failed to create jito_health gauge: {}", e))?;
        registry
            .register(Box::new(jito_health.clone()))
            .map_err(|e| format!("Failed to register jito_health: {}", e))?;

        // Jito fallback trigger counter (by reason)
        let jito_fallback_total = IntCounterVec::new(
            Opts::new(
                "chimera_jito_fallback_total",
                "Total Jito fallback triggers by reason",
            ),
            &["reason"],
        )
        .map_err(|e| format!("Failed to create jito_fallback_total counter: {}", e))?;
        registry
            .register(Box::new(jito_fallback_total.clone()))
            .map_err(|e| format!("Failed to register jito_fallback: {}", e))?;

        // Jito retry counter (by attempt number)
        let jito_retry_total = IntCounterVec::new(
            Opts::new(
                "chimera_jito_retry_total",
                "Total Jito retry attempts by attempt number",
            ),
            &["attempt"],
        )
        .map_err(|e| format!("Failed to create jito_retry_total counter: {}", e))?;
        registry
            .register(Box::new(jito_retry_total.clone()))
            .map_err(|e| format!("Failed to register jito_retry: {}", e))?;

        Ok(Self {
            registry,
            queue_depth,
            trade_latency,
            rpc_health,
            circuit_breaker_state,
            circuit_breaker_trips,
            active_positions,
            total_trades,
            rpc_latency,
            reconciliation_checked,
            reconciliation_discrepancies,
            reconciliation_unresolved,
            secret_rotation_last_success,
            secret_rotation_days_until_due,
            cost_per_trade,
            signal_quality_score,
            portfolio_heat_percent,
            webhook_rate_limiter_health,
            webhook_rate_limiter_usage,
            db_query_latency,
            jito_submissions,
            jito_resolutions,
            jito_health,
            jito_fallback_total,
            jito_retry_total,
        })
    }

    /// Get the Prometheus registry
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Get database query latency statistics
    pub fn get_db_query_stats(&self) -> QueryLatencyStats {
        // Gather the histogram metric
        let metric_families = self.registry.gather();
        let mut sample_count = 0u32;
        let mut sum_ms = 0.0;
        let mut slow_queries_count = 0u32;
        const SLOW_QUERY_THRESHOLD_MS: f64 = 100.0; // Queries > 100ms considered slow

        for metric_family in metric_families {
            if metric_family.name() == "chimera_db_query_latency_ms" {
                if let Some(metric) = metric_family.get_metric().first() {
                    let histogram = metric.get_histogram();
                    if !histogram.bucket.is_empty() {
                        sample_count = histogram.get_sample_count() as u32;
                        sum_ms = histogram.get_sample_sum();

                        // Count slow queries (> 100ms) from histogram buckets
                        for bucket in histogram.bucket.iter() {
                            if bucket.upper_bound() >= SLOW_QUERY_THRESHOLD_MS {
                                slow_queries_count += bucket.cumulative_count() as u32;
                                break; // This gives us count of queries >= threshold
                            }
                        }
                    }
                }
            }
        }

        // Calculate approximate percentiles from the histogram buckets
        let avg_ms = if sample_count > 0 { sum_ms / sample_count as f64 } else { 0.0 };

        // For now, use estimated percentiles based on avg (this could be improved with proper percentile calculation)
        let p95_ms = if sample_count > 0 { avg_ms * 3.0 } else { 0.0 };
        let p99_ms = if sample_count > 0 { avg_ms * 5.0 } else { 0.0 };

        QueryLatencyStats {
            avg_ms,
            p95_ms,
            p99_ms,
            slow_queries_count,
            total_queries_count: sample_count,
        }
    }
}

impl Default for MetricsState {
    fn default() -> Self {
        // In production code, MetricsState::new() should be called and its Result handled.
        // For Default trait (used in tests), we panic on failure to maintain the trait contract.
        Self::new().expect("Failed to create MetricsState - metrics system initialization failed")
    }
}

/// Query latency statistics for API responses
#[derive(Debug, Clone, serde::Serialize)]
pub struct QueryLatencyStats {
    pub avg_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub slow_queries_count: u32,
    pub total_queries_count: u32,
}

/// Metrics handler - returns Prometheus metrics in text format
///
/// GET /metrics
pub async fn metrics_handler(State(state): State<Arc<MetricsState>>) -> impl IntoResponse {
    let encoder = TextEncoder::new();
    let metric_families = state.registry().gather();
    let mut buffer = Vec::new();

    // If encoding fails, return a 500 error instead of panicking
    match encoder.encode(&metric_families, &mut buffer) {
        Ok(_) => (
            StatusCode::OK,
            [("Content-Type", "text/plain; version=0.0.4")],
            buffer,
        ),
        Err(e) => {
            tracing::error!(error = %e, "Failed to encode metrics");
            let error_body = format!("Failed to encode metrics: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                [("Content-Type", "text/plain")],
                error_body.into_bytes(),
            )
        }
    }
}

/// Create metrics router
pub fn metrics_router() -> Router<Arc<MetricsState>> {
    Router::new().route("/metrics", get(metrics_handler))
}

// =============================================================================
// RPC latency instrumentation
// =============================================================================

/// Bucket bounds (ms) for the RPC latency histogram. RPC calls range from a few
/// milliseconds (blockhash) to seconds (getTransaction), so bounds span 1ms..10s.
const RPC_LATENCY_BUCKETS_MS: &[f64] =
    &[1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0, 10000.0];

// Process-global metrics so RPC call sites can observe latency without holding a
// MetricsState handle. MetricsState registers clones of these for /metrics scraping.
static RPC_LATENCY: std::sync::OnceLock<HistogramVec> = std::sync::OnceLock::new();
static RPC_ERRORS: std::sync::OnceLock<IntCounterVec> = std::sync::OnceLock::new();

/// The process-global RPC latency histogram (labels: `endpoint`, `method`).
/// Clones share the same underlying collectors, so observing here is visible to any
/// registry that registered a clone.
pub fn rpc_latency_metric() -> HistogramVec {
    RPC_LATENCY
        .get_or_init(|| {
            HistogramVec::new(
                HistogramOpts::new("chimera_rpc_latency_ms", "RPC call latency in milliseconds")
                    .buckets(RPC_LATENCY_BUCKETS_MS.to_vec()),
                &["endpoint", "method"],
            )
            .expect("chimera_rpc_latency_ms is a valid HistogramVec")
        })
        .clone()
}

/// The process-global RPC error counter (labels: `endpoint`, `method`).
pub fn rpc_errors_metric() -> IntCounterVec {
    RPC_ERRORS
        .get_or_init(|| {
            IntCounterVec::new(
                Opts::new(
                    "chimera_rpc_errors_total",
                    "Total RPC call errors by endpoint and method",
                ),
                &["endpoint", "method"],
            )
            .expect("chimera_rpc_errors_total is a valid IntCounterVec")
        })
        .clone()
}

/// Time an RPC call future, recording its latency to `chimera_rpc_latency_ms` (and an
/// error to `chimera_rpc_errors_total` on `Err`). Records on BOTH success and error
/// paths. `endpoint` is a coarse endpoint category (e.g. "primary", "polling", "jito");
/// `method` is the Solana RPC method name (e.g. "getLatestBlockhash").
///
/// Wrap a call site like:
/// ```ignore
/// let blockhash = timed_rpc("primary", "getLatestBlockhash", async {
///     self.rpc_client.get_latest_blockhash().await
/// }).await.map_err(|e| ...)?;
/// ```
pub async fn timed_rpc<F, T, E>(endpoint: &str, method: &str, fut: F) -> Result<T, E>
where
    F: std::future::Future<Output = Result<T, E>>,
{
    let start = std::time::Instant::now();
    let result = fut.await;
    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    rpc_latency_metric()
        .with_label_values(&[endpoint, method])
        .observe(elapsed_ms);
    if result.is_err() {
        rpc_errors_metric().with_label_values(&[endpoint, method]).inc();
    }
    result
}

/// Prometheus-style histogram quantile estimation via linear interpolation between
/// cumulative bucket boundaries (the `histogram_quantile()` algorithm). `q` is in
/// `[0, 1]`. Returns `0.0` when there are no samples, and the highest finite bucket
/// bound when the quantile falls in the `+Inf` overflow bucket.
pub fn histogram_quantile(hist: &prometheus::proto::Histogram, q: f64) -> f64 {
    let total = hist.get_sample_count();
    let bounds: Vec<f64> = hist.get_bucket().iter().map(|b| b.upper_bound()).collect();
    let cum: Vec<u64> = hist
        .get_bucket()
        .iter()
        .map(|b| b.cumulative_count())
        .collect();
    quantile_from_buckets(&bounds, &cum, total, q)
}

/// Core quantile estimator operating on raw bucket data (upper bounds + cumulative
/// counts), shared by the single-histogram and merged-histogram paths. Exposed so
/// `get_rpc_latency` can compute overall quantiles across multiple (endpoint, method)
/// children without reconstructing a proto `Histogram`.
pub fn quantile_from_buckets(bounds: &[f64], cum: &[u64], total_count: u64, q: f64) -> f64 {
    if total_count == 0 {
        return 0.0;
    }
    let target = q * total_count as f64;

    let mut prev_count: u64 = 0;
    let mut prev_bound: f64 = 0.0;
    for (upper, &cum_i) in bounds.iter().zip(cum.iter()) {
        if (cum_i as f64) >= target {
            // Quantile falls in this bucket. If the bucket is +Inf (overflow) we cannot
            // interpolate beyond the last finite bound, so report it.
            if !upper.is_finite() {
                return prev_bound;
            }
            let bucket_count = cum_i.saturating_sub(prev_count);
            if bucket_count == 0 {
                return *upper;
            }
            return prev_bound + (*upper - prev_bound) * (target - prev_count as f64)
                / bucket_count as f64;
        }
        prev_count = cum_i;
        if upper.is_finite() {
            prev_bound = *upper;
        }
    }
    // Target beyond all buckets (shouldn't happen since +Inf bucket holds everything).
    prev_bound
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_state_creation() {
        let state = MetricsState::new().expect("Failed to create metrics state for test");
        assert_eq!(state.queue_depth.get(), 0);
        assert_eq!(state.rpc_health.get(), 0);
        assert_eq!(state.circuit_breaker_state.get(), 0);
        assert_eq!(state.circuit_breaker_trips.get(), 0);
    }

    #[test]
    fn test_metrics_update() {
        let state = MetricsState::new().expect("Failed to create metrics state for test");
        state.queue_depth.set(42);
        assert_eq!(state.queue_depth.get(), 42);

        state.rpc_health.set(1);
        assert_eq!(state.rpc_health.get(), 1);
    }

    #[test]
    fn test_quantile_from_buckets() {
        // bounds [10, 50, 100, +Inf], cumulative [10, 60, 100, 100], total 100.
        let bounds = vec![10.0, 50.0, 100.0, f64::INFINITY];
        let cum = vec![10u64, 60, 100, 100];
        let total = 100u64;

        // median: target 50 → bucket [10,50), interpolate: 10 + 40*(50-10)/50 = 42
        assert!((quantile_from_buckets(&bounds, &cum, total, 0.5) - 42.0).abs() < 1e-9);
        // q=0.1: target 10 → bucket [0,10), interpolate: 0 + 10*(10-0)/10 = 10
        assert!((quantile_from_buckets(&bounds, &cum, total, 0.1) - 10.0).abs() < 1e-9);
        // q=0.95: target 95 → bucket [50,100), interpolate: 50 + 50*(95-60)/40 = 93.75
        assert!((quantile_from_buckets(&bounds, &cum, total, 0.95) - 93.75).abs() < 1e-9);
        // q=1.0: target 100 → top of [50,100) bucket = 100
        assert!((quantile_from_buckets(&bounds, &cum, total, 1.0) - 100.0).abs() < 1e-9);

        // Overflow into +Inf bucket reports the highest finite bound (10).
        let bounds_inf = vec![10.0, f64::INFINITY];
        let cum_inf = vec![5u64, 100];
        assert!((quantile_from_buckets(&bounds_inf, &cum_inf, 100, 0.5) - 10.0).abs() < 1e-9);

        // No samples.
        assert_eq!(quantile_from_buckets(&[], &[], 0, 0.5), 0.0);
    }

    #[tokio::test]
    async fn test_timed_rpc_records_latency_and_errors() {
        let registry = Registry::new();
        registry
            .register(Box::new(rpc_latency_metric()))
            .expect("register rpc_latency");
        registry
            .register(Box::new(rpc_errors_metric()))
            .expect("register rpc_errors");

        // Unique (endpoint, method) so this test's observations are isolated from other
        // tests that share the process-global metrics.
        let endpoint = "test_ep";
        let method = "test_timed_rpc_records_latency_and_errors";

        let ok: Result<u32, String> = timed_rpc(endpoint, method, async { Ok(42) }).await;
        assert_eq!(ok.unwrap(), 42);
        let err: Result<u32, String> = timed_rpc(endpoint, method, async { Err("boom".into()) }).await;
        assert!(err.is_err());

        let mut latency_count = 0u64;
        let mut error_value = 0.0f64;
        for fam in registry.gather() {
            if fam.name() == "chimera_rpc_latency_ms" {
                for m in fam.get_metric() {
                    if label_eq(m, "endpoint", endpoint) && label_eq(m, "method", method) {
                        latency_count = m.get_histogram().get_sample_count();
                    }
                }
            } else if fam.name() == "chimera_rpc_errors_total" {
                for m in fam.get_metric() {
                    if label_eq(m, "endpoint", endpoint) && label_eq(m, "method", method) {
                        error_value = m.get_counter().value();
                    }
                }
            }
        }
        assert_eq!(latency_count, 2, "both calls recorded");
        assert!(
            (error_value - 1.0).abs() < 1e-9,
            "exactly one error recorded"
        );
    }

    fn label_eq(m: &prometheus::proto::Metric, name: &str, value: &str) -> bool {
        m.get_label()
            .iter()
            .any(|l| l.name() == name && l.value() == value)
    }
}
