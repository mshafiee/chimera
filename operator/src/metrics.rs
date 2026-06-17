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
    Encoder, Gauge, Histogram, HistogramOpts, HistogramVec, IntGauge, Opts, Registry, TextEncoder,
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

        // RPC latency histogram by endpoint
        let rpc_latency = HistogramVec::new(
            HistogramOpts::new("chimera_rpc_latency_ms", "RPC call latency in milliseconds"),
            &["endpoint", "method"],
        )
        .map_err(|e| format!("Failed to create rpc_latency histogram: {}", e))?;
        registry
            .register(Box::new(rpc_latency.clone()))
            .map_err(|e| format!("Failed to register rpc_latency: {}", e))?;

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
        .map_err(|e| format!("Failed to create reconciliation_discrepancies counter: {}", e))?;
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
        .map_err(|e| format!("Failed to create secret_rotation_days_until_due gauge: {}", e))?;
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

        Ok(Self {
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
            cost_per_trade,
            signal_quality_score,
            portfolio_heat_percent,
            webhook_rate_limiter_health,
            webhook_rate_limiter_usage,
        })
    }

    /// Get the Prometheus registry
    pub fn registry(&self) -> &Registry {
        &self.registry
    }
}

impl Default for MetricsState {
    fn default() -> Self {
        // In production code, MetricsState::new() should be called and its Result handled.
        // For Default trait (used in tests), we panic on failure to maintain the trait contract.
        Self::new().expect("Failed to create MetricsState - metrics system initialization failed")
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_state_creation() {
        let state = MetricsState::new().expect("Failed to create metrics state for test");
        assert_eq!(state.queue_depth.get(), 0);
        assert_eq!(state.rpc_health.get(), 0);
        assert_eq!(state.circuit_breaker_state.get(), 0);
    }

    #[test]
    fn test_metrics_update() {
        let state = MetricsState::new().expect("Failed to create metrics state for test");
        state.queue_depth.set(42);
        assert_eq!(state.queue_depth.get(), 42);

        state.rpc_health.set(1);
        assert_eq!(state.rpc_health.get(), 1);
    }
}
