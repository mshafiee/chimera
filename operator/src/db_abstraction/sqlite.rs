//! SQLite backend implementation for Database trait

use super::types::SqlitePool;
use super::{
    decimal_to_f64, f64_to_decimal, opt_decimal_to_f64, opt_f64_to_decimal,
    CircuitBreakerState, Database, DbPool, InsertPosition, InsertTrade, KillSwitchState,
    Position, Trade, TradeStatistics, UpdatePosition, UpdateTradeStatus, Wallet,
    WalletPerformance,
};
use crate::config::DatabaseConfig;
use crate::error::{AppError, AppResult};
use rust_decimal::prelude::*;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Pool, Row, Sqlite};
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
            .acquire_timeout(std::time::Duration::from_secs(config.acquire_timeout_seconds))
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
        .bind(decimal_to_f64(trade.amount_sol))
        .bind(&trade.status)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    async fn update_trade_status(&self, update: &UpdateTradeStatus) -> AppResult<()> {
        let result = if let Some(sig) = &update.tx_signature {
            sqlx::query(
                r#"
                UPDATE trades
                SET status = ?, tx_signature = ?, error_message = ?
                WHERE trade_uuid = ?
                "#,
            )
            .bind(&update.status)
            .bind(sig)
            .bind(&update.error_message)
            .bind(&update.trade_uuid)
            .execute(&self.pool)
            .await?
        } else {
            sqlx::query(
                r#"
                UPDATE trades
                SET status = ?, error_message = COALESCE(?, error_message)
                WHERE trade_uuid = ?
                "#,
            )
            .bind(&update.status)
            .bind(&update.error_message)
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

        rows.into_iter()
            .map(|r| self.row_to_trade(r))
            .collect()
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

        rows.into_iter()
            .map(|r| self.row_to_trade(r))
            .collect()
    }

    async fn update_trade_execution(
        &self,
        trade_uuid: &str,
        tx_signature: &str,
        jito_tip_sol: Decimal,
        dex_fee_sol: Decimal,
        slippage_cost_sol: Decimal,
    ) -> AppResult<()> {
        sqlx::query(
            r#"
            UPDATE trades
            SET tx_signature = ?, jito_tip_sol = ?, dex_fee_sol = ?,
                slippage_cost_sol = ?, total_cost_sol = jito_tip_sol + dex_fee_sol + slippage_cost_sol
            WHERE trade_uuid = ?
            "#,
        )
        .bind(tx_signature)
        .bind(decimal_to_f64(jito_tip_sol))
        .bind(decimal_to_f64(dex_fee_sol))
        .bind(decimal_to_f64(slippage_cost_sol))
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
        sqlx::query(
            r#"
            UPDATE trades
            SET pnl_sol = ?, pnl_usd = ?, net_pnl_sol = pnl_sol - total_cost_sol
            WHERE trade_uuid = ?
            "#,
        )
        .bind(decimal_to_f64(pnl_sol))
        .bind(decimal_to_f64(pnl_usd))
        .bind(trade_uuid)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    // ========================================================================
    // POSITION OPERATIONS
    // ========================================================================

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
        .bind(decimal_to_f64(position.entry_amount_sol))
        .bind(decimal_to_f64(position.entry_price))
        .bind(&position.entry_tx_signature)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    async fn update_position(&self, update: &UpdatePosition) -> AppResult<()> {
        let mut set_clauses = Vec::new();
        let mut binds = Vec::new();

        if let Some(price) = update.current_price {
            set_clauses.push("current_price = ?");
            binds.push(decimal_to_f64(price));
        }
        if let Some(pnl) = update.unrealized_pnl_sol {
            set_clauses.push("unrealized_pnl_sol = ?");
            binds.push(decimal_to_f64(pnl));
        }
        if let Some(pnl_pct) = update.unrealized_pnl_percent {
            set_clauses.push("unrealized_pnl_percent = ?");
            binds.push(decimal_to_f64(pnl_pct));
        }
        if let Some(state) = &update.state {
            set_clauses.push("state = ?");
            binds.push(state.clone());
        }
        if let Some(exit_price) = update.exit_price {
            set_clauses.push("exit_price = ?");
            binds.push(decimal_to_f64(exit_price));
        }
        if let Some(exit_sig) = &update.exit_tx_signature {
            set_clauses.push("exit_tx_signature = ?");
            binds.push(exit_sig.clone());
        }
        if let Some(pnl) = update.realized_pnl_sol {
            set_clauses.push("realized_pnl_sol = ?");
            binds.push(decimal_to_f64(pnl));
        }
        if let Some(pnl_usd) = update.realized_pnl_usd {
            set_clauses.push("realized_pnl_usd = ?");
            binds.push(decimal_to_f64(pnl_usd));
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
                realized_pnl_usd, entry_sol_price_usd, opened_at, last_updated, closed_at
            FROM positions
            WHERE state = 'ACTIVE'
            ORDER BY opened_at DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::Database)?;

        rows.into_iter()
            .map(|r| self.row_to_position(r))
            .collect()
    }

    async fn get_position_by_trade_uuid(&self, trade_uuid: &str) -> AppResult<Option<Position>> {
        let row = sqlx::query(
            r#"
            SELECT
                id, trade_uuid, wallet_address, token_address, token_symbol,
                strategy, entry_amount_sol, entry_price, entry_tx_signature,
                current_price, unrealized_pnl_sol, unrealized_pnl_percent,
                state, exit_price, exit_tx_signature, realized_pnl_sol,
                realized_pnl_usd, entry_sol_price_usd, opened_at, last_updated, closed_at
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
        .bind(decimal_to_f64(exit_price))
        .bind(exit_tx_signature)
        .bind(decimal_to_f64(realized_pnl_sol))
        .bind(decimal_to_f64(realized_pnl_usd))
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

        rows.into_iter()
            .map(|r| self.row_to_wallet(r))
            .collect()
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
        sqlx::query(&format!("ATTACH DATABASE ? AS roster"))
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

        rows.into_iter()
            .map(|r| self.row_to_wallet(r))
            .collect()
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

        if let Some(t) = tripped_at {
            sql.push_str(&format!(", tripped_at = '{}'", t));
        }
        if let Some(r) = trip_reason {
            sql.push_str(&format!(", trip_reason = '{}' ", r.replace("'", "''")));
        }

        sql.push_str(" WHERE id = 1");

        sqlx::query(&sql).bind(state).execute(&self.pool).await?;

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
            changed_by: row
                .try_get("changed_by")
                .unwrap_or("SYSTEM".to_string()),
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
        let role: Option<String> = sqlx::query_scalar("SELECT role FROM admin_wallets WHERE wallet_address = ?")
            .bind(wallet_address)
            .fetch_optional(&self.pool)
            .await
            .map_err(AppError::Database)?;

        Ok(role)
    }

    // ========================================================================
    // STATISTICS & REPORTING
    // ========================================================================

    async fn get_trade_statistics(&self) -> AppResult<TradeStatistics> {
        let row = sqlx::query(
            r#"
            SELECT
                COUNT(*) as total_trades,
                SUM(CASE WHEN status = 'CLOSED' THEN 1 ELSE 0 END) as successful_trades,
                SUM(CASE WHEN status = 'FAILED' OR status = 'DEAD_LETTER' THEN 1 ELSE 0 END) as failed_trades,
                COALESCE(SUM(net_pnl_sol), 0) as total_pnl_sol,
                COALESCE(SUM(amount_sol), 0) as total_volume_sol
            FROM trades
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::Database)?;

        Ok(TradeStatistics {
            total_trades: row.try_get("total_trades").unwrap_or(0),
            successful_trades: row.try_get("successful_trades").unwrap_or(0),
            failed_trades: row.try_get("failed_trades").unwrap_or(0),
            total_pnl_sol: f64_to_decimal(
                row.try_get("total_pnl_sol").unwrap_or(0.0),
            ),
            total_volume_sol: f64_to_decimal(
                row.try_get("total_volume_sol").unwrap_or(0.0),
            ),
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

        rows.into_iter()
            .map(|r| self.row_to_trade(r))
            .collect()
    }

    async fn get_wallet_performance(&self, wallet_address: &str) -> AppResult<Option<WalletPerformance>> {
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
                wallet_address: r.try_get("wallet_address").unwrap_or(wallet_address.to_string()),
                copy_pnl_7d: f64_to_decimal(r.try_get("copy_pnl_7d").unwrap_or(0.0)),
                copy_pnl_30d: f64_to_decimal(r.try_get("copy_pnl_30d").unwrap_or(0.0)),
                signal_success_rate: f64_to_decimal(r.try_get("signal_success_rate").unwrap_or(0.0)),
                total_trades: r.try_get("total_trades").unwrap_or(0),
                winning_trades: r.try_get("winning_trades").unwrap_or(0),
            })),
            None => Ok(None),
        }
    }
}

// ========================================================================
// ROW CONVERSION HELPERS
// ========================================================================

impl SqliteBackend {
    fn row_to_trade(&self, row: sqlx::sqlite::SqliteRow) -> AppResult<Trade> {
        Ok(Trade {
            id: row.try_get("id").unwrap_or(0),
            trade_uuid: row.try_get("trade_uuid").unwrap_or_default(),
            wallet_address: row.try_get("wallet_address").unwrap_or_default(),
            token_address: row.try_get("token_address").unwrap_or_default(),
            token_symbol: row.try_get("token_symbol").ok(),
            strategy: row.try_get("strategy").unwrap_or_default(),
            side: row.try_get("side").unwrap_or_default(),
            amount_sol: f64_to_decimal(row.try_get("amount_sol").unwrap_or(0.0)),
            price_at_signal: opt_f64_to_decimal(row.try_get("price_at_signal").ok()),
            tx_signature: row.try_get("tx_signature").ok(),
            status: row.try_get("status").unwrap_or_default(),
            retry_count: row.try_get("retry_count").unwrap_or(0),
            error_message: row.try_get("error_message").ok(),
            pnl_sol: opt_f64_to_decimal(row.try_get("pnl_sol").ok()),
            pnl_usd: opt_f64_to_decimal(row.try_get("pnl_usd").ok()),
            jito_tip_sol: f64_to_decimal(row.try_get("jito_tip_sol").unwrap_or(0.0)),
            dex_fee_sol: f64_to_decimal(row.try_get("dex_fee_sol").unwrap_or(0.0)),
            slippage_cost_sol: f64_to_decimal(row.try_get("slippage_cost_sol").unwrap_or(0.0)),
            total_cost_sol: f64_to_decimal(row.try_get("total_cost_sol").unwrap_or(0.0)),
            net_pnl_sol: opt_f64_to_decimal(row.try_get("net_pnl_sol").ok()),
            created_at: self.parse_datetime(row.try_get("created_at").ok().as_deref())?,
            updated_at: self.parse_datetime(row.try_get("updated_at").ok().as_deref())?,
        })
    }

    fn row_to_position(&self, row: sqlx::sqlite::SqliteRow) -> AppResult<Position> {
        Ok(Position {
            id: row.try_get("id").unwrap_or(0),
            trade_uuid: row.try_get("trade_uuid").unwrap_or_default(),
            wallet_address: row.try_get("wallet_address").unwrap_or_default(),
            token_address: row.try_get("token_address").unwrap_or_default(),
            token_symbol: row.try_get("token_symbol").ok(),
            strategy: row.try_get("strategy").unwrap_or_default(),
            entry_amount_sol: f64_to_decimal(row.try_get("entry_amount_sol").unwrap_or(0.0)),
            entry_price: f64_to_decimal(row.try_get("entry_price").unwrap_or(0.0)),
            entry_tx_signature: row.try_get("entry_tx_signature").unwrap_or_default(),
            current_price: opt_f64_to_decimal(row.try_get("current_price").ok()),
            unrealized_pnl_sol: opt_f64_to_decimal(row.try_get("unrealized_pnl_sol").ok()),
            unrealized_pnl_percent: opt_f64_to_decimal(row.try_get("unrealized_pnl_percent").ok()),
            state: row.try_get("state").unwrap_or_default(),
            exit_price: opt_f64_to_decimal(row.try_get("exit_price").ok()),
            exit_tx_signature: row.try_get("exit_tx_signature").ok(),
            realized_pnl_sol: opt_f64_to_decimal(row.try_get("realized_pnl_sol").ok()),
            realized_pnl_usd: opt_f64_to_decimal(row.try_get("realized_pnl_usd").ok()),
            entry_sol_price_usd: opt_f64_to_decimal(row.try_get("entry_sol_price_usd").ok()),
            opened_at: self.parse_datetime(row.try_get("opened_at").ok().as_deref())?,
            last_updated: self.parse_datetime(row.try_get("last_updated").ok().as_deref())?,
            closed_at: row.try_get::<String, _>("closed_at").ok().and_then(|s| self.parse_datetime(Some(&s)).ok()),
        })
    }

    fn row_to_wallet(&self, row: sqlx::sqlite::SqliteRow) -> AppResult<Wallet> {
        Ok(Wallet {
            id: row.try_get("id").unwrap_or(0),
            address: row.try_get("address").unwrap_or_default(),
            status: row.try_get("status").unwrap_or_default(),
            wqs_score: opt_f64_to_decimal(row.try_get("wqs_score").ok()),
            wqs_confidence: opt_f64_to_decimal(row.try_get("wqs_confidence").ok()),
            roi_7d: opt_f64_to_decimal(row.try_get("roi_7d").ok()),
            roi_30d: opt_f64_to_decimal(row.try_get("roi_30d").ok()),
            trade_count_30d: row.try_get("trade_count_30d").ok(),
            win_rate: opt_f64_to_decimal(row.try_get("win_rate").ok()),
            max_drawdown_30d: opt_f64_to_decimal(row.try_get("max_drawdown_30d").ok()),
            avg_trade_size_sol: opt_f64_to_decimal(row.try_get("avg_trade_size_sol").ok()),
            avg_win_sol: opt_f64_to_decimal(row.try_get("avg_win_sol").ok()),
            avg_loss_sol: opt_f64_to_decimal(row.try_get("avg_loss_sol").ok()),
            profit_factor: opt_f64_to_decimal(row.try_get("profit_factor").ok()),
            realized_pnl_30d_sol: opt_f64_to_decimal(row.try_get("realized_pnl_30d_sol").ok()),
            last_trade_at: row.try_get::<String, _>("last_trade_at").ok().and_then(|s| self.parse_datetime(Some(&s)).ok()),
            promoted_at: row.try_get::<String, _>("promoted_at").ok().and_then(|s| self.parse_datetime(Some(&s)).ok()),
            ttl_expires_at: row.try_get::<String, _>("ttl_expires_at").ok().and_then(|s| self.parse_datetime(Some(&s)).ok()),
            notes: row.try_get("notes").ok(),
            archetype: row.try_get("archetype").ok(),
            avg_entry_delay_seconds: opt_f64_to_decimal(row.try_get("avg_entry_delay_seconds").ok()),
            created_at: self.parse_datetime(row.try_get("created_at").ok().as_deref())?,
            updated_at: self.parse_datetime(row.try_get("updated_at").ok().as_deref())?,
        })
    }

    fn parse_datetime(&self, s: Option<&str>) -> AppResult<chrono::DateTime<chrono::Utc>> {
        match s {
            Some(ts) => {
                chrono::DateTime::parse_from_rfc3339(ts)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .map_err(|e| AppError::Internal(format!("Invalid datetime: {}", e)))
            }
            None => Ok(chrono::Utc::now()),
        }
    }
}
