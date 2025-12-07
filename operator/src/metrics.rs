//! Prometheus metrics for Chimera Operator
//!
//! Exposes metrics endpoint for monitoring:
//! - Queue depth gauge
//! - Trade latency histogram
//! - RPC health metrics
//! - Circuit breaker state gauge
//! - Position count gauge

use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Router,
};
use prometheus::{
    Encoder, Histogram, HistogramOpts, HistogramVec, IntGauge, Opts, Registry, TextEncoder,
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
    /// Circuit breaker state gauge (1 = active, 0 = tripped)
    pub circuit_breaker_state: IntGauge,
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
}

impl MetricsState {
    /// Create a new metrics state with all metrics registered
    pub fn new() -> Self {
        let registry = Registry::new();

        // Queue depth gauge
        let queue_depth = IntGauge::with_opts(Opts::new(
            "chimera_queue_depth",
            "Current depth of the priority queue",
        ))
        .expect("Failed to create queue_depth gauge");
        registry
            .register(Box::new(queue_depth.clone()))
            .expect("Failed to register queue_depth");

        // Trade latency histogram
        let trade_latency = Histogram::with_opts(HistogramOpts::new(
            "chimera_trade_latency_ms",
            "Trade execution latency in milliseconds",
        ))
        .expect("Failed to create trade_latency histogram");
        registry
            .register(Box::new(trade_latency.clone()))
            .expect("Failed to register trade_latency");

        // RPC health gauge
        let rpc_health = IntGauge::with_opts(Opts::new(
            "chimera_rpc_health",
            "RPC endpoint health (1 = healthy, 0 = unhealthy)",
        ))
        .expect("Failed to create rpc_health gauge");
        registry
            .register(Box::new(rpc_health.clone()))
            .expect("Failed to register rpc_health");

        // Circuit breaker state gauge
        let circuit_breaker_state = IntGauge::with_opts(Opts::new(
            "chimera_circuit_breaker_state",
            "Circuit breaker state (1 = active, 0 = tripped)",
        ))
        .expect("Failed to create circuit_breaker_state gauge");
        registry
            .register(Box::new(circuit_breaker_state.clone()))
            .expect("Failed to register circuit_breaker_state");

        // Active positions count
        let active_positions = IntGauge::with_opts(Opts::new(
            "chimera_active_positions",
            "Number of active positions",
        ))
        .expect("Failed to create active_positions gauge");
        registry
            .register(Box::new(active_positions.clone()))
            .expect("Failed to register active_positions");

        // Total trades count
        let total_trades = IntGauge::with_opts(Opts::new(
            "chimera_total_trades",
            "Total number of trades processed",
        ))
        .expect("Failed to create total_trades gauge");
        registry
            .register(Box::new(total_trades.clone()))
            .expect("Failed to register total_trades");

        // RPC latency histogram by endpoint
        let rpc_latency = HistogramVec::new(
            HistogramOpts::new(
                "chimera_rpc_latency_ms",
                "RPC call latency in milliseconds",
            ),
            &["endpoint", "method"],
        )
        .expect("Failed to create rpc_latency histogram");
        registry
            .register(Box::new(rpc_latency.clone()))
            .expect("Failed to register rpc_latency");

        // Reconciliation checked counter
        let reconciliation_checked = prometheus::IntCounter::with_opts(Opts::new(
            "chimera_reconciliation_checked_total",
            "Total number of positions checked during reconciliation",
        ))
        .expect("Failed to create reconciliation_checked counter");
        registry
            .register(Box::new(reconciliation_checked.clone()))
            .expect("Failed to register reconciliation_checked");

        // Reconciliation discrepancies counter
        let reconciliation_discrepancies = prometheus::IntCounter::with_opts(Opts::new(
            "chimera_reconciliation_discrepancies_total",
            "Total number of reconciliation discrepancies found",
        ))
        .expect("Failed to create reconciliation_discrepancies counter");
        registry
            .register(Box::new(reconciliation_discrepancies.clone()))
            .expect("Failed to register reconciliation_discrepancies");

        // Reconciliation unresolved gauge
        let reconciliation_unresolved = IntGauge::with_opts(Opts::new(
            "chimera_reconciliation_unresolved_total",
            "Number of unresolved reconciliation discrepancies",
        ))
        .expect("Failed to create reconciliation_unresolved gauge");
        registry
            .register(Box::new(reconciliation_unresolved.clone()))
            .expect("Failed to register reconciliation_unresolved");

        // Secret rotation last success timestamp
        let secret_rotation_last_success = IntGauge::with_opts(Opts::new(
            "chimera_secret_rotation_last_success_timestamp",
            "Unix timestamp of last successful secret rotation",
        ))
        .expect("Failed to create secret_rotation_last_success gauge");
        registry
            .register(Box::new(secret_rotation_last_success.clone()))
            .expect("Failed to register secret_rotation_last_success");

        // Secret rotation days until due
        let secret_rotation_days_until_due = IntGauge::with_opts(Opts::new(
            "chimera_secret_rotation_days_until_due",
            "Number of days until next secret rotation is due",
        ))
        .expect("Failed to create secret_rotation_days_until_due gauge");
        registry
            .register(Box::new(secret_rotation_days_until_due.clone()))
            .expect("Failed to register secret_rotation_days_until_due");

        Self {
            registry,
            queue_depth,
            trade_latency,
            rpc_health,
            circuit_breaker_state,
            active_positions,
            total_trades,
            rpc_latency,
            reconciliation_checked,
            reconciliation_discrepancies,
            reconciliation_unresolved,
            secret_rotation_last_success,
            secret_rotation_days_until_due,
        }
    }

    /// Get the Prometheus registry
    pub fn registry(&self) -> &Registry {
        &self.registry
    }
}

impl Default for MetricsState {
    fn default() -> Self {
        Self::new()
    }
}

/// Metrics handler - returns Prometheus metrics in text format
///
/// GET /metrics
pub async fn metrics_handler(State(state): State<Arc<MetricsState>>) -> impl IntoResponse {
    let encoder = TextEncoder::new();
    let metric_families = state.registry().gather();
    let mut buffer = Vec::new();

    encoder
        .encode(&metric_families, &mut buffer)
        .expect("Failed to encode metrics");

    (
        StatusCode::OK,
        [("Content-Type", "text/plain; version=0.0.4")],
        buffer,
    )
}

/// Create metrics router
pub fn metrics_router() -> Router<Arc<MetricsState>> {
    Router::new().route("/metrics", get(metrics_handler))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_state_creation() {
        let state = MetricsState::new();
        assert_eq!(state.queue_depth.get(), 0);
        assert_eq!(state.rpc_health.get(), 0);
        assert_eq!(state.circuit_breaker_state.get(), 0);
    }

    #[test]
    fn test_metrics_update() {
        let state = MetricsState::new();
        state.queue_depth.set(42);
        assert_eq!(state.queue_depth.get(), 42);
        
        state.rpc_health.set(1);
        assert_eq!(state.rpc_health.get(), 1);
    }
}
