//! Health monitoring for Helius LaserStream WebSocket
//!
//! Tracks connection metrics, detects unhealthy connections, and manages
//! circuit breaker integration for automatic fallback.

use super::helius_wss::ConnectionState;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;

/// Health monitoring for WebSocket connections
pub struct WebSocketHealth {
    connection_state: Arc<RwLock<ConnectionState>>,
    messages_received: Arc<AtomicU64>,
    connection_failures: Arc<AtomicU64>,
    last_message_time: Arc<RwLock<Option<SystemTime>>>,
    last_pong_time: Arc<RwLock<Option<SystemTime>>>,
    unhealthy_threshold_secs: u64,
    pong_timeout_secs: u64,
}

impl WebSocketHealth {
    pub fn new(unhealthy_threshold_secs: u64) -> Self {
        Self {
            connection_state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
            messages_received: Arc::new(AtomicU64::new(0)),
            connection_failures: Arc::new(AtomicU64::new(0)),
            last_message_time: Arc::new(RwLock::new(None)),
            last_pong_time: Arc::new(RwLock::new(None)),
            unhealthy_threshold_secs,
            pong_timeout_secs: 30,
        }
    }

    /// Record a received message
    pub async fn record_message(&self) {
        self.messages_received.fetch_add(1, Ordering::Relaxed);
        *self.last_message_time.write().await = Some(SystemTime::now());
    }

    /// Record a pong response
    pub async fn record_pong(&self) {
        *self.last_pong_time.write().await = Some(SystemTime::now());
    }

    /// Record a connection failure
    pub fn record_failure(&self) {
        self.connection_failures.fetch_add(1, Ordering::Relaxed);
    }

    /// Reset failure count (successful connection)
    pub fn reset_failures(&self) {
        self.connection_failures.store(0, Ordering::Relaxed);
    }

    /// Set connection state
    pub async fn set_state(&self, state: ConnectionState) {
        *self.connection_state.write().await = state;
    }

    /// Check if connection is healthy
    pub async fn is_healthy(&self) -> bool {
        // Check if we've received a message recently
        let last_message = *self.last_message_time.read().await;
        let last_pong = *self.last_pong_time.read().await;

        match (last_message, last_pong) {
            (Some(msg_time), Some(pong_time)) => {
                let now = SystemTime::now();

                // Check message timeout
                let msg_duration = now.duration_since(msg_time).unwrap_or(Duration::ZERO);
                if msg_duration.as_secs() > self.unhealthy_threshold_secs {
                    tracing::warn!(
                        elapsed_secs = msg_duration.as_secs(),
                        threshold_secs = self.unhealthy_threshold_secs,
                        "Unhealthy: No messages received recently"
                    );
                    return false;
                }

                // Check pong timeout
                let pong_duration = now.duration_since(pong_time).unwrap_or(Duration::ZERO);
                if pong_duration.as_secs() > self.pong_timeout_secs {
                    tracing::warn!(
                        elapsed_secs = pong_duration.as_secs(),
                        threshold_secs = self.pong_timeout_secs,
                        "Unhealthy: No pong response recently"
                    );
                    return false;
                }

                true
            }
            _ => false,
        }
    }

    /// Get current health metrics
    pub async fn get_metrics(&self) -> HealthMetrics {
        let state = *self.connection_state.read().await;
        let messages_received = self.messages_received.load(Ordering::Relaxed);
        let connection_failures = self.connection_failures.load(Ordering::Relaxed);
        let last_message_at = *self.last_message_time.read().await;
        let last_pong_at = *self.last_pong_time.read().await;

        HealthMetrics {
            connection_state: state,
            messages_received,
            connection_failures,
            last_message_at,
            last_pong_at,
            uptime_seconds: self.calculate_uptime().await,
        }
    }

    /// Calculate connection uptime
    async fn calculate_uptime(&self) -> u64 {
        let state = *self.connection_state.read().await;

        if state == ConnectionState::Connected {
            // For simplicity, return 0 if we don't track connection start time
            // In production, you'd track when the connection was established
            0
        } else {
            0
        }
    }

    /// Check if circuit breaker should be triggered
    pub fn should_trigger_circuit_breaker(&self) -> bool {
        const FAILURE_THRESHOLD: u64 = 5;

        let failures = self.connection_failures.load(Ordering::Relaxed);
        failures >= FAILURE_THRESHOLD
    }
}

/// Health metrics for WebSocket connection
#[derive(Debug, Clone)]
pub struct HealthMetrics {
    pub connection_state: ConnectionState,
    pub messages_received: u64,
    pub connection_failures: u64,
    pub last_message_at: Option<SystemTime>,
    pub last_pong_at: Option<SystemTime>,
    pub uptime_seconds: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[tokio::test]
    async fn test_health_tracking() {
        let health = WebSocketHealth::new(60);

        // Initially unhealthy (no messages)
        assert!(!health.is_healthy().await);

        // Record message
        health.record_message().await;
        assert!(health.is_healthy().await);

        // Check metrics
        let metrics = health.get_metrics().await;
        assert_eq!(metrics.messages_received, 1);
        assert_eq!(metrics.connection_failures, 0);
    }

    #[tokio::test]
    async fn test_failure_tracking() {
        let health = WebSocketHealth::new(60);

        // Record failures
        health.record_failure();
        health.record_failure();

        let metrics = health.get_metrics().await;
        assert_eq!(metrics.connection_failures, 2);

        // Should not trigger circuit breaker yet (threshold is 5)
        assert!(!health.should_trigger_circuit_breaker());

        // Add more failures
        health.record_failure();
        health.record_failure();
        health.record_failure();

        // Should trigger circuit breaker now
        assert!(health.should_trigger_circuit_breaker());
    }

    #[tokio::test]
    async fn test_unhealthy_threshold() {
        let health = WebSocketHealth::new(1); // 1 second threshold for testing

        // Record message
        health.record_message().await;
        assert!(health.is_healthy().await);

        // Wait for threshold to expire
        tokio::time::sleep(Duration::from_millis(1100)).await;

        // Should be unhealthy now
        assert!(!health.is_healthy().await);
    }
}
