//! Database abstraction layer
//!
//! PostgreSQL is the only supported backend (SQLite was decommissioned 2026-07).
//! The `DbPool`/`DatabaseBackend` enum shapes are retained for API stability
//! but always resolve to PostgreSQL.

pub mod export;
pub mod postgres;
pub mod types;

pub use export::{trades_to_csv, trades_to_pdf};
pub use types::{
    ActivePositionEntry, ActivePositionSummary, ConfigAuditItem, DatabaseBackend, DatabaseConfig,
    DbPool, DeadLetterItem, DiscrepancyRow, DiscrepancyTypeStats, ExitTargetData, InsertPosition,
    InsertTrade, LatencyBucket, PoolStats, PositionDetail, PositionRecord, ReconciliationRun,
    ReconciliationStats, ReconciliationStatus, RetryableDlqItem, TradeDetail, TradeLatencyStats,
    UpdateDlqItemParams, UpdatePosition, UpdateTradeStatus, WalletCopyPerformance, WalletDetail,
    WalletMonitoring, WalletMonitoringExtended, WebhookAuditLog, WebhookEligibility, WebhookStats,
};

use chimera_core::error::{AppError, AppResult};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;

/// Database query timing and monitoring utility
///
/// Records query execution time and logs slow queries (>100ms)
pub async fn timed_query<F, T>(
    metric_name: &str,
    operation: F,
) -> AppResult<T>
where
    F: std::future::Future<Output = AppResult<T>>,
{
    let start = Instant::now();
    let result = operation.await;
    let duration = start.elapsed();

    // Log slow queries
    if duration.as_millis() > 100 {
        tracing::warn!(
            query = metric_name,
            duration_ms = duration.as_millis(),
            "Slow database query detected"
        );
    } else {
        tracing::debug!(
            query = metric_name,
            duration_ms = duration.as_millis(),
            "Database query completed"
        );
    }

    result
}

/// Database trait defining all database operations
#[async_trait::async_trait]
pub trait Database: Send + Sync {
    // ========================================================================
    // CONNECTION LIFECYCLE
    // ========================================================================

    /// Close the database connection pool
    async fn close(&self) -> AppResult<()>;

    // ========================================================================
    // MIGRATION & STARTUP
    // ========================================================================

    /// Run database migrations
    async fn run_migrations(&self) -> AppResult<()>;

    /// Run integrity check on startup
    async fn startup_integrity_check(&self) -> AppResult<()>;

    /// Recover trades stuck in EXECUTING state
    async fn recover_executing_trades(&self) -> AppResult<u32>;

    // ========================================================================
    // TRADE OPERATIONS
    // ========================================================================

    /// Check if a trade_uuid already exists
    async fn trade_uuid_exists(&self, trade_uuid: &str) -> AppResult<bool>;

    /// Insert a new trade record
    async fn insert_trade(&self, trade: &InsertTrade) -> AppResult<i64>;

    /// Update trade status
    async fn update_trade_status(&self, update: &UpdateTradeStatus) -> AppResult<()>;

    /// Get trade by UUID
    async fn get_trade_by_uuid(&self, trade_uuid: &str) -> AppResult<Option<Trade>>;

    /// Get queued trades for execution
    async fn get_queued_trades(&self, limit: i32) -> AppResult<Vec<Trade>>;

    /// Get trades by status
    async fn get_trades_by_status(&self, status: &str, limit: i32) -> AppResult<Vec<Trade>>;

    // ========================================================================
    // POSITION OPERATIONS
    // ========================================================================

    /// Insert a new position record
    async fn insert_position(&self, position: &InsertPosition) -> AppResult<i64>;

    /// Update position
    async fn update_position(&self, update: &UpdatePosition) -> AppResult<()>;

    /// Get active positions
    async fn get_active_positions(&self) -> AppResult<Vec<Position>>;

    /// Get position by trade UUID
    async fn get_position_by_trade_uuid(&self, trade_uuid: &str) -> AppResult<Option<Position>>;

    /// Get the active (or EXITING) position for a (wallet, token) pair.
    ///
    /// BUY and SELL signals generate different trade UUIDs, so a SELL/EXIT cannot
    /// look up its position by the SELL signal's UUID. Positions are uniquely held
    /// one-per-token-per-wallet (enforced by the registry), so (wallet, token) is
    /// the correct key for matching an exit to its opening position.
    async fn get_active_position_by_wallet_token(
        &self,
        wallet_address: &str,
        token_address: &str,
    ) -> AppResult<Option<Position>>;

    /// Get the most recent unresolved (PENDING/QUEUED/EXECUTING/PENDING_CONFIRMATION)
    /// trade for a (wallet, token) pair, if any.
    ///
    /// An unconfirmed BUY never inserts a position row, so duplicate admission
    /// must also reject when an unresolved trade already exists for the same
    /// wallet/token — otherwise a second concurrent BUY passes the
    /// position-based pre-check and submits another on-chain order.
    async fn get_unresolved_trade_by_wallet_token(
        &self,
        wallet_address: &str,
        token_address: &str,
    ) -> AppResult<Option<String>>;

    /// Close position
    async fn close_position(
        &self,
        trade_uuid: &str,
        exit_price: rust_decimal::Decimal,
        exit_tx_signature: &str,
        realized_pnl_sol: rust_decimal::Decimal,
        realized_pnl_usd: rust_decimal::Decimal,
    ) -> AppResult<()>;

    /// Force-close an orphaned ACTIVE position whose `token_amount` is NULL.
    ///
    /// Non-destructive: sets state=CLOSED, closed_at=NOW(), zero realized PnL,
    /// and records the reason in `exit_tx_signature`. Idempotent — the SQL only
    /// matches rows that are still ACTIVE with a NULL token_amount, so a
    /// repeated call (or a row already resolved) is a safe no-op.
    ///
    /// Used by the signal-pipeline BUY path and the main startup/monitor sweep
    /// to free `max_concurrent_positions` slots that would otherwise be
    /// permanently blocked by unsellable positions (paper SELL requires
    /// token_amount).
    async fn force_close_orphan_position(
        &self,
        trade_uuid: &str,
        reason: &str,
    ) -> AppResult<()>;

    // ========================================================================
    // WALLET OPERATIONS
    // ========================================================================

    /// Get wallet by address
    async fn get_wallet(&self, address: &str) -> AppResult<Option<Wallet>>;

    /// Get all active wallets
    async fn get_active_wallets(&self) -> AppResult<Vec<Wallet>>;

    /// Update wallet status
    async fn update_wallet_status(&self, address: &str, status: &str) -> AppResult<()>;

    /// Merge wallet roster from external database
    async fn merge_roster(&self, roster_db_path: &str) -> AppResult<u32>;

    /// Get wallets by status
    async fn get_wallets_by_status(&self, status: &str) -> AppResult<Vec<Wallet>>;

    /// Get ACTIVE or CANDIDATE wallets grouped by conviction tier for tiered polling
    async fn get_wallets_by_conviction_tier(&self, tier: chimera_core::config::ConvictionTier) -> AppResult<Vec<Wallet>>;

    /// Get wallets with WQS scores for batch processing
    async fn get_wallets_with_wqs(
        &self,
        status: Option<&str>,
        min_wqs: Option<i32>,
        max_wqs: Option<i32>,
    ) -> AppResult<Vec<Wallet>>;

    /// Get top CANDIDATE wallets eligible for auto-promotion: status=CANDIDATE,
    /// wqs_score >= min_wqs, last_trade_at within `max_age_days`, ordered by
    /// recency (most recently traded first) then WQS, limited to `limit`.
    /// REJECTED wallets are excluded. Recency-first ordering surfaces wallets
    /// that actually trade rather than dormant high-historical-WQS ones.
    async fn get_promotion_candidates(
        &self,
        min_wqs: f64,
        max_age_days: i64,
        limit: i64,
    ) -> AppResult<Vec<Wallet>>;

    /// Demote ACTIVE wallets whose last on-chain trade is older than
    /// `max_age_days` back to CANDIDATE, freeing roster slots for active
    /// candidates. Returns the number demoted. Bypasses the inactivity-rotation
    /// grace period (which otherwise keeps freshly-promoted dormant wallets
    /// ACTIVE). Orphan webhooks are cleaned up by the webhook-lifecycle task.
    async fn demote_dormant_active_wallets(&self, max_age_days: i64) -> AppResult<u64>;

    // ========================================================================
    // SYSTEM OPERATIONS
    // ========================================================================

    /// Get circuit breaker state
    async fn get_circuit_breaker_state(&self) -> AppResult<CircuitBreakerState>;

    /// Update circuit breaker state
    async fn update_circuit_breaker_state(
        &self,
        state: &str,
        tripped_at: Option<chrono::DateTime<chrono::Utc>>,
        trip_reason: Option<&str>,
    ) -> AppResult<()>;

    /// Get kill switch state
    async fn get_kill_switch_state(&self) -> AppResult<KillSwitchState>;

    /// Set kill switch state
    async fn set_kill_switch_state(&self, state: &str, reason: Option<&str>) -> AppResult<()>;

    /// Insert into dead letter queue
    async fn insert_dlq(
        &self,
        trade_uuid: Option<&str>,
        payload: &str,
        reason: &str,
        error_details: Option<&str>,
        source_ip: Option<&str>,
    ) -> AppResult<i64>;

    /// Get admin wallet role
    async fn get_admin_wallet_role(&self, wallet_address: &str) -> AppResult<Option<String>>;

    // ========================================================================
    // STATISTICS & REPORTING
    // ========================================================================

    /// Get trade statistics
    async fn get_trade_statistics(&self) -> AppResult<TradeStatistics>;

    /// Get recent trades with pagination
    async fn get_recent_trades(&self, limit: i64, offset: i64) -> AppResult<Vec<Trade>>;

    /// Get wallet performance
    async fn get_wallet_performance(
        &self,
        wallet_address: &str,
    ) -> AppResult<Option<WalletPerformance>>;

    /// Get database connection pool statistics
    async fn get_pool_stats(&self) -> AppResult<PoolStats>;

    // ========================================================================
    // JITO TIP HISTORY
    // ========================================================================

    /// Insert a Jito tip record
    async fn insert_jito_tip(
        &self,
        tip_amount_sol: &rust_decimal::Decimal,
        bundle_signature: Option<&str>,
        strategy: Option<&str>,
        success: bool,
    ) -> AppResult<i64>;

    /// Get recent successful tips for percentile calculation
    async fn get_recent_jito_tips(&self, limit: i32) -> AppResult<Vec<rust_decimal::Decimal>>;

    /// Get count of successful tips (for cold start detection)
    async fn get_jito_tip_count(&self) -> AppResult<u32>;

    /// Clean up old tip history (keep only last 7 days)
    async fn prune_old_jito_tips(&self) -> AppResult<u64>;

    // ========================================================================
    // PnL QUERIES
    // ========================================================================

    /// Get PnL for a trailing window (from_hours to to_hours ago)
    async fn get_pnl_window(
        &self,
        from_hours: &str,
        to_hours: Option<&str>,
    ) -> AppResult<rust_decimal::Decimal>;

    /// Get total PnL for the last 24 hours
    async fn get_pnl_24h(&self) -> AppResult<rust_decimal::Decimal>;

    /// Get total PnL for the last 7 days
    async fn get_pnl_7d(&self) -> AppResult<rust_decimal::Decimal>;

    /// Get total PnL for the last 30 days
    async fn get_pnl_30d(&self) -> AppResult<rust_decimal::Decimal>;

    /// Get cumulative realized PnL (all-time, from CLOSED positions)
    async fn get_total_realized_pnl(&self) -> AppResult<rust_decimal::Decimal>;

    /// Record one mark-to-market NAV snapshot (equity-curve point).
    async fn record_portfolio_snapshot(
        &self,
        nav_sol: rust_decimal::Decimal,
        capital_sol: rust_decimal::Decimal,
        realized_pnl_sol: rust_decimal::Decimal,
        unrealized_pnl_sol: rust_decimal::Decimal,
        open_positions: i32,
        sol_price_usd: Option<rust_decimal::Decimal>,
        trade_mode: Option<String>,
    ) -> AppResult<()>;

    /// Read the NAV time series for the last `days` days (oldest first).
    async fn get_portfolio_nav_history(
        &self,
        days: u32,
    ) -> AppResult<Vec<crate::db_abstraction::types::PortfolioSnapshot>>;

    /// Delete NAV snapshots older than `days` days. Returns rows deleted.
    async fn delete_portfolio_snapshots_before(&self, days: i32) -> AppResult<u64>;

    /// Get total capital deployed (sum of entry_amount_sol for CLOSED positions) in the last 30 days
    async fn get_capital_deployed_30d(&self) -> AppResult<rust_decimal::Decimal>;

    /// Cancel PENDING/QUEUED trades older than max_age_minutes. Returns count of cancelled trades.
    async fn cancel_stale_trades(&self, max_age_minutes: i32) -> AppResult<u64>;

    /// Get strategy performance metrics (win rate, avg return, trade count)
    async fn get_strategy_performance(
        &self,
        strategy: &str,
        days: i32,
    ) -> AppResult<(f64, rust_decimal::Decimal, u32)>;

    // ========================================================================
    // LOSS TRACKING
    // ========================================================================

    /// Get count of consecutive losses
    async fn get_consecutive_losses(&self) -> AppResult<u32>;

    /// Get count of consecutive losses, counting only CLOSED trades created
    /// AFTER `since` (if provided). Used by the circuit breaker so a manual
    /// reset (which sets the baseline) doesn't immediately re-trip on the
    /// historical losing streak still present in the trades table.
    async fn get_consecutive_losses_since(
        &self,
        since: Option<chrono::DateTime<chrono::Utc>>,
    ) -> AppResult<u32>;

    /// Get drawdown percentages from peak.
    ///
    /// Returns `(current_drawdown_percent, max_drawdown_percent)`: the
    /// drawdown at the current point in time, and the historical worst
    /// peak-to-trough drawdown over the closed-trade series.
    async fn get_max_drawdown_percent(
        &self,
        total_capital_sol: rust_decimal::Decimal,
    ) -> AppResult<(rust_decimal::Decimal, rust_decimal::Decimal)>;

    // ========================================================================
    // POSITIONS - ADVANCED OPERATIONS
    // ========================================================================

    /// Atomically mark a trade ACTIVE and insert the corresponding position row
    #[allow(clippy::too_many_arguments)]
    async fn activate_trade_and_open_position(
        &self,
        trade_uuid: &str,
        wallet_address: &str,
        token_address: &str,
        token_symbol: Option<&str>,
        strategy: &str,
        amount_sol: rust_decimal::Decimal,
        entry_price: rust_decimal::Decimal,
        tx_signature: &str,
        max_heat_sol: Option<rust_decimal::Decimal>,
        entry_sol_price_usd: Option<rust_decimal::Decimal>,
    ) -> AppResult<()>;

    /// Atomic portfolio heat check and position open with retry
    #[allow(clippy::too_many_arguments)]
    async fn atomic_portfolio_heat_check_and_open_position(
        &self,
        trade_uuid: &str,
        wallet_address: &str,
        token_address: &str,
        token_symbol: Option<&str>,
        strategy: &str,
        amount_sol: rust_decimal::Decimal,
        entry_price: rust_decimal::Decimal,
        tx_signature: &str,
        max_heat_sol: Option<rust_decimal::Decimal>,
        entry_sol_price_usd: Option<rust_decimal::Decimal>,
    ) -> AppResult<()>;

    /// Close a position from a successful sell trade (full version with partial close support)
    #[allow(clippy::too_many_arguments)]
    async fn close_position_full(
        &self,
        trade_uuid: &str,
        wallet_address: &str,
        token_address: &str,
        exit_price: rust_decimal::Decimal,
        signature: &str,
        sol_price_usd: Option<rust_decimal::Decimal>,
        exit_fraction: rust_decimal::Decimal,
        confirmed: bool,
    ) -> AppResult<bool>;

    async fn update_position_token_amount(
        &self,
        trade_uuid: &str,
        token_amount: u64,
    ) -> AppResult<()>;

    /// Revert a failed exit transaction for a position back to ACTIVE state
    async fn revert_position_exit(&self, position_trade_uuid: &str) -> AppResult<()>;

    /// Get positions stuck in EXITING state for too long
    async fn get_stuck_positions(&self, stuck_seconds: i64) -> AppResult<Vec<PositionRecord>>;

    /// Update position state
    async fn update_position_state(&self, trade_uuid: &str, new_state: &str) -> AppResult<()>;

    /// Update position unrealized PnL for active/exiting positions
    async fn update_position_unrealized_pnl(
        &self,
        trade_uuid: &str,
        current_price: rust_decimal::Decimal,
        pnl_sol: rust_decimal::Decimal,
        pnl_pct: rust_decimal::Decimal,
    ) -> AppResult<()>;

    /// Fetch all ACTIVE positions with their entry data for monitoring
    async fn get_active_positions_with_entry(&self) -> AppResult<Vec<ActivePositionEntry>>;

    /// Get trade_uuid, token_address, entry_price, and size for all ACTIVE/EXITING positions
    async fn get_active_position_tokens(&self) -> AppResult<Vec<ActivePositionSummary>>;

    /// Get the peak price recorded for a position
    async fn get_position_peak_price(&self, trade_uuid: &str) -> AppResult<Option<String>>;

    // ========================================================================
    // WALLET OPERATIONS - ADVANCED
    // ========================================================================

    /// Add or update a wallet (atomic upsert)
    #[allow(clippy::too_many_arguments)]
    async fn upsert_wallet(
        &self,
        address: &str,
        wqs_score: Option<rust_decimal::Decimal>,
        roi_7d: Option<rust_decimal::Decimal>,
        roi_30d: Option<rust_decimal::Decimal>,
        trade_count_30d: Option<i32>,
        win_rate: Option<rust_decimal::Decimal>,
        max_drawdown_30d: Option<rust_decimal::Decimal>,
        avg_trade_size_sol: Option<rust_decimal::Decimal>,
        notes: Option<&str>,
    ) -> AppResult<bool>;

    /// Update wallet status with optional TTL and reason
    async fn update_wallet_status_ext(
        &self,
        address: &str,
        status: &str,
        ttl_hours: Option<i32>,
        reason: Option<&str>,
    ) -> AppResult<bool>;

    /// Get wallets with expired TTL that need to be demoted
    async fn get_expired_ttl_wallets(&self) -> AppResult<Vec<String>>;

    /// Demote a wallet from ACTIVE to CANDIDATE (for TTL expiration)
    async fn demote_wallet(&self, address: &str, reason: &str) -> AppResult<()>;

    /// Check if a token had a significant losing trade closed within the
    /// cooldown window. Used to prevent re-entering tokens that just dumped.
    async fn has_recent_token_loss(
        &self,
        token_address: &str,
        within_minutes: i64,
    ) -> AppResult<bool>;

    // ========================================================================
    // WALLET MONITORING
    // ========================================================================

    /// Get wallet monitoring information
    async fn get_wallet_monitoring(
        &self,
        wallet_address: &str,
    ) -> AppResult<Option<WalletMonitoring>>;

    /// Find an existing Helius webhook that has room for more wallets
    /// (fewer than `max_wallets` accounts). Returns the fullest such webhook
    /// so batching minimizes the number of webhooks used — the Helius plan
    /// caps total webhooks (50), but each supports many accountAddresses.
    async fn find_webhook_with_capacity(
        &self,
        max_wallets: i64,
    ) -> AppResult<Option<String>>;

    /// Clear the helius_webhook_id for ALL wallets referencing a given
    /// webhook ID (used when the webhook no longer exists in Helius).
    async fn clear_webhook_id_for_webhook(&self, webhook_id: &str) -> AppResult<()>;

    /// Insert or update wallet monitoring record
    async fn upsert_wallet_monitoring(
        &self,
        wallet_address: &str,
        helius_webhook_id: Option<&str>,
        monitoring_enabled: bool,
    ) -> AppResult<()>;

    /// Update wallet monitoring last transaction signature
    async fn update_wallet_monitoring_signature(
        &self,
        wallet_address: &str,
        signature: &str,
    ) -> AppResult<()>;

    /// Get wallets that need webhook registration (ACTIVE but no webhook)
    async fn get_wallets_needing_webhook_registration(&self) -> AppResult<Vec<String>>;

    /// Get ACTIVE wallets that have a webhook_id stored (for staleness verification)
    async fn get_active_wallets_with_webhook_ids(&self) -> AppResult<Vec<(String, String)>>;

    /// Clear a stale webhook_id from wallet_monitoring (sets to NULL)
    async fn clear_webhook_id(&self, wallet_address: &str) -> AppResult<()>;

    /// Get stale webhook wallets for cleanup (inactive for threshold days)
    async fn get_stale_webhook_wallets(&self, threshold_days: i32) -> AppResult<Vec<String>>;

    /// Get all wallet monitoring records for webhook reconciliation
    async fn get_all_wallet_monitoring(&self) -> AppResult<Vec<WalletMonitoring>>;

    /// Update webhook health status with timestamp
    async fn update_webhook_health_status(
        &self,
        wallet_address: &str,
        health_status: &str,
        webhook_id: Option<&str>,
    ) -> AppResult<()>;

    /// Update webhook status (active, paused, failed, orphaned)
    async fn update_webhook_status(
        &self,
        wallet_address: &str,
        webhook_status: &str,
    ) -> AppResult<()>;

    /// Update last speculative signal timestamp for a wallet
    async fn update_last_speculative_signal(&self, wallet_address: &str, timestamp: chrono::DateTime<chrono::Utc>) -> AppResult<()>;

    /// Get inactivity demotion count for a wallet
    async fn get_inactivity_demotion_count(&self, wallet_address: &str) -> AppResult<i32>;

    /// Increment inactivity demotion count for a wallet
    async fn increment_inactivity_demotion_count(&self, wallet_address: &str) -> AppResult<()>;

    /// Reset inactivity demotion count for a wallet
    async fn reset_inactivity_demotion_count(&self, wallet_address: &str) -> AppResult<()>;

    /// Log webhook lifecycle event with comprehensive tracking
    #[allow(clippy::too_many_arguments)]
    async fn log_webhook_lifecycle_event(
        &self,
        wallet_address: &str,
        action: &str,
        status: &str,
        webhook_id: Option<&str>,
        details: Option<&str>,
        error_message: Option<&str>,
        duration_ms: Option<i32>,
    ) -> AppResult<()>;

    /// Increment webhook registration attempts with error tracking
    async fn increment_webhook_registration_attempts(
        &self,
        wallet_address: &str,
        error: Option<&str>,
    ) -> AppResult<()>;

    /// Get webhook configuration for change detection
    async fn get_webhook_configuration(&self, key: &str) -> AppResult<Option<String>>;

    /// Update webhook configuration with audit trail
    async fn update_webhook_configuration(
        &self,
        key: &str,
        value: &str,
        updated_by: &str,
    ) -> AppResult<()>;

    /// Get orphaned webhooks (exist in Helius but not in our database)
    async fn get_orphaned_webhooks(&self, helius_webhook_ids: &[String]) -> AppResult<Vec<String>>;

    // ========================================================================
    // EXIT TARGETS
    // ========================================================================

    /// Upsert profit target state for a position
    #[allow(clippy::too_many_arguments)]
    async fn upsert_exit_target(
        &self,
        trade_uuid: &str,
        entry_price: rust_decimal::Decimal,
        entry_amount_sol: rust_decimal::Decimal,
        peak_price: rust_decimal::Decimal,
        peak_profit_percent: rust_decimal::Decimal,
        targets_hit_json: &str,
        trailing_stop_active: bool,
        trailing_stop_price: rust_decimal::Decimal,
        remaining_fraction: rust_decimal::Decimal,
    ) -> AppResult<()>;

    /// Load saved profit target state for a position
    async fn load_exit_target(&self, trade_uuid: &str) -> AppResult<Option<ExitTargetData>>;

    /// Delete profit target state for a closed position
    async fn delete_exit_target(&self, trade_uuid: &str) -> AppResult<()>;

    // ========================================================================
    // RECONCILIATION
    // ========================================================================

    /// Insert reconciliation log entry
    async fn insert_reconciliation_log(
        &self,
        trade_uuid: &str,
        expected_state: &str,
        actual_on_chain: Option<&str>,
        discrepancy: &str,
        on_chain_tx_signature: Option<&str>,
        notes: Option<&str>,
    ) -> AppResult<i64>;

    /// Get current reconciliation status with recent discrepancies
    async fn get_reconciliation_status(
        &self,
        discrepancies_limit: i32,
    ) -> AppResult<ReconciliationStatus>;

    /// Get reconciliation history (grouped by day)
    async fn get_reconciliation_history(&self, limit: i32) -> AppResult<Vec<ReconciliationRun>>;

    /// Count total reconciliation runs
    async fn count_reconciliation_runs(&self) -> AppResult<i64>;

    /// Get reconciliation statistics
    async fn get_reconciliation_stats(&self, time_range: &str) -> AppResult<ReconciliationStats>;

    /// Resolve a discrepancy by ID
    async fn resolve_discrepancy(
        &self,
        id: i64,
        resolved_by: &str,
        resolution: &str,
    ) -> AppResult<()>;

    // ========================================================================
    // TRADES - FILTERED QUERIES
    // ========================================================================

    /// Get trades with optional filters for API and export
    #[allow(clippy::too_many_arguments)]
    async fn get_trades_filtered(
        &self,
        from_date: Option<&str>,
        to_date: Option<&str>,
        status_filter: Option<&str>,
        strategy_filter: Option<&str>,
        wallet_address_filter: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> AppResult<Vec<TradeDetail>>;

    /// Count trades with optional filters (for pagination)
    async fn count_trades_filtered(
        &self,
        from_date: Option<&str>,
        to_date: Option<&str>,
        status_filter: Option<&str>,
        strategy_filter: Option<&str>,
        wallet_address_filter: Option<&str>,
    ) -> AppResult<i64>;

    /// Update trade costs — ACCUMULATES onto existing values. Retrying a call
    /// adds the same costs again (not idempotent).
    async fn update_trade_costs(
        &self,
        trade_uuid: &str,
        jito_tip_sol: rust_decimal::Decimal,
        dex_fee_sol: rust_decimal::Decimal,
        slippage_cost_sol: rust_decimal::Decimal,
    ) -> AppResult<()>;

    /// Atomically mark a trade as DEAD_LETTER and insert into DLQ
    async fn mark_trade_dead_letter(
        &self,
        trade_uuid: &str,
        payload: &str,
        error: &str,
    ) -> AppResult<()>;

    // ========================================================================
    // CONFIG AUDIT
    // ========================================================================

    /// Log a configuration change
    async fn log_config_change(
        &self,
        key: &str,
        old_value: Option<&str>,
        new_value: &str,
        changed_by: &str,
        reason: Option<&str>,
    ) -> AppResult<()>;

    // ========================================================================
    // INCIDENTS API (Dead Letter Queue & Config Audit)
    // ========================================================================

    /// Get dead letter queue items
    async fn get_dead_letter_entries(
        &self,
        limit: i32,
        offset: i32,
    ) -> AppResult<Vec<DeadLetterItem>>;

    /// Get a single dead letter queue item by trade UUID
    async fn get_dead_letter_entry(
        &self,
        trade_uuid: &str,
    ) -> AppResult<Option<DeadLetterItem>>;

    /// Count dead letter queue items
    async fn count_dead_letter_entries(&self) -> AppResult<i64>;

    /// Get retryable DLQ items (can_retry = true, processed_at IS NULL)
    async fn get_retryable_dlq_items(&self, limit: i64) -> AppResult<Vec<RetryableDlqItem>>;

    /// Update DLQ item retry count and optionally mark as processed
    async fn update_dlq_item(
        &self,
        trade_uuid: &str,
        retry_count: i64,
        can_retry: bool,
        mark_processed: bool,
    ) -> AppResult<()>;

    /// Batch update multiple DLQ items in a single transaction
    async fn update_dlq_items_batch(&self, items: Vec<UpdateDlqItemParams>) -> AppResult<usize>;

    /// Get config audit log
    async fn get_config_audit_entries(
        &self,
        limit: i32,
        offset: i32,
    ) -> AppResult<Vec<ConfigAuditItem>>;

    /// Get config audit entries filtered by key prefix (newest first)
    async fn get_config_audit_entries_by_key_prefix(
        &self,
        prefix: &str,
        limit: i32,
    ) -> AppResult<Vec<ConfigAuditItem>>;

    /// Count config audit entries
    async fn count_config_audit_entries(&self) -> AppResult<i64>;

    // ========================================================================
    // WEBHOOK AUDIT LOG
    // ========================================================================

    /// Get webhook lifecycle audit log entries with optional filters
    async fn get_webhook_audit_log(
        &self,
        wallet_address: Option<&str>,
        action: Option<&str>,
        status: Option<&str>,
        limit: Option<i64>,
    ) -> AppResult<Vec<WebhookAuditLog>>;

    // ========================================================================
    // TRADE STATISTICS
    // ========================================================================

    /// Get count of trades in a specific status
    async fn count_trades_by_status(&self, status: &str) -> AppResult<i64>;

    /// Count closed trades for a specific wallet
    async fn get_closed_trade_count_for_wallet(&self, wallet_address: &str) -> AppResult<i64>;

    /// Closed copy-trade count and realized net PnL for a wallet — the
    /// "proven wallet" basis for the consensus-OR-proven admission gate.
    /// Reads the live `trades` ledger (the `wallet_copy_performance` table is
    /// currently unmaintained and would keep the proven branch permanently
    /// empty). Returns (closed_trade_count, sum(net_pnl_sol)).
    async fn get_wallet_copy_stats(
        &self,
        wallet_address: &str,
    ) -> AppResult<(i64, rust_decimal::Decimal)>;

    /// Rolling shadow-mirror average PnL% for a token (`exit_strategy =
    /// 'mirror_main'` — the whale's own round trip under our exit rails,
    /// pre-cost). Returns `Some(avg)` only when at least `min_samples` exits
    /// exist within the `window_hours` window, else `None`. Drives the
    /// shadow-mirror admission gate (token-level EV).
    async fn get_token_mirror_avg_pnl(
        &self,
        token_address: &str,
        window_hours: i32,
        min_samples: i32,
    ) -> AppResult<Option<rust_decimal::Decimal>>;

    /// Get wallet copy performance metrics
    async fn get_wallet_copy_performance(
        &self,
        wallet_address: &str,
    ) -> AppResult<Option<WalletCopyPerformance>>;

    /// Get trade latency statistics including percentiles
    async fn get_trade_latency_stats(&self, hours: i32) -> AppResult<TradeLatencyStats>;

    /// Get trade latency histogram data for visualization
    async fn get_trade_latency_histogram(
        &self,
        hours: i32,
        bucket_bounds: &[f64],
    ) -> AppResult<Vec<LatencyBucket>>;

    // ========================================================================
    // API CONVENIENCE METHODS
    // ========================================================================

    /// Get positions with optional state filter (returns API detail type)
    async fn get_positions(&self, state_filter: Option<&str>) -> AppResult<Vec<PositionDetail>>;

    /// Get wallets with optional status filter (returns API detail type)
    async fn get_wallets(&self, status_filter: Option<&str>) -> AppResult<Vec<WalletDetail>>;

    // ========================================================================
    // POOL ACCESS (for raw sqlx queries in helpers)
    // ========================================================================

    /// Get a reference to the underlying database pool
    fn pool(&self) -> DbPool;

    /// Get pool statistics for monitoring
    fn pool_stats(&self) -> PoolStats {
        let pool = self.pool();
        PoolStats {
            active_connections: pool.size() - pool.num_idle(),
            idle_connections: pool.num_idle(),
            max_connections: pool.max_connections(),
            utilization_percent: pool.utilization() * 100.0,
        }
    }

    // ========================================================================
    // ATOMIC BATCH OPERATIONS
    // ========================================================================

    /// Atomic operation: Insert trade and create position in a single transaction
    ///
    /// This is used for trade entry to ensure both the trade record and position
    /// are created atomically, preventing inconsistent state.
    async fn insert_trade_and_create_position(
        &self,
        trade: &InsertTrade,
        position: &InsertPosition,
    ) -> AppResult<i64>;

    /// Atomic operation: Update trade status and position state in a single transaction
    ///
    /// This is used for trade lifecycle transitions to ensure both the trade status
    /// and position state remain consistent.
    async fn update_trade_status_and_position(
        &self,
        trade_uuid: &str,
        trade_status: &str,
        position_state: Option<&str>,
    ) -> AppResult<()>;

    /// Default implementation for atomic trade insertion.
    ///
    /// A genuine transaction cannot be expressed through this trait generically,
    /// so backends must override this with real transaction support (as
    /// `PostgresBackend` does). The non-transactional fallback is not used:
    /// pretending two independent writes succeeded would leave orphaned rows.
    async fn insert_trade_and_create_position_default(
        &self,
        _trade: &InsertTrade,
        _position: &InsertPosition,
    ) -> AppResult<i64> {
        Err(AppError::Internal(
            "insert_trade_and_create_position requires a transactional backend override".to_string(),
        ))
    }

    /// Default implementation for atomic status update.
    ///
    /// Updates the trade status and forwards the position-state change to
    /// `update_position_state`. Backends should override this with a genuine
    /// transaction (as `PostgresBackend` does).
    async fn update_trade_status_and_position_default(
        &self,
        trade_uuid: &str,
        trade_status: &str,
        position_state: Option<&str>,
    ) -> AppResult<()> {
        let update = UpdateTradeStatus {
            trade_uuid: trade_uuid.to_string(),
            status: trade_status.to_string(),
            tx_signature: None,
            error_message: None,
            network_fee_sol: None,
        };

        timed_query("update_trade_status_and_position_update_trade", self.update_trade_status(&update)).await?;

        if let Some(state) = position_state {
            self.update_position_state(trade_uuid, state).await?;
        }

        Ok(())
    }

    /// Get circuit breaker evaluation data: (unrealized_sol, realized_sol_24h, realized_usd_24h, null_price_sol_24h)
    async fn get_evaluation_data(
        &self,
    ) -> AppResult<(
        rust_decimal::Decimal,
        rust_decimal::Decimal,
        rust_decimal::Decimal,
        rust_decimal::Decimal,
    )>;
}

// ========================================================================
// DATA STRUCTURES
// ========================================================================

/// Trade record
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Trade {
    pub id: i64,
    pub trade_uuid: String,
    pub wallet_address: String,
    pub token_address: String,
    pub token_symbol: Option<String>,
    pub strategy: String,
    pub side: String,
    pub amount_sol: rust_decimal::Decimal,
    pub price_at_signal: Option<rust_decimal::Decimal>,
    pub tx_signature: Option<String>,
    pub status: String,
    pub retry_count: i32,
    pub error_message: Option<String>,
    pub pnl_sol: Option<rust_decimal::Decimal>,
    pub pnl_usd: Option<rust_decimal::Decimal>,
    pub jito_tip_sol: rust_decimal::Decimal,
    pub dex_fee_sol: rust_decimal::Decimal,
    pub slippage_cost_sol: rust_decimal::Decimal,
    pub total_cost_sol: rust_decimal::Decimal,
    pub net_pnl_sol: Option<rust_decimal::Decimal>,
    pub pnl_data_valid: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Position record
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Position {
    pub id: i64,
    pub trade_uuid: String,
    pub wallet_address: String,
    pub token_address: String,
    pub token_symbol: Option<String>,
    pub strategy: String,
    pub entry_amount_sol: rust_decimal::Decimal,
    pub entry_price: rust_decimal::Decimal,
    pub entry_tx_signature: String,
    pub current_price: Option<rust_decimal::Decimal>,
    pub unrealized_pnl_sol: Option<rust_decimal::Decimal>,
    pub unrealized_pnl_percent: Option<rust_decimal::Decimal>,
    pub state: String,
    pub exit_price: Option<rust_decimal::Decimal>,
    pub exit_tx_signature: Option<String>,
    pub realized_pnl_sol: Option<rust_decimal::Decimal>,
    pub realized_pnl_usd: Option<rust_decimal::Decimal>,
    pub entry_sol_price_usd: Option<rust_decimal::Decimal>,
    pub opened_at: chrono::DateTime<chrono::Utc>,
    pub last_updated: chrono::DateTime<chrono::Utc>,
    pub closed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub token_amount: Option<rust_decimal::Decimal>,
}

/// Wallet record
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Wallet {
    pub id: i64,
    pub address: String,
    pub status: String,
    pub wqs_score: Option<rust_decimal::Decimal>,
    pub wqs_confidence: Option<rust_decimal::Decimal>,
    pub roi_7d: Option<rust_decimal::Decimal>,
    pub roi_30d: Option<rust_decimal::Decimal>,
    pub trade_count_30d: Option<i32>,
    pub win_rate: Option<rust_decimal::Decimal>,
    pub max_drawdown_30d: Option<rust_decimal::Decimal>,
    pub avg_trade_size_sol: Option<rust_decimal::Decimal>,
    pub avg_win_sol: Option<rust_decimal::Decimal>,
    pub avg_loss_sol: Option<rust_decimal::Decimal>,
    pub profit_factor: Option<rust_decimal::Decimal>,
    pub realized_pnl_30d_sol: Option<rust_decimal::Decimal>,
    pub last_trade_at: Option<chrono::DateTime<chrono::Utc>>,
    pub promoted_at: Option<chrono::DateTime<chrono::Utc>>,
    pub ttl_expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub notes: Option<String>,
    pub archetype: Option<String>,
    pub avg_entry_delay_seconds: Option<rust_decimal::Decimal>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Circuit breaker state
#[derive(Debug, Clone)]
pub struct CircuitBreakerState {
    pub state: String,
    pub tripped_at: Option<String>,
    pub trip_reason: Option<String>,
    pub updated_at: String,
}

/// Kill switch state
#[derive(Debug, Clone)]
pub struct KillSwitchState {
    pub state: String,
    pub changed_at: String,
    pub changed_by: String,
    pub reason: Option<String>,
}

/// Trade statistics
#[derive(Debug, Clone, serde::Serialize)]
pub struct TradeStatistics {
    pub total_trades: i64,
    pub successful_trades: i64,
    pub failed_trades: i64,
    pub total_pnl_sol: rust_decimal::Decimal,
    pub total_volume_sol: rust_decimal::Decimal,
}

/// Wallet performance
#[derive(Debug, Clone, serde::Serialize)]
pub struct WalletPerformance {
    pub wallet_address: String,
    pub copy_pnl_7d: rust_decimal::Decimal,
    pub copy_pnl_30d: rust_decimal::Decimal,
    pub signal_success_rate: rust_decimal::Decimal,
    pub total_trades: i64,
    pub winning_trades: i64,
}

// ========================================================================
// FACTORY FUNCTION
// ========================================================================

/// Database mode selection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseMode {
    /// PostgreSQL only (production)
    PostgreSQLOnly,
}

impl DatabaseMode {
    pub fn from_env() -> Self {
        match std::env::var("CHIMERA_DB_MODE")
            .as_deref()
            .unwrap_or("postgres")
            .to_lowercase()
            .as_str()
        {
            "postgres" | "postgresql" | "postgres-only" => DatabaseMode::PostgreSQLOnly,
            other => {
                tracing::warn!(
                    mode = other,
                    "Unknown CHIMERA_DB_MODE value — defaulting to PostgreSQL"
                );
                DatabaseMode::PostgreSQLOnly
            }
        }
    }
}

/// Create database instance based on configuration
pub async fn create_database(config: &DatabaseConfig) -> AppResult<Arc<dyn Database>> {
    tracing::info!("Using PostgreSQL-only mode");
    Ok(Arc::new(postgres::PostgresBackend::new(config).await?))
}

// ========================================================================
// HELPERS — TEXT ↔ Decimal for financial values
// ========================================================================

/// Parse a TEXT (Decimal string) column value to Decimal, returning an error
/// on failure instead of silently defaulting to zero. Use in all critical
/// paths (price, PnL, amount reads) so corrupted values surface rather than
/// flowing into PnL, position sizing, or risk calculations as a zero.
pub fn text_to_dec(s: &str) -> AppResult<rust_decimal::Decimal> {
    rust_decimal::Decimal::from_str(s)
        .map_err(|e| AppError::Internal(format!("Failed to parse Decimal from '{}': {}", s, e)))
}

/// Format a Decimal as a TEXT column value
pub fn dec_to_text(val: &rust_decimal::Decimal) -> String {
    val.to_string()
}

/// Parse an optional TEXT column value
pub fn opt_text_to_dec(val: Option<&str>) -> Option<rust_decimal::Decimal> {
    val.and_then(|s| rust_decimal::Decimal::from_str(s).ok())
}

/// Format an optional Decimal as an optional TEXT value
pub fn opt_dec_to_text(val: Option<&rust_decimal::Decimal>) -> Option<String> {
    val.as_ref().map(|d| d.to_string())
}

/// Convert chrono DateTime to string
pub fn datetime_to_string(dt: chrono::DateTime<chrono::Utc>) -> String {
    dt.to_rfc3339()
}

/// Parse string to chrono DateTime
pub fn string_to_datetime(s: &str) -> Result<chrono::DateTime<chrono::Utc>, AppError> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .map_err(|e| AppError::Internal(format!("Invalid datetime format: {}", e)))
}
