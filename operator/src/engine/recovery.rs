//! Stuck-State Recovery for positions
//!
//! Implements recovery logic for positions stuck in EXITING state > 60 seconds.
//! This can happen when:
//! - Transaction signature was generated but blockhash expired
//! - Network issues prevented confirmation
//! - DB state became inconsistent with on-chain state
//!
//! Recovery process:
//! 1. Query positions in EXITING state for > 60s
//! 2. Check blockhash validity (or transaction confirmation)
//! 3. If expired/not found: revert to ACTIVE
//! 4. If confirmed on-chain: update to CLOSED
//! 5. Log to reconciliation_log

use crate::db::{self, DbPool, PositionRecord};
use crate::error::AppResult;
use chrono::{Duration, Utc};
use std::sync::Arc;
use tokio::time::interval;

/// Default stuck threshold in seconds
pub const DEFAULT_STUCK_THRESHOLD_SECS: i64 = 60;

/// Recovery check interval in seconds
const RECOVERY_CHECK_INTERVAL_SECS: u64 = 30;

/// Stuck-state recovery manager
pub struct RecoveryManager {
    /// Database pool
    db: DbPool,
    /// Stuck threshold in seconds
    stuck_threshold_secs: i64,
    /// Whether recovery is enabled
    enabled: bool,
}

impl RecoveryManager {
    /// Create a new recovery manager
    pub fn new(db: DbPool) -> Self {
        Self {
            db,
            stuck_threshold_secs: DEFAULT_STUCK_THRESHOLD_SECS,
            enabled: true,
        }
    }

    /// Create with custom threshold
    pub fn with_threshold(db: DbPool, stuck_threshold_secs: i64) -> Self {
        Self {
            db,
            stuck_threshold_secs,
            enabled: true,
        }
    }

    /// Start the recovery background task
    pub async fn start_background_task(self: Arc<Self>) {
        if !self.enabled {
            tracing::info!("Stuck-state recovery is disabled");
            return;
        }

        tracing::info!(
            threshold_secs = self.stuck_threshold_secs,
            interval_secs = RECOVERY_CHECK_INTERVAL_SECS,
            "Starting stuck-state recovery background task"
        );

        let mut check_interval = interval(std::time::Duration::from_secs(RECOVERY_CHECK_INTERVAL_SECS));

        loop {
            check_interval.tick().await;

            if let Err(e) = self.recover_stuck_positions().await {
                tracing::error!(error = %e, "Failed to recover stuck positions");
            }
        }
    }

    /// Recover stuck positions
    pub async fn recover_stuck_positions(&self) -> AppResult<u32> {
        let stuck_positions = db::get_stuck_positions(&self.db, self.stuck_threshold_secs).await?;

        if stuck_positions.is_empty() {
            return Ok(0);
        }

        tracing::info!(
            count = stuck_positions.len(),
            "Found stuck positions to recover"
        );

        let mut recovered = 0;
        for position in stuck_positions {
            match self.recover_position(&position).await {
                Ok(action) => {
                    tracing::info!(
                        trade_uuid = %position.trade_uuid,
                        action = %action,
                        "Position recovered"
                    );
                    recovered += 1;
                }
                Err(e) => {
                    tracing::error!(
                        trade_uuid = %position.trade_uuid,
                        error = %e,
                        "Failed to recover position"
                    );
                }
            }
        }

        Ok(recovered)
    }

    /// Recover a single position
    async fn recover_position(&self, position: &PositionRecord) -> AppResult<RecoveryAction> {
        let stuck_duration = Utc::now().signed_duration_since(position.last_updated);

        tracing::debug!(
            trade_uuid = %position.trade_uuid,
            stuck_secs = stuck_duration.num_seconds(),
            "Evaluating stuck position"
        );

        // Check on-chain state
        // In production, this would query the Solana blockchain
        let on_chain_state = self.check_on_chain_state(&position.entry_tx_signature).await?;

        match on_chain_state {
            OnChainState::TransactionConfirmed => {
                // Transaction confirmed, update to CLOSED
                db::update_position_state(&self.db, &position.trade_uuid, "CLOSED").await?;

                db::insert_reconciliation_log(
                    &self.db,
                    &position.trade_uuid,
                    "EXITING",
                    Some("FOUND"),
                    "NONE",
                    Some(&position.entry_tx_signature),
                    Some("Auto-recovery: transaction confirmed on-chain"),
                )
                .await?;

                Ok(RecoveryAction::MarkedClosed)
            }
            OnChainState::TransactionNotFound | OnChainState::BlockhashExpired => {
                // Transaction not found or blockhash expired, revert to ACTIVE
                db::update_position_state(&self.db, &position.trade_uuid, "ACTIVE").await?;

                db::insert_reconciliation_log(
                    &self.db,
                    &position.trade_uuid,
                    "EXITING",
                    Some("MISSING"),
                    "MISSING_TX",
                    None,
                    Some(&format!(
                        "Auto-recovery: {:?}, reverted to ACTIVE after {}s stuck",
                        on_chain_state,
                        stuck_duration.num_seconds()
                    )),
                )
                .await?;

                db::log_config_change(
                    &self.db,
                    &format!("position:{}", position.trade_uuid),
                    Some("EXITING"),
                    "ACTIVE",
                    "SYSTEM_RECOVERY",
                    Some("Stuck position reverted to ACTIVE"),
                )
                .await?;

                Ok(RecoveryAction::RevertedToActive)
            }
            OnChainState::Pending => {
                // Transaction still pending, don't take action yet
                Ok(RecoveryAction::StillPending)
            }
        }
    }

    /// Check on-chain state for a transaction
    async fn check_on_chain_state(&self, _tx_signature: &str) -> AppResult<OnChainState> {
        // TODO: Implement actual on-chain check using Solana RPC
        // This is a stub that simulates the check
        //
        // In production:
        // 1. Check if transaction signature exists using getTransaction
        // 2. If not found, check blockhash validity using isBlockhashValid
        // 3. Return appropriate state

        // For now, assume transactions not found after threshold are expired
        // In production, replace with actual RPC calls:
        //
        // let client = RpcClient::new(&self.rpc_url);
        // match client.get_transaction(signature).await {
        //     Ok(tx) => OnChainState::TransactionConfirmed,
        //     Err(ClientError { kind: RpcError::ForUser("Transaction not found"), .. }) => {
        //         // Check blockhash
        //         OnChainState::TransactionNotFound
        //     }
        //     Err(e) => return Err(e.into())
        // }

        // Simulate: 80% chance of not found, 20% chance of confirmed
        let random = (chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0) % 100) as u32;
        if random < 80 {
            Ok(OnChainState::TransactionNotFound)
        } else {
            Ok(OnChainState::TransactionConfirmed)
        }
    }
}

/// On-chain transaction state
#[derive(Debug, Clone, PartialEq, Eq)]
enum OnChainState {
    /// Transaction found and confirmed
    TransactionConfirmed,
    /// Transaction not found
    TransactionNotFound,
    /// Blockhash expired
    BlockhashExpired,
    /// Transaction still pending
    Pending,
}

/// Recovery action taken
#[derive(Debug, Clone)]
pub enum RecoveryAction {
    /// Position was marked as closed
    MarkedClosed,
    /// Position was reverted to active
    RevertedToActive,
    /// Position is still pending, no action taken
    StillPending,
}

impl std::fmt::Display for RecoveryAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MarkedClosed => write!(f, "MARKED_CLOSED"),
            Self::RevertedToActive => write!(f, "REVERTED_TO_ACTIVE"),
            Self::StillPending => write!(f, "STILL_PENDING"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recovery_action_display() {
        assert_eq!(RecoveryAction::MarkedClosed.to_string(), "MARKED_CLOSED");
        assert_eq!(RecoveryAction::RevertedToActive.to_string(), "REVERTED_TO_ACTIVE");
    }

    #[test]
    fn test_default_threshold() {
        assert_eq!(DEFAULT_STUCK_THRESHOLD_SECS, 60);
    }
}
