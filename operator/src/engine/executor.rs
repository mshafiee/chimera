//! Trade executor for Solana transactions
//!
//! Handles the actual submission of trades to the Solana network.
//! Includes RPC failover with automatic recovery to primary.

use crate::config::AppConfig;
use crate::db::DbPool;
use crate::models::{Signal, Strategy};
use chrono::{DateTime, Utc};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

/// RPC mode for trade execution
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcMode {
    /// Primary RPC with Jito bundles
    Jito,
    /// Fallback to standard TPU
    Standard,
}

/// RPC health status
#[derive(Debug, Clone)]
pub struct RpcHealth {
    /// Whether the RPC is healthy
    pub healthy: bool,
    /// Last check timestamp
    pub last_check: DateTime<Utc>,
    /// Latency in milliseconds (if healthy)
    pub latency_ms: Option<u64>,
}

/// Trade executor
pub struct Executor {
    /// Configuration
    config: Arc<AppConfig>,
    /// Database pool
    db: DbPool,
    /// Current RPC mode
    rpc_mode: RpcMode,
    /// Consecutive failure count
    failure_count: u32,
    /// When fallback mode was activated
    fallback_since: Option<DateTime<Utc>>,
    /// Recovery check interval (default 5 minutes)
    recovery_interval: Duration,
    /// Last recovery attempt
    last_recovery_attempt: Option<DateTime<Utc>>,
}

impl Executor {
    /// Create a new executor
    pub fn new(config: Arc<AppConfig>, db: DbPool) -> Self {
        let rpc_mode = if config.jito.enabled {
            RpcMode::Jito
        } else {
            RpcMode::Standard
        };

        Self {
            config,
            db,
            rpc_mode,
            failure_count: 0,
            fallback_since: None,
            recovery_interval: Duration::from_secs(300), // 5 minutes
            last_recovery_attempt: None,
        }
    }

    /// Execute a trade signal
    ///
    /// Returns the transaction signature on success
    pub async fn execute(&mut self, signal: &Signal) -> Result<String, ExecutorError> {
        // Check if we should try to recover to primary
        if self.should_attempt_recovery() {
            self.try_recover_to_primary().await;
        }

        tracing::info!(
            trade_uuid = %signal.trade_uuid,
            strategy = %signal.payload.strategy,
            token = %signal.payload.token,
            action = %signal.payload.action,
            amount_sol = signal.payload.amount_sol,
            rpc_mode = ?self.rpc_mode,
            "Executing trade"
        );

        // Check if Spear is allowed in current mode
        if signal.payload.strategy == Strategy::Spear && self.rpc_mode == RpcMode::Standard {
            return Err(ExecutorError::SpearDisabled);
        }

        // Validate amount bounds
        if signal.payload.amount_sol < self.config.strategy.min_position_sol {
            return Err(ExecutorError::AmountTooSmall(
                signal.payload.amount_sol,
                self.config.strategy.min_position_sol,
            ));
        }

        if signal.payload.amount_sol > self.config.strategy.max_position_sol {
            return Err(ExecutorError::AmountTooLarge(
                signal.payload.amount_sol,
                self.config.strategy.max_position_sol,
            ));
        }

        // Execute based on mode
        let result = match self.rpc_mode {
            RpcMode::Jito => self.execute_jito(signal).await,
            RpcMode::Standard => self.execute_standard(signal).await,
        };

        // Handle result and track failures
        match &result {
            Ok(sig) => {
                self.failure_count = 0;
                tracing::info!(
                    trade_uuid = %signal.trade_uuid,
                    signature = %sig,
                    "Trade executed successfully"
                );
            }
            Err(e) => {
                self.failure_count += 1;
                tracing::error!(
                    trade_uuid = %signal.trade_uuid,
                    error = %e,
                    failure_count = self.failure_count,
                    "Trade execution failed"
                );

                // Check if we need to switch to fallback
                if self.failure_count >= self.config.rpc.max_consecutive_failures
                    && self.rpc_mode == RpcMode::Jito
                {
                    self.switch_to_fallback().await;
                }
            }
        }

        result
    }

    /// Check if we should attempt recovery to primary RPC
    fn should_attempt_recovery(&self) -> bool {
        // Only attempt recovery if we're in fallback mode
        if self.rpc_mode != RpcMode::Standard || self.fallback_since.is_none() {
            return false;
        }

        // Check if Jito is configured
        if !self.config.jito.enabled {
            return false;
        }

        let now = Utc::now();

        // Check if enough time has passed since fallback
        if let Some(fallback_time) = self.fallback_since {
            let elapsed = now.signed_duration_since(fallback_time);
            if elapsed < chrono::Duration::from_std(self.recovery_interval).unwrap_or_default() {
                return false;
            }
        }

        // Check if enough time has passed since last recovery attempt
        if let Some(last_attempt) = self.last_recovery_attempt {
            let elapsed = now.signed_duration_since(last_attempt);
            if elapsed < chrono::Duration::from_std(self.recovery_interval).unwrap_or_default() {
                return false;
            }
        }

        true
    }

    /// Attempt to recover to primary RPC
    async fn try_recover_to_primary(&mut self) {
        self.last_recovery_attempt = Some(Utc::now());

        tracing::info!("Attempting to recover to primary RPC (Jito)");

        // Perform health check on primary RPC
        match self.check_primary_health().await {
            Ok(health) if health.healthy => {
                tracing::info!(
                    latency_ms = health.latency_ms,
                    "Primary RPC is healthy, switching back to Jito mode"
                );

                self.rpc_mode = RpcMode::Jito;
                self.fallback_since = None;
                self.failure_count = 0;

                // Log recovery to config audit
                if let Err(e) = crate::db::log_config_change(
                    &self.db,
                    "rpc_mode",
                    Some("STANDARD"),
                    "JITO",
                    "SYSTEM_RECOVERY",
                    Some("Primary RPC recovered, switching back from fallback"),
                )
                .await
                {
                    tracing::error!(error = %e, "Failed to log RPC mode recovery");
                }
            }
            Ok(_) => {
                tracing::warn!("Primary RPC health check failed, staying in fallback mode");
            }
            Err(e) => {
                tracing::warn!(error = %e, "Primary RPC health check error, staying in fallback mode");
            }
        }
    }

    /// Check health of primary RPC
    async fn check_primary_health(&self) -> Result<RpcHealth, ExecutorError> {
        let start = std::time::Instant::now();

        // Perform a simple RPC call to check health
        // In production, this would use the actual Solana RPC client
        // For now, we simulate a health check

        let health_check = async {
            // Simulate RPC call
            tokio::time::sleep(Duration::from_millis(50)).await;

            // TODO: Replace with actual RPC health check:
            // let client = RpcClient::new(&self.config.rpc.primary_url);
            // let _ = client.get_latest_blockhash().await?;

            Ok::<(), ExecutorError>(())
        };

        // Apply timeout
        let timeout_duration = Duration::from_millis(self.config.rpc.timeout_ms);
        match timeout(timeout_duration, health_check).await {
            Ok(Ok(())) => {
                let latency = start.elapsed().as_millis() as u64;
                Ok(RpcHealth {
                    healthy: true,
                    last_check: Utc::now(),
                    latency_ms: Some(latency),
                })
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Err(ExecutorError::Timeout),
        }
    }

    /// Execute via Jito bundle
    async fn execute_jito(&self, signal: &Signal) -> Result<String, ExecutorError> {
        // TODO: Implement actual Jito bundle submission
        // This is a stub that simulates execution

        tracing::debug!(
            trade_uuid = %signal.trade_uuid,
            "Simulating Jito bundle execution"
        );

        // Calculate dynamic tip
        let tip = self.calculate_jito_tip(signal);
        tracing::debug!(tip_sol = tip, "Calculated Jito tip");

        // Simulate network delay
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Generate a fake signature for now
        let signature = format!(
            "{}{}{}",
            &signal.trade_uuid[..8],
            Utc::now().timestamp(),
            "jito"
        );

        Ok(signature)
    }

    /// Execute via standard TPU
    async fn execute_standard(&self, signal: &Signal) -> Result<String, ExecutorError> {
        // TODO: Implement actual Solana RPC transaction submission
        // This is a stub that simulates execution

        tracing::debug!(
            trade_uuid = %signal.trade_uuid,
            "Simulating standard TPU execution"
        );

        // Simulate network delay
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Generate a fake signature for now
        let signature = format!(
            "{}{}{}",
            &signal.trade_uuid[..8],
            Utc::now().timestamp(),
            "std"
        );

        Ok(signature)
    }

    /// Calculate dynamic Jito tip based on strategy and history
    pub fn calculate_jito_tip(&self, signal: &Signal) -> f64 {
        // For MVP, use a simple strategy-based tip
        // TODO: Use TipManager for percentile-based calculation

        let base_tip = match signal.payload.strategy {
            Strategy::Shield => self.config.jito.tip_floor_sol,
            Strategy::Spear => {
                // Use higher tip for Spear to ensure bundle inclusion
                (self.config.jito.tip_floor_sol + self.config.jito.tip_ceiling_sol) / 2.0
            }
            Strategy::Exit => self.config.jito.tip_ceiling_sol, // Max tip for exits
        };

        // Apply percentage cap
        let max_by_percent = signal.payload.amount_sol * self.config.jito.tip_percent_max;
        let tip = base_tip.min(max_by_percent).min(self.config.jito.tip_ceiling_sol);

        tip.max(self.config.jito.tip_floor_sol)
    }

    /// Switch to fallback RPC mode
    async fn switch_to_fallback(&mut self) {
        if self.config.rpc.fallback_url.is_some() {
            tracing::warn!(
                previous_mode = ?self.rpc_mode,
                failure_count = self.failure_count,
                "Switching to fallback RPC mode"
            );

            self.rpc_mode = RpcMode::Standard;
            self.fallback_since = Some(Utc::now());
            self.failure_count = 0;

            // Log to config audit
            if let Err(e) = crate::db::log_config_change(
                &self.db,
                "rpc_mode",
                Some("JITO"),
                "STANDARD",
                "SYSTEM_FAILOVER",
                Some("Consecutive RPC failures exceeded threshold"),
            )
            .await
            {
                tracing::error!(error = %e, "Failed to log RPC mode change");
            }
        }
    }

    /// Get current RPC mode
    pub fn rpc_mode(&self) -> RpcMode {
        self.rpc_mode
    }

    /// Check if currently in fallback mode
    pub fn is_in_fallback(&self) -> bool {
        self.fallback_since.is_some()
    }

    /// Get time spent in fallback mode
    pub fn fallback_duration(&self) -> Option<chrono::Duration> {
        self.fallback_since.map(|t| Utc::now().signed_duration_since(t))
    }
}

/// Executor errors
#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    /// Spear strategy disabled in fallback mode
    #[error("Spear strategy is disabled in fallback RPC mode")]
    SpearDisabled,

    /// Amount too small
    #[error("Amount {0} SOL is below minimum {1} SOL")]
    AmountTooSmall(f64, f64),

    /// Amount too large
    #[error("Amount {0} SOL exceeds maximum {1} SOL")]
    AmountTooLarge(f64, f64),

    /// RPC error
    #[error("RPC error: {0}")]
    Rpc(String),

    /// Transaction failed
    #[error("Transaction failed: {0}")]
    TransactionFailed(String),

    /// Timeout
    #[error("Execution timed out")]
    Timeout,

    /// Insufficient balance
    #[error("Insufficient balance: required {required} SOL, available {available} SOL")]
    InsufficientBalance { required: f64, available: f64 },

    /// Circuit breaker tripped
    #[error("Circuit breaker tripped: {0}")]
    CircuitBreakerTripped(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_executor_error_display() {
        let err = ExecutorError::AmountTooSmall(0.001, 0.01);
        assert!(err.to_string().contains("0.001"));
        assert!(err.to_string().contains("0.01"));
    }

    #[test]
    fn test_rpc_mode_debug() {
        assert_eq!(format!("{:?}", RpcMode::Jito), "Jito");
        assert_eq!(format!("{:?}", RpcMode::Standard), "Standard");
    }
}
