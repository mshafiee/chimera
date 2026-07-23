//! Jupiter API Monitoring Metrics
//!
//! Comprehensive monitoring for Jupiter API integration including:
//! - API health metrics
//! - Response time tracking
//! - Error rate monitoring
//! - Quota usage tracking
//! - Performance indicators

use prometheus::{
    Gauge, Histogram, IntCounter, IntGauge, Registry,
};
use std::sync::Arc;
use parking_lot::RwLock;
use std::collections::HashMap;

/// Jupiter API metrics collector
#[derive(Clone)]
pub struct JupiterMetrics {
    /// Registry for all metrics
    registry: Arc<Registry>,

    // Request metrics
    /// Total Jupiter API requests
    jupiter_requests_total: IntCounter,
    /// Successful Jupiter API requests
    jupiter_requests_success_total: IntCounter,
    /// Failed Jupiter API requests
    jupiter_requests_failed_total: IntCounter,

    // Response time metrics
    /// Jupiter API request duration in seconds
    jupiter_request_duration_seconds: Histogram,
    /// Jupiter API request duration by endpoint
    jupiter_request_duration_by_endpoint_seconds: Histogram,

    // Error metrics
    /// Jupiter API errors by type
    jupiter_errors_total: IntCounter,
    /// Jupiter API rate limit errors
    jupiter_rate_limit_errors_total: IntCounter,
    /// Jupiter API timeout errors
    jupiter_timeout_errors_total: IntCounter,

    // Circuit breaker metrics
    /// Jupiter API circuit breaker trips
    jupiter_circuit_breaker_trips_total: IntCounter,
    /// Jupiter API consecutive failures
    jupiter_consecutive_failures: IntGauge,

    // Connection pool metrics
    /// Active Jupiter API connections
    jupiter_active_connections: IntGauge,
    /// Idle Jupiter API connections
    jupiter_idle_connections: IntGauge,
    /// Connection reuse count
    jupiter_connection_reuses_total: IntCounter,

    // Performance metrics
    /// Average Jupiter API response time (ms)
    jupiter_avg_response_time_ms: Gauge,
    /// Jupiter API p95 response time (ms)
    jupiter_p95_response_time_ms: Gauge,
    /// Jupiter API p99 response time (ms)
    jupiter_p99_response_time_ms: Gauge,

    // Feature usage metrics
    /// RTSE usage count
    jupiter_rtse_usage_total: IntCounter,
    /// Jupiter Beam usage count
    jupiter_jupiter_beam_usage_total: IntCounter,
    /// Gasless swap usage count
    jupiter_gasless_swap_usage_total: IntCounter,

    // Version metrics
    /// Jupiter API v2 usage
    jupiter_v2_usage_total: IntCounter,
    /// Jupiter API v1 usage (deprecated)
    jupiter_v1_usage_total: IntCounter,

    // Cache metrics
    /// Jupiter DEX route cache hits
    jupiter_route_cache_hits_total: IntCounter,
    /// Jupiter DEX route cache misses
    jupiter_route_cache_misses_total: IntCounter,

    // Response size metrics
    /// Jupiter API response size in bytes
    jupiter_response_size_bytes: Histogram,

    // Quota metrics
    /// Jupiter API quota usage percentage
    jupiter_quota_usage_percent: Gauge,
    /// Jupiter API remaining quota
    jupiter_quota_remaining: IntGauge,

    /// Custom metrics registry
    custom_metrics: Arc<RwLock<HashMap<String, f64>>>,
}

impl JupiterMetrics {
    /// Create new Jupiter metrics
    pub fn new() -> Self {
        let registry = Registry::new();

        // Request metrics
        let jupiter_requests_total = IntCounter::new(
            "jupiter_requests_total",
            "Total Jupiter API requests"
        ).unwrap();
        let jupiter_requests_success_total = IntCounter::new(
            "jupiter_requests_success_total",
            "Successful Jupiter API requests"
        ).unwrap();
        let jupiter_requests_failed_total = IntCounter::new(
            "jupiter_requests_failed_total",
            "Failed Jupiter API requests"
        ).unwrap();

        // Response time metrics
        let jupiter_request_duration_seconds = Histogram::with_opts(
            prometheus::HistogramOpts::new(
                "jupiter_request_duration_seconds",
                "Jupiter API request duration in seconds"
            ).buckets(vec![0.01, 0.05, 0.1, 0.5, 1.0, 2.0, 5.0, 10.0]),
        ).unwrap();

        let jupiter_request_duration_by_endpoint_seconds = Histogram::with_opts(
            prometheus::HistogramOpts::new(
                "jupiter_request_duration_by_endpoint_seconds",
                "Jupiter API request duration by endpoint"
            ).buckets(vec![0.01, 0.05, 0.1, 0.5, 1.0, 2.0, 5.0, 10.0])
                .const_label("endpoint", "unknown"),
        ).unwrap();

        // Error metrics
        let jupiter_errors_total = IntCounter::new(
            "jupiter_errors_total",
            "Jupiter API errors by type"
        ).unwrap();
        let jupiter_rate_limit_errors_total = IntCounter::new(
            "jupiter_rate_limit_errors_total",
            "Jupiter API rate limit errors"
        ).unwrap();
        let jupiter_timeout_errors_total = IntCounter::new(
            "jupiter_timeout_errors_total",
            "Jupiter API timeout errors"
        ).unwrap();

        // Circuit breaker metrics
        let jupiter_circuit_breaker_trips_total = IntCounter::new(
            "jupiter_circuit_breaker_trips_total",
            "Jupiter API circuit breaker trips"
        ).unwrap();
        let jupiter_consecutive_failures = IntGauge::new(
            "jupiter_consecutive_failures",
            "Jupiter API consecutive failures"
        ).unwrap();

        // Connection pool metrics
        let jupiter_active_connections = IntGauge::new(
            "jupiter_active_connections",
            "Active Jupiter API connections"
        ).unwrap();
        let jupiter_idle_connections = IntGauge::new(
            "jupiter_idle_connections",
            "Idle Jupiter API connections"
        ).unwrap();
        let jupiter_connection_reuses_total = IntCounter::new(
            "jupiter_connection_reuses_total",
            "Jupiter API connection reuses"
        ).unwrap();

        // Performance metrics
        let jupiter_avg_response_time_ms = Gauge::new(
            "jupiter_avg_response_time_ms",
            "Average Jupiter API response time in milliseconds"
        ).unwrap();
        let jupiter_p95_response_time_ms = Gauge::new(
            "jupiter_p95_response_time_ms",
            "Jupiter API p95 response time in milliseconds"
        ).unwrap();
        let jupiter_p99_response_time_ms = Gauge::new(
            "jupiter_p99_response_time_ms",
            "Jupiter API p99 response time in milliseconds"
        ).unwrap();

        // Feature usage metrics
        let jupiter_rtse_usage_total = IntCounter::new(
            "jupiter_rtse_usage_total",
            "RTSE usage count"
        ).unwrap();
        let jupiter_jupiter_beam_usage_total = IntCounter::new(
            "jupiter_jupiter_beam_usage_total",
            "Jupiter Beam usage count"
        ).unwrap();
        let jupiter_gasless_swap_usage_total = IntCounter::new(
            "jupiter_gasless_swap_usage_total",
            "Gasless swap usage count"
        ).unwrap();

        // Version metrics
        let jupiter_v2_usage_total = IntCounter::new(
            "jupiter_v2_usage_total",
            "Jupiter API v2 usage"
        ).unwrap();
        let jupiter_v1_usage_total = IntCounter::new(
            "jupiter_v1_usage_total",
            "Jupiter API v1 usage (deprecated)"
        ).unwrap();

        // Cache metrics
        let jupiter_route_cache_hits_total = IntCounter::new(
            "jupiter_route_cache_hits_total",
            "Jupiter route cache hits"
        ).unwrap();
        let jupiter_route_cache_misses_total = IntCounter::new(
            "jupiter_route_cache_misses_total",
            "Jupiter route cache misses"
        ).unwrap();

        // Response size metrics
        let jupiter_response_size_bytes = Histogram::with_opts(
            prometheus::HistogramOpts::new(
                "jupiter_response_size_bytes",
                "Jupiter API response size in bytes"
            ).buckets(vec![100.0, 1000.0, 10000.0, 100000.0, 1000000.0]),
        ).unwrap();

        // Quota metrics
        let jupiter_quota_usage_percent = Gauge::new(
            "jupiter_quota_usage_percent",
            "Jupiter API quota usage percentage"
        ).unwrap();
        let jupiter_quota_remaining = IntGauge::new(
            "jupiter_quota_remaining",
            "Jupiter API remaining quota"
        ).unwrap();

        // Register metrics
        registry.register(Box::new(jupiter_requests_total.clone())).unwrap();
        registry.register(Box::new(jupiter_requests_success_total.clone())).unwrap();
        registry.register(Box::new(jupiter_requests_failed_total.clone())).unwrap();
        registry.register(Box::new(jupiter_request_duration_seconds.clone())).unwrap();
        registry.register(Box::new(jupiter_request_duration_by_endpoint_seconds.clone())).unwrap();
        registry.register(Box::new(jupiter_errors_total.clone())).unwrap();
        registry.register(Box::new(jupiter_rate_limit_errors_total.clone())).unwrap();
        registry.register(Box::new(jupiter_timeout_errors_total.clone())).unwrap();
        registry.register(Box::new(jupiter_circuit_breaker_trips_total.clone())).unwrap();
        registry.register(Box::new(jupiter_consecutive_failures.clone())).unwrap();
        registry.register(Box::new(jupiter_active_connections.clone())).unwrap();
        registry.register(Box::new(jupiter_idle_connections.clone())).unwrap();
        registry.register(Box::new(jupiter_connection_reuses_total.clone())).unwrap();
        registry.register(Box::new(jupiter_avg_response_time_ms.clone())).unwrap();
        registry.register(Box::new(jupiter_p95_response_time_ms.clone())).unwrap();
        registry.register(Box::new(jupiter_p99_response_time_ms.clone())).unwrap();
        registry.register(Box::new(jupiter_rtse_usage_total.clone())).unwrap();
        registry.register(Box::new(jupiter_jupiter_beam_usage_total.clone())).unwrap();
        registry.register(Box::new(jupiter_gasless_swap_usage_total.clone())).unwrap();
        registry.register(Box::new(jupiter_v2_usage_total.clone())).unwrap();
        registry.register(Box::new(jupiter_v1_usage_total.clone())).unwrap();
        registry.register(Box::new(jupiter_route_cache_hits_total.clone())).unwrap();
        registry.register(Box::new(jupiter_route_cache_misses_total.clone())).unwrap();
        registry.register(Box::new(jupiter_response_size_bytes.clone())).unwrap();
        registry.register(Box::new(jupiter_quota_usage_percent.clone())).unwrap();
        registry.register(Box::new(jupiter_quota_remaining.clone())).unwrap();

        Self {
            registry: Arc::new(registry),
            jupiter_requests_total,
            jupiter_requests_success_total,
            jupiter_requests_failed_total,
            jupiter_request_duration_seconds,
            jupiter_request_duration_by_endpoint_seconds,
            jupiter_errors_total,
            jupiter_rate_limit_errors_total,
            jupiter_timeout_errors_total,
            jupiter_circuit_breaker_trips_total,
            jupiter_consecutive_failures,
            jupiter_active_connections,
            jupiter_idle_connections,
            jupiter_connection_reuses_total,
            jupiter_avg_response_time_ms,
            jupiter_p95_response_time_ms,
            jupiter_p99_response_time_ms,
            jupiter_rtse_usage_total,
            jupiter_jupiter_beam_usage_total,
            jupiter_gasless_swap_usage_total,
            jupiter_v2_usage_total,
            jupiter_v1_usage_total,
            jupiter_route_cache_hits_total,
            jupiter_route_cache_misses_total,
            jupiter_response_size_bytes,
            jupiter_quota_usage_percent,
            jupiter_quota_remaining,
            custom_metrics: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Record a Jupiter API request
    pub fn record_request(&self, _endpoint: &str, duration_secs: f64, success: bool) {
        self.jupiter_requests_total.inc();

        if success {
            self.jupiter_requests_success_total.inc();
        } else {
            self.jupiter_requests_failed_total.inc();
        }

        self.jupiter_request_duration_seconds.observe(duration_secs);
    }

    /// Record a Jupiter API error
    pub fn record_error(&self, error_type: &str) {
        self.jupiter_errors_total.inc();

        match error_type {
            "rate_limit" => self.jupiter_rate_limit_errors_total.inc(),
            "timeout" => self.jupiter_timeout_errors_total.inc(),
            _ => {}
        }
    }

    /// Record circuit breaker trip
    pub fn record_circuit_breaker_trip(&self) {
        self.jupiter_circuit_breaker_trips_total.inc();
    }

    /// Update consecutive failures count
    pub fn update_consecutive_failures(&self, count: u32) {
        self.jupiter_consecutive_failures.set(count as i64);
    }

    /// Update connection pool metrics
    pub fn update_connection_metrics(&self, active: u32, idle: u32) {
        self.jupiter_active_connections.set(active as i64);
        self.jupiter_idle_connections.set(idle as i64);
    }

    /// Record connection reuse
    pub fn record_connection_reuse(&self) {
        self.jupiter_connection_reuses_total.inc();
    }

    /// Update performance metrics
    pub fn update_performance_metrics(&self, avg_ms: f64, p95_ms: f64, p99_ms: f64) {
        self.jupiter_avg_response_time_ms.set(avg_ms);
        self.jupiter_p95_response_time_ms.set(p95_ms);
        self.jupiter_p99_response_time_ms.set(p99_ms);
    }

    /// Record feature usage
    pub fn record_feature_usage(&self, feature: &str) {
        match feature {
            "rtse" => self.jupiter_rtse_usage_total.inc(),
            "jupiter_beam" => self.jupiter_jupiter_beam_usage_total.inc(),
            "gasless" => self.jupiter_gasless_swap_usage_total.inc(),
            _ => {}
        }
    }

    /// Record API version usage
    pub fn record_version_usage(&self, version: &str) {
        match version {
            "v2" => self.jupiter_v2_usage_total.inc(),
            "v1" => self.jupiter_v1_usage_total.inc(),
            _ => {}
        }
    }

    /// Record cache metrics
    pub fn record_cache_hit(&self) {
        self.jupiter_route_cache_hits_total.inc();
    }

    pub fn record_cache_miss(&self) {
        self.jupiter_route_cache_misses_total.inc();
    }

    /// Record response size
    pub fn record_response_size(&self, size_bytes: f64) {
        self.jupiter_response_size_bytes.observe(size_bytes);
    }

    /// Update quota metrics
    pub fn update_quota_metrics(&self, usage_percent: f64, remaining: u32) {
        self.jupiter_quota_usage_percent.set(usage_percent);
        self.jupiter_quota_remaining.set(remaining as i64);
    }

    /// Set custom metric
    pub fn set_custom_metric(&self, name: &str, value: f64) {
        let mut metrics = self.custom_metrics.write();
        metrics.insert(name.to_string(), value);
    }

    /// Get custom metric
    pub fn get_custom_metric(&self, name: &str) -> Option<f64> {
        let metrics = self.custom_metrics.read();
        metrics.get(name).copied()
    }

    /// Get the Prometheus registry
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Export metrics in Prometheus text format
    pub fn export_metrics(&self) -> String {
        use prometheus::Encoder;

        let mut buffer = Vec::new();
        let encoder = prometheus::TextEncoder::new();

        let result = encoder.encode(&self.registry.gather(), &mut buffer);
        match result {
            Ok(_) => String::from_utf8(buffer).unwrap_or_else(|_| "Error encoding metrics".to_string()),
            Err(e) => format!("Error exporting metrics: {}", e),
        }
    }

    /// Get metrics summary
    pub fn get_summary(&self) -> JupiterMetricsSummary {
        let custom_metrics = self.custom_metrics.read();
        JupiterMetricsSummary {
            total_requests: self.jupiter_requests_total.get() as i64,
            successful_requests: self.jupiter_requests_success_total.get() as i64,
            failed_requests: self.jupiter_requests_failed_total.get() as i64,
            error_count: self.jupiter_errors_total.get() as i64,
            rate_limit_errors: self.jupiter_rate_limit_errors_total.get() as i64,
            timeout_errors: self.jupiter_timeout_errors_total.get() as i64,
            circuit_breaker_trips: self.jupiter_circuit_breaker_trips_total.get() as i64,
            consecutive_failures: self.jupiter_consecutive_failures.get(),
            active_connections: self.jupiter_active_connections.get(),
            idle_connections: self.jupiter_idle_connections.get(),
            connection_reuses: self.jupiter_connection_reuses_total.get() as i64,
            avg_response_time_ms: self.jupiter_avg_response_time_ms.get(),
            p95_response_time_ms: self.jupiter_p95_response_time_ms.get(),
            p99_response_time_ms: self.jupiter_p99_response_time_ms.get(),
            rtse_usage: self.jupiter_rtse_usage_total.get() as i64,
            jupiter_beam_usage: self.jupiter_jupiter_beam_usage_total.get() as i64,
            gasless_usage: self.jupiter_gasless_swap_usage_total.get() as i64,
            v2_usage: self.jupiter_v2_usage_total.get() as i64,
            v1_usage: self.jupiter_v1_usage_total.get() as i64,
            cache_hits: self.jupiter_route_cache_hits_total.get() as i64,
            cache_misses: self.jupiter_route_cache_misses_total.get() as i64,
            quota_usage_percent: self.jupiter_quota_usage_percent.get(),
            quota_remaining: self.jupiter_quota_remaining.get(),
            custom_metrics: custom_metrics.clone(),
        }
    }
}

/// Summary of Jupiter metrics
#[derive(Debug, Clone)]
pub struct JupiterMetricsSummary {
    pub total_requests: i64,
    pub successful_requests: i64,
    pub failed_requests: i64,
    pub error_count: i64,
    pub rate_limit_errors: i64,
    pub timeout_errors: i64,
    pub circuit_breaker_trips: i64,
    pub consecutive_failures: i64,
    pub active_connections: i64,
    pub idle_connections: i64,
    pub connection_reuses: i64,
    pub avg_response_time_ms: f64,
    pub p95_response_time_ms: f64,
    pub p99_response_time_ms: f64,
    pub rtse_usage: i64,
    pub jupiter_beam_usage: i64,
    pub gasless_usage: i64,
    pub v2_usage: i64,
    pub v1_usage: i64,
    pub cache_hits: i64,
    pub cache_misses: i64,
    pub quota_usage_percent: f64,
    pub quota_remaining: i64,
    pub custom_metrics: HashMap<String, f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jupiter_metrics_creation() {
        let metrics = JupiterMetrics::new();

        // Verify all metrics are initialized
        assert_eq!(metrics.jupiter_requests_total.get(), 0);
        assert_eq!(metrics.jupiter_requests_success_total.get(), 0);
        assert_eq!(metrics.jupiter_requests_failed_total.get(), 0);
    }

    #[test]
    fn test_request_recording() {
        let metrics = JupiterMetrics::new();

        metrics.record_request("/order", 0.5, true);
        metrics.record_request("/order", 0.3, false);

        assert_eq!(metrics.jupiter_requests_total.get(), 2);
        assert_eq!(metrics.jupiter_requests_success_total.get(), 1);
        assert_eq!(metrics.jupiter_requests_failed_total.get(), 1);
    }

    #[test]
    fn test_error_recording() {
        let metrics = JupiterMetrics::new();

        metrics.record_error("rate_limit");
        metrics.record_error("timeout");
        metrics.record_error("unknown");

        assert_eq!(metrics.jupiter_errors_total.get(), 3);
        assert_eq!(metrics.jupiter_rate_limit_errors_total.get(), 1);
        assert_eq!(metrics.jupiter_timeout_errors_total.get(), 1);
    }

    #[test]
    fn test_circuit_breaker_metrics() {
        let metrics = JupiterMetrics::new();

        metrics.record_circuit_breaker_trip();
        metrics.update_consecutive_failures(5);

        assert_eq!(metrics.jupiter_circuit_breaker_trips_total.get(), 1);
        assert_eq!(metrics.jupiter_consecutive_failures.get(), 5);
    }

    #[test]
    fn test_connection_metrics() {
        let metrics = JupiterMetrics::new();

        metrics.update_connection_metrics(5, 3);
        metrics.record_connection_reuse();

        assert_eq!(metrics.jupiter_active_connections.get(), 5);
        assert_eq!(metrics.jupiter_idle_connections.get(), 3);
        assert_eq!(metrics.jupiter_connection_reuses_total.get(), 1);
    }

    #[test]
    fn test_feature_usage_metrics() {
        let metrics = JupiterMetrics::new();

        metrics.record_feature_usage("rtse");
        metrics.record_feature_usage("jupiter_beam");
        metrics.record_feature_usage("gasless");

        assert_eq!(metrics.jupiter_rtse_usage_total.get(), 1);
        assert_eq!(metrics.jupiter_jupiter_beam_usage_total.get(), 1);
        assert_eq!(metrics.jupiter_gasless_swap_usage_total.get(), 1);
    }

    #[test]
    fn test_version_usage_metrics() {
        let metrics = JupiterMetrics::new();

        metrics.record_version_usage("v2");
        metrics.record_version_usage("v2");
        metrics.record_version_usage("v1");

        assert_eq!(metrics.jupiter_v2_usage_total.get(), 2);
        assert_eq!(metrics.jupiter_v1_usage_total.get(), 1);
    }

    #[test]
    fn test_cache_metrics() {
        let metrics = JupiterMetrics::new();

        metrics.record_cache_hit();
        metrics.record_cache_hit();
        metrics.record_cache_miss();

        assert_eq!(metrics.jupiter_route_cache_hits_total.get(), 2);
        assert_eq!(metrics.jupiter_route_cache_misses_total.get(), 1);
    }

    #[test]
    fn test_custom_metrics() {
        let metrics = JupiterMetrics::new();

        metrics.set_custom_metric("custom_latency_ms", 150.0);
        assert_eq!(metrics.get_custom_metric("custom_latency_ms"), Some(150.0));

        metrics.set_custom_metric("custom_latency_ms", 200.0);
        assert_eq!(metrics.get_custom_metric("custom_latency_ms"), Some(200.0));
    }

    #[test]
    fn test_metrics_export() {
        let metrics = JupiterMetrics::new();

        metrics.record_request("/order", 0.5, true);
        metrics.record_error("rate_limit");

        let export = metrics.export_metrics();
        assert!(export.contains("jupiter_requests_total"));
        assert!(export.contains("jupiter_rate_limit_errors_total"));
    }

    #[test]
    fn test_metrics_summary() {
        let metrics = JupiterMetrics::new();

        metrics.record_request("/order", 0.5, true);
        metrics.record_error("timeout");
        metrics.update_consecutive_failures(2);

        let summary = metrics.get_summary();
        assert_eq!(summary.total_requests, 1);
        assert_eq!(summary.successful_requests, 1);
        assert_eq!(summary.error_count, 1);
        assert_eq!(summary.consecutive_failures, 2);
    }
}