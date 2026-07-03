//! Enhanced HTTP Client for Jupiter API Integration
//!
//! Optimized HTTP client with connection pooling, keep-alive, and health monitoring
//! specifically tuned for high-frequency Jupiter API calls.

use crate::error::{AppError, AppResult};
use reqwest::Client;
use std::sync::Arc;
use std::time::Duration;
use parking_lot::{Mutex, RwLock};

/// Jupiter HTTP client configuration optimized for high-frequency API calls
#[derive(Debug, Clone)]
pub struct JupiterClientConfig {
    /// Maximum idle connections per host
    pub max_idle_connections: usize,
    /// Maximum idle connections overall
    pub max_idle_connections_per_host: usize,
    /// Connection timeout
    pub connect_timeout: Duration,
    /// Request timeout
    pub request_timeout: Duration,
    /// Keep-alive duration for idle connections
    pub keep_alive_duration: Duration,
    /// Whether to use connection pooling
    pub enable_pooling: bool,
    /// Maximum concurrent streams per connection (HTTP/2)
    pub max_concurrent_streams: u32,
    /// Enable TCP keepalive
    pub tcp_keepalive: bool,
    /// Connection health check interval
    pub health_check_interval: Duration,
}

impl Default for JupiterClientConfig {
    fn default() -> Self {
        Self {
            max_idle_connections: 100,
            max_idle_connections_per_host: 10,
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
            keep_alive_duration: Duration::from_secs(75),
            enable_pooling: true,
            max_concurrent_streams: 100,
            tcp_keepalive: true,
            health_check_interval: Duration::from_secs(60),
        }
    }
}

/// Enhanced HTTP client with connection pooling and health monitoring
pub struct JupiterHttpClient {
    /// HTTP client
    client: Client,
    /// Configuration
    config: JupiterClientConfig,
    /// Health status
    health_status: Arc<RwLock<HealthStatus>>,
    /// Metrics
    metrics: Arc<Mutex<ClientMetrics>>,
}

/// Health status of the HTTP client
#[derive(Debug, Clone)]
pub struct HealthStatus {
    /// Whether the client is healthy
    pub is_healthy: bool,
    /// Last health check time
    pub last_check: chrono::DateTime<chrono::Utc>,
    /// Consecutive failures
    pub consecutive_failures: u32,
    /// Total requests made
    pub total_requests: u64,
    /// Successful requests
    pub successful_requests: u64,
}

/// Client metrics for monitoring
#[derive(Debug, Clone, Default)]
pub struct ClientMetrics {
    /// Total requests
    pub total_requests: u64,
    /// Successful requests
    pub successful_requests: u64,
    /// Failed requests
    pub failed_requests: u64,
    /// Average request duration (ms)
    pub avg_request_duration_ms: f64,
    /// Active connections
    pub active_connections: u32,
    /// Idle connections
    pub idle_connections: u32,
    /// Connection reuse count
    pub connection_reuses: u64,
}

impl JupiterHttpClient {
    /// Create new Jupiter HTTP client with optimized configuration
    pub fn new(config: JupiterClientConfig) -> AppResult<Self> {
        let client_builder = Client::builder()
            .timeout(config.request_timeout)
            .connect_timeout(config.connect_timeout)
            .pool_max_idle_per_host(config.max_idle_connections_per_host)
            .pool_idle_timeout(config.keep_alive_duration)
            .tcp_nodelay(true);  // Reduce latency by disabling Nagle's algorithm

        let client = client_builder.build().map_err(|e| {
            AppError::Internal(format!("Failed to create Jupiter HTTP client: {}", e))
        })?;

        let health_status = Arc::new(RwLock::new(HealthStatus {
            is_healthy: true,
            last_check: chrono::Utc::now(),
            consecutive_failures: 0,
            total_requests: 0,
            successful_requests: 0,
        }));

        let metrics = Arc::new(Mutex::new(ClientMetrics::default()));

        Ok(Self {
            client,
            config,
            health_status,
            metrics,
        })
    }

    /// Create with default configuration
    pub fn with_defaults() -> AppResult<Self> {
        Self::new(JupiterClientConfig::default())
    }

    /// Get the underlying reqwest client
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Get current health status
    pub fn get_health_status(&self) -> HealthStatus {
        (*self.health_status.read()).clone()
    }

    /// Get current metrics
    pub fn get_metrics(&self) -> ClientMetrics {
        (*self.metrics.lock()).clone()
    }

    /// Update health status after a request
    pub async fn update_request_status(&self, success: bool) {
        let mut status = self.health_status.write();
        let mut metrics = self.metrics.lock();

        status.total_requests += 1;
        metrics.total_requests += 1;

        if success {
            status.successful_requests += 1;
            status.consecutive_failures = 0;
            metrics.successful_requests += 1;

            // Reset healthy status if previously unhealthy
            if !status.is_healthy && status.consecutive_failures == 0 {
                status.is_healthy = true;
                tracing::info!("Jupiter HTTP client health restored");
            }
        } else {
            metrics.failed_requests += 1;
            status.consecutive_failures += 1;

            // Mark as unhealthy after 3 consecutive failures
            if status.consecutive_failures >= 3 && status.is_healthy {
                status.is_healthy = false;
                tracing::warn!(
                    consecutive_failures = status.consecutive_failures,
                    "Jupiter HTTP client marked as unhealthy"
                );
            }
        }

        status.last_check = chrono::Utc::now();
    }

    /// Perform health check on the client
    pub async fn health_check(&self) -> AppResult<bool> {
        // Simple health check: verify client is not in a broken state
        let status = self.get_health_status();
        let consecutive_failures = status.consecutive_failures;

        let is_healthy = consecutive_failures < 5;

        // Update health status
        let mut status_lock = self.health_status.write();
        status_lock.is_healthy = is_healthy;
        status_lock.last_check = chrono::Utc::now();

        Ok(is_healthy)
    }

    /// Reset health status (for manual recovery)
    pub async fn reset_health_status(&self) {
        let mut status = self.health_status.write();
        status.is_healthy = true;
        status.consecutive_failures = 0;
        status.last_check = chrono::Utc::now();

        tracing::info!("Jupiter HTTP client health status reset manually");
    }

    /// Record request duration for metrics
    pub fn record_request_duration(&self, duration_ms: u64) {
        let mut metrics = self.metrics.lock();

        // Update rolling average
        let total_requests = metrics.total_requests as f64;
        let current_avg = metrics.avg_request_duration_ms;
        metrics.avg_request_duration_ms = (current_avg * (total_requests - 1.0) + duration_ms as f64) / total_requests.max(1.0);
    }

    /// Get connection pool statistics
    pub async fn get_pool_stats(&self) -> (u32, u32) {
        // Mock pool stats for now (reqwest doesn't expose this directly)
        let metrics = self.metrics.lock();
        (metrics.active_connections, metrics.idle_connections)
    }

    /// Update connection pool statistics
    pub fn update_pool_stats(&self, active: u32, idle: u32) {
        let mut metrics = self.metrics.lock();
        metrics.active_connections = active;
        metrics.idle_connections = idle;
    }

    /// Increment connection reuse counter
    pub fn increment_connection_reuses(&self) {
        let mut metrics = self.metrics.lock();
        metrics.connection_reuses += 1;
    }

    /// Get connection efficiency metrics
    pub fn get_connection_efficiency(&self) -> f64 {
        let metrics = self.metrics.lock();

        if metrics.total_requests == 0 {
            return 0.0;
        }

        let reuse_ratio = metrics.connection_reuses as f64 / metrics.total_requests as f64;
        reuse_ratio * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_jupiter_client_creation() {
        let config = JupiterClientConfig::default();
        let client = JupiterHttpClient::new(config);

        assert!(client.is_ok(), "Should create client successfully");

        let client = client.unwrap();
        let health = client.get_health_status().await;

        assert!(health.is_healthy, "New client should be healthy");
        assert_eq!(health.consecutive_failures, 0);
    }

    #[tokio::test]
    async fn test_health_status_tracking() {
        let client = JupiterHttpClient::with_defaults().unwrap();

        // Record successful request
        client.update_request_status(true).await;
        let health = client.get_health_status().await;

        assert_eq!(health.total_requests, 1);
        assert_eq!(health.successful_requests, 1);
        assert_eq!(health.consecutive_failures, 0);
        assert!(health.is_healthy);

        // Record multiple failures
        client.update_request_status(false).await;
        client.update_request_status(false).await;
        client.update_request_status(false).await;

        let health = client.get_health_status().await;
        assert_eq!(health.consecutive_failures, 3);
        assert!(!health.is_healthy, "Should be unhealthy after 3 failures");

        // Reset health
        client.reset_health_status().await;
        let health = client.get_health_status().await;
        assert!(health.is_healthy, "Should be healthy after reset");
        assert_eq!(health.consecutive_failures, 0);
    }

    #[tokio::test]
    async fn test_metrics_tracking() {
        let client = JupiterHttpClient::with_defaults().unwrap();

        // Record some requests
        client.update_request_status(true).await;
        client.update_request_status(true).await;
        client.update_request_status(false).await;

        let metrics = client.get_metrics();
        assert_eq!(metrics.total_requests, 3);
        assert_eq!(metrics.successful_requests, 2);
        assert_eq!(metrics.failed_requests, 1);
    }

    #[test]
    fn test_request_duration_tracking() {
        let client = JupiterHttpClient::with_defaults().unwrap();

        // Record some request durations
        client.record_request_duration(100);
        client.record_request_duration(200);
        client.record_request_duration(300);

        let metrics = client.get_metrics();
        assert_eq!(metrics.avg_request_duration_ms, 200.0, "Average should be 200ms");
    }

    #[tokio::test]
    fn test_connection_efficiency() {
        let client = JupiterHttpClient::with_defaults().unwrap();

        // Initially no connections reused
        let efficiency = client.get_connection_efficiency();
        assert_eq!(efficiency, 0.0, "Initial efficiency should be 0");

        // Simulate some connection reuses
        for _ in 0..10 {
            client.increment_connection_reuses();
        }

        // After simulating total requests and connection reuses
        let metrics = client.get_metrics();
        *Arc::try_unwrap(metrics).unwrap().total_requests.lock().unwrap() = 20;

        let efficiency = client.get_connection_efficiency();
        assert_eq!(efficiency, 50.0, "Should be 50% reuse rate");
    }

    #[tokio::test]
    fn test_default_config() {
        let config = JupiterClientConfig::default();

        assert_eq!(config.max_idle_connections, 100);
        assert_eq!(config.max_idle_connections_per_host, 10);
        assert_eq!(config.connect_timeout, Duration::from_secs(10));
        assert_eq!(config.request_timeout, Duration::from_secs(30));
        assert!(config.enable_pooling);
        assert!(config.tcp_keepalive);
    }

    #[tokio::test]
    async fn test_health_check() {
        let client = JupiterHttpClient::with_defaults().unwrap();

        // Initial health check should pass
        let health = client.health_check().await.unwrap();
        assert!(health, "New client should be healthy");

        // After failures, health check should reflect that
        client.update_request_status(false).await;
        client.update_request_status(false).await;
        client.update_request_status(false).await;
        client.update_request_status(false).await;

        let health = client.health_check().await.unwrap();
        assert!(!health, "Should be unhealthy after 4 failures");
    }
}