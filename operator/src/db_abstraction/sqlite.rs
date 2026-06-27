//! SQLite backend implementation for Database trait

use super::types::DatabaseConfig;
use super::types::SqlitePool;
use super::{
    dec_to_text, opt_text_to_dec, text_to_dec, ActivePositionEntry, ActivePositionSummary,
    CircuitBreakerState, ConfigAuditItem, Database, DbPool, DeadLetterItem, DiscrepancyRow,
    DiscrepancyTypeStats, ExitTargetData, InsertPosition, InsertTrade, KillSwitchState,
    LatencyBucket, Position, PositionDetail, PositionRecord, ReconciliationRun,
    ReconciliationStats, ReconciliationStatus, RetryableDlqItem, Trade, TradeDetail,
    TradeLatencyStats, TradeStatistics, UpdateDlqItemParams, UpdatePosition, UpdateTradeStatus,
    Wallet, WalletCopyPerformance, WalletDetail, WalletMonitoring, WalletPerformance,
    WebhookAuditLog,
};
use crate::error::{AppError, AppResult};
use rust_decimal::prelude::*;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::Row;
use std::collections::HashMap;
use std::str::FromStr;
use tracing::{info, warn};

/// SQLite backend implementation
pub struct SqliteBackend {
    pool: SqlitePool,
}

impl SqliteBackend {
    /// Create new SQLite backend
    pub async fn new(config: &DatabaseConfig) -> AppResult<Self> {
        let pool = Self::init_pool(config).await?;
        Ok(Self { pool })
    }

    /// Initialize SQLite connection pool
    async fn init_pool(config: &DatabaseConfig) -> AppResult<SqlitePool> {
        // Ensure data directory exists
        if let Some(parent) = config.path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    AppError::Database(sqlx::Error::Io(std::io::Error::other(format!(
                        "Failed to create database directory: {}",
                        e
                    ))))
                })?;
                info!("Created database directory: {:?}", parent);
            }
        }

        let db_url = format!("sqlite:{}?mode=rwc", config.path.display());

        let connect_options = SqliteConnectOptions::from_str(&db_url)
            .map_err(AppError::Database)?
            // Enable WAL mode for concurrent reads
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            // Set busy timeout to 60 seconds to cover large roster merges
            .busy_timeout(std::time::Duration::from_secs(60))
            // Enable foreign keys
            .foreign_keys(true)
            // Create if not exists
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(config.max_connections)
            .acquire_timeout(std::time::Duration::from_secs(30))
            .connect_with(connect_options)
            .await?;

        info!(
            "SQLite pool initialized: {:?} (max {} connections)",
            config.path, config.max_connections
        );

        Ok(pool)
    }

    /// Get reference to the pool
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

#[async_trait::async_trait]
impl Database for SqliteBackend {
    // ========================================================================
    // CONNECTION LIFECYCLE
    // ========================================================================

    async fn close(&self) -> AppResult<()> {
        self.pool.close().await;
        Ok(())
    }

    // ========================================================================
    // MIGRATION & STARTUP
    // ========================================================================

    async fn run_migrations(&self) -> AppResult<()> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(|e| AppError::Database(e.into()))?;

        info!("SQLite migrations applied successfully");
        Ok(())
    }

    async fn startup_integrity_check(&self) -> AppResult<()> {
        let result: String = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_one(&self.pool)
            .await
            .map_err(AppError::Database)?;

        if result != "ok" {
            return Err(AppError::Internal(format!(
                "Database integrity check failed: {}",
                result
            )));
        }
        info!("SQLite integrity check passed");
        Ok(())
    }

    async fn recover_executing_trades(&self) -> AppResult<u32> {
        let rows_affected = sqlx::query(
            "UPDATE trades SET status = 'FAILED', error_message = 'Recovered from EXECUTING state after restart' WHERE status = 'EXECUTING'",
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::Database)?
        .rows_affected();

        if rows_affected > 0 {
            warn!(
                count = rows_affected,
                "Recovered EXECUTING-stuck trades → FAILED"
            );
        }
        Ok(rows_affected as u32)
    }

    // ========================================================================
    // TRADE OPERATIONS
    // ========================================================================

    async fn trade_uuid_exists(&self, trade_uuid: &str) -> AppResult<bool> {
        // Check trades table
        let trade_exists: (i32,) =
            sqlx::query_as("SELECT COUNT(*) FROM trades WHERE trade_uuid = ?")
                .bind(trade_uuid)
                .fetch_one(&self.pool)
                .await?;

        if trade_exists.0 > 0 {
            return Ok(true);
        }

        // Check dead letter queue
        let dlq_exists: (i32,) =
            sqlx::query_as("SELECT COUNT(*) FROM dead_letter_queue WHERE trade_uuid = ?")
                .bind(trade_uuid)
                .fetch_one(&self.pool)
                .await?;

        Ok(dlq_exists.0 > 0)
    }

    #[tracing::instrument(skip(self, trade))]
    async fn insert_trade(&self, trade: &InsertTrade) -> AppResult<i64> {
        let result = sqlx::query(
            r#"
            INSERT INTO trades (
                trade_uuid, wallet_address, token_address, token_symbol,
                strategy, side, amount_sol, status
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&trade.trade_uuid)
        .bind(&trade.wallet_address)
        .bind(&trade.token_address)
        .bind(&trade.token_symbol)
        .bind(&trade.strategy)
        .bind(&trade.side)
        .bind(dec_to_text(&trade.amount_sol))
        .bind(&trade.status)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    #[tracing::instrument(skip(self, update))]
    async fn update_trade_status(&self, update: &UpdateTradeStatus) -> AppResult<()> {
        let result = if let Some(sig) = &update.tx_signature {
            sqlx::query(
                r#"
                UPDATE trades
                SET status = ?, tx_signature = ?, error_message = ?,
                    network_fee_sol = COALESCE(?, network_fee_sol)
                WHERE trade_uuid = ?
                "#,
            )
            .bind(&update.status)
            .bind(sig)
            .bind(&update.error_message)
            .bind(update.network_fee_sol.map(|v| dec_to_text(&v)))
            .bind(&update.trade_uuid)
            .execute(&self.pool)
            .await?
        } else {
            sqlx::query(
                r#"
                UPDATE trades
                SET status = ?, error_message = COALESCE(?, error_message),
                    network_fee_sol = COALESCE(?, network_fee_sol)
                WHERE trade_uuid = ?
                "#,
            )
            .bind(&update.status)
            .bind(&update.error_message)
            .bind(update.network_fee_sol.map(|v| dec_to_text(&v)))
            .bind(&update.trade_uuid)
            .execute(&self.pool)
            .await?
        };

        if result.rows_affected() == 0 {
            return Err(AppError::NotFound(format!(
                "Trade not found: {}",
                update.trade_uuid
            )));
        }

        Ok(())
    }

    async fn get_trade_by_uuid(&self, trade_uuid: &str) -> AppResult<Option<Trade>> {
        let row = sqlx::query(
            r#"
            SELECT
                id, trade_uuid, wallet_address, token_address, token_symbol,
                strategy, side, amount_sol, price_at_signal, tx_signature,
                status, retry_count, error_message, pnl_sol, pnl_usd,
                jito_tip_sol, dex_fee_sol, slippage_cost_sol, total_cost_sol,
                net_pnl_sol, created_at, updated_at
            FROM trades
            WHERE trade_uuid = ?
            "#,
        )
        .bind(trade_uuid)
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::Database)?;

        match row {
            Some(r) => Ok(Some(self.row_to_trade(r)?)),
            None => Ok(None),
        }
    }

    async fn get_queued_trades(&self, limit: i32) -> AppResult<Vec<Trade>> {
        let rows = sqlx::query(
            r#"
            SELECT
                id, trade_uuid, wallet_address, token_address, token_symbol,
                strategy, side, amount_sol, price_at_signal, tx_signature,
                status, retry_count, error_message, pnl_sol, pnl_usd,
                jito_tip_sol, dex_fee_sol, slippage_cost_sol, total_cost_sol,
                net_pnl_sol, created_at, updated_at
            FROM trades
            WHERE status = 'QUEUED'
            ORDER BY created_at ASC
            LIMIT ?
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::Database)?;

        rows.into_iter().map(|r| self.row_to_trade(r)).collect()
    }

    async fn get_trades_by_status(&self, status: &str, limit: i32) -> AppResult<Vec<Trade>> {
        let rows = sqlx::query(
            r#"
            SELECT
                id, trade_uuid, wallet_address, token_address, token_symbol,
                strategy, side, amount_sol, price_at_signal, tx_signature,
                status, retry_count, error_message, pnl_sol, pnl_usd,
                jito_tip_sol, dex_fee_sol, slippage_cost_sol, total_cost_sol,
                net_pnl_sol, created_at, updated_at
            FROM trades
            WHERE status = ?
            ORDER BY created_at DESC
            LIMIT ?
            "#,
        )
        .bind(status)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::Database)?;

        rows.into_iter().map(|r| self.row_to_trade(r)).collect()
    }

    async fn update_trade_execution(
        &self,
        trade_uuid: &str,
        tx_signature: &str,
        jito_tip_sol: Decimal,
        dex_fee_sol: Decimal,
        slippage_cost_sol: Decimal,
    ) -> AppResult<()> {
        let total_cost_sol = jito_tip_sol + dex_fee_sol + slippage_cost_sol;

        sqlx::query(
            r#"
            UPDATE trades
            SET tx_signature = ?, jito_tip_sol = ?, dex_fee_sol = ?,
                slippage_cost_sol = ?, total_cost_sol = ?
            WHERE trade_uuid = ?
            "#,
        )
        .bind(tx_signature)
        .bind(dec_to_text(&jito_tip_sol))
        .bind(dec_to_text(&dex_fee_sol))
        .bind(dec_to_text(&slippage_cost_sol))
        .bind(dec_to_text(&total_cost_sol))
        .bind(trade_uuid)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn update_trade_pnl(
        &self,
        trade_uuid: &str,
        pnl_sol: Decimal,
        pnl_usd: Decimal,
    ) -> AppResult<()> {
        let total_cost_str: Option<String> =
            sqlx::query_scalar("SELECT total_cost_sol FROM trades WHERE trade_uuid = ?")
                .bind(trade_uuid)
                .fetch_optional(&self.pool)
                .await?;

        let total_cost_sol = total_cost_str
            .as_deref()
            .map(text_to_dec)
            .unwrap_or(Decimal::ZERO);

        let net_pnl_sol = pnl_sol - total_cost_sol;

        sqlx::query(
            r#"
            UPDATE trades
            SET pnl_sol = ?, pnl_usd = ?, net_pnl_sol = ?
            WHERE trade_uuid = ?
            "#,
        )
        .bind(dec_to_text(&pnl_sol))
        .bind(dec_to_text(&pnl_usd))
        .bind(dec_to_text(&net_pnl_sol))
        .bind(trade_uuid)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    // ========================================================================
    // POSITION OPERATIONS
    // ========================================================================

    #[tracing::instrument(skip(self, position))]
    async fn insert_position(&self, position: &InsertPosition) -> AppResult<i64> {
        let result = sqlx::query(
            r#"
            INSERT INTO positions (
                trade_uuid, wallet_address, token_address, token_symbol,
                strategy, entry_amount_sol, entry_price, entry_tx_signature
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&position.trade_uuid)
        .bind(&position.wallet_address)
        .bind(&position.token_address)
        .bind(&position.token_symbol)
        .bind(&position.strategy)
        .bind(dec_to_text(&position.entry_amount_sol))
        .bind(dec_to_text(&position.entry_price))
        .bind(&position.entry_tx_signature)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    async fn update_position(&self, update: &UpdatePosition) -> AppResult<()> {
        let mut set_clauses: Vec<String> = Vec::new();
        let mut binds: Vec<String> = Vec::new();

        if let Some(price) = update.current_price {
            set_clauses.push("current_price = ?".to_string());
            binds.push(dec_to_text(&price));
        }
        if let Some(pnl) = update.unrealized_pnl_sol {
            set_clauses.push("unrealized_pnl_sol = ?".to_string());
            binds.push(dec_to_text(&pnl));
        }
        if let Some(pnl_pct) = update.unrealized_pnl_percent {
            set_clauses.push("unrealized_pnl_percent = ?".to_string());
            binds.push(dec_to_text(&pnl_pct));
        }
        if let Some(state) = &update.state {
            set_clauses.push("state = ?".to_string());
            binds.push(state.clone());
        }
        if let Some(exit_price) = update.exit_price {
            set_clauses.push("exit_price = ?".to_string());
            binds.push(dec_to_text(&exit_price));
        }
        if let Some(exit_sig) = &update.exit_tx_signature {
            set_clauses.push("exit_tx_signature = ?".to_string());
            binds.push(exit_sig.clone());
        }
        if let Some(pnl) = update.realized_pnl_sol {
            set_clauses.push("realized_pnl_sol = ?".to_string());
            binds.push(dec_to_text(&pnl));
        }
        if let Some(pnl_usd) = update.realized_pnl_usd {
            set_clauses.push("realized_pnl_usd = ?".to_string());
            binds.push(dec_to_text(&pnl_usd));
        }

        if set_clauses.is_empty() {
            return Ok(()); // Nothing to update
        }

        let sql = format!(
            "UPDATE positions SET {} WHERE trade_uuid = ?",
            set_clauses.join(", ")
        );

        let mut query = sqlx::query(&sql);
        for bind in binds {
            query = query.bind(bind);
        }
        query = query.bind(&update.trade_uuid);

        let result = query.execute(&self.pool).await?;

        if result.rows_affected() == 0 {
            return Err(AppError::NotFound(format!(
                "Position not found: {}",
                update.trade_uuid
            )));
        }

        Ok(())
    }

    async fn get_active_positions(&self) -> AppResult<Vec<Position>> {
        let rows = sqlx::query(
            r#"
            SELECT
                id, trade_uuid, wallet_address, token_address, token_symbol,
                strategy, entry_amount_sol, entry_price, entry_tx_signature,
                current_price, unrealized_pnl_sol, unrealized_pnl_percent,
                state, exit_price, exit_tx_signature, realized_pnl_sol,
                realized_pnl_usd, entry_sol_price_usd, opened_at, last_updated, closed_at,
                token_amount
            FROM positions
            WHERE state = 'ACTIVE'
            ORDER BY opened_at DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::Database)?;

        rows.into_iter().map(|r| self.row_to_position(r)).collect()
    }

    async fn get_position_by_trade_uuid(&self, trade_uuid: &str) -> AppResult<Option<Position>> {
        let row = sqlx::query(
            r#"
            SELECT
                id, trade_uuid, wallet_address, token_address, token_symbol,
                strategy, entry_amount_sol, entry_price, entry_tx_signature,
                current_price, unrealized_pnl_sol, unrealized_pnl_percent,
                state, exit_price, exit_tx_signature, realized_pnl_sol,
                realized_pnl_usd, entry_sol_price_usd, opened_at, last_updated, closed_at,
                token_amount
            FROM positions
            WHERE trade_uuid = ?
            "#,
        )
        .bind(trade_uuid)
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::Database)?;

        match row {
            Some(r) => Ok(Some(self.row_to_position(r)?)),
            None => Ok(None),
        }
    }

    async fn close_position(
        &self,
        trade_uuid: &str,
        exit_price: Decimal,
        exit_tx_signature: &str,
        realized_pnl_sol: Decimal,
        realized_pnl_usd: Decimal,
    ) -> AppResult<()> {
        sqlx::query(
            r#"
            UPDATE positions
            SET state = 'CLOSED', exit_price = ?, exit_tx_signature = ?,
                realized_pnl_sol = ?, realized_pnl_usd = ?, closed_at = CURRENT_TIMESTAMP
            WHERE trade_uuid = ?
            "#,
        )
        .bind(dec_to_text(&exit_price))
        .bind(exit_tx_signature)
        .bind(dec_to_text(&realized_pnl_sol))
        .bind(dec_to_text(&realized_pnl_usd))
        .bind(trade_uuid)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    // ========================================================================
    // WALLET OPERATIONS
    // ========================================================================

    async fn get_wallet(&self, address: &str) -> AppResult<Option<Wallet>> {
        let row = sqlx::query(
            r#"
            SELECT
                id, address, status, wqs_score, wqs_confidence,
                roi_7d, roi_30d, trade_count_30d, win_rate, max_drawdown_30d,
                avg_trade_size_sol, avg_win_sol, avg_loss_sol, profit_factor,
                realized_pnl_30d_sol, last_trade_at, promoted_at, ttl_expires_at,
                notes, archetype, avg_entry_delay_seconds, created_at, updated_at
            FROM wallets
            WHERE address = ?
            "#,
        )
        .bind(address)
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::Database)?;

        match row {
            Some(r) => Ok(Some(self.row_to_wallet(r)?)),
            None => Ok(None),
        }
    }

    async fn get_active_wallets(&self) -> AppResult<Vec<Wallet>> {
        let rows = sqlx::query(
            r#"
            SELECT
                id, address, status, wqs_score, wqs_confidence,
                roi_7d, roi_30d, trade_count_30d, win_rate, max_drawdown_30d,
                avg_trade_size_sol, avg_win_sol, avg_loss_sol, profit_factor,
                realized_pnl_30d_sol, last_trade_at, promoted_at, ttl_expires_at,
                notes, archetype, avg_entry_delay_seconds, created_at, updated_at
            FROM wallets
            WHERE status = 'ACTIVE'
            ORDER BY wqs_score DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::Database)?;

        rows.into_iter().map(|r| self.row_to_wallet(r)).collect()
    }

    async fn update_wallet_status(&self, address: &str, status: &str) -> AppResult<()> {
        sqlx::query("UPDATE wallets SET status = ? WHERE address = ?")
            .bind(status)
            .bind(address)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn merge_roster(&self, roster_db_path: &str) -> AppResult<u32> {
        // ATTACH DATABASE and merge wallets
        sqlx::query("ATTACH DATABASE ? AS roster")
            .bind(roster_db_path)
            .execute(&self.pool)
            .await?;

        let result = sqlx::query(
            r#"
            INSERT OR REPLACE INTO wallets (
                address, status, wqs_score, wqs_confidence, roi_7d, roi_30d,
                trade_count_30d, win_rate, max_drawdown_30d, avg_trade_size_sol,
                avg_win_sol, avg_loss_sol, profit_factor, realized_pnl_30d_sol,
                last_trade_at, promoted_at, ttl_expires_at, notes, archetype,
                avg_entry_delay_seconds, created_at, updated_at
            )
            SELECT
                address, status, wqs_score, wqs_confidence, roi_7d, roi_30d,
                trade_count_30d, win_rate, max_drawdown_30d, avg_trade_size_sol,
                avg_win_sol, avg_loss_sol, profit_factor, realized_pnl_30d_sol,
                last_trade_at, promoted_at, ttl_expires_at, notes, archetype,
                avg_entry_delay_seconds, created_at, updated_at
            FROM roster.wallets
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query("DETACH DATABASE roster")
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() as u32)
    }

    async fn get_wallets_by_status(&self, status: &str) -> AppResult<Vec<Wallet>> {
        let rows = sqlx::query(
            r#"
            SELECT
                id, address, status, wqs_score, wqs_confidence,
                roi_7d, roi_30d, trade_count_30d, win_rate, max_drawdown_30d,
                avg_trade_size_sol, avg_win_sol, avg_loss_sol, profit_factor,
                realized_pnl_30d_sol, last_trade_at, promoted_at, ttl_expires_at,
                notes, archetype, avg_entry_delay_seconds, created_at, updated_at
            FROM wallets
            WHERE status = ?
            ORDER BY wqs_score DESC
            "#,
        )
        .bind(status)
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::Database)?;

        rows.into_iter().map(|r| self.row_to_wallet(r)).collect()
    }

    // ========================================================================
    // SYSTEM OPERATIONS
    // ========================================================================

    async fn get_circuit_breaker_state(&self) -> AppResult<CircuitBreakerState> {
        let row = sqlx::query(
            "SELECT id, state, tripped_at, trip_reason, updated_at FROM circuit_breaker_state WHERE id = 1",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::Database)?;

        Ok(CircuitBreakerState {
            state: row.try_get("state").unwrap_or("Active".to_string()),
            tripped_at: row.try_get("tripped_at").ok(),
            trip_reason: row.try_get("trip_reason").ok(),
            updated_at: row
                .try_get::<String, _>("updated_at")
                .unwrap_or_else(|_| chrono::Utc::now().to_rfc3339()),
        })
    }

    async fn update_circuit_breaker_state(
        &self,
        state: &str,
        tripped_at: Option<&str>,
        trip_reason: Option<&str>,
    ) -> AppResult<()> {
        let mut sql =
            "UPDATE circuit_breaker_state SET state = ?, updated_at = datetime('now')".to_string();

        if tripped_at.is_some() {
            sql.push_str(", tripped_at = ?");
        }
        if trip_reason.is_some() {
            sql.push_str(", trip_reason = ?");
        }

        sql.push_str(" WHERE id = 1");

        let mut query = sqlx::query(&sql).bind(state);
        if let Some(t) = tripped_at {
            query = query.bind(t);
        }
        if let Some(r) = trip_reason {
            query = query.bind(r);
        }

        query.execute(&self.pool).await?;

        Ok(())
    }

    async fn get_kill_switch_state(&self) -> AppResult<KillSwitchState> {
        let row = sqlx::query("SELECT * FROM kill_switch_state WHERE id = 1")
            .fetch_one(&self.pool)
            .await
            .map_err(AppError::Database)?;

        Ok(KillSwitchState {
            state: row.try_get("state").unwrap_or("INACTIVE".to_string()),
            changed_at: row
                .try_get::<String, _>("changed_at")
                .unwrap_or_else(|_| chrono::Utc::now().to_rfc3339()),
            changed_by: row.try_get("changed_by").unwrap_or("SYSTEM".to_string()),
            reason: row.try_get("reason").ok(),
        })
    }

    async fn set_kill_switch_state(&self, state: &str, reason: Option<&str>) -> AppResult<()> {
        sqlx::query(
            r#"
            INSERT INTO kill_switch_state (id, state, changed_at, changed_by, reason)
            VALUES (1, ?, datetime('now'), 'SYSTEM', ?)
            ON CONFLICT(id) DO UPDATE SET
                state = excluded.state,
                changed_at = excluded.changed_at,
                changed_by = excluded.changed_by,
                reason = excluded.reason
            "#,
        )
        .bind(state)
        .bind(reason)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn insert_dlq(
        &self,
        trade_uuid: Option<&str>,
        payload: &str,
        reason: &str,
        error_details: Option<&str>,
        source_ip: Option<&str>,
    ) -> AppResult<i64> {
        let result = sqlx::query(
            r#"
            INSERT INTO dead_letter_queue (trade_uuid, payload, reason, error_details, source_ip)
            VALUES (?, ?, ?, ?, ?)
            "#,
        )
        .bind(trade_uuid)
        .bind(payload)
        .bind(reason)
        .bind(error_details)
        .bind(source_ip)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    async fn get_admin_wallet_role(&self, wallet_address: &str) -> AppResult<Option<String>> {
        let role: Option<String> =
            sqlx::query_scalar("SELECT role FROM admin_wallets WHERE wallet_address = ?")
                .bind(wallet_address)
                .fetch_optional(&self.pool)
                .await
                .map_err(AppError::Database)?;

        Ok(role)
    }

    // ========================================================================
    // STATISTICS & REPORTING
    // ========================================================================

    #[tracing::instrument(skip(self))]
    async fn get_trade_statistics(&self) -> AppResult<TradeStatistics> {
        let row = sqlx::query(
            r#"
            SELECT
                COUNT(*) as total_trades,
                SUM(CASE WHEN status = 'CLOSED' THEN 1 ELSE 0 END) as successful_trades,
                SUM(CASE WHEN status = 'FAILED' OR status = 'DEAD_LETTER' THEN 1 ELSE 0 END) as failed_trades
            FROM trades
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::Database)?;

        let pnl_values: Vec<String> =
            sqlx::query_scalar("SELECT net_pnl_sol FROM trades WHERE net_pnl_sol IS NOT NULL")
                .fetch_all(&self.pool)
                .await?;
        let total_pnl_sol: Decimal = pnl_values
            .iter()
            .filter_map(|s| Decimal::from_str(s).ok())
            .sum();

        let volume_values: Vec<String> =
            sqlx::query_scalar("SELECT amount_sol FROM trades WHERE amount_sol IS NOT NULL")
                .fetch_all(&self.pool)
                .await?;
        let total_volume_sol: Decimal = volume_values
            .iter()
            .filter_map(|s| Decimal::from_str(s).ok())
            .sum();

        Ok(TradeStatistics {
            total_trades: row.try_get("total_trades").unwrap_or(0),
            successful_trades: row.try_get("successful_trades").unwrap_or(0),
            failed_trades: row.try_get("failed_trades").unwrap_or(0),
            total_pnl_sol,
            total_volume_sol,
        })
    }

    async fn get_recent_trades(&self, limit: i64, offset: i64) -> AppResult<Vec<Trade>> {
        let rows = sqlx::query(
            r#"
            SELECT
                id, trade_uuid, wallet_address, token_address, token_symbol,
                strategy, side, amount_sol, price_at_signal, tx_signature,
                status, retry_count, error_message, pnl_sol, pnl_usd,
                jito_tip_sol, dex_fee_sol, slippage_cost_sol, total_cost_sol,
                net_pnl_sol, created_at, updated_at
            FROM trades
            ORDER BY created_at DESC
            LIMIT ? OFFSET ?
            "#,
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::Database)?;

        rows.into_iter().map(|r| self.row_to_trade(r)).collect()
    }

    async fn get_wallet_performance(
        &self,
        wallet_address: &str,
    ) -> AppResult<Option<WalletPerformance>> {
        let row = sqlx::query(
            r#"
            SELECT
                wallet_address, copy_pnl_7d, copy_pnl_30d,
                signal_success_rate, total_trades, winning_trades
            FROM wallet_copy_performance
            WHERE wallet_address = ?
            "#,
        )
        .bind(wallet_address)
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::Database)?;

        match row {
            Some(r) => Ok(Some(WalletPerformance {
                wallet_address: r
                    .try_get("wallet_address")
                    .unwrap_or(wallet_address.to_string()),
                copy_pnl_7d: text_to_dec(
                    &r.try_get::<String, _>("copy_pnl_7d").unwrap_or_default(),
                ),
                copy_pnl_30d: text_to_dec(
                    &r.try_get::<String, _>("copy_pnl_30d").unwrap_or_default(),
                ),
                signal_success_rate: Decimal::from_f64(
                    r.try_get("signal_success_rate").unwrap_or(0.0),
                )
                .unwrap_or(Decimal::ZERO),
                total_trades: r.try_get("total_trades").unwrap_or(0),
                winning_trades: r.try_get("winning_trades").unwrap_or(0),
            })),
            None => Ok(None),
        }
    }

    // ========================================================================
    // JITO TIP HISTORY
    // ========================================================================

    async fn insert_jito_tip(
        &self,
        tip_amount_sol: &Decimal,
        bundle_signature: Option<&str>,
        strategy: Option<&str>,
        success: bool,
    ) -> AppResult<i64> {
        let result = sqlx::query(
            r#"
            INSERT INTO jito_tip_history (tip_amount_sol, bundle_signature, strategy, success)
            VALUES (?, ?, ?, ?)
            "#,
        )
        .bind(dec_to_text(tip_amount_sol))
        .bind(bundle_signature)
        .bind(strategy)
        .bind(if success { 1 } else { 0 })
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    async fn get_recent_jito_tips(&self, limit: i32) -> AppResult<Vec<Decimal>> {
        #[allow(clippy::type_complexity)]
        let rows: Vec<(String,)> = sqlx::query_as(
            r#"
            SELECT tip_amount_sol
            FROM jito_tip_history
            WHERE success = 1
            ORDER BY created_at DESC
            LIMIT ?
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|(t,)| text_to_dec(&t)).collect())
    }

    async fn get_jito_tip_count(&self) -> AppResult<u32> {
        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM jito_tip_history WHERE success = 1")
                .fetch_one(&self.pool)
                .await?;

        Ok(count.0 as u32)
    }

    async fn prune_old_jito_tips(&self) -> AppResult<u64> {
        let result = sqlx::query(
            "DELETE FROM jito_tip_history WHERE created_at < datetime('now', '-7 days')",
        )
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    // ========================================================================
    // PnL QUERIES
    // ========================================================================

    async fn get_pnl_window(&self, from_hours: &str, to_hours: Option<&str>) -> AppResult<Decimal> {
        let from_modifier = format!("-{} hours", from_hours);

        let rows: Vec<(String,)> = if let Some(h) = to_hours {
            let to_modifier = format!("-{} hours", h);
            sqlx::query_as(
                r#"SELECT COALESCE(realized_pnl_sol, '0') FROM positions
                   WHERE state = 'CLOSED' AND closed_at >= datetime('now', ?)
                   AND closed_at < datetime('now', ?)"#,
            )
            .bind(&from_modifier)
            .bind(&to_modifier)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as(
                r#"SELECT COALESCE(realized_pnl_sol, '0') FROM positions
                   WHERE state = 'CLOSED' AND closed_at >= datetime('now', ?)"#,
            )
            .bind(&from_modifier)
            .fetch_all(&self.pool)
            .await?
        };

        let total = rows
            .into_iter()
            .map(|(s,)| text_to_dec(&s))
            .fold(Decimal::ZERO, |acc, v| acc + v);
        Ok(total)
    }

    async fn get_pnl_24h(&self) -> AppResult<Decimal> {
        let rows: Vec<(String,)> = sqlx::query_as(
            r#"SELECT COALESCE(realized_pnl_sol, '0') FROM positions
               WHERE state = 'CLOSED' AND closed_at >= datetime('now', '-24 hours')"#,
        )
        .fetch_all(&self.pool)
        .await?;

        let total = rows
            .into_iter()
            .map(|(s,)| text_to_dec(&s))
            .fold(Decimal::ZERO, |acc, v| acc + v);
        Ok(total)
    }

    async fn get_pnl_7d(&self) -> AppResult<Decimal> {
        let rows: Vec<(String,)> = sqlx::query_as(
            r#"SELECT COALESCE(realized_pnl_sol, '0') FROM positions
               WHERE state = 'CLOSED' AND closed_at >= datetime('now', '-7 days')"#,
        )
        .fetch_all(&self.pool)
        .await?;

        let total = rows
            .into_iter()
            .map(|(s,)| text_to_dec(&s))
            .fold(Decimal::ZERO, |acc, v| acc + v);
        Ok(total)
    }

    async fn get_pnl_30d(&self) -> AppResult<Decimal> {
        let rows: Vec<(String,)> = sqlx::query_as(
            r#"SELECT COALESCE(realized_pnl_sol, '0') FROM positions
               WHERE state = 'CLOSED' AND closed_at >= datetime('now', '-30 days')"#,
        )
        .fetch_all(&self.pool)
        .await?;

        let total = rows
            .into_iter()
            .map(|(s,)| text_to_dec(&s))
            .fold(Decimal::ZERO, |acc, v| acc + v);
        Ok(total)
    }

    async fn get_strategy_performance(
        &self,
        strategy: &str,
        days: i32,
    ) -> AppResult<(f64, Decimal, u32)> {
        let days_clamped = days.clamp(1, 365);
        let days_interval = format!("-{} days", days_clamped);

        let rows: Vec<(String,)> = sqlx::query_as(
            r#"
            SELECT COALESCE(net_pnl_sol, '0')
            FROM trades
            WHERE status = 'CLOSED'
            AND strategy = ?
            AND created_at >= datetime('now', ?)
            ORDER BY created_at DESC
            "#,
        )
        .bind(strategy)
        .bind(&days_interval)
        .fetch_all(&self.pool)
        .await?;

        if rows.is_empty() {
            return Ok((0.0, Decimal::ZERO, 0));
        }

        let mut total_pnl = Decimal::ZERO;
        let mut winning_trades = 0u32;
        let total_trades = rows.len() as u32;

        for (pnl_str,) in rows {
            let pnl = text_to_dec(&pnl_str);
            total_pnl += pnl;
            if pnl > Decimal::ZERO {
                winning_trades += 1;
            }
        }

        let win_rate = if total_trades > 0 {
            (winning_trades as f64 / total_trades as f64) * 100.0
        } else {
            0.0
        };

        let avg_return = if total_trades > 0 {
            total_pnl / Decimal::from(total_trades)
        } else {
            Decimal::ZERO
        };

        Ok((win_rate, avg_return, total_trades))
    }

    // ========================================================================
    // LOSS TRACKING
    // ========================================================================

    async fn get_consecutive_losses(&self) -> AppResult<u32> {
        let rows: Vec<(String,)> = sqlx::query_as(
            r#"
            SELECT COALESCE(p.realized_pnl_sol, '0')
            FROM trades t
            LEFT JOIN positions p ON p.trade_uuid = t.trade_uuid
            WHERE t.status = 'CLOSED'
            ORDER BY t.created_at DESC
            LIMIT 20
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let mut consecutive = 0u32;
        for (pnl_str,) in rows {
            let pnl = text_to_dec(&pnl_str);
            if pnl < Decimal::ZERO {
                consecutive += 1;
            } else {
                break;
            }
        }

        Ok(consecutive)
    }

    async fn get_max_drawdown_percent(&self, total_capital_sol: Decimal) -> AppResult<Decimal> {
        // Fetch closed positions' realized PnL in order (TEXT → Decimal)
        let closed_rows: Vec<(String,)> = sqlx::query_as(
            r#"SELECT COALESCE(realized_pnl_sol, '0') FROM positions WHERE state = 'CLOSED' ORDER BY closed_at ASC"#,
        )
        .fetch_all(&self.pool)
        .await?;

        // Compute cumulative PnL and find peak in Decimal
        let mut peak_pnl = Decimal::ZERO;
        let mut running_pnl = Decimal::ZERO;
        for (pnl_str,) in &closed_rows {
            running_pnl += text_to_dec(pnl_str);
            if running_pnl > peak_pnl {
                peak_pnl = running_pnl;
            }
        }

        // Add unrealized PnL from open positions
        let unrealized_rows: Vec<(String,)> = sqlx::query_as(
            r#"SELECT COALESCE(unrealized_pnl_sol, '0') FROM positions WHERE state IN ('ACTIVE', 'EXITING')"#,
        )
        .fetch_all(&self.pool)
        .await?;
        let unrealized_pnl: Decimal = unrealized_rows
            .into_iter()
            .map(|(s,)| text_to_dec(&s))
            .fold(Decimal::ZERO, |acc, v| acc + v);

        let current_pnl = running_pnl + unrealized_pnl;

        // Drawdown = (peak - current) / (total_capital + peak) * 100
        let denominator = total_capital_sol + peak_pnl;
        if denominator > Decimal::ZERO {
            let drawdown = ((peak_pnl - current_pnl) / denominator) * Decimal::from(100);
            Ok(drawdown.max(Decimal::ZERO))
        } else {
            Ok(Decimal::ZERO)
        }
    }

    // ========================================================================
    // POSITIONS - ADVANCED OPERATIONS
    // ========================================================================

    #[allow(clippy::too_many_arguments)]
    async fn activate_trade_and_open_position(
        &self,
        trade_uuid: &str,
        wallet_address: &str,
        token_address: &str,
        token_symbol: Option<&str>,
        strategy: &str,
        amount_sol: Decimal,
        entry_price: Decimal,
        tx_signature: &str,
        max_heat_sol: Option<Decimal>,
        entry_sol_price_usd: Option<Decimal>,
    ) -> AppResult<()> {
        let mut tx = self.pool.begin().await?;

        // Validate entry_price is positive
        if entry_price <= Decimal::ZERO {
            tracing::warn!(
                trade_uuid = %trade_uuid,
                entry_price = %entry_price,
                "Entry price must be positive — rejecting position open"
            );
            return Err(AppError::Validation(
                "Entry price must be positive".to_string(),
            ));
        }

        if let Some(limit) = max_heat_sol {
            let exposure_values: Vec<String> = sqlx::query_scalar(
                "SELECT entry_amount_sol FROM positions WHERE state IN ('ACTIVE', 'EXITING') AND entry_amount_sol IS NOT NULL",
            )
            .fetch_all(&mut *tx)
            .await?;
            let current: Decimal = exposure_values
                .iter()
                .filter_map(|s| Decimal::from_str(s).ok())
                .sum();
            if current + amount_sol > limit {
                tracing::warn!(
                    trade_uuid = %trade_uuid,
                    current_exposure_sol = %current,
                    new_size_sol = %amount_sol,
                    max_heat_sol = %limit,
                    "Portfolio heat limit reached at write time — rolling back position open"
                );
                return Err(AppError::Internal(
                    "Portfolio heat limit reached at write time".to_string(),
                ));
            }
        }

        let dupe_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM positions WHERE token_address = ? AND state IN ('ACTIVE','EXITING')",
        )
        .bind(token_address)
        .fetch_one(&mut *tx)
        .await?;
        if dupe_count > 0 {
            tracing::warn!(
                trade_uuid = %trade_uuid,
                token_address = %token_address,
                "Duplicate position detected at write time — rolling back"
            );
            return Err(AppError::Internal(
                "Duplicate position detected at write time".to_string(),
            ));
        }

        sqlx::query(
            r#"
            UPDATE trades
            SET status = 'ACTIVE', tx_signature = ?
            WHERE trade_uuid = ?
            "#,
        )
        .bind(tx_signature)
        .bind(trade_uuid)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO positions (
                trade_uuid, wallet_address, token_address, token_symbol, strategy,
                entry_amount_sol, entry_price, entry_tx_signature, entry_sol_price_usd,
                state, unrealized_pnl_sol, unrealized_pnl_percent, token_amount
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'ACTIVE', '0', '0', NULL)
            "#,
        )
        .bind(trade_uuid)
        .bind(wallet_address)
        .bind(token_address)
        .bind(token_symbol)
        .bind(strategy)
        .bind(dec_to_text(&amount_sol))
        .bind(dec_to_text(&entry_price))
        .bind(tx_signature)
        .bind(entry_sol_price_usd.as_ref().map(dec_to_text))
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    async fn update_position_token_amount(
        &self,
        trade_uuid: &str,
        token_amount: u64,
    ) -> AppResult<()> {
        sqlx::query("UPDATE positions SET token_amount = ? WHERE trade_uuid = ?")
            .bind(token_amount.to_string())
            .bind(trade_uuid)
            .execute(&self.pool)
            .await
            .map_err(AppError::Database)?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn atomic_portfolio_heat_check_and_open_position(
        &self,
        trade_uuid: &str,
        wallet_address: &str,
        token_address: &str,
        token_symbol: Option<&str>,
        strategy: &str,
        amount_sol: Decimal,
        entry_price: Decimal,
        tx_signature: &str,
        max_heat_sol: Option<Decimal>,
        entry_sol_price_usd: Option<Decimal>,
    ) -> AppResult<()> {
        const MAX_RETRIES: u32 = 3;
        let mut attempt = 0;

        loop {
            attempt += 1;

            match self
                .activate_trade_and_open_position(
                    trade_uuid,
                    wallet_address,
                    token_address,
                    token_symbol,
                    strategy,
                    amount_sol,
                    entry_price,
                    tx_signature,
                    max_heat_sol,
                    entry_sol_price_usd,
                )
                .await
            {
                Ok(_) => return Ok(()),
                Err(AppError::Database(sqlx::Error::Database(db_err)))
                    if db_err.to_string().contains("database is locked")
                        && attempt < MAX_RETRIES =>
                {
                    let backoff = std::time::Duration::from_millis(50 * (1 << (attempt - 1)));
                    tracing::debug!(
                        attempt = attempt,
                        backoff_ms = backoff.as_millis(),
                        trade_uuid = %trade_uuid,
                        "Database locked, retrying portfolio heat check"
                    );
                    tokio::time::sleep(backoff).await;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn close_position_full(
        &self,
        trade_uuid: &str,
        wallet_address: &str,
        token_address: &str,
        exit_price: Decimal,
        signature: &str,
        sol_price_usd: Option<Decimal>,
        exit_fraction: Decimal,
        confirmed: bool,
    ) -> AppResult<()> {
        if exit_price.is_zero() {
            return Err(AppError::Validation(
                "exit_price cannot be zero — PnL calculations would produce -100% loss".to_string(),
            ));
        }

        let exit_fraction = exit_fraction.max(Decimal::ZERO).min(Decimal::ONE);

        let mut tx = self.pool.begin().await?;

        #[allow(clippy::type_complexity)]
        let active_positions: Vec<(i64, String, String, String, Option<String>)> =
            sqlx::query_as(
                r#"
                SELECT id, entry_price, entry_amount_sol, trade_uuid, entry_sol_price_usd
                FROM positions
                WHERE wallet_address = ? AND token_address = ? AND trade_uuid = ? AND state IN ('ACTIVE', 'EXITING')
                "#,
            )
            .bind(wallet_address)
            .bind(token_address)
            .bind(trade_uuid)
            .fetch_all(&mut *tx)
            .await?;

        if active_positions.is_empty() {
            tracing::warn!(
                wallet = %wallet_address,
                token = %token_address,
                "No active positions found to close"
            );
            tx.commit().await?;
            return Ok(());
        }

        let exit_costs: Option<(String, String, String, Option<String>)> = sqlx::query_as(
            "SELECT jito_tip_sol, dex_fee_sol, slippage_cost_sol, network_fee_sol FROM trades WHERE trade_uuid = ?",
        )
        .bind(trade_uuid)
        .fetch_optional(&mut *tx)
        .await?;

        let exit_total_costs = exit_costs
            .as_ref()
            .map(|(t, d, s, _)| text_to_dec(t) + text_to_dec(d) + text_to_dec(s))
            .unwrap_or(Decimal::ZERO);
        let exit_network_fee = exit_costs
            .and_then(|(_, _, _, nf)| nf)
            .map(|nf| text_to_dec(&nf))
            .unwrap_or(Decimal::ZERO);

        if sol_price_usd.is_none() {
            tracing::warn!(
                trade_uuid = %trade_uuid,
                "SOL/USD price unavailable — realized_pnl_usd will be NULL for this close"
            );
        }

        let entry_uuids: Vec<String> = active_positions
            .iter()
            .map(|(_, _, _, uuid, _)| uuid.clone())
            .collect();

        let mut entry_costs_map: HashMap<String, (String, String, String, String, Option<String>)> =
            HashMap::new();
        if !entry_uuids.is_empty() {
            let placeholders = entry_uuids
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(", ");
            let bulk_sql = format!(
                "SELECT trade_uuid, jito_tip_sol, dex_fee_sol, slippage_cost_sol, amount_sol, network_fee_sol FROM trades WHERE trade_uuid IN ({})",
                placeholders
            );
            let mut bulk_q = sqlx::query_as::<
                _,
                (String, String, String, String, String, Option<String>),
            >(&bulk_sql);
            for uuid in &entry_uuids {
                bulk_q = bulk_q.bind(uuid);
            }
            let cost_rows: Vec<(String, String, String, String, String, Option<String>)> =
                bulk_q.fetch_all(&mut *tx).await?;
            for (uuid, tip, dex, slip, amount, nf) in cost_rows {
                entry_costs_map.insert(uuid, (tip, dex, slip, amount, nf));
            }
        }

        let mut gross_pnl = Decimal::ZERO;
        let mut entry_total_costs = Decimal::ZERO;

        let is_full_close = exit_fraction >= Decimal::ONE;

        for (id, entry_price_str, entry_amount_str, entry_trade_uuid, entry_sol_price_str) in
            active_positions.iter()
        {
            let entry_price_dec = text_to_dec(entry_price_str);
            let entry_amount_dec = text_to_dec(entry_amount_str);
            let mut net_pnl_sol_str: Option<String> = None;
            let entry_sol_price_dec = entry_sol_price_str.as_deref().map(text_to_dec);

            let exited_amount = entry_amount_dec * exit_fraction;

            let pnl_sol = if !entry_price_dec.is_zero() {
                if let (Some(entry_sol_price), Some(exit_sol_price)) =
                    (entry_sol_price_dec, sol_price_usd)
                {
                    if !exit_sol_price.is_zero() && !entry_sol_price.is_zero() {
                        let exit_price_sol = exit_price
                            .checked_div(exit_sol_price)
                            .unwrap_or(Decimal::ZERO);
                        let entry_price_sol = entry_price_dec
                            .checked_div(entry_sol_price)
                            .unwrap_or(Decimal::ZERO);
                        if !entry_price_sol.is_zero() {
                            let diff = exit_price_sol - entry_price_sol;
                            let ratio = diff / entry_price_sol;
                            ratio * exited_amount
                        } else {
                            Decimal::ZERO
                        }
                    } else if !entry_sol_price.is_zero() {
                        tracing::warn!(
                            trade_uuid = %trade_uuid,
                            "Current SOL price is zero; using entry-time SOL price for PnL conversion"
                        );
                        let usd_diff = exit_price - entry_price_dec;
                        usd_diff / entry_sol_price
                    } else {
                        tracing::error!(
                            trade_uuid = %trade_uuid,
                            "Cannot compute SOL PnL: entry_sol_price is zero"
                        );
                        Decimal::ZERO
                    }
                } else if let Some(entry_sol_price) = entry_sol_price_dec {
                    if !entry_sol_price.is_zero() {
                        tracing::warn!(
                            trade_uuid = %trade_uuid,
                            "Current SOL price unavailable; using entry-time SOL price for PnL conversion"
                        );
                        let usd_diff = exit_price - entry_price_dec;
                        usd_diff / entry_sol_price
                    } else {
                        tracing::error!(
                            trade_uuid = %trade_uuid,
                            "Cannot compute SOL PnL: entry_sol_price is zero"
                        );
                        Decimal::ZERO
                    }
                } else {
                    tracing::error!(
                        trade_uuid = %trade_uuid,
                        entry_price = %entry_price_dec,
                        exit_price = %exit_price,
                        "No SOL/USD price data available (neither entry nor current)"
                    );
                    if !entry_price_dec.is_zero() {
                        let diff = exit_price - entry_price_dec;
                        let ratio = diff / entry_price_dec;
                        ratio * exited_amount
                    } else {
                        Decimal::ZERO
                    }
                }
            } else {
                Decimal::ZERO
            };

            if let Some((et, ed, es, orig_amount_str, entry_nf)) =
                entry_costs_map.get(entry_trade_uuid.as_str())
            {
                let orig_amount = text_to_dec(orig_amount_str);
                let total_entry_cost = text_to_dec(et) + text_to_dec(ed) + text_to_dec(es);
                let entry_network_fee = entry_nf
                    .as_deref()
                    .map(text_to_dec)
                    .unwrap_or(Decimal::ZERO);
                let exited_fraction_of_original = if !orig_amount.is_zero() {
                    exited_amount
                        .checked_div(orig_amount)
                        .unwrap_or(exit_fraction)
                } else {
                    exit_fraction
                };
                let proportional_entry_cost = total_entry_cost * exited_fraction_of_original;
                let proportional_entry_network_fee =
                    entry_network_fee * exited_fraction_of_original;
                entry_total_costs += proportional_entry_cost + proportional_entry_network_fee;
                let proportional_exit_network_fee = exit_network_fee * exit_fraction;
                let net_pnl_sol = pnl_sol
                    - proportional_entry_cost
                    - proportional_entry_network_fee
                    - exit_total_costs
                    - proportional_exit_network_fee;
                net_pnl_sol_str = Some(dec_to_text(&net_pnl_sol));
            }

            let pnl_usd_opt: Option<String> =
                sol_price_usd.map(|sol_usd| dec_to_text(&(pnl_sol * sol_usd)));

            let pnl_sol_str = dec_to_text(&pnl_sol);
            let exit_price_str = dec_to_text(&exit_price);

            if is_full_close {
                let rows = sqlx::query(
                    r#"
                    UPDATE positions
                    SET
                        exit_price = ?,
                        exit_tx_signature = ?,
                        realized_pnl_sol = ?,
                        realized_pnl_usd = ?,
                        realized_net_pnl_sol = ?,
                        closed_at = CASE WHEN ? = 1 THEN CURRENT_TIMESTAMP ELSE NULL END,
                        state = ?
                    WHERE id = ? AND state IN ('ACTIVE', 'EXITING')
                    "#,
                )
                .bind(&exit_price_str)
                .bind(signature)
                .bind(&pnl_sol_str)
                .bind(&pnl_usd_opt)
                .bind(&net_pnl_sol_str)
                .bind(if confirmed { 1 } else { 0 })
                .bind(if confirmed { "CLOSED" } else { "EXITING" })
                .bind(id)
                .execute(&mut *tx)
                .await?;

                if rows.rows_affected() == 0 {
                    tracing::warn!(
                        position_id = id,
                        "Position already closed by concurrent call — skipping"
                    );
                    continue;
                }
            } else {
                let remaining_amount = entry_amount_dec - exited_amount;
                let remaining_str = dec_to_text(&remaining_amount);

                let current_realized: Option<String> =
                    sqlx::query_scalar("SELECT realized_pnl_sol FROM positions WHERE id = ?")
                        .bind(id)
                        .fetch_optional(&mut *tx)
                        .await?;
                let current_sol = current_realized
                    .as_deref()
                    .and_then(|s| Decimal::from_str(s).ok())
                    .unwrap_or(Decimal::ZERO);
                let new_realized_sol = current_sol + pnl_sol;
                let new_realized_sol_str = dec_to_text(&new_realized_sol);

                let new_realized_usd_str = if let Some(ref pnl_usd_str) = pnl_usd_opt {
                    let pnl_usd = Decimal::from_str(pnl_usd_str).unwrap_or(Decimal::ZERO);
                    let current_realized_usd: Option<String> =
                        sqlx::query_scalar("SELECT realized_pnl_usd FROM positions WHERE id = ?")
                            .bind(id)
                            .fetch_optional(&mut *tx)
                            .await?;
                    let current_usd = current_realized_usd
                        .as_deref()
                        .and_then(|s| Decimal::from_str(s).ok())
                        .unwrap_or(Decimal::ZERO);
                    Some(dec_to_text(&(current_usd + pnl_usd)))
                } else {
                    None
                };

                let rows = sqlx::query(
                    r#"
                    UPDATE positions
                    SET
                        entry_amount_sol = ?,
                        exit_price = ?,
                        exit_tx_signature = ?,
                        realized_pnl_sol = ?,
                        realized_pnl_usd = ?,
                        realized_net_pnl_sol = COALESCE(
                            CAST(COALESCE(realized_net_pnl_sol, '0') AS REAL) + CAST(? AS REAL),
                            ?
                        ),
                        token_amount = CAST(CAST(token_amount AS REAL) * (1 - ?) AS INTEGER),
                        state = ?,
                        last_updated = CURRENT_TIMESTAMP
                    WHERE id = ? AND state IN ('ACTIVE', 'EXITING')
                    "#,
                )
                .bind(&remaining_str)
                .bind(&exit_price_str)
                .bind(signature)
                .bind(&new_realized_sol_str)
                .bind(&new_realized_usd_str)
                .bind(net_pnl_sol_str.clone().unwrap_or_else(|| "0".to_string()))
                .bind(&net_pnl_sol_str)
                .bind(exit_fraction.to_f64().unwrap_or(1.0))
                .bind(if confirmed { "ACTIVE" } else { "EXITING" })
                .bind(id)
                .execute(&mut *tx)
                .await?;

                if rows.rows_affected() == 0 {
                    tracing::warn!(
                        position_id = id,
                        "Position already closed by concurrent call — skipping partial close"
                    );
                    continue;
                }
            }

            gross_pnl += pnl_sol;
        }

        let net_pnl = gross_pnl - entry_total_costs - exit_total_costs;
        let current_net_str: Option<String> =
            sqlx::query_scalar("SELECT net_pnl_sol FROM trades WHERE trade_uuid = ?")
                .bind(trade_uuid)
                .fetch_optional(&mut *tx)
                .await?;
        let current_net = current_net_str
            .as_deref()
            .and_then(|s| Decimal::from_str(s).ok())
            .unwrap_or(Decimal::ZERO);
        let new_net_str = dec_to_text(&(current_net + net_pnl));
        sqlx::query("UPDATE trades SET net_pnl_sol = ? WHERE trade_uuid = ?")
            .bind(&new_net_str)
            .bind(trade_uuid)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(())
    }

    async fn revert_position_exit(&self, position_trade_uuid: &str) -> AppResult<()> {
        let mut tx = self.pool.begin().await?;

        #[allow(clippy::type_complexity)]
        let pos: Option<(String, String, Option<String>, Option<String>, Option<String>, String, String)> =
            sqlx::query_as(
                r#"
                SELECT entry_price, entry_amount_sol, exit_tx_signature, realized_pnl_sol, realized_pnl_usd, wallet_address, token_address
                FROM positions WHERE trade_uuid = ?
                "#,
            )
            .bind(position_trade_uuid)
            .fetch_optional(&mut *tx)
            .await?;

        if let Some((
            _entry_price_str,
            _entry_amount_str,
            Some(ref exit_sig),
            realized_pnl_sol_str,
            realized_pnl_usd_str,
            ref wallet_address,
            ref token_address,
        )) = pos
        {
            if !exit_sig.is_empty() {
                let exit_trade: Option<(String, String)> = sqlx::query_as(
                    "SELECT trade_uuid, amount_sol FROM trades WHERE tx_signature = ? AND side = 'SELL'",
                )
                .bind(exit_sig)
                .fetch_optional(&mut *tx)
                .await?;

                if let Some((ref exit_trade_uuid, _exit_amount_str)) = exit_trade {
                    let buy_signal_amount_str: String =
                        sqlx::query_scalar("SELECT amount_sol FROM trades WHERE trade_uuid = ?")
                            .bind(position_trade_uuid)
                            .fetch_one(&mut *tx)
                            .await?;
                    let buy_signal_amount_sol = text_to_dec(&buy_signal_amount_str);

                    let confirmed_exit_values: Vec<String> = sqlx::query_scalar(
                        "SELECT amount_sol FROM trades WHERE wallet_address = ? AND token_address = ? AND side = 'SELL' AND status = 'CLOSED' AND tx_signature != ? AND amount_sol IS NOT NULL",
                    )
                    .bind(wallet_address)
                    .bind(token_address)
                    .bind(exit_sig)
                    .fetch_all(&mut *tx)
                    .await?;
                    let confirmed_exit_amount: Decimal = confirmed_exit_values
                        .iter()
                        .filter_map(|s| Decimal::from_str(s).ok())
                        .sum();

                    let reverted_amount = buy_signal_amount_sol - confirmed_exit_amount;

                    let mut new_realized_pnl_sol: Option<String> = None;
                    let mut new_realized_pnl_usd: Option<String> = None;

                    if confirmed_exit_amount > Decimal::ZERO {
                        let (failed_net, failed_tip, failed_dex, failed_slip): (
                            Option<String>,
                            Option<String>,
                            Option<String>,
                            Option<String>,
                        ) = sqlx::query_as(
                            "SELECT net_pnl_sol, jito_tip_sol, dex_fee_sol, slippage_cost_sol FROM trades WHERE trade_uuid = ?",
                        )
                        .bind(exit_trade_uuid)
                        .fetch_one(&mut *tx)
                        .await?;

                        let failed_gross = match (failed_net, failed_tip, failed_dex, failed_slip) {
                            (Some(net), Some(tip), Some(dex), Some(slip)) => {
                                let net_dec = text_to_dec(&net);
                                let costs =
                                    text_to_dec(&tip) + text_to_dec(&dex) + text_to_dec(&slip);
                                net_dec + costs
                            }
                            _ => Decimal::ZERO,
                        };

                        let current_pnl_sol = realized_pnl_sol_str
                            .as_deref()
                            .map(text_to_dec)
                            .unwrap_or(Decimal::ZERO);
                        let reverted_pnl = current_pnl_sol - failed_gross;
                        new_realized_pnl_sol = Some(dec_to_text(&reverted_pnl));

                        if realized_pnl_usd_str.is_some() {
                            tracing::warn!(
                                exit_trade_uuid = %exit_trade_uuid,
                                "Reverting position with prior confirmed exits — setting realized_pnl_usd to NULL"
                            );
                            new_realized_pnl_usd = None;
                        }
                    }

                    let reverted_amount_str = dec_to_text(&reverted_amount);
                    sqlx::query(
                        r#"
                        UPDATE positions
                        SET
                            state = 'ACTIVE',
                            entry_amount_sol = ?,
                            exit_price = NULL,
                            exit_tx_signature = NULL,
                            realized_pnl_sol = ?,
                            realized_pnl_usd = ?,
                            closed_at = NULL,
                            last_updated = CURRENT_TIMESTAMP
                        WHERE trade_uuid = ?
                        "#,
                    )
                    .bind(reverted_amount_str)
                    .bind(&new_realized_pnl_sol)
                    .bind(&new_realized_pnl_usd)
                    .bind(position_trade_uuid)
                    .execute(&mut *tx)
                    .await?;

                    sqlx::query(
                        r#"
                        UPDATE trades
                        SET
                            status = 'FAILED',
                            net_pnl_sol = NULL,
                            error_message = 'Exit transaction failed to confirm on-chain (reverted by recovery manager)'
                        WHERE trade_uuid = ?
                        "#,
                    )
                    .bind(exit_trade_uuid)
                    .execute(&mut *tx)
                    .await?;
                }
            }
        }

        tx.commit().await?;
        Ok(())
    }

    async fn get_stuck_positions(&self, stuck_seconds: i64) -> AppResult<Vec<PositionRecord>> {
        let modifier = format!("-{} seconds", stuck_seconds);
        #[allow(clippy::type_complexity)]
        let rows: Vec<(
            i64,
            String,
            String,
            String,
            String,
            String,
            String,
            Option<String>,
            String,
        )> = sqlx::query_as(
            r#"
            SELECT id, trade_uuid, wallet_address, token_address, strategy, state,
                   entry_tx_signature, exit_tx_signature, last_updated
            FROM positions
            WHERE state = 'EXITING'
            AND last_updated < datetime('now', ?)
            "#,
        )
        .bind(&modifier)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(
                |(
                    id,
                    trade_uuid,
                    wallet_address,
                    token_address,
                    strategy,
                    state,
                    entry_tx_signature,
                    exit_tx_signature,
                    last_updated,
                )| {
                    Ok(PositionRecord {
                        id,
                        trade_uuid,
                        wallet_address,
                        token_address,
                        strategy,
                        state,
                        entry_tx_signature,
                        exit_tx_signature,
                        last_updated: chrono::DateTime::parse_from_rfc3339(&last_updated)
                            .map(|dt| dt.with_timezone(&chrono::Utc))
                            .unwrap_or_else(|_| chrono::Utc::now()),
                    })
                },
            )
            .collect()
    }

    async fn update_position_state(&self, trade_uuid: &str, new_state: &str) -> AppResult<()> {
        sqlx::query("UPDATE positions SET state = ? WHERE trade_uuid = ?")
            .bind(new_state)
            .bind(trade_uuid)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn update_position_unrealized_pnl(
        &self,
        trade_uuid: &str,
        current_price: Decimal,
        pnl_sol: Decimal,
        pnl_pct: Decimal,
    ) -> AppResult<()> {
        sqlx::query(
            r#"
            UPDATE positions
            SET current_price = ?,
                unrealized_pnl_sol = ?,
                unrealized_pnl_percent = ?,
                last_updated = datetime('now')
            WHERE trade_uuid = ?
              AND state IN ('ACTIVE', 'EXITING')
            "#,
        )
        .bind(dec_to_text(&current_price))
        .bind(dec_to_text(&pnl_sol))
        .bind(dec_to_text(&pnl_pct))
        .bind(trade_uuid)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_active_positions_with_entry(&self) -> AppResult<Vec<ActivePositionEntry>> {
        #[allow(clippy::type_complexity)]
        let rows: Vec<(
            String,
            String,
            String,
            Option<String>,
            String,
            String,
            String,
            String,
        )> = sqlx::query_as(
            r#"
            SELECT
                p.trade_uuid,
                p.wallet_address,
                p.token_address,
                t.token_symbol,
                p.strategy,
                COALESCE(p.entry_price, '0'),
                COALESCE(p.entry_amount_sol, '0'),
                COALESCE(p.opened_at, datetime('now'))
            FROM positions p
            LEFT JOIN trades t ON t.trade_uuid = p.trade_uuid
            WHERE p.state = 'ACTIVE'
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let entries = rows
            .into_iter()
            .map(
                |(
                    trade_uuid,
                    wallet_address,
                    token_address,
                    token_opt,
                    strategy,
                    entry_price_str,
                    entry_amount_str,
                    created_at_str,
                )| {
                    let entry_price = text_to_dec(&entry_price_str);
                    let entry_amount_sol = text_to_dec(&entry_amount_str);
                    let entry_time = chrono::DateTime::parse_from_rfc3339(&created_at_str)
                        .map(|dt| dt.with_timezone(&chrono::Utc))
                        .unwrap_or_else(|_| chrono::Utc::now());
                    ActivePositionEntry {
                        token_symbol: token_opt.unwrap_or_else(|| token_address.clone()),
                        trade_uuid,
                        wallet_address,
                        token_address,
                        strategy,
                        entry_price,
                        entry_amount_sol,
                        entry_time,
                    }
                },
            )
            .collect();

        Ok(entries)
    }

    async fn get_active_position_tokens(&self) -> AppResult<Vec<ActivePositionSummary>> {
        let rows: Vec<(String, String, String, String, Option<String>)> = sqlx::query_as(
            r#"
            SELECT trade_uuid, token_address, entry_price, entry_amount_sol, entry_sol_price_usd
            FROM positions
            WHERE state IN ('ACTIVE', 'EXITING')
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(uuid, token, price, size, sol_price)| ActivePositionSummary {
                    trade_uuid: uuid,
                    token_address: token,
                    entry_price: text_to_dec(&price),
                    entry_amount_sol: text_to_dec(&size),
                    entry_sol_price_usd: sol_price.as_deref().map(text_to_dec),
                },
            )
            .collect())
    }

    async fn get_position_peak_price(&self, trade_uuid: &str) -> AppResult<Option<String>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT peak_price FROM exit_targets WHERE trade_uuid = ?")
                .bind(trade_uuid)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(p,)| p))
    }

    // ========================================================================
    // WALLET OPERATIONS - ADVANCED
    // ========================================================================

    async fn upsert_wallet(
        &self,
        address: &str,
        wqs_score: Option<Decimal>,
        roi_7d: Option<Decimal>,
        roi_30d: Option<Decimal>,
        trade_count_30d: Option<i32>,
        win_rate: Option<Decimal>,
        max_drawdown_30d: Option<Decimal>,
        avg_trade_size_sol: Option<Decimal>,
        notes: Option<&str>,
    ) -> AppResult<bool> {
        let result = sqlx::query(
            r#"
            INSERT INTO wallets (
                address, status, wqs_score, roi_7d, roi_30d,
                trade_count_30d, win_rate, max_drawdown_30d,
                avg_trade_size_sol, notes,
                created_at, updated_at
            )
            VALUES (?, 'CANDIDATE', ?, ?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
            ON CONFLICT(address) DO UPDATE SET
                wqs_score        = COALESCE(excluded.wqs_score, wqs_score),
                roi_7d           = COALESCE(excluded.roi_7d, roi_7d),
                roi_30d          = COALESCE(excluded.roi_30d, roi_30d),
                trade_count_30d  = COALESCE(excluded.trade_count_30d, trade_count_30d),
                win_rate         = COALESCE(excluded.win_rate, win_rate),
                max_drawdown_30d = COALESCE(excluded.max_drawdown_30d, max_drawdown_30d),
                avg_trade_size_sol = COALESCE(excluded.avg_trade_size_sol, avg_trade_size_sol),
                notes            = COALESCE(excluded.notes, notes),
                updated_at       = CURRENT_TIMESTAMP
            "#,
        )
        .bind(address)
        .bind(wqs_score.map(|d| dec_to_text(&d)))
        .bind(roi_7d.map(|d| dec_to_text(&d)))
        .bind(roi_30d.map(|d| dec_to_text(&d)))
        .bind(trade_count_30d)
        .bind(win_rate.map(|d| dec_to_text(&d)))
        .bind(max_drawdown_30d.map(|d| dec_to_text(&d)))
        .bind(avg_trade_size_sol.map(|d| dec_to_text(&d)))
        .bind(notes)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() != 1)
    }

    async fn update_wallet_status_ext(
        &self,
        address: &str,
        status: &str,
        ttl_hours: Option<i32>,
        reason: Option<&str>,
    ) -> AppResult<bool> {
        let ttl_expires_at =
            ttl_hours.map(|hours| chrono::Utc::now() + chrono::Duration::hours(hours as i64));

        let promoted_at = if status == "ACTIVE" {
            Some(chrono::Utc::now().to_rfc3339())
        } else {
            None
        };

        let result = sqlx::query(
            r#"
            UPDATE wallets
            SET status = ?,
                promoted_at = COALESCE(?, promoted_at),
                ttl_expires_at = ?,
                notes = COALESCE(?, notes)
            WHERE address = ?
            "#,
        )
        .bind(status)
        .bind(promoted_at)
        .bind(ttl_expires_at.map(|dt| dt.to_rfc3339()))
        .bind(reason)
        .bind(address)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    async fn get_expired_ttl_wallets(&self) -> AppResult<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            r#"
            SELECT address FROM wallets
            WHERE status = 'ACTIVE'
            AND ttl_expires_at IS NOT NULL
            AND ttl_expires_at < datetime('now')
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|(addr,)| addr).collect())
    }

    async fn demote_wallet(&self, address: &str, reason: &str) -> AppResult<()> {
        sqlx::query(
            r#"
            UPDATE wallets
            SET status = 'CANDIDATE',
                ttl_expires_at = NULL,
                notes = ?
            WHERE address = ?
            "#,
        )
        .bind(reason)
        .bind(address)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ========================================================================
    // WALLET MONITORING
    // ========================================================================

    async fn get_wallet_monitoring(
        &self,
        wallet_address: &str,
    ) -> AppResult<Option<WalletMonitoring>> {
        let row = sqlx::query(
            r#"
            SELECT
                wallet_address,
                helius_webhook_id,
                rpc_polling_active,
                last_transaction_signature,
                last_monitored_at,
                monitoring_enabled,
                webhook_status,
                webhook_registered_at,
                webhook_last_health_check,
                webhook_health_status,
                registration_attempts,
                last_registration_error,
                last_updated_url,
                created_at,
                updated_at
            FROM wallet_monitoring
            WHERE wallet_address = ?
            "#,
        )
        .bind(wallet_address)
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::Database)?;

        match row {
            Some(r) => Ok(Some(WalletMonitoring {
                wallet_address: r.try_get("wallet_address").unwrap_or_default(),
                helius_webhook_id: r.try_get("helius_webhook_id").ok(),
                rpc_polling_active: r.try_get("rpc_polling_active").unwrap_or(0),
                last_transaction_signature: r.try_get("last_transaction_signature").ok(),
                last_monitored_at: r.try_get("last_monitored_at").ok(),
                monitoring_enabled: r.try_get("monitoring_enabled").unwrap_or(0),
                created_at: r.try_get("created_at").unwrap_or_default(),
                updated_at: r.try_get("updated_at").unwrap_or_default(),
                webhook_status: r.try_get("webhook_status").ok(),
                webhook_registered_at: r.try_get("webhook_registered_at").ok(),
                webhook_last_health_check: r.try_get("webhook_last_health_check").ok(),
                webhook_health_status: r.try_get("webhook_health_status").ok(),
                registration_attempts: r.try_get("registration_attempts").unwrap_or(0),
                last_registration_error: r.try_get("last_registration_error").ok(),
                last_updated_url: r.try_get("last_updated_url").ok(),
            })),
            None => Ok(None),
        }
    }

    async fn upsert_wallet_monitoring(
        &self,
        wallet_address: &str,
        helius_webhook_id: Option<&str>,
        monitoring_enabled: bool,
    ) -> AppResult<()> {
        sqlx::query(
            r#"
            INSERT INTO wallet_monitoring (
                wallet_address, helius_webhook_id, monitoring_enabled, last_monitored_at
            )
            VALUES (?, ?, ?, CURRENT_TIMESTAMP)
            ON CONFLICT(wallet_address) DO UPDATE SET
                helius_webhook_id = COALESCE(?, helius_webhook_id),
                monitoring_enabled = ?,
                last_monitored_at = CURRENT_TIMESTAMP,
                updated_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(wallet_address)
        .bind(helius_webhook_id)
        .bind(if monitoring_enabled { 1 } else { 0 })
        .bind(helius_webhook_id)
        .bind(if monitoring_enabled { 1 } else { 0 })
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_wallet_monitoring_signature(
        &self,
        wallet_address: &str,
        signature: &str,
    ) -> AppResult<()> {
        sqlx::query(
            r#"
            UPDATE wallet_monitoring
            SET last_transaction_signature = ?,
                last_monitored_at = CURRENT_TIMESTAMP
            WHERE wallet_address = ?
            "#,
        )
        .bind(signature)
        .bind(wallet_address)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_wallets_needing_webhook_registration(&self) -> AppResult<Vec<String>> {
        let wallets = sqlx::query_scalar(
            r#"
            SELECT w.address
            FROM wallets w
            LEFT JOIN wallet_monitoring wm ON w.address = wm.wallet_address
            WHERE w.status = 'ACTIVE'
            AND (wm.helius_webhook_id IS NULL OR wm.helius_webhook_id = '')
            AND w.address IS NOT NULL
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(wallets)
    }

    async fn get_stale_webhook_wallets(&self, threshold_days: i32) -> AppResult<Vec<String>> {
        let threshold_timestamp =
            chrono::Utc::now() - chrono::Duration::days(threshold_days as i64);

        let wallets = sqlx::query_scalar(
            r#"
            SELECT wallet_address
            FROM wallet_monitoring
            WHERE webhook_status = 'active'
            AND (webhook_last_health_check IS NULL OR webhook_last_health_check < ?)
            AND helius_webhook_id IS NOT NULL
            "#,
        )
        .bind(threshold_timestamp.format("%Y-%m-%d %H:%M:%S").to_string())
        .fetch_all(&self.pool)
        .await?;
        Ok(wallets)
    }

    async fn get_all_wallet_monitoring(&self) -> AppResult<Vec<WalletMonitoring>> {
        let rows = sqlx::query(
            r#"
            SELECT
                wallet_address,
                helius_webhook_id,
                rpc_polling_active,
                last_transaction_signature,
                last_monitored_at,
                monitoring_enabled,
                webhook_status,
                webhook_registered_at,
                webhook_last_health_check,
                webhook_health_status,
                registration_attempts,
                last_registration_error,
                last_updated_url,
                created_at,
                updated_at
            FROM wallet_monitoring
            WHERE wallet_address IS NOT NULL
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::Database)?;

        let records = rows
            .iter()
            .map(|r| WalletMonitoring {
                wallet_address: r.try_get("wallet_address").unwrap_or_default(),
                helius_webhook_id: r.try_get("helius_webhook_id").ok(),
                rpc_polling_active: r.try_get("rpc_polling_active").unwrap_or(0),
                last_transaction_signature: r.try_get("last_transaction_signature").ok(),
                last_monitored_at: r.try_get("last_monitored_at").ok(),
                monitoring_enabled: r.try_get("monitoring_enabled").unwrap_or(0),
                created_at: r.try_get("created_at").unwrap_or_default(),
                updated_at: r.try_get("updated_at").unwrap_or_default(),
                webhook_status: r.try_get("webhook_status").ok(),
                webhook_registered_at: r.try_get("webhook_registered_at").ok(),
                webhook_last_health_check: r.try_get("webhook_last_health_check").ok(),
                webhook_health_status: r.try_get("webhook_health_status").ok(),
                registration_attempts: r.try_get("registration_attempts").unwrap_or(0),
                last_registration_error: r.try_get("last_registration_error").ok(),
                last_updated_url: r.try_get("last_updated_url").ok(),
            })
            .collect();

        Ok(records)
    }

    async fn update_webhook_health_status(
        &self,
        wallet_address: &str,
        health_status: &str,
        webhook_id: Option<&str>,
    ) -> AppResult<()> {
        sqlx::query(
            r#"
            UPDATE wallet_monitoring
            SET webhook_health_status = ?,
                webhook_last_health_check = CURRENT_TIMESTAMP,
                webhook_status = CASE
                    WHEN ? = 'healthy' THEN 'active'
                    WHEN ? = 'unhealthy' THEN 'paused'
                    ELSE webhook_status
                END
            WHERE wallet_address = ?
            "#,
        )
        .bind(health_status)
        .bind(health_status)
        .bind(health_status)
        .bind(wallet_address)
        .execute(&self.pool)
        .await?;

        if let Some(webhook_id) = webhook_id {
            info!(
                wallet = %wallet_address,
                webhook_id = %webhook_id,
                status = %health_status,
                "Updated webhook health status"
            );
        }

        Ok(())
    }

    async fn update_webhook_status(
        &self,
        wallet_address: &str,
        webhook_status: &str,
    ) -> AppResult<()> {
        sqlx::query(
            r#"
            UPDATE wallet_monitoring
            SET webhook_status = ?
            WHERE wallet_address = ?
            "#,
        )
        .bind(webhook_status)
        .bind(wallet_address)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

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
    ) -> AppResult<()> {
        let _ = sqlx::query(
            r#"
            INSERT INTO webhook_lifecycle_audit
            (wallet_address, action, status, webhook_id, details, error_message, duration_ms)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(wallet_address)
        .bind(action)
        .bind(status)
        .bind(webhook_id)
        .bind(details)
        .bind(error_message)
        .bind(duration_ms)
        .execute(&self.pool)
        .await;

        Ok(())
    }

    async fn get_webhook_audit_log(
        &self,
        wallet_address: Option<&str>,
        action: Option<&str>,
        status: Option<&str>,
        limit: Option<i64>,
    ) -> AppResult<Vec<WebhookAuditLog>> {
        let mut sql = String::from(
            r#"SELECT id, wallet_address, action, status, webhook_id, details, error_message, duration_ms, created_at
               FROM webhook_lifecycle_audit WHERE 1=1"#,
        );
        let mut binds: Vec<String> = Vec::new();

        if let Some(wa) = wallet_address {
            sql.push_str(" AND wallet_address = ?");
            binds.push(wa.to_string());
        }
        if let Some(a) = action {
            sql.push_str(" AND action = ?");
            binds.push(a.to_string());
        }
        if let Some(s) = status {
            sql.push_str(" AND status = ?");
            binds.push(s.to_string());
        }

        sql.push_str(" ORDER BY created_at DESC");

        let limit_val = limit.unwrap_or(100).clamp(1, 1000);
        sql.push_str(" LIMIT ?");

        let mut query = sqlx::query_as::<_, WebhookAuditLogSqliteRow>(&sql);
        for b in binds {
            query = query.bind(b);
        }
        query = query.bind(limit_val);

        let rows = query.fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(|r| r.into_audit_log()).collect())
    }

    async fn increment_webhook_registration_attempts(
        &self,
        wallet_address: &str,
        error: Option<&str>,
    ) -> AppResult<()> {
        sqlx::query(
            r#"
            UPDATE wallet_monitoring
            SET registration_attempts = registration_attempts + 1,
                last_registration_error = ?,
                webhook_status = CASE
                    WHEN registration_attempts >= 2 THEN 'failed'
                    ELSE webhook_status
                END
            WHERE wallet_address = ?
            "#,
        )
        .bind(error)
        .bind(wallet_address)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_webhook_configuration(&self, key: &str) -> AppResult<Option<String>> {
        let result: Option<String> = sqlx::query_scalar(
            r#"
            SELECT config_value FROM webhook_configuration WHERE config_key = ?
            "#,
        )
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;
        Ok(result)
    }

    async fn update_webhook_configuration(
        &self,
        key: &str,
        value: &str,
        updated_by: &str,
    ) -> AppResult<()> {
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO webhook_configuration
            (config_key, config_value, last_updated_at, updated_by)
            VALUES (?, ?, CURRENT_TIMESTAMP, ?)
            "#,
        )
        .bind(key)
        .bind(value)
        .bind(updated_by)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_orphaned_webhooks(&self, helius_webhook_ids: &[String]) -> AppResult<Vec<String>> {
        if helius_webhook_ids.is_empty() {
            return Ok(Vec::new());
        }

        let placeholders: Vec<String> =
            std::iter::repeat_n("?".to_string(), helius_webhook_ids.len()).collect();

        let query = format!(
            "SELECT DISTINCT helius_webhook_id FROM wallet_monitoring WHERE helius_webhook_id IN ({})",
            placeholders.join(", ")
        );

        let mut q = sqlx::query_scalar::<_, String>(&query);
        for id in helius_webhook_ids {
            q = q.bind(id);
        }
        let existing: Vec<String> = q.fetch_all(&self.pool).await?;

        Ok(helius_webhook_ids
            .iter()
            .filter(|id| !existing.contains(id))
            .cloned()
            .collect())
    }

    // ========================================================================
    // EXIT TARGETS
    // ========================================================================

    #[allow(clippy::too_many_arguments)]
    async fn upsert_exit_target(
        &self,
        trade_uuid: &str,
        entry_price: Decimal,
        entry_amount_sol: Decimal,
        peak_price: Decimal,
        peak_profit_percent: Decimal,
        targets_hit_json: &str,
        trailing_stop_active: bool,
        trailing_stop_price: Decimal,
        remaining_fraction: Decimal,
    ) -> AppResult<()> {
        sqlx::query(
            r#"
            INSERT INTO exit_targets (
                trade_uuid, entry_price, entry_amount_sol, peak_price,
                peak_profit_percent, targets_hit, trailing_stop_active, trailing_stop_price,
                remaining_fraction
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(trade_uuid) DO UPDATE SET
                peak_price = excluded.peak_price,
                peak_profit_percent = excluded.peak_profit_percent,
                targets_hit = excluded.targets_hit,
                trailing_stop_active = excluded.trailing_stop_active,
                trailing_stop_price = excluded.trailing_stop_price,
                remaining_fraction = excluded.remaining_fraction,
                last_updated = CURRENT_TIMESTAMP
            "#,
        )
        .bind(trade_uuid)
        .bind(dec_to_text(&entry_price))
        .bind(dec_to_text(&entry_amount_sol))
        .bind(dec_to_text(&peak_price))
        .bind(dec_to_text(&peak_profit_percent))
        .bind(targets_hit_json)
        .bind(trailing_stop_active as i64)
        .bind(dec_to_text(&trailing_stop_price))
        .bind(dec_to_text(&remaining_fraction))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn load_exit_target(&self, trade_uuid: &str) -> AppResult<Option<ExitTargetData>> {
        #[allow(clippy::type_complexity)]
        let row: Option<(String, String, String, String, String, i64, String, String)> =
            sqlx::query_as(
                r#"
                SELECT entry_price, entry_amount_sol, peak_price, peak_profit_percent,
                       COALESCE(targets_hit, '[]'), trailing_stop_active, COALESCE(trailing_stop_price, '0'),
                       COALESCE(remaining_fraction, '1')
                FROM exit_targets
                WHERE trade_uuid = ?
                "#,
            )
            .bind(trade_uuid)
            .fetch_optional(&self.pool)
            .await?;

        Ok(
            row.map(|(ep, ea, pp, ppp, th, tsa, tsp, rf)| ExitTargetData {
                entry_price: text_to_dec(&ep),
                entry_amount_sol: text_to_dec(&ea),
                peak_price: text_to_dec(&pp),
                peak_profit_percent: text_to_dec(&ppp),
                targets_hit: th,
                trailing_stop_active: tsa != 0,
                trailing_stop_price: text_to_dec(&tsp),
                remaining_fraction: text_to_dec(&rf),
            }),
        )
    }

    async fn delete_exit_target(&self, trade_uuid: &str) -> AppResult<()> {
        sqlx::query("DELETE FROM exit_targets WHERE trade_uuid = ?")
            .bind(trade_uuid)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ========================================================================
    // RECONCILIATION
    // ========================================================================

    async fn insert_reconciliation_log(
        &self,
        trade_uuid: &str,
        expected_state: &str,
        actual_on_chain: Option<&str>,
        discrepancy: &str,
        on_chain_tx_signature: Option<&str>,
        notes: Option<&str>,
    ) -> AppResult<i64> {
        let result = sqlx::query(
            r#"
            INSERT INTO reconciliation_log (
                trade_uuid, expected_state, actual_on_chain, discrepancy,
                on_chain_tx_signature, notes
            ) VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(trade_uuid)
        .bind(expected_state)
        .bind(actual_on_chain)
        .bind(discrepancy)
        .bind(on_chain_tx_signature)
        .bind(notes)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    async fn get_reconciliation_status(
        &self,
        discrepancies_limit: i32,
    ) -> AppResult<ReconciliationStatus> {
        let limit = discrepancies_limit.clamp(1, 100) as i64;

        let latest_row = sqlx::query_as::<_, (Option<String>, Option<i64>)>(
            r#"
            SELECT
                datetime(created_at) as created_at,
                CAST(strftime('%s', created_at) AS INTEGER) as created_ts
            FROM reconciliation_log
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .fetch_optional(&self.pool)
        .await?;

        let (last_at, last_ts) = latest_row.unwrap_or((None, None));

        let next_at = last_ts.map(|ts| {
            let next = ts + 86400 - (ts % 86400) + 14400;
            datetime_from_timestamp(next as f64)
        });

        let (checked_count, discrepancy_count, unresolved_count): (i64, i64, i64) =
            sqlx::query_as(
                r#"
                SELECT
                    COALESCE(COUNT(*), 0) as checked,
                    COALESCE(SUM(CASE WHEN discrepancy != 'NONE' THEN 1 ELSE 0 END), 0) as discrepancies,
                    COALESCE(SUM(CASE WHEN discrepancy != 'NONE' AND resolved_at IS NULL THEN 1 ELSE 0 END), 0) as unresolved
                FROM reconciliation_log
                "#,
            )
            .fetch_one(&self.pool)
            .await?;

        let recent_rows = sqlx::query(
            r#"
            SELECT
                id,
                trade_uuid,
                discrepancy,
                notes,
                expected_state,
                actual_on_chain,
                datetime(created_at) as detected_at,
                resolved_at IS NOT NULL as resolved,
                datetime(resolved_at) as resolved_at
            FROM reconciliation_log
            WHERE discrepancy != 'NONE'
            ORDER BY created_at DESC
            LIMIT ?
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let recent_discrepancies = recent_rows
            .iter()
            .map(|row| {
                let discrepancy: String = row.get("discrepancy");
                let notes: Option<String> = row.get("notes");

                DiscrepancyRow {
                    id: row.get("id"),
                    trade_uuid: row.get("trade_uuid"),
                    discrepancy_type: normalize_discrepancy_type(&discrepancy),
                    severity: infer_severity(&discrepancy),
                    description: notes.unwrap_or_else(|| discrepancy.clone()),
                    db_value: row.get("expected_state"),
                    on_chain_value: row.get("actual_on_chain"),
                    detected_at: row.get("detected_at"),
                    resolved: row.get("resolved"),
                    resolved_at: row.get("resolved_at"),
                }
            })
            .collect();

        Ok(ReconciliationStatus {
            last_reconciliation_at: last_at,
            next_reconciliation_at: next_at,
            status: "completed".to_string(),
            checked_count,
            discrepancy_count,
            unresolved_count,
            duration_seconds: None,
            recent_discrepancies,
        })
    }

    async fn get_reconciliation_history(&self, limit: i32) -> AppResult<Vec<ReconciliationRun>> {
        let limit_val = limit.clamp(1, 100) as i64;

        let rows = sqlx::query(
            r#"
            WITH daily_runs AS (
                SELECT
                    DATE(created_at) as run_date,
                    MIN(id) as id,
                    MIN(created_at) as started_at,
                    MAX(created_at) as completed_at,
                    'completed' as status,
                    COUNT(*) as checked_count,
                    SUM(CASE WHEN discrepancy != 'NONE' THEN 1 ELSE 0 END) as discrepancy_count,
                    SUM(CASE WHEN discrepancy != 'NONE' AND resolved_at IS NULL THEN 1 ELSE 0 END) as unresolved_count,
                    CAST((julianday(MAX(created_at)) - julianday(MIN(created_at))) * 86400.0 AS REAL) as duration_seconds
                FROM reconciliation_log
                GROUP BY DATE(created_at)
                ORDER BY run_date DESC
                LIMIT ?
            )
            SELECT
                id,
                datetime(started_at) as started_at,
                CASE WHEN completed_at IS NOT NULL THEN datetime(completed_at) ELSE NULL END as completed_at,
                status,
                checked_count,
                discrepancy_count,
                unresolved_count,
                duration_seconds
            FROM daily_runs
            "#,
        )
        .bind(limit_val)
        .fetch_all(&self.pool)
        .await?;

        let runs = rows
            .iter()
            .map(|row| ReconciliationRun {
                id: row.get("id"),
                started_at: row.get("started_at"),
                completed_at: row.get("completed_at"),
                status: row.get("status"),
                checked_count: row.get("checked_count"),
                discrepancy_count: row.get("discrepancy_count"),
                unresolved_count: row.get("unresolved_count"),
                duration_seconds: row.get("duration_seconds"),
            })
            .collect();

        Ok(runs)
    }

    async fn count_reconciliation_runs(&self) -> AppResult<i64> {
        let result: (i64,) = sqlx::query_as(
            r#"
            SELECT COUNT(DISTINCT DATE(created_at)) as count
            FROM reconciliation_log
            "#,
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(result.0)
    }

    async fn get_reconciliation_stats(&self, _time_range: &str) -> AppResult<ReconciliationStats> {
        let (total_reconciliations, total_checked, total_discrepancies, total_unresolved): (
            i64,
            i64,
            i64,
            i64,
        ) = sqlx::query_as(
            r#"
            WITH stats AS (
                SELECT
                    COUNT(DISTINCT DATE(created_at)) as total_runs,
                    COUNT(*) as total_checked,
                    SUM(CASE WHEN discrepancy != 'NONE' THEN 1 ELSE 0 END) as total_discrepancies,
                    SUM(CASE WHEN discrepancy != 'NONE' AND resolved_at IS NULL THEN 1 ELSE 0 END) as total_unresolved
                FROM reconciliation_log
            )
            SELECT
                total_runs,
                total_checked,
                COALESCE(total_discrepancies, 0) as total_discrepancies,
                COALESCE(total_unresolved, 0) as total_unresolved
            FROM stats
            "#,
        )
        .fetch_one(&self.pool)
        .await?;

        let successful_reconciliations = sqlx::query_as::<_, (i64,)>(
            r#"
            SELECT COUNT(DISTINCT DATE(created_at))
            FROM reconciliation_log
            WHERE discrepancy = 'NONE' OR resolved_at IS NOT NULL
            "#,
        )
        .fetch_one(&self.pool)
        .await?
        .0;

        let failed_reconciliations = total_reconciliations - successful_reconciliations;

        let avg_discrepancies_per_run = if total_reconciliations > 0 {
            total_discrepancies as f64 / total_reconciliations as f64
        } else {
            0.0
        };

        let discrepancy_types = sqlx::query(
            r#"
            SELECT
                discrepancy,
                COUNT(*) as count
            FROM reconciliation_log
            WHERE discrepancy != 'NONE'
            GROUP BY discrepancy
            ORDER BY count DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let most_common_discrepancy_types = discrepancy_types
            .iter()
            .map(|row| {
                let discrepancy_type: String = row.get("discrepancy");
                let count: i64 = row.get("count");
                let percentage = if total_discrepancies > 0 {
                    (count as f64 / total_discrepancies as f64) * 100.0
                } else {
                    0.0
                };

                DiscrepancyTypeStats {
                    discrepancy_type: normalize_discrepancy_type(&discrepancy_type),
                    count,
                    percentage,
                }
            })
            .collect();

        Ok(ReconciliationStats {
            total_reconciliations,
            successful_reconciliations,
            failed_reconciliations,
            total_checked,
            total_discrepancies,
            total_unresolved,
            avg_discrepancies_per_run,
            most_common_discrepancy_types,
        })
    }

    async fn resolve_discrepancy(
        &self,
        id: i64,
        resolved_by: &str,
        resolution: &str,
    ) -> AppResult<()> {
        sqlx::query(
            r#"
            UPDATE reconciliation_log
            SET resolved_at = CURRENT_TIMESTAMP,
                resolved_by = ?,
                notes = COALESCE(notes || '; ', '') || ?
            WHERE id = ? AND resolved_at IS NULL
            "#,
        )
        .bind(resolved_by)
        .bind(resolution)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ========================================================================
    // TRADES - FILTERED QUERIES
    // ========================================================================

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
    ) -> AppResult<Vec<TradeDetail>> {
        let mut query = String::from(
            r#"
            SELECT id, trade_uuid, wallet_address, token_address, token_symbol, strategy,
                   side, amount_sol, price_at_signal, tx_signature, status, retry_count,
                   error_message, pnl_sol, pnl_usd, jito_tip_sol, dex_fee_sol, slippage_cost_sol,
                   total_cost_sol, net_pnl_sol, created_at, updated_at
            FROM trades
            WHERE 1=1
            "#,
        );

        let mut bindings: Vec<String> = Vec::new();

        if let Some(from) = from_date {
            query.push_str(" AND created_at >= ?");
            bindings.push(from.to_string());
        }
        if let Some(to) = to_date {
            query.push_str(" AND created_at <= ?");
            bindings.push(to.to_string());
        }
        if let Some(status) = status_filter {
            query.push_str(" AND status = ?");
            bindings.push(status.to_string());
        }
        if let Some(strategy) = strategy_filter {
            query.push_str(" AND strategy = ?");
            bindings.push(strategy.to_string());
        }
        if let Some(wallet) = wallet_address_filter {
            query.push_str(" AND wallet_address = ?");
            bindings.push(wallet.to_string());
        }

        query.push_str(" ORDER BY created_at DESC LIMIT ? OFFSET ?");

        let mut q = sqlx::query_as::<_, TradeDetailSqliteRow>(&query);
        for binding in bindings {
            q = q.bind(binding);
        }
        q = q.bind(limit).bind(offset);

        let rows = q.fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(|r| r.into_trade_detail()).collect())
    }

    async fn count_trades_filtered(
        &self,
        from_date: Option<&str>,
        to_date: Option<&str>,
        status_filter: Option<&str>,
        strategy_filter: Option<&str>,
        wallet_address_filter: Option<&str>,
    ) -> AppResult<i64> {
        let mut query = String::from("SELECT COUNT(*) FROM trades WHERE 1=1");
        let mut bindings: Vec<String> = Vec::new();

        if let Some(from) = from_date {
            query.push_str(" AND created_at >= ?");
            bindings.push(from.to_string());
        }
        if let Some(to) = to_date {
            query.push_str(" AND created_at <= ?");
            bindings.push(to.to_string());
        }
        if let Some(status) = status_filter {
            query.push_str(" AND status = ?");
            bindings.push(status.to_string());
        }
        if let Some(strategy) = strategy_filter {
            query.push_str(" AND strategy = ?");
            bindings.push(strategy.to_string());
        }
        if let Some(wallet) = wallet_address_filter {
            query.push_str(" AND wallet_address = ?");
            bindings.push(wallet.to_string());
        }

        let mut q = sqlx::query_as::<_, (i64,)>(&query);
        for binding in bindings {
            q = q.bind(binding);
        }

        let (count,) = q.fetch_one(&self.pool).await?;
        Ok(count)
    }

    async fn update_trade_costs(
        &self,
        trade_uuid: &str,
        jito_tip_sol: Decimal,
        dex_fee_sol: Decimal,
        slippage_cost_sol: Decimal,
    ) -> AppResult<()> {
        let row: Option<(String, String, String)> = sqlx::query_as(
            "SELECT jito_tip_sol, dex_fee_sol, slippage_cost_sol FROM trades WHERE trade_uuid = ?",
        )
        .bind(trade_uuid)
        .fetch_optional(&self.pool)
        .await?;

        let (current_jito, current_dex, current_slip) = row
            .map(|(j, d, s)| (text_to_dec(&j), text_to_dec(&d), text_to_dec(&s)))
            .unwrap_or((Decimal::ZERO, Decimal::ZERO, Decimal::ZERO));

        let new_jito = current_jito + jito_tip_sol;
        let new_dex = current_dex + dex_fee_sol;
        let new_slip = current_slip + slippage_cost_sol;
        let total = new_jito + new_dex + new_slip;

        let result = sqlx::query(
            r#"
            UPDATE trades
            SET jito_tip_sol = ?,
                dex_fee_sol = ?,
                slippage_cost_sol = ?,
                total_cost_sol = ?
            WHERE trade_uuid = ?
            "#,
        )
        .bind(dec_to_text(&new_jito))
        .bind(dec_to_text(&new_dex))
        .bind(dec_to_text(&new_slip))
        .bind(dec_to_text(&total))
        .bind(trade_uuid)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(AppError::NotFound(format!(
                "trade_uuid {} not found",
                trade_uuid
            )));
        }

        Ok(())
    }

    async fn update_trade_net_pnl(&self, trade_uuid: &str, net_pnl_sol: Decimal) -> AppResult<()> {
        let result = sqlx::query(
            r#"
            UPDATE trades
            SET net_pnl_sol = ?
            WHERE trade_uuid = ?
            "#,
        )
        .bind(dec_to_text(&net_pnl_sol))
        .bind(trade_uuid)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(AppError::NotFound(format!(
                "trade_uuid {} not found",
                trade_uuid
            )));
        }

        Ok(())
    }

    async fn mark_trade_dead_letter(
        &self,
        trade_uuid: &str,
        payload: &str,
        error: &str,
    ) -> AppResult<()> {
        let mut tx = self.pool.begin().await?;

        let result = sqlx::query(
            r#"
            UPDATE trades
            SET status = 'DEAD_LETTER', error_message = ?
            WHERE trade_uuid = ?
            "#,
        )
        .bind(error)
        .bind(trade_uuid)
        .execute(&mut *tx)
        .await?;

        if result.rows_affected() == 0 {
            return Err(AppError::NotFound(format!(
                "trade_uuid '{}' not found",
                trade_uuid
            )));
        }

        sqlx::query(
            r#"
            INSERT INTO dead_letter_queue (trade_uuid, payload, reason, error_details)
            VALUES (?, ?, 'DEAD_LETTER', ?)
            "#,
        )
        .bind(trade_uuid)
        .bind(payload)
        .bind(error)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    // ========================================================================
    // CONFIG AUDIT
    // ========================================================================

    async fn log_config_change(
        &self,
        key: &str,
        old_value: Option<&str>,
        new_value: &str,
        changed_by: &str,
        reason: Option<&str>,
    ) -> AppResult<()> {
        sqlx::query(
            r#"
            INSERT INTO config_audit (key, old_value, new_value, changed_by, change_reason)
            VALUES (?, ?, ?, ?, ?)
            "#,
        )
        .bind(key)
        .bind(old_value)
        .bind(new_value)
        .bind(changed_by)
        .bind(reason)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ========================================================================
    // INCIDENTS API (Dead Letter Queue & Config Audit)
    // ========================================================================

    async fn get_dead_letter_entries(
        &self,
        limit: i32,
        offset: i32,
    ) -> AppResult<Vec<DeadLetterItem>> {
        #[allow(clippy::type_complexity)]
        let rows: Vec<(
            i64,
            Option<String>,
            String,
            String,
            Option<String>,
            Option<String>,
            i32,
            i64,
            String,
            Option<String>,
        )> = sqlx::query_as(
            "SELECT id, trade_uuid, payload, reason, error_details, source_ip, retry_count, can_retry, received_at, processed_at FROM dead_letter_queue ORDER BY received_at DESC LIMIT ? OFFSET ?",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        let items = rows
            .into_iter()
            .map(
                |(
                    id,
                    trade_uuid,
                    payload,
                    reason,
                    error_details,
                    source_ip,
                    retry_count,
                    can_retry_int,
                    received_at,
                    processed_at,
                )| {
                    DeadLetterItem {
                        id,
                        trade_uuid,
                        payload,
                        reason,
                        error_details,
                        source_ip,
                        retry_count,
                        can_retry: can_retry_int != 0,
                        received_at,
                        processed_at,
                    }
                },
            )
            .collect();

        Ok(items)
    }

    async fn count_dead_letter_entries(&self) -> AppResult<i64> {
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM dead_letter_queue")
            .fetch_one(&self.pool)
            .await?;
        Ok(count.0)
    }

    async fn get_retryable_dlq_items(&self, limit: i64) -> AppResult<Vec<RetryableDlqItem>> {
        let rows = sqlx::query_as::<_, (String, String, i64)>(
            "SELECT trade_uuid, payload, retry_count FROM dead_letter_queue WHERE can_retry = 1 AND processed_at IS NULL LIMIT ?"
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|(trade_uuid, payload, retry_count)| RetryableDlqItem {
                trade_uuid,
                payload,
                retry_count,
            })
            .collect())
    }

    async fn update_dlq_item(
        &self,
        trade_uuid: &str,
        retry_count: i64,
        can_retry: bool,
        mark_processed: bool,
    ) -> AppResult<()> {
        if mark_processed {
            sqlx::query(
                "UPDATE dead_letter_queue SET retry_count = ?, can_retry = ?, processed_at = CURRENT_TIMESTAMP WHERE trade_uuid = ? AND processed_at IS NULL"
            )
            .bind(retry_count)
            .bind(can_retry as i64)
            .bind(trade_uuid)
            .execute(&self.pool)
            .await?;
        } else {
            sqlx::query(
                "UPDATE dead_letter_queue SET retry_count = ?, can_retry = ? WHERE trade_uuid = ? AND processed_at IS NULL"
            )
            .bind(retry_count)
            .bind(can_retry as i64)
            .bind(trade_uuid)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    async fn update_dlq_items_batch(&self, items: Vec<UpdateDlqItemParams>) -> AppResult<usize> {
        if items.is_empty() {
            return Ok(0);
        }

        let mut tx = self.pool.begin().await?;

        let mut processed_count = 0usize;
        for item in &items {
            let result = if item.mark_processed {
                sqlx::query(
                    "UPDATE dead_letter_queue SET retry_count = ?, can_retry = ?, processed_at = CURRENT_TIMESTAMP WHERE trade_uuid = ? AND processed_at IS NULL"
                )
                .bind(item.retry_count)
                .bind(item.can_retry as i64)
                .bind(&item.trade_uuid)
                .execute(&mut *tx)
                .await?
            } else {
                sqlx::query(
                    "UPDATE dead_letter_queue SET retry_count = ?, can_retry = ? WHERE trade_uuid = ? AND processed_at IS NULL"
                )
                .bind(item.retry_count)
                .bind(item.can_retry as i64)
                .bind(&item.trade_uuid)
                .execute(&mut *tx)
                .await?
            };

            processed_count += result.rows_affected() as usize;
        }

        tx.commit().await?;
        Ok(processed_count)
    }

    async fn get_config_audit_entries(
        &self,
        limit: i32,
        offset: i32,
    ) -> AppResult<Vec<ConfigAuditItem>> {
        let rows = sqlx::query(
            "SELECT id, key, old_value, new_value, changed_by, change_reason, changed_at FROM config_audit ORDER BY changed_at DESC LIMIT ? OFFSET ?",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        let items = rows
            .iter()
            .map(|row| ConfigAuditItem {
                id: row.get("id"),
                key: row.get("key"),
                old_value: row.get("old_value"),
                new_value: row.get("new_value"),
                changed_by: row.get("changed_by"),
                change_reason: row.get("change_reason"),
                changed_at: row.get("changed_at"),
            })
            .collect();

        Ok(items)
    }

    async fn count_config_audit_entries(&self) -> AppResult<i64> {
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM config_audit")
            .fetch_one(&self.pool)
            .await?;
        Ok(count.0)
    }

    // ========================================================================
    // TRADE STATISTICS
    // ========================================================================

    async fn count_trades_by_status(&self, status: &str) -> AppResult<i64> {
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM trades WHERE status = ?")
            .bind(status)
            .fetch_one(&self.pool)
            .await?;
        Ok(count.0)
    }

    async fn get_closed_trade_count_for_wallet(&self, wallet_address: &str) -> AppResult<i64> {
        let result: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM trades WHERE wallet_address = ? AND status = 'CLOSED'",
        )
        .bind(wallet_address)
        .fetch_one(&self.pool)
        .await?;
        Ok(result.0)
    }

    async fn get_wallet_copy_performance(
        &self,
        wallet_address: &str,
    ) -> AppResult<Option<WalletCopyPerformance>> {
        let row = sqlx::query(
            r#"
            SELECT
                wallet_address,
                copy_pnl_7d,
                copy_pnl_30d,
                signal_success_rate,
                avg_return_per_trade,
                total_trades,
                winning_trades,
                last_updated
            FROM wallet_copy_performance
            WHERE wallet_address = ?
            "#,
        )
        .bind(wallet_address)
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::Database)?;

        match row {
            Some(r) => Ok(Some(WalletCopyPerformance {
                wallet_address: r
                    .try_get("wallet_address")
                    .unwrap_or(wallet_address.to_string()),
                copy_pnl_7d: text_to_dec(
                    &r.try_get::<String, _>("copy_pnl_7d").unwrap_or_default(),
                ),
                copy_pnl_30d: text_to_dec(
                    &r.try_get::<String, _>("copy_pnl_30d").unwrap_or_default(),
                ),
                signal_success_rate: text_to_dec(
                    &r.try_get::<String, _>("signal_success_rate")
                        .unwrap_or_default(),
                ),
                avg_return_per_trade: text_to_dec(
                    &r.try_get::<String, _>("avg_return_per_trade")
                        .unwrap_or_default(),
                ),
                total_trades: r.try_get("total_trades").unwrap_or(0),
                winning_trades: r.try_get("winning_trades").unwrap_or(0),
                last_updated: r.try_get("last_updated").unwrap_or_default(),
            })),
            None => Ok(None),
        }
    }

    async fn get_trade_latency_stats(&self, hours: i32) -> AppResult<TradeLatencyStats> {
        let time_filter = format!("-{} hours", hours);

        let latencies: Vec<f64> = sqlx::query_scalar(
            r#"
            SELECT CAST((julianday(updated_at) - julianday(created_at)) * 86400000 AS REAL)
             FROM trades
             WHERE status = 'CLOSED'
             AND created_at >= datetime('now', ?)
             AND updated_at IS NOT NULL
             AND updated_at > created_at
            "#,
        )
        .bind(&time_filter)
        .fetch_all(&self.pool)
        .await?;

        if latencies.is_empty() {
            return Ok(TradeLatencyStats {
                count: 0,
                avg_ms: 0.0,
                p50_ms: 0.0,
                p95_ms: 0.0,
                p99_ms: 0.0,
                max_ms: 0.0,
            });
        }

        let count = latencies.len() as u32;
        let avg_ms = latencies.iter().sum::<f64>() / count as f64;

        let mut sorted_latencies = latencies.clone();
        sorted_latencies.sort_by(|a, b| a.total_cmp(b));

        let p50_ms = *sorted_latencies
            .get((count as f64 * 0.50) as usize)
            .unwrap_or(&avg_ms);
        let p95_ms = *sorted_latencies
            .get((count as f64 * 0.95) as usize)
            .unwrap_or(&avg_ms);
        let p99_ms = *sorted_latencies
            .get((count as f64 * 0.99) as usize)
            .unwrap_or(&avg_ms);
        let max_ms = *sorted_latencies.last().unwrap_or(&avg_ms);

        Ok(TradeLatencyStats {
            count,
            avg_ms,
            p50_ms,
            p95_ms,
            p99_ms,
            max_ms,
        })
    }

    async fn get_trade_latency_histogram(
        &self,
        hours: i32,
        bucket_bounds: &[f64],
    ) -> AppResult<Vec<LatencyBucket>> {
        let time_filter = format!("-{} hours", hours);
        let total_count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*) FROM trades
             WHERE status = 'CLOSED'
             AND created_at >= datetime('now', ?)
             AND updated_at IS NOT NULL
            "#,
        )
        .bind(&time_filter)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);

        if total_count == 0 {
            return Ok(vec![]);
        }

        let mut buckets = Vec::new();
        let mut lower_bound = 0.0;

        for (i, &upper_bound) in bucket_bounds.iter().enumerate() {
            let count: i64 = sqlx::query_scalar(
                r#"
                SELECT COUNT(*) FROM trades
                 WHERE status = 'CLOSED'
                 AND created_at >= datetime('now', ?)
                 AND updated_at IS NOT NULL
                 AND (julianday(updated_at) - julianday(created_at)) * 86400000 >= ?
                 AND (julianday(updated_at) - julianday(created_at)) * 86400000 < ?
                "#,
            )
            .bind(&time_filter)
            .bind(lower_bound)
            .bind(upper_bound)
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0);

            let percentage = (count as f64 / total_count as f64) * 100.0;
            let range = if i == bucket_bounds.len() - 1 {
                format!("{}ms+", upper_bound)
            } else {
                format!("{}-{}ms", lower_bound, upper_bound)
            };

            buckets.push(LatencyBucket {
                range,
                count: count as u32,
                percentage,
            });

            lower_bound = upper_bound;
        }

        Ok(buckets)
    }

    async fn get_positions(&self, state_filter: Option<&str>) -> AppResult<Vec<PositionDetail>> {
        let rows =
            match state_filter {
                Some(state) => sqlx::query(
                    r#"SELECT id, trade_uuid, wallet_address, token_address, token_symbol, strategy,
                           entry_amount_sol, entry_price, entry_tx_signature, current_price,
                           unrealized_pnl_sol, unrealized_pnl_percent, state, exit_price,
                           exit_tx_signature, realized_pnl_sol, realized_pnl_usd,
                           opened_at, last_updated, closed_at
                    FROM positions WHERE state = ? ORDER BY last_updated DESC"#,
                )
                .bind(state)
                .fetch_all(&self.pool)
                .await?,
                None => sqlx::query(
                    r#"SELECT id, trade_uuid, wallet_address, token_address, token_symbol, strategy,
                           entry_amount_sol, entry_price, entry_tx_signature, current_price,
                           unrealized_pnl_sol, unrealized_pnl_percent, state, exit_price,
                           exit_tx_signature, realized_pnl_sol, realized_pnl_usd,
                           opened_at, last_updated, closed_at
                    FROM positions ORDER BY last_updated DESC"#,
                )
                .fetch_all(&self.pool)
                .await?,
            };

        let positions = rows
            .into_iter()
            .map(|row| PositionDetail {
                id: row.try_get("id").unwrap_or(0),
                trade_uuid: row.try_get("trade_uuid").unwrap_or_default(),
                wallet_address: row.try_get("wallet_address").unwrap_or_default(),
                token_address: row.try_get("token_address").unwrap_or_default(),
                token_symbol: row.try_get("token_symbol").ok(),
                strategy: row.try_get("strategy").unwrap_or_default(),
                entry_amount_sol: text_to_dec(
                    &row.try_get::<String, _>("entry_amount_sol")
                        .unwrap_or_default(),
                ),
                entry_price: text_to_dec(
                    &row.try_get::<String, _>("entry_price").unwrap_or_default(),
                ),
                entry_tx_signature: row.try_get("entry_tx_signature").unwrap_or_default(),
                current_price: opt_text_to_dec(
                    row.try_get::<String, _>("current_price").ok().as_deref(),
                ),
                unrealized_pnl_sol: opt_text_to_dec(
                    row.try_get::<String, _>("unrealized_pnl_sol")
                        .ok()
                        .as_deref(),
                ),
                unrealized_pnl_percent: opt_text_to_dec(
                    row.try_get::<String, _>("unrealized_pnl_percent")
                        .ok()
                        .as_deref(),
                ),
                state: row.try_get("state").unwrap_or_default(),
                exit_price: opt_text_to_dec(row.try_get::<String, _>("exit_price").ok().as_deref()),
                exit_tx_signature: row.try_get("exit_tx_signature").ok(),
                realized_pnl_sol: opt_text_to_dec(
                    row.try_get::<String, _>("realized_pnl_sol").ok().as_deref(),
                ),
                realized_pnl_usd: opt_text_to_dec(
                    row.try_get::<String, _>("realized_pnl_usd").ok().as_deref(),
                ),
                opened_at: row.try_get("opened_at").unwrap_or_default(),
                last_updated: row.try_get("last_updated").unwrap_or_default(),
                closed_at: row.try_get("closed_at").ok(),
            })
            .collect();
        Ok(positions)
    }

    async fn get_wallets(&self, status_filter: Option<&str>) -> AppResult<Vec<WalletDetail>> {
        let rows = match status_filter {
            Some(status) => {
                sqlx::query(
                    r#"SELECT id, address, status, wqs_score, roi_7d, roi_30d, trade_count_30d,
                           win_rate, max_drawdown_30d, avg_trade_size_sol,
                           avg_win_sol, avg_loss_sol, profit_factor, realized_pnl_30d_sol,
                           last_trade_at,
                           promoted_at, ttl_expires_at, notes, archetype,
                           avg_entry_delay_seconds, created_at, updated_at
                    FROM wallets
                    WHERE status = ?
                    ORDER BY wqs_score DESC NULLS LAST
                    LIMIT 1000"#,
                )
                .bind(status)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query(
                    r#"SELECT id, address, status, wqs_score, roi_7d, roi_30d, trade_count_30d,
                           win_rate, max_drawdown_30d, avg_trade_size_sol,
                           avg_win_sol, avg_loss_sol, profit_factor, realized_pnl_30d_sol,
                           last_trade_at,
                           promoted_at, ttl_expires_at, notes, archetype,
                           avg_entry_delay_seconds, created_at, updated_at
                    FROM wallets
                    ORDER BY wqs_score DESC NULLS LAST
                    LIMIT 1000"#,
                )
                .fetch_all(&self.pool)
                .await?
            }
        };
        let wallets = rows
            .into_iter()
            .map(|row| WalletDetail {
                id: row.try_get("id").unwrap_or(0),
                address: row.try_get("address").unwrap_or_default(),
                status: row.try_get("status").unwrap_or_default(),
                wqs_score: row
                    .try_get::<f64, _>("wqs_score")
                    .ok()
                    .and_then(Decimal::from_f64),
                roi_7d: opt_text_to_dec(row.try_get::<String, _>("roi_7d").ok().as_deref()),
                roi_30d: opt_text_to_dec(row.try_get::<String, _>("roi_30d").ok().as_deref()),
                trade_count_30d: row.try_get("trade_count_30d").ok(),
                win_rate: row
                    .try_get::<f64, _>("win_rate")
                    .ok()
                    .and_then(Decimal::from_f64),
                max_drawdown_30d: opt_text_to_dec(
                    row.try_get::<String, _>("max_drawdown_30d").ok().as_deref(),
                ),
                avg_trade_size_sol: opt_text_to_dec(
                    row.try_get::<String, _>("avg_trade_size_sol")
                        .ok()
                        .as_deref(),
                ),
                avg_win_sol: opt_text_to_dec(
                    row.try_get::<String, _>("avg_win_sol").ok().as_deref(),
                ),
                avg_loss_sol: opt_text_to_dec(
                    row.try_get::<String, _>("avg_loss_sol").ok().as_deref(),
                ),
                profit_factor: opt_text_to_dec(
                    row.try_get::<String, _>("profit_factor").ok().as_deref(),
                ),
                realized_pnl_30d_sol: opt_text_to_dec(
                    row.try_get::<String, _>("realized_pnl_30d_sol")
                        .ok()
                        .as_deref(),
                ),
                last_trade_at: row.try_get("last_trade_at").ok(),
                promoted_at: row.try_get("promoted_at").ok(),
                ttl_expires_at: row.try_get("ttl_expires_at").ok(),
                notes: row.try_get("notes").ok(),
                archetype: row.try_get("archetype").ok(),
                avg_entry_delay_seconds: row
                    .try_get::<f64, _>("avg_entry_delay_seconds")
                    .ok()
                    .and_then(Decimal::from_f64),
                created_at: row.try_get("created_at").unwrap_or_default(),
                updated_at: row.try_get("updated_at").unwrap_or_default(),
            })
            .collect();
        Ok(wallets)
    }

    fn pool(&self) -> DbPool {
        DbPool::SQLite(self.pool.clone())
    }

    async fn get_evaluation_data(&self) -> AppResult<(Decimal, Decimal, Decimal, Decimal)> {
        // Single query fetches all four data sources — one scan of positions table
        // instead of 4 separate scans. Each column uses CASE expressions to zero out
        // rows that don't belong to that accumulator.
        let rows: Vec<(String, String, String, String)> = sqlx::query_as(
            r#"
            SELECT
                COALESCE(unrealized_pnl_sol, '0'),
                CASE WHEN state = 'CLOSED' AND closed_at >= datetime('now', '-24 hours')
                     THEN COALESCE(realized_pnl_sol, '0') ELSE '0' END,
                CASE WHEN state = 'CLOSED' AND closed_at >= datetime('now', '-24 hours')
                          AND realized_pnl_usd IS NOT NULL
                     THEN COALESCE(realized_pnl_usd, '0') ELSE '0' END,
                CASE WHEN state = 'CLOSED' AND closed_at >= datetime('now', '-24 hours')
                          AND realized_pnl_usd IS NULL
                     THEN COALESCE(realized_pnl_sol, '0') ELSE '0' END
            FROM positions
            WHERE state IN ('ACTIVE', 'EXITING')
               OR (state = 'CLOSED' AND closed_at >= datetime('now', '-24 hours'))
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let mut unrealized_sol = Decimal::ZERO;
        let mut realized_pnl_sol = Decimal::ZERO;
        let mut realized_usd = Decimal::ZERO;
        let mut null_price_pnl_sol = Decimal::ZERO;

        for (u, r, ru, np) in &rows {
            unrealized_sol += text_to_dec(u);
            realized_pnl_sol += text_to_dec(r);
            realized_usd += text_to_dec(ru);
            null_price_pnl_sol += text_to_dec(np);
        }

        Ok((
            unrealized_sol,
            realized_pnl_sol,
            realized_usd,
            null_price_pnl_sol,
        ))
    }
}

// ========================================================================
// ROW CONVERSION HELPERS
// ========================================================================

impl SqliteBackend {
    #[tracing::instrument(skip(self, row))]
    fn row_to_trade(&self, row: sqlx::sqlite::SqliteRow) -> AppResult<Trade> {
        Ok(Trade {
            id: row.try_get("id").unwrap_or(0),
            trade_uuid: row.try_get("trade_uuid").unwrap_or_default(),
            wallet_address: row.try_get("wallet_address").unwrap_or_default(),
            token_address: row.try_get("token_address").unwrap_or_default(),
            token_symbol: row.try_get("token_symbol").ok(),
            strategy: row.try_get("strategy").unwrap_or_default(),
            side: row.try_get("side").unwrap_or_default(),
            amount_sol: text_to_dec(&row.try_get::<String, _>("amount_sol").unwrap_or_default()),
            price_at_signal: opt_text_to_dec(
                row.try_get::<String, _>("price_at_signal").ok().as_deref(),
            ),
            tx_signature: row.try_get("tx_signature").ok(),
            status: row.try_get("status").unwrap_or_default(),
            retry_count: row.try_get("retry_count").unwrap_or(0),
            error_message: row.try_get("error_message").ok(),
            pnl_sol: opt_text_to_dec(row.try_get::<String, _>("pnl_sol").ok().as_deref()),
            pnl_usd: opt_text_to_dec(row.try_get::<String, _>("pnl_usd").ok().as_deref()),
            jito_tip_sol: text_to_dec(
                &row.try_get::<String, _>("jito_tip_sol").unwrap_or_default(),
            ),
            dex_fee_sol: text_to_dec(&row.try_get::<String, _>("dex_fee_sol").unwrap_or_default()),
            slippage_cost_sol: text_to_dec(
                &row.try_get::<String, _>("slippage_cost_sol")
                    .unwrap_or_default(),
            ),
            total_cost_sol: text_to_dec(
                &row.try_get::<String, _>("total_cost_sol")
                    .unwrap_or_default(),
            ),
            net_pnl_sol: opt_text_to_dec(row.try_get::<String, _>("net_pnl_sol").ok().as_deref()),
            created_at: self
                .parse_datetime(row.try_get::<String, _>("created_at").ok().as_deref())?,
            updated_at: self
                .parse_datetime(row.try_get::<String, _>("updated_at").ok().as_deref())?,
        })
    }

    #[tracing::instrument(skip(self, row))]
    fn row_to_position(&self, row: sqlx::sqlite::SqliteRow) -> AppResult<Position> {
        Ok(Position {
            id: row.try_get("id").unwrap_or(0),
            trade_uuid: row.try_get("trade_uuid").unwrap_or_default(),
            wallet_address: row.try_get("wallet_address").unwrap_or_default(),
            token_address: row.try_get("token_address").unwrap_or_default(),
            token_symbol: row.try_get("token_symbol").ok(),
            strategy: row.try_get("strategy").unwrap_or_default(),
            entry_amount_sol: text_to_dec(
                &row.try_get::<String, _>("entry_amount_sol")
                    .unwrap_or_default(),
            ),
            entry_price: text_to_dec(&row.try_get::<String, _>("entry_price").unwrap_or_default()),
            entry_tx_signature: row.try_get("entry_tx_signature").unwrap_or_default(),
            current_price: opt_text_to_dec(
                row.try_get::<String, _>("current_price").ok().as_deref(),
            ),
            unrealized_pnl_sol: opt_text_to_dec(
                row.try_get::<String, _>("unrealized_pnl_sol")
                    .ok()
                    .as_deref(),
            ),
            unrealized_pnl_percent: opt_text_to_dec(
                row.try_get::<String, _>("unrealized_pnl_percent")
                    .ok()
                    .as_deref(),
            ),
            state: row.try_get("state").unwrap_or_default(),
            exit_price: opt_text_to_dec(row.try_get::<String, _>("exit_price").ok().as_deref()),
            exit_tx_signature: row.try_get("exit_tx_signature").ok(),
            realized_pnl_sol: opt_text_to_dec(
                row.try_get::<String, _>("realized_pnl_sol").ok().as_deref(),
            ),
            realized_pnl_usd: opt_text_to_dec(
                row.try_get::<String, _>("realized_pnl_usd").ok().as_deref(),
            ),
            entry_sol_price_usd: opt_text_to_dec(
                row.try_get::<String, _>("entry_sol_price_usd")
                    .ok()
                    .as_deref(),
            ),
            opened_at: self
                .parse_datetime(row.try_get::<String, _>("opened_at").ok().as_deref())?,
            last_updated: self
                .parse_datetime(row.try_get::<String, _>("last_updated").ok().as_deref())?,
            closed_at: row
                .try_get::<String, _>("closed_at")
                .ok()
                .and_then(|s| self.parse_datetime(Some(&s)).ok()),
            token_amount: opt_text_to_dec(row.try_get::<String, _>("token_amount").ok().as_deref()),
        })
    }

    #[tracing::instrument(skip(self, row))]
    fn row_to_wallet(&self, row: sqlx::sqlite::SqliteRow) -> AppResult<Wallet> {
        Ok(Wallet {
            id: row.try_get("id").unwrap_or(0),
            address: row.try_get("address").unwrap_or_default(),
            status: row.try_get("status").unwrap_or_default(),
            wqs_score: row.try_get("wqs_score").ok().and_then(Decimal::from_f64),
            wqs_confidence: row
                .try_get("wqs_confidence")
                .ok()
                .and_then(Decimal::from_f64),
            roi_7d: opt_text_to_dec(row.try_get::<String, _>("roi_7d").ok().as_deref()),
            roi_30d: opt_text_to_dec(row.try_get::<String, _>("roi_30d").ok().as_deref()),
            trade_count_30d: row.try_get("trade_count_30d").ok(),
            win_rate: row.try_get("win_rate").ok().and_then(Decimal::from_f64),
            max_drawdown_30d: opt_text_to_dec(
                row.try_get::<String, _>("max_drawdown_30d").ok().as_deref(),
            ),
            avg_trade_size_sol: opt_text_to_dec(
                row.try_get::<String, _>("avg_trade_size_sol")
                    .ok()
                    .as_deref(),
            ),
            avg_win_sol: opt_text_to_dec(row.try_get::<String, _>("avg_win_sol").ok().as_deref()),
            avg_loss_sol: opt_text_to_dec(row.try_get::<String, _>("avg_loss_sol").ok().as_deref()),
            profit_factor: opt_text_to_dec(
                row.try_get::<String, _>("profit_factor").ok().as_deref(),
            ),
            realized_pnl_30d_sol: opt_text_to_dec(
                row.try_get::<String, _>("realized_pnl_30d_sol")
                    .ok()
                    .as_deref(),
            ),
            last_trade_at: row
                .try_get::<String, _>("last_trade_at")
                .ok()
                .and_then(|s| self.parse_datetime(Some(&s)).ok()),
            promoted_at: row
                .try_get::<String, _>("promoted_at")
                .ok()
                .and_then(|s| self.parse_datetime(Some(&s)).ok()),
            ttl_expires_at: row
                .try_get::<String, _>("ttl_expires_at")
                .ok()
                .and_then(|s| self.parse_datetime(Some(&s)).ok()),
            notes: row.try_get("notes").ok(),
            archetype: row.try_get("archetype").ok(),
            avg_entry_delay_seconds: row
                .try_get("avg_entry_delay_seconds")
                .ok()
                .and_then(Decimal::from_f64),
            created_at: self
                .parse_datetime(row.try_get::<String, _>("created_at").ok().as_deref())?,
            updated_at: self
                .parse_datetime(row.try_get::<String, _>("updated_at").ok().as_deref())?,
        })
    }

    fn parse_datetime(&self, s: Option<&str>) -> AppResult<chrono::DateTime<chrono::Utc>> {
        match s {
            Some(ts) => {
                // Try RFC3339 first (e.g. "2024-01-15T10:30:00Z")
                if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
                    return Ok(dt.with_timezone(&chrono::Utc));
                }
                // Try SQLite format with sub-seconds (e.g. "2024-01-15 10:30:00.123")
                if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S%.3f") {
                    return Ok(dt.and_utc());
                }
                // Try SQLite format without sub-seconds (e.g. "2024-01-15 10:30:00")
                if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S") {
                    return Ok(dt.and_utc());
                }
                Err(AppError::Internal(format!("Invalid datetime: {}", ts)))
            }
            None => Ok(chrono::Utc::now()),
        }
    }
}

// ========================================================================
// HELPER STRUCT FOR TradeDetail (TEXT-based row mapping)
// ========================================================================

/// Intermediate row type for reading TradeDetail from SQLite TEXT columns
struct TradeDetailSqliteRow {
    id: i64,
    trade_uuid: String,
    wallet_address: String,
    token_address: String,
    token_symbol: Option<String>,
    strategy: String,
    side: String,
    amount_sol: String,
    price_at_signal: Option<String>,
    tx_signature: Option<String>,
    status: String,
    retry_count: i32,
    error_message: Option<String>,
    pnl_sol: Option<String>,
    pnl_usd: Option<String>,
    jito_tip_sol: Option<String>,
    dex_fee_sol: Option<String>,
    slippage_cost_sol: Option<String>,
    total_cost_sol: Option<String>,
    net_pnl_sol: Option<String>,
    created_at: String,
    updated_at: String,
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for TradeDetailSqliteRow {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(TradeDetailSqliteRow {
            id: row.try_get("id")?,
            trade_uuid: row.try_get("trade_uuid")?,
            wallet_address: row.try_get("wallet_address")?,
            token_address: row.try_get("token_address")?,
            token_symbol: row.try_get("token_symbol")?,
            strategy: row.try_get("strategy")?,
            side: row.try_get("side")?,
            amount_sol: row.try_get::<String, _>("amount_sol").unwrap_or_default(),
            price_at_signal: row.try_get("price_at_signal")?,
            tx_signature: row.try_get("tx_signature")?,
            status: row.try_get("status")?,
            retry_count: row.try_get("retry_count")?,
            error_message: row.try_get("error_message")?,
            pnl_sol: row.try_get("pnl_sol")?,
            pnl_usd: row.try_get("pnl_usd")?,
            jito_tip_sol: row.try_get("jito_tip_sol")?,
            dex_fee_sol: row.try_get("dex_fee_sol")?,
            slippage_cost_sol: row.try_get("slippage_cost_sol")?,
            total_cost_sol: row.try_get("total_cost_sol")?,
            net_pnl_sol: row.try_get("net_pnl_sol")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

impl TradeDetailSqliteRow {
    fn into_trade_detail(self) -> TradeDetail {
        TradeDetail {
            id: self.id,
            trade_uuid: self.trade_uuid,
            wallet_address: self.wallet_address,
            token_address: self.token_address,
            token_symbol: self.token_symbol,
            strategy: self.strategy,
            side: self.side,
            amount_sol: text_to_dec(&self.amount_sol),
            price_at_signal: self.price_at_signal.as_deref().map(text_to_dec),
            tx_signature: self.tx_signature,
            status: self.status,
            retry_count: self.retry_count,
            error_message: self.error_message,
            pnl_sol: self.pnl_sol.as_deref().map(text_to_dec),
            pnl_usd: self.pnl_usd.as_deref().map(text_to_dec),
            jito_tip_sol: self.jito_tip_sol.as_deref().map(text_to_dec),
            dex_fee_sol: self.dex_fee_sol.as_deref().map(text_to_dec),
            slippage_cost_sol: self.slippage_cost_sol.as_deref().map(text_to_dec),
            total_cost_sol: self.total_cost_sol.as_deref().map(text_to_dec),
            net_pnl_sol: self.net_pnl_sol.as_deref().map(text_to_dec),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

// ========================================================================
// HELPER STRUCT FOR WebhookAuditLog (TEXT-based row mapping)
// ========================================================================

/// Intermediate row type for reading WebhookAuditLog from SQLite TEXT columns
struct WebhookAuditLogSqliteRow {
    id: i64,
    wallet_address: String,
    action: String,
    status: String,
    webhook_id: Option<String>,
    details: Option<String>,
    error_message: Option<String>,
    duration_ms: Option<i32>,
    created_at: String,
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for WebhookAuditLogSqliteRow {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(WebhookAuditLogSqliteRow {
            id: row.try_get("id")?,
            wallet_address: row.try_get("wallet_address")?,
            action: row.try_get("action")?,
            status: row.try_get("status")?,
            webhook_id: row.try_get("webhook_id")?,
            details: row.try_get("details")?,
            error_message: row.try_get("error_message")?,
            duration_ms: row.try_get("duration_ms")?,
            created_at: row.try_get("created_at")?,
        })
    }
}

impl WebhookAuditLogSqliteRow {
    fn into_audit_log(self) -> WebhookAuditLog {
        WebhookAuditLog {
            id: self.id,
            wallet_address: self.wallet_address,
            action: self.action,
            status: self.status,
            webhook_id: self.webhook_id,
            details: self.details,
            error_message: self.error_message,
            duration_ms: self.duration_ms,
            created_at: self.created_at,
        }
    }
}

// ========================================================================
// HELPER FUNCTIONS
// ========================================================================

fn normalize_discrepancy_type(discrepancy: &str) -> String {
    match discrepancy {
        "NONE" => "none".to_string(),
        "MISSING_TX" => "missing_position".to_string(),
        "AMOUNT_MISMATCH" => "pnl_mismatch".to_string(),
        "STATE_MISMATCH" => "state_mismatch".to_string(),
        "COST_MISMATCH" => "cost_mismatch".to_string(),
        _ => discrepancy.to_lowercase(),
    }
}

fn infer_severity(discrepancy: &str) -> String {
    match discrepancy {
        "NONE" => "low".to_string(),
        "MISSING_TX" => "critical".to_string(),
        "AMOUNT_MISMATCH" => "high".to_string(),
        "STATE_MISMATCH" => "medium".to_string(),
        "COST_MISMATCH" => "medium".to_string(),
        _ => "low".to_string(),
    }
}

fn datetime_from_timestamp(ts: f64) -> String {
    format!("{}", ts)
}
