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
use crate::handlers::{PositionUpdateData, WsEvent, WsState};
use chrono::Utc;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Signature, Signer};
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
    /// Engine handle for dynamic RPC failover
    engine_handle: Option<crate::engine::EngineHandle>,
    /// Fallback static RPC client (for tests or legacy constructors)
    static_rpc_client: Option<Arc<RpcClient>>,
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
            engine_handle: None,
            static_rpc_client: Some(rpc_client),
            stuck_threshold_secs: DEFAULT_STUCK_THRESHOLD_SECS,
            enabled: true,
            ws_state,
        }
    }

    /// Create with a pre-constructed EngineHandle to share the executor's client.
    ///
    /// Preferred over `new_with_ws` — sharing the executor's client means the
    /// recovery manager automatically benefits from any failover logic applied
    /// to that client, and avoids creating a separate single-point-of-failure
    /// connection that has no fallback during an outage.
    pub fn new_with_rpc(
        db: DbPool,
        engine_handle: crate::engine::EngineHandle,
        ws_state: Option<Arc<WsState>>,
    ) -> Self {
        Self {
            db,
            engine_handle: Some(engine_handle),
            static_rpc_client: None,
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
            engine_handle: None,
            static_rpc_client: Some(rpc_client),
            stuck_threshold_secs,
            enabled: true,
            ws_state,
        }
    }

    /// Get the active RPC client from the engine handle, or fallback to static client
    async fn get_active_rpc(&self) -> AppResult<Arc<RpcClient>> {
        if let Some(ref handle) = self.engine_handle {
            if let Some(client) = handle.active_rpc_client().await {
                return Ok(client);
            }
        }
        if let Some(ref client) = self.static_rpc_client {
            return Ok(client.clone());
        }
        Err(AppError::Internal("No RPC client available".to_string()))
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

        let mut check_interval =
            interval(std::time::Duration::from_secs(RECOVERY_CHECK_INTERVAL_SECS));

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

        // [R-H6] Check on-chain state.
        // For EXITING positions, only check the exit transaction signature.
        // If there is no exit signature, the exit tx was never submitted — do NOT fall back to
        // the entry signature, which would confirm the BUY and mark the position CLOSED without
        // ever executing the sell. Instead revert to ACTIVE so stop_loss can manage the exit.
        let exit_sig = position.exit_tx_signature.as_deref().filter(|s| !s.is_empty());

        if exit_sig.is_none() {
            return self.revert_or_close_position(
                position,
                Some("NO_EXIT_SIG"),
                "REVERTED_ACTIVE",
                "Auto-recovery: no exit signature; reverted to ACTIVE for stop-loss management",
            )
            .await;
        }

        let tx_signature = exit_sig.unwrap();
        let on_chain_state = self.check_on_chain_state(tx_signature).await?;

        match on_chain_state {
            OnChainState::TransactionConfirmed => {
                // Exit transaction confirmed on-chain, mark CLOSED.
                db::update_position_state(&self.db, &position.trade_uuid, "CLOSED").await?;

                let tx_sig = position
                    .exit_tx_signature
                    .as_deref()
                    .unwrap_or(tx_signature);

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
                let note = format!(
                    "Auto-recovery: {:?}, reverted to ACTIVE after {}s stuck",
                    on_chain_state,
                    stuck_duration.num_seconds()
                );
                self.revert_or_close_position(position, Some("MISSING"), "MISSING_TX", &note).await
            }
            OnChainState::RpcError(ref rpc_err) => {
                // Transient RPC error. If the position has been stuck long enough
                // (> 5 min) we escalate to dead letter rather than waiting forever.
                const RPC_ERROR_ESCALATION_SECS: i64 = 300;
                if stuck_duration.num_seconds() >= RPC_ERROR_ESCALATION_SECS {
                    tracing::error!(
                        trade_uuid = %position.trade_uuid,
                        stuck_secs = stuck_duration.num_seconds(),
                        rpc_error = %rpc_err,
                        "Persistent RPC errors checking EXITING position — escalating to dead letter and reverting to ACTIVE"
                    );
                    db::insert_dead_letter(
                        &self.db,
                        Some(&position.trade_uuid),
                        &position.trade_uuid,
                        "RPC_CHECK_FAILED",
                        Some(rpc_err.as_str()),
                        None,
                    )
                    .await?;
                    db::insert_reconciliation_log(
                        &self.db,
                        &position.trade_uuid,
                        "EXITING",
                        None,
                        "STATE_MISMATCH",
                        None,
                        Some(&format!(
                            "Escalated to dead letter after {}s: RPC error: {}",
                            stuck_duration.num_seconds(),
                            rpc_err
                        )),
                    )
                    .await?;
                    // Revert to ACTIVE so portfolio heat is freed and the exit can be
                    // retried by the normal exit path. Leaving the position in EXITING
                    // would permanently lock capital in the heat calculation.
                    // The dead letter entry ensures a human reviews the final outcome.
                    let note = format!(
                        "Escalated to dead letter after {}s: RPC error: {}",
                        stuck_duration.num_seconds(),
                        rpc_err
                    );
                    self.revert_or_close_position(position, None, "STATE_MISMATCH", &note).await
                } else {
                    tracing::warn!(
                        trade_uuid = %position.trade_uuid,
                        stuck_secs = stuck_duration.num_seconds(),
                        rpc_error = %rpc_err,
                        "RPC error checking stuck position — will retry next cycle"
                    );
                    Ok(RecoveryAction::StillPending)
                }
            }
        }
    }

    /// Check if the on-chain SPL token balance is zero, indicating the position has already been exited.
    /// Returns `Ok(true)` if balance is confirmed to be 0 or if the token account does not exist.
    /// Returns `Ok(false)` if balance is > 0.
    async fn check_is_balance_zero(&self, token_address: &str) -> AppResult<bool> {
        let secrets = crate::vault::load_secrets_with_fallback()
            .map_err(|e| AppError::Internal(format!("Failed to load secrets: {}", e)))?;
        let wallet_keypair = crate::engine::transaction_builder::load_wallet_keypair(&secrets)?;
        let wallet_pubkey = wallet_keypair.pubkey();

        let token_mint = token_address
            .parse::<Pubkey>()
            .map_err(|e| AppError::Internal(format!("Invalid token address: {}", e)))?;

        use solana_account_decoder::UiAccountData;
        use solana_client::rpc_request::TokenAccountsFilter;

        let rpc = self.get_active_rpc().await?;
        let accounts = rpc
            .get_token_accounts_by_owner(&wallet_pubkey, TokenAccountsFilter::Mint(token_mint))
            .await
            .map_err(|e| AppError::Rpc(format!("Failed to fetch token accounts for balance check: {}", e)))?;

        let max_balance = accounts
            .iter()
            .filter_map(|keyed| {
                if let UiAccountData::Json(parsed) = &keyed.account.data {
                    parsed
                        .parsed
                        .get("info")
                        .and_then(|i| i.get("tokenAmount"))
                        .and_then(|ta| ta.get("amount"))
                        .and_then(|a| a.as_str())
                        .and_then(|s| s.parse::<u64>().ok())
                } else {
                    None
                }
            })
            .max()
            .unwrap_or(0);

        Ok(max_balance == 0)
    }

    /// Reverts a stuck position to ACTIVE or marks it CLOSED if the on-chain balance is 0.
    async fn revert_or_close_position(
        &self,
        position: &PositionRecord,
        actual_on_chain: Option<&str>,
        discrepancy: &str,
        notes: &str,
    ) -> AppResult<RecoveryAction> {
        let is_zero = match self.check_is_balance_zero(&position.token_address).await {
            Ok(zero) => zero,
            Err(e) => {
                tracing::error!(
                    trade_uuid = %position.trade_uuid,
                    error = %e,
                    "Failed to check on-chain token balance during recovery; assuming non-zero to be safe"
                );
                false
            }
        };

        if is_zero {
            tracing::warn!(
                trade_uuid = %position.trade_uuid,
                "Position is stuck in EXITING but on-chain token balance is 0 — marking CLOSED directly to avoid zombie loop"
            );
            db::update_position_state(&self.db, &position.trade_uuid, "CLOSED").await?;
            db::insert_reconciliation_log(
                &self.db,
                &position.trade_uuid,
                "EXITING",
                Some("ZERO_BALANCE"),
                "NONE",
                None,
                Some("Auto-recovery: stuck in EXITING but on-chain balance is zero; marked CLOSED directly"),
            )
            .await?;
            if let Some(ref ws) = self.ws_state {
                ws.broadcast(WsEvent::PositionUpdate(PositionUpdateData {
                    trade_uuid: position.trade_uuid.clone(),
                    state: "CLOSED".to_string(),
                    unrealized_pnl_percent: None,
                }));
            }
            Ok(RecoveryAction::MarkedClosed)
        } else {
            tracing::info!(
                trade_uuid = %position.trade_uuid,
                "On-chain balance is non-zero — reverting position to ACTIVE so stop-loss can manage exit"
            );
            db::revert_position_exit(&self.db, &position.trade_uuid).await?;
            db::insert_reconciliation_log(
                &self.db,
                &position.trade_uuid,
                "EXITING",
                actual_on_chain,
                discrepancy,
                None,
                Some(notes),
            )
            .await?;

            db::log_config_change(
                &self.db,
                &format!("position:{}", position.trade_uuid),
                Some("EXITING"),
                "ACTIVE",
                "SYSTEM_RECOVERY",
                Some(notes),
            )
            .await?;

            if let Some(ref ws) = self.ws_state {
                ws.broadcast(WsEvent::PositionUpdate(PositionUpdateData {
                    trade_uuid: position.trade_uuid.clone(),
                    state: "ACTIVE".to_string(),
                    unrealized_pnl_percent: None,
                }));
            }
            Ok(RecoveryAction::RevertedToActive)
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

        let rpc = self.get_active_rpc().await?;
        match rpc
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
                    // Other RPC errors (timeouts, 429s, etc.) — do not treat as Pending
                    // because that would keep the position in EXITING indefinitely.
                    tracing::warn!(
                        signature = %tx_signature,
                        error = %e,
                        "RPC error checking transaction status"
                    );
                    Ok(OnChainState::RpcError(e.to_string()))
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
    /// Transient RPC error (timeout, 429, etc.) — error message included
    RpcError(String),
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
        assert_eq!(
            RecoveryAction::RevertedToActive.to_string(),
            "REVERTED_TO_ACTIVE"
        );
    }

    #[test]
    fn test_default_threshold() {
        assert_eq!(DEFAULT_STUCK_THRESHOLD_SECS, 60);
    }
}
