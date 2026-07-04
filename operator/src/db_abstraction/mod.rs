//! Database abstraction layer
//!
//! Provides a unified interface for SQLite and PostgreSQL backends.
//! Select backend via CHIMERA_DB_MODE environment variable (sqlite | postgres).

pub mod export;
pub mod postgres;
pub mod sqlite;
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

use crate::error::{AppError, AppResult};
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

    /// Update trade with execution results
    async fn update_trade_execution(
        &self,
        trade_uuid: &str,
        tx_signature: &str,
        jito_tip_sol: rust_decimal::Decimal,
        dex_fee_sol: rust_decimal::Decimal,
        slippage_cost_sol: rust_decimal::Decimal,
    ) -> AppResult<()>;

    /// Update trade PnL
    async fn update_trade_pnl(
        &self,
        trade_uuid: &str,
        pnl_sol: rust_decimal::Decimal,
        pnl_usd: rust_decimal::Decimal,
    ) -> AppResult<()>;

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

    /// Close position
    async fn close_position(
        &self,
        trade_uuid: &str,
        exit_price: rust_decimal::Decimal,
        exit_tx_signature: &str,
        realized_pnl_sol: rust_decimal::Decimal,
        realized_pnl_usd: rust_decimal::Decimal,
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

    // ========================================================================
    // SYSTEM OPERATIONS
    // ========================================================================

    /// Get circuit breaker state
    async fn get_circuit_breaker_state(&self) -> AppResult<CircuitBreakerState>;

    /// Update circuit breaker state
    async fn update_circuit_breaker_state(
        &self,
        state: &str,
        tripped_at: Option<&str>,
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

    /// Get max drawdown percent from peak
    async fn get_max_drawdown_percent(
        &self,
        total_capital_sol: rust_decimal::Decimal,
    ) -> AppResult<rust_decimal::Decimal>;

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
    ) -> AppResult<()>;

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

    // ========================================================================
    // WALLET MONITORING
    // ========================================================================

    /// Get wallet monitoring information
    async fn get_wallet_monitoring(
        &self,
        wallet_address: &str,
    ) -> AppResult<Option<WalletMonitoring>>;

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

    /// Update trade costs (idempotent - adds to existing values)
    async fn update_trade_costs(
        &self,
        trade_uuid: &str,
        jito_tip_sol: rust_decimal::Decimal,
        dex_fee_sol: rust_decimal::Decimal,
        slippage_cost_sol: rust_decimal::Decimal,
    ) -> AppResult<()>;

    /// Update trade net PnL (after costs)
    async fn update_trade_net_pnl(
        &self,
        trade_uuid: &str,
        net_pnl_sol: rust_decimal::Decimal,
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
            max_connections: pool.size(),
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

    /// Default implementation for atomic trade insertion (non-transactional fallback)
    ///
    /// Backends can override this with proper transaction support.
    fn insert_trade_and_create_position_default(
        &self,
        trade: &InsertTrade,
        position: &InsertPosition,
    ) -> AppResult<i64> {
        // Fallback: execute as separate operations
        let trade_id = timed_query("insert_trade_and_create_position_insert_trade", self.insert_trade(trade)).await?;

        // Create position with the trade_id
        let mut position = position.clone();
        // Note: InsertPosition would need a trade_id field for proper foreign key
        timed_query("insert_trade_and_create_position_insert_position", self.insert_position(&position)).await?;

        Ok(trade_id)
    }

    /// Default implementation for atomic status update (non-transactional fallback)
    ///
    /// Backends can override this with proper transaction support.
    fn update_trade_status_and_position_default(
        &self,
        trade_uuid: &str,
        trade_status: &str,
        position_state: Option<&str>,
    ) -> AppResult<()> {
        // Update trade status
        let update = UpdateTradeStatus {
            trade_uuid: trade_uuid.to_string(),
            status: trade_status.to_string(),
            tx_signature: None,
            error_message: None,
            updated_at: chrono::Utc::now().to_string(),
        };

        timed_query("update_trade_status_and_position_update_trade", self.update_trade_status(&update)).await?;

        // Update position state if provided
        if let Some(state) = position_state {
            // Would need to implement update_position_state method
            tracing::debug!("Position state update requested: {}", state);
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
    /// SQLite only (development/staging)
    SQLiteOnly,
    /// PostgreSQL only (production)
    PostgreSQLOnly,
}

impl DatabaseMode {
    /// Get mode from environment variable
    pub fn from_env() -> Self {
        match std::env::var("CHIMERA_DB_MODE")
            .as_deref()
            .unwrap_or("sqlite")
            .to_lowercase()
            .as_str()
        {
            "postgres" | "postgresql" | "postgres-only" => DatabaseMode::PostgreSQLOnly,
            _ => DatabaseMode::SQLiteOnly,
        }
    }
}

/// Create database instance based on configuration
pub async fn create_database(config: &DatabaseConfig) -> AppResult<Arc<dyn Database>> {
    let mode = DatabaseMode::from_env();

    match mode {
        DatabaseMode::SQLiteOnly => {
            tracing::info!("Using SQLite-only mode");
            Ok(Arc::new(sqlite::SqliteBackend::new(config).await?))
        }
        DatabaseMode::PostgreSQLOnly => {
            tracing::info!("Using PostgreSQL-only mode");
            Ok(Arc::new(postgres::PostgresBackend::new(config).await?))
        }
    }
}

// ========================================================================
// HELPERS — TEXT ↔ Decimal for financial values
// ========================================================================

/// Parse a TEXT (Decimal string) column value to Decimal
pub fn text_to_dec(s: &str) -> rust_decimal::Decimal {
    rust_decimal::Decimal::from_str(s).unwrap_or_else(|_| {
        if !s.is_empty() {
            tracing::warn!(raw_value = %s, "Failed to parse TEXT column as Decimal — using 0");
        }
        rust_decimal::Decimal::ZERO
    })
}

/// Parse a TEXT column value as Decimal, returning an error on failure instead of silently defaulting to 0.
/// Use in critical paths (price, PnL, amount reads) where corruption must be surfaced.
pub fn text_to_dec_res(s: &str) -> AppResult<rust_decimal::Decimal> {
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
