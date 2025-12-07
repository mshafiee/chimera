//! Trade executor for Solana transactions
//!
//! Handles the actual submission of trades to the Solana network.
//! Currently a stub implementation - Jito bundle support to be added.

use crate::config::AppConfig;
use crate::db::DbPool;
use crate::models::{Action, Signal, Strategy};
use std::sync::Arc;

/// RPC mode for trade execution
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcMode {
    /// Primary RPC with Jito bundles
    Jito,
    /// Fallback to standard TPU
    Standard,
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
        }
    }

    /// Execute a trade signal
    ///
    /// Returns the transaction signature on success
    pub async fn execute(&mut self, signal: &Signal) -> Result<String, ExecutorError> {
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
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Generate a fake signature for now
        let signature = format!(
            "{}{}{}",
            &signal.trade_uuid[..8],
            chrono::Utc::now().timestamp(),
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
        tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

        // Generate a fake signature for now
        let signature = format!(
            "{}{}{}",
            &signal.trade_uuid[..8],
            chrono::Utc::now().timestamp(),
            "std"
        );

        Ok(signature)
    }

    /// Calculate dynamic Jito tip based on strategy and history
    fn calculate_jito_tip(&self, signal: &Signal) -> f64 {
        // For MVP, use a simple strategy-based tip
        // TODO: Implement percentile-based calculation from tip history

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
}
