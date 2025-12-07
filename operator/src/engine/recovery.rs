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
use crate::error::{AppError, AppResult};
use crate::handlers::{WsEvent, WsState, PositionUpdateData};
use chrono::Utc;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::signature::Signature;
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
    /// RPC client for on-chain checks
    rpc_client: Arc<RpcClient>,
    /// Stuck threshold in seconds
    stuck_threshold_secs: i64,
    /// Whether recovery is enabled
    enabled: bool,
    /// WebSocket state for broadcasting updates
    ws_state: Option<Arc<WsState>>,
}

impl RecoveryManager {
    /// Create a new recovery manager
    pub fn new(db: DbPool, rpc_url: String) -> Self {
        Self::new_with_ws(db, rpc_url, None)
    }

    /// Create with custom threshold
    pub fn with_threshold(db: DbPool, rpc_url: String, stuck_threshold_secs: i64) -> Self {
        Self::new_with_ws_and_threshold(db, rpc_url, stuck_threshold_secs, None)
    }

    /// Create with WebSocket support
    pub fn new_with_ws(db: DbPool, rpc_url: String, ws_state: Option<Arc<WsState>>) -> Self {
        let rpc_client = Arc::new(RpcClient::new(rpc_url));
        Self {
            db,
            rpc_client,
            stuck_threshold_secs: DEFAULT_STUCK_THRESHOLD_SECS,
            enabled: true,
            ws_state,
        }
    }

    /// Create with custom threshold and WebSocket support
    pub fn new_with_ws_and_threshold(
        db: DbPool,
        rpc_url: String,
        stuck_threshold_secs: i64,
        ws_state: Option<Arc<WsState>>,
    ) -> Self {
        let rpc_client = Arc::new(RpcClient::new(rpc_url));
        Self {
            db,
            rpc_client,
            stuck_threshold_secs,
            enabled: true,
            ws_state,
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
        // For EXITING positions, check the exit transaction signature
        // If no exit signature yet, check entry signature as fallback
        let tx_signature = position
            .exit_tx_signature
            .as_ref()
            .map(|s| s.as_str())
            .unwrap_or(&position.entry_tx_signature);
        
        let on_chain_state = self.check_on_chain_state(tx_signature).await?;

        match on_chain_state {
            OnChainState::TransactionConfirmed => {
                // Transaction confirmed, update to CLOSED
                db::update_position_state(&self.db, &position.trade_uuid, "CLOSED").await?;

                let tx_sig = position
                    .exit_tx_signature
                    .as_ref()
                    .unwrap_or(&position.entry_tx_signature);
                
                db::insert_reconciliation_log(
                    &self.db,
                    &position.trade_uuid,
                    "EXITING",
                    Some("FOUND"),
                    "NONE",
                    Some(tx_sig),
                    Some("Auto-recovery: transaction confirmed on-chain"),
                )
                .await?;

                // Broadcast position update via WebSocket
                if let Some(ref ws) = self.ws_state {
                    ws.broadcast(WsEvent::PositionUpdate(PositionUpdateData {
                        trade_uuid: position.trade_uuid.clone(),
                        state: "CLOSED".to_string(),
                        unrealized_pnl_percent: None, // Would need to fetch from DB
                    }));
                }

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

                // Broadcast position update via WebSocket
                if let Some(ref ws) = self.ws_state {
                    ws.broadcast(WsEvent::PositionUpdate(PositionUpdateData {
                        trade_uuid: position.trade_uuid.clone(),
                        state: "ACTIVE".to_string(),
                        unrealized_pnl_percent: None,
                    }));
                }

                Ok(RecoveryAction::RevertedToActive)
            }
            OnChainState::Pending => {
                // Transaction still pending, don't take action yet
                Ok(RecoveryAction::StillPending)
            }
        }
    }

    /// Check on-chain state for a transaction
    async fn check_on_chain_state(&self, tx_signature: &str) -> AppResult<OnChainState> {
        // Parse transaction signature
        let signature = tx_signature
            .parse::<Signature>()
            .map_err(|e| AppError::Internal(format!("Invalid transaction signature: {}", e)))?;

        // Check if transaction exists on-chain
        let config = solana_client::rpc_config::RpcTransactionConfig {
            max_supported_transaction_version: Some(0),
            ..Default::default()
        };
        
        match self
            .rpc_client
            .get_transaction_with_config(&signature, config)
            .await
        {
            Ok(tx) => {
                // Transaction found - check if it's confirmed
                if let Some(meta) = tx.transaction.meta {
                    if meta.err.is_some() {
                        // Transaction failed on-chain
                        tracing::warn!(
                            signature = %tx_signature,
                            error = ?meta.err,
                            "Transaction found but failed on-chain"
                        );
                        Ok(OnChainState::TransactionNotFound)
                    } else {
                        // Transaction confirmed successfully
                        tracing::debug!(
                            signature = %tx_signature,
                            slot = tx.slot,
                            "Transaction confirmed on-chain"
                        );
                        Ok(OnChainState::TransactionConfirmed)
                    }
                } else {
                    // Metadata missing, assume confirmed
                    Ok(OnChainState::TransactionConfirmed)
                }
            }
            Err(e) => {
                // Check if it's a "transaction not found" error
                let error_str = e.to_string().to_lowercase();
                if error_str.contains("not found") 
                    || error_str.contains("transaction not found")
                    || error_str.contains("-32004")
                {
                    // Transaction not found - likely blockhash expired or never submitted
                    tracing::debug!(
                        signature = %tx_signature,
                        "Transaction not found on-chain"
                    );
                    Ok(OnChainState::TransactionNotFound)
                } else {
                    // Other RPC errors - log and assume pending (don't take action)
                    tracing::warn!(
                        signature = %tx_signature,
                        error = %e,
                        "RPC error checking transaction, assuming pending"
                    );
                    Ok(OnChainState::Pending)
                }
            }
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
    #[allow(dead_code)] // Reserved for future use
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
