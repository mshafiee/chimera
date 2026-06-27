//! PostgreSQL backend implementation for Database trait

use super::types::DatabaseConfig;
use super::types::PostgresPool;
use super::{
    ActivePositionEntry, ActivePositionSummary, CircuitBreakerState, ConfigAuditItem, Database,
    DbPool, DeadLetterItem, ExitTargetData, InsertPosition, InsertTrade, KillSwitchState,
    LatencyBucket, Position, PositionDetail, PositionRecord, ReconciliationRun,
    ReconciliationStats, ReconciliationStatus, RetryableDlqItem, Trade, TradeDetail,
    TradeLatencyStats, TradeStatistics, UpdateDlqItemParams, UpdatePosition, UpdateTradeStatus,
    Wallet, WalletCopyPerformance, WalletDetail, WalletMonitoring, WalletPerformance,
    WebhookAuditLog,
};
use crate::dec_to_text;
use crate::error::{AppError, AppResult};
use rust_decimal::prelude::*;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::Row;
use std::str::FromStr;
use tracing::info;

// Legacy f64 helpers — PostgreSQL still uses REAL columns (not yet migrated to TEXT)
fn f64_to_decimal(val: f64) -> Decimal {
    Decimal::from_f64_retain(val).unwrap_or(Decimal::ZERO)
}
fn decimal_to_f64(val: Decimal) -> f64 {
    val.to_f64().unwrap_or(0.0)
}
fn opt_f64_to_decimal(val: Option<f64>) -> Option<Decimal> {
    val.and_then(Decimal::from_f64_retain)
}

/// PostgreSQL backend implementation
pub struct PostgresBackend {
    pool: PostgresPool,
}

impl PostgresBackend {
    /// Create new PostgreSQL backend
    pub async fn new(config: &DatabaseConfig) -> AppResult<Self> {
        let pool = Self::init_pool(config).await?;
        Ok(Self { pool })
    }

    /// Initialize PostgreSQL connection pool
    async fn init_pool(config: &DatabaseConfig) -> AppResult<PostgresPool> {
        let db_url = config
            .url
            .as_ref()
            .ok_or_else(|| AppError::Internal("PostgreSQL URL not provided".to_string()))?;

        let connect_options = PgConnectOptions::from_str(db_url)
            .map_err(AppError::Database)?
            // Application name for monitoring
            .application_name("chimera-operator");

        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .acquire_timeout(std::time::Duration::from_secs(30))
            .connect_with(connect_options)
            .await?;

        info!(
            "PostgreSQL pool initialized: max {} connections",
            config.max_connections
        );

        Ok(pool)
    }

    /// Get reference to the pool
    pub fn pool(&self) -> &PostgresPool {
        &self.pool
    }
}

#[async_trait::async_trait]
impl Database for PostgresBackend {
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
        // For PostgreSQL, we run the schema directly instead of migrations
        // In production, you'd use sqlx::migrate!() with PostgreSQL migration files
        let schema = std::fs::read_to_string("database/schema_postgres.sql")
            .map_err(|e| AppError::Internal(format!("Failed to read schema: {}", e)))?;

        // Split by semicolon and execute each statement
        // Note: This is a simplified approach - production should use proper migrations
        for statement in schema.split(';') {
            let statement = statement.trim();
            if !statement.is_empty() && !statement.starts_with("--") {
                sqlx::query(statement)
                    .execute(&self.pool)
                    .await
                    .map_err(AppError::Database)?;
            }
        }

        info!("PostgreSQL schema applied successfully");
        Ok(())
    }

    async fn startup_integrity_check(&self) -> AppResult<()> {
        // PostgreSQL doesn't have PRAGMA integrity_check
        // Instead, we verify connectivity
        sqlx::query("SELECT 1")
            .fetch_one(&self.pool)
            .await
            .map_err(AppError::Database)?;

        info!("PostgreSQL connection verified");
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
            tracing::warn!(
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
        let trade_exists: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM trades WHERE trade_uuid = $1")
                .bind(trade_uuid)
                .fetch_one(&self.pool)
                .await?;

        if trade_exists.0 > 0 {
            return Ok(true);
        }

        // Check dead letter queue
        let dlq_exists: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM dead_letter_queue WHERE trade_uuid = $1")
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
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            RETURNING id
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
        .fetch_one(&self.pool)
        .await?;

        Ok(result.try_get("id").unwrap_or(0))
    }

    async fn update_trade_status(&self, update: &UpdateTradeStatus) -> AppResult<()> {
        let result = if let Some(sig) = &update.tx_signature {
            sqlx::query(
                r#"
                UPDATE trades
                SET status = $1, tx_signature = $2, error_message = $3,
                    network_fee_sol = COALESCE($5, network_fee_sol)
                WHERE trade_uuid = $4
                "#,
            )
            .bind(&update.status)
            .bind(sig)
            .bind(&update.error_message)
            .bind(&update.trade_uuid)
            .bind(update.network_fee_sol.map(|v| dec_to_text(&v)))
            .execute(&self.pool)
            .await?
        } else {
            sqlx::query(
                r#"
                UPDATE trades
                SET status = $1, error_message = COALESCE($2, error_message),
                    network_fee_sol = COALESCE($4, network_fee_sol)
                WHERE trade_uuid = $3
                "#,
            )
            .bind(&update.status)
            .bind(&update.error_message)
            .bind(&update.trade_uuid)
            .bind(update.network_fee_sol.map(|v| dec_to_text(&v)))
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
            WHERE trade_uuid = $1
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
            LIMIT $1
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
            WHERE status = $1
            ORDER BY created_at DESC
            LIMIT $2
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
        sqlx::query(
            r#"
            UPDATE trades
            SET tx_signature = $1, jito_tip_sol = $2, dex_fee_sol = $3,
                slippage_cost_sol = $4, total_cost_sol = jito_tip_sol + dex_fee_sol + slippage_cost_sol
            WHERE trade_uuid = $5
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
            SET pnl_sol = $1, pnl_usd = $2, net_pnl_sol = pnl_sol - total_cost_sol
            WHERE trade_uuid = $3
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
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            RETURNING id
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
        .fetch_one(&self.pool)
        .await?;

        Ok(result.try_get("id").unwrap_or(0))
    }

    async fn update_position(&self, update: &UpdatePosition) -> AppResult<()> {
        let mut set_clauses: Vec<String> = Vec::new();
        let mut f64_binds: Vec<f64> = Vec::new();
        let mut str_binds: Vec<String> = Vec::new();
        let mut param_idx = 1;

        if let Some(price) = update.current_price {
            set_clauses.push(format!("current_price = ${}", param_idx));
            f64_binds.push(decimal_to_f64(price));
            param_idx += 1;
        }
        if let Some(pnl) = update.unrealized_pnl_sol {
            set_clauses.push(format!("unrealized_pnl_sol = ${}", param_idx));
            f64_binds.push(decimal_to_f64(pnl));
            param_idx += 1;
        }
        if let Some(pnl_pct) = update.unrealized_pnl_percent {
            set_clauses.push(format!("unrealized_pnl_percent = ${}", param_idx));
            f64_binds.push(decimal_to_f64(pnl_pct));
            param_idx += 1;
        }
        if let Some(state) = &update.state {
            set_clauses.push(format!("state = ${}", param_idx));
            str_binds.push(state.clone());
            param_idx += 1;
        }
        if let Some(exit_price) = update.exit_price {
            set_clauses.push(format!("exit_price = ${}", param_idx));
            f64_binds.push(decimal_to_f64(exit_price));
            param_idx += 1;
        }
        if let Some(exit_sig) = &update.exit_tx_signature {
            set_clauses.push(format!("exit_tx_signature = ${}", param_idx));
            str_binds.push(exit_sig.clone());
            param_idx += 1;
        }
        if let Some(pnl) = update.realized_pnl_sol {
            set_clauses.push(format!("realized_pnl_sol = ${}", param_idx));
            f64_binds.push(decimal_to_f64(pnl));
            param_idx += 1;
        }
        if let Some(pnl_usd) = update.realized_pnl_usd {
            set_clauses.push(format!("realized_pnl_usd = ${}", param_idx));
            f64_binds.push(decimal_to_f64(pnl_usd));
            param_idx += 1;
        }

        if set_clauses.is_empty() {
            return Ok(()); // Nothing to update
        }

        let sql = format!(
            "UPDATE positions SET {} WHERE trade_uuid = ${}",
            set_clauses.join(", "),
            param_idx
        );

        let mut query = sqlx::query(&sql);
        for bind in f64_binds {
            query = query.bind(bind);
        }
        for bind in str_binds {
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
            WHERE trade_uuid = $1
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
            SET state = 'CLOSED', exit_price = $1, exit_tx_signature = $2,
                realized_pnl_sol = $3, realized_pnl_usd = $4, closed_at = CURRENT_TIMESTAMP
            WHERE trade_uuid = $5
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
            WHERE address = $1
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
        sqlx::query("UPDATE wallets SET status = $1 WHERE address = $2")
            .bind(status)
            .bind(address)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn merge_roster(&self, _roster_db_path: &str) -> AppResult<u32> {
        // PostgreSQL roster merge: not yet implemented.
        // Use the SQLite backend for roster operations.
        // TODO: implement via pg_bulkload or COPY
        Err(AppError::Internal(
            "Roster merge not yet supported on PostgreSQL backend".to_string(),
        ))
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
            WHERE status = $1
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
                .try_get::<chrono::DateTime<chrono::Utc>, _>("updated_at")
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_else(|_| chrono::Utc::now().to_rfc3339()),
        })
    }

    async fn update_circuit_breaker_state(
        &self,
        state: &str,
        tripped_at: Option<&str>,
        trip_reason: Option<&str>,
    ) -> AppResult<()> {
        let mut sql = "UPDATE circuit_breaker_state SET state = $1, updated_at = CURRENT_TIMESTAMP"
            .to_string();
        let mut param_idx = 2;

        if let Some(_t) = tripped_at {
            sql.push_str(&format!(", tripped_at = ${}", param_idx));
            param_idx += 1;
        }
        if let Some(_r) = trip_reason {
            sql.push_str(&format!(", trip_reason = ${}", param_idx));
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
                .try_get::<chrono::DateTime<chrono::Utc>, _>("changed_at")
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_else(|_| chrono::Utc::now().to_rfc3339()),
            changed_by: row.try_get("changed_by").unwrap_or("SYSTEM".to_string()),
            reason: row.try_get("reason").ok(),
        })
    }

    async fn set_kill_switch_state(&self, state: &str, reason: Option<&str>) -> AppResult<()> {
        sqlx::query(
            r#"
            INSERT INTO kill_switch_state (id, state, changed_at, changed_by, reason)
            VALUES (1, $1, CURRENT_TIMESTAMP, 'SYSTEM', $2)
            ON CONFLICT (id) DO UPDATE SET
                state = EXCLUDED.state,
                changed_at = EXCLUDED.changed_at,
                changed_by = EXCLUDED.changed_by,
                reason = EXCLUDED.reason
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
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id
            "#,
        )
        .bind(trade_uuid)
        .bind(payload)
        .bind(reason)
        .bind(error_details)
        .bind(source_ip)
        .fetch_one(&self.pool)
        .await?;

        Ok(result.try_get("id").unwrap_or(0))
    }

    async fn get_admin_wallet_role(&self, wallet_address: &str) -> AppResult<Option<String>> {
        let role: Option<String> =
            sqlx::query_scalar("SELECT role FROM admin_wallets WHERE wallet_address = $1")
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
                SUM(CASE WHEN status = 'CLOSED' THEN 1 ELSE 0 END)::bigint as successful_trades,
                SUM(CASE WHEN status = 'FAILED' OR status = 'DEAD_LETTER' THEN 1 ELSE 0 END)::bigint as failed_trades,
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
            total_pnl_sol: f64_to_decimal(row.try_get("total_pnl_sol").unwrap_or(0.0)),
            total_volume_sol: f64_to_decimal(row.try_get("total_volume_sol").unwrap_or(0.0)),
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
            LIMIT $1 OFFSET $2
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
            WHERE wallet_address = $1
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
                copy_pnl_7d: f64_to_decimal(r.try_get("copy_pnl_7d").unwrap_or(0.0)),
                copy_pnl_30d: f64_to_decimal(r.try_get("copy_pnl_30d").unwrap_or(0.0)),
                signal_success_rate: f64_to_decimal(
                    r.try_get("signal_success_rate").unwrap_or(0.0),
                ),
                total_trades: r.try_get("total_trades").unwrap_or(0),
                winning_trades: r.try_get("winning_trades").unwrap_or(0),
            })),
            None => Ok(None),
        }
    }

    // ========================================================================
    // NEW METHODS — STUBS (PostgreSQL migration pending)
    // ========================================================================

    async fn insert_jito_tip(
        &self,
        _tip: &Decimal,
        _bsig: Option<&str>,
        _strat: Option<&str>,
        _success: bool,
    ) -> AppResult<i64> {
        Err(AppError::Internal(
            "insert_jito_tip not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_recent_jito_tips(&self, _limit: i32) -> AppResult<Vec<Decimal>> {
        Err(AppError::Internal(
            "get_recent_jito_tips not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_jito_tip_count(&self) -> AppResult<u32> {
        Err(AppError::Internal(
            "get_jito_tip_count not implemented for PostgreSQL".into(),
        ))
    }
    async fn prune_old_jito_tips(&self) -> AppResult<u64> {
        Err(AppError::Internal(
            "prune_old_jito_tips not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_pnl_window(&self, _from: &str, _to: Option<&str>) -> AppResult<Decimal> {
        Err(AppError::Internal(
            "get_pnl_window not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_pnl_24h(&self) -> AppResult<Decimal> {
        Err(AppError::Internal(
            "get_pnl_24h not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_pnl_7d(&self) -> AppResult<Decimal> {
        Err(AppError::Internal(
            "get_pnl_7d not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_pnl_30d(&self) -> AppResult<Decimal> {
        Err(AppError::Internal(
            "get_pnl_30d not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_strategy_performance(
        &self,
        _strat: &str,
        _days: i32,
    ) -> AppResult<(f64, Decimal, u32)> {
        Err(AppError::Internal(
            "get_strategy_performance not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_consecutive_losses(&self) -> AppResult<u32> {
        Err(AppError::Internal(
            "get_consecutive_losses not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_max_drawdown_percent(&self, _cap: Decimal) -> AppResult<Decimal> {
        Err(AppError::Internal(
            "get_max_drawdown_percent not implemented for PostgreSQL".into(),
        ))
    }
    #[allow(clippy::too_many_arguments)]
    async fn activate_trade_and_open_position(
        &self,
        _uuid: &str,
        _wallet: &str,
        _token: &str,
        _sym: Option<&str>,
        _strat: &str,
        _amt: Decimal,
        _price: Decimal,
        _sig: &str,
        _heat: Option<Decimal>,
        _sol_price: Option<Decimal>,
    ) -> AppResult<()> {
        Err(AppError::Internal(
            "activate_trade_and_open_position not implemented for PostgreSQL".into(),
        ))
    }
    #[allow(clippy::too_many_arguments)]
    async fn atomic_portfolio_heat_check_and_open_position(
        &self,
        _uuid: &str,
        _wallet: &str,
        _token: &str,
        _sym: Option<&str>,
        _strat: &str,
        _amt: Decimal,
        _price: Decimal,
        _sig: &str,
        _heat: Option<Decimal>,
        _sol_price: Option<Decimal>,
    ) -> AppResult<()> {
        Err(AppError::Internal(
            "atomic_portfolio_heat_check_and_open_position not implemented for PostgreSQL".into(),
        ))
    }
    #[allow(clippy::too_many_arguments)]
    async fn close_position_full(
        &self,
        _uuid: &str,
        _wallet: &str,
        _token: &str,
        _price: Decimal,
        _sig: &str,
        _sol_price: Option<Decimal>,
        _frac: Decimal,
        _confirmed: bool,
    ) -> AppResult<()> {
        Err(AppError::Internal(
            "close_position_full not implemented for PostgreSQL".into(),
        ))
    }
    async fn update_position_token_amount(&self, uuid: &str, token_amount: u64) -> AppResult<()> {
        sqlx::query("UPDATE positions SET token_amount = $1 WHERE trade_uuid = $2")
            .bind(token_amount.to_string())
            .bind(uuid)
            .execute(&self.pool)
            .await
            .map_err(AppError::Database)?;
        Ok(())
    }
    async fn revert_position_exit(&self, _uuid: &str) -> AppResult<()> {
        Err(AppError::Internal(
            "revert_position_exit not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_stuck_positions(&self, _secs: i64) -> AppResult<Vec<PositionRecord>> {
        Err(AppError::Internal(
            "get_stuck_positions not implemented for PostgreSQL".into(),
        ))
    }
    async fn update_position_state(&self, _uuid: &str, _state: &str) -> AppResult<()> {
        Err(AppError::Internal(
            "update_position_state not implemented for PostgreSQL".into(),
        ))
    }
    async fn update_position_unrealized_pnl(
        &self,
        _uuid: &str,
        _price: Decimal,
        _pnl: Decimal,
        _pct: Decimal,
    ) -> AppResult<()> {
        Err(AppError::Internal(
            "update_position_unrealized_pnl not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_active_positions_with_entry(&self) -> AppResult<Vec<ActivePositionEntry>> {
        Err(AppError::Internal(
            "get_active_positions_with_entry not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_active_position_tokens(&self) -> AppResult<Vec<ActivePositionSummary>> {
        Err(AppError::Internal(
            "get_active_position_tokens not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_position_peak_price(&self, _uuid: &str) -> AppResult<Option<String>> {
        Err(AppError::Internal(
            "get_position_peak_price not implemented for PostgreSQL".into(),
        ))
    }
    async fn upsert_wallet(
        &self,
        _addr: &str,
        _wqs: Option<Decimal>,
        _r7: Option<Decimal>,
        _r30: Option<Decimal>,
        _tc: Option<i32>,
        _wr: Option<Decimal>,
        _mdd: Option<Decimal>,
        _ats: Option<Decimal>,
        _notes: Option<&str>,
    ) -> AppResult<bool> {
        Err(AppError::Internal(
            "upsert_wallet not implemented for PostgreSQL".into(),
        ))
    }
    async fn update_wallet_status_ext(
        &self,
        _addr: &str,
        _status: &str,
        _ttl: Option<i32>,
        _reason: Option<&str>,
    ) -> AppResult<bool> {
        Err(AppError::Internal(
            "update_wallet_status_ext not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_expired_ttl_wallets(&self) -> AppResult<Vec<String>> {
        Err(AppError::Internal(
            "get_expired_ttl_wallets not implemented for PostgreSQL".into(),
        ))
    }
    async fn demote_wallet(&self, _addr: &str, _reason: &str) -> AppResult<()> {
        Err(AppError::Internal(
            "demote_wallet not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_wallet_monitoring(&self, _addr: &str) -> AppResult<Option<WalletMonitoring>> {
        Err(AppError::Internal(
            "get_wallet_monitoring not implemented for PostgreSQL".into(),
        ))
    }
    async fn upsert_wallet_monitoring(
        &self,
        _addr: &str,
        _wid: Option<&str>,
        _enabled: bool,
    ) -> AppResult<()> {
        Err(AppError::Internal(
            "upsert_wallet_monitoring not implemented for PostgreSQL".into(),
        ))
    }
    async fn update_wallet_monitoring_signature(&self, _addr: &str, _sig: &str) -> AppResult<()> {
        Err(AppError::Internal(
            "update_wallet_monitoring_signature not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_wallets_needing_webhook_registration(&self) -> AppResult<Vec<String>> {
        Err(AppError::Internal(
            "get_wallets_needing_webhook_registration not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_stale_webhook_wallets(&self, _days: i32) -> AppResult<Vec<String>> {
        Err(AppError::Internal(
            "get_stale_webhook_wallets not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_all_wallet_monitoring(&self) -> AppResult<Vec<WalletMonitoring>> {
        Err(AppError::Internal(
            "get_all_wallet_monitoring not implemented for PostgreSQL".into(),
        ))
    }
    async fn update_webhook_health_status(
        &self,
        _addr: &str,
        _status: &str,
        _wid: Option<&str>,
    ) -> AppResult<()> {
        Err(AppError::Internal(
            "update_webhook_health_status not implemented for PostgreSQL".into(),
        ))
    }
    async fn update_webhook_status(&self, _addr: &str, _status: &str) -> AppResult<()> {
        Err(AppError::Internal(
            "update_webhook_status not implemented for PostgreSQL".into(),
        ))
    }
    #[allow(clippy::too_many_arguments)]
    async fn log_webhook_lifecycle_event(
        &self,
        _addr: &str,
        _action: &str,
        _status: &str,
        _wid: Option<&str>,
        _details: Option<&str>,
        _err: Option<&str>,
        _dur: Option<i32>,
    ) -> AppResult<()> {
        Err(AppError::Internal(
            "log_webhook_lifecycle_event not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_webhook_audit_log(
        &self,
        _wallet_address: Option<&str>,
        _action: Option<&str>,
        _status: Option<&str>,
        _limit: Option<i64>,
    ) -> AppResult<Vec<WebhookAuditLog>> {
        Err(AppError::Internal(
            "get_webhook_audit_log not implemented for PostgreSQL".into(),
        ))
    }
    async fn increment_webhook_registration_attempts(
        &self,
        _addr: &str,
        _err: Option<&str>,
    ) -> AppResult<()> {
        Err(AppError::Internal(
            "increment_webhook_registration_attempts not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_webhook_configuration(&self, _key: &str) -> AppResult<Option<String>> {
        Err(AppError::Internal(
            "get_webhook_configuration not implemented for PostgreSQL".into(),
        ))
    }
    async fn update_webhook_configuration(
        &self,
        _key: &str,
        _val: &str,
        _by: &str,
    ) -> AppResult<()> {
        Err(AppError::Internal(
            "update_webhook_configuration not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_orphaned_webhooks(&self, _ids: &[String]) -> AppResult<Vec<String>> {
        Err(AppError::Internal(
            "get_orphaned_webhooks not implemented for PostgreSQL".into(),
        ))
    }
    #[allow(clippy::too_many_arguments)]
    async fn upsert_exit_target(
        &self,
        _uuid: &str,
        _ep: Decimal,
        _eas: Decimal,
        _pp: Decimal,
        _ppp: Decimal,
        _th: &str,
        _tsa: bool,
        _tsp: Decimal,
        _rf: Decimal,
    ) -> AppResult<()> {
        Err(AppError::Internal(
            "upsert_exit_target not implemented for PostgreSQL".into(),
        ))
    }
    async fn load_exit_target(&self, _uuid: &str) -> AppResult<Option<ExitTargetData>> {
        Err(AppError::Internal(
            "load_exit_target not implemented for PostgreSQL".into(),
        ))
    }
    async fn delete_exit_target(&self, _uuid: &str) -> AppResult<()> {
        Err(AppError::Internal(
            "delete_exit_target not implemented for PostgreSQL".into(),
        ))
    }
    async fn insert_reconciliation_log(
        &self,
        _uuid: &str,
        _exp: &str,
        _actual: Option<&str>,
        _disc: &str,
        _tx: Option<&str>,
        _notes: Option<&str>,
    ) -> AppResult<i64> {
        Err(AppError::Internal(
            "insert_reconciliation_log not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_reconciliation_status(&self, _limit: i32) -> AppResult<ReconciliationStatus> {
        Err(AppError::Internal(
            "get_reconciliation_status not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_reconciliation_history(&self, _limit: i32) -> AppResult<Vec<ReconciliationRun>> {
        Err(AppError::Internal(
            "get_reconciliation_history not implemented for PostgreSQL".into(),
        ))
    }
    async fn count_reconciliation_runs(&self) -> AppResult<i64> {
        Err(AppError::Internal(
            "count_reconciliation_runs not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_reconciliation_stats(&self, _range: &str) -> AppResult<ReconciliationStats> {
        Err(AppError::Internal(
            "get_reconciliation_stats not implemented for PostgreSQL".into(),
        ))
    }
    async fn resolve_discrepancy(&self, _id: i64, _by: &str, _res: &str) -> AppResult<()> {
        Err(AppError::Internal(
            "resolve_discrepancy not implemented for PostgreSQL".into(),
        ))
    }
    #[allow(clippy::too_many_arguments)]
    async fn get_trades_filtered(
        &self,
        _from: Option<&str>,
        _to: Option<&str>,
        _status: Option<&str>,
        _strat: Option<&str>,
        _wallet: Option<&str>,
        _limit: i64,
        _offset: i64,
    ) -> AppResult<Vec<TradeDetail>> {
        Err(AppError::Internal(
            "get_trades_filtered not implemented for PostgreSQL".into(),
        ))
    }
    async fn count_trades_filtered(
        &self,
        _from: Option<&str>,
        _to: Option<&str>,
        _status: Option<&str>,
        _strat: Option<&str>,
        _wallet: Option<&str>,
    ) -> AppResult<i64> {
        Err(AppError::Internal(
            "count_trades_filtered not implemented for PostgreSQL".into(),
        ))
    }
    async fn update_trade_costs(
        &self,
        _uuid: &str,
        _jito: Decimal,
        _dex: Decimal,
        _slip: Decimal,
    ) -> AppResult<()> {
        Err(AppError::Internal(
            "update_trade_costs not implemented for PostgreSQL".into(),
        ))
    }
    async fn update_trade_net_pnl(&self, _uuid: &str, _pnl: Decimal) -> AppResult<()> {
        Err(AppError::Internal(
            "update_trade_net_pnl not implemented for PostgreSQL".into(),
        ))
    }
    async fn mark_trade_dead_letter(
        &self,
        _uuid: &str,
        _payload: &str,
        _err: &str,
    ) -> AppResult<()> {
        Err(AppError::Internal(
            "mark_trade_dead_letter not implemented for PostgreSQL".into(),
        ))
    }
    async fn log_config_change(
        &self,
        _key: &str,
        _old: Option<&str>,
        _new: &str,
        _by: &str,
        _reason: Option<&str>,
    ) -> AppResult<()> {
        Err(AppError::Internal(
            "log_config_change not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_dead_letter_entries(
        &self,
        _limit: i32,
        _offset: i32,
    ) -> AppResult<Vec<DeadLetterItem>> {
        Err(AppError::Internal(
            "get_dead_letter_entries not implemented for PostgreSQL".into(),
        ))
    }
    async fn count_dead_letter_entries(&self) -> AppResult<i64> {
        Err(AppError::Internal(
            "count_dead_letter_entries not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_retryable_dlq_items(&self, _limit: i64) -> AppResult<Vec<RetryableDlqItem>> {
        Err(AppError::Internal(
            "get_retryable_dlq_items not implemented for PostgreSQL".into(),
        ))
    }
    async fn update_dlq_item(
        &self,
        _trade_uuid: &str,
        _retry_count: i64,
        _can_retry: bool,
        _mark_processed: bool,
    ) -> AppResult<()> {
        Err(AppError::Internal(
            "update_dlq_item not implemented for PostgreSQL".into(),
        ))
    }
    async fn update_dlq_items_batch(&self, _items: Vec<UpdateDlqItemParams>) -> AppResult<usize> {
        Err(AppError::Internal(
            "update_dlq_items_batch not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_config_audit_entries(
        &self,
        _limit: i32,
        _offset: i32,
    ) -> AppResult<Vec<ConfigAuditItem>> {
        Err(AppError::Internal(
            "get_config_audit_entries not implemented for PostgreSQL".into(),
        ))
    }
    async fn count_config_audit_entries(&self) -> AppResult<i64> {
        Err(AppError::Internal(
            "count_config_audit_entries not implemented for PostgreSQL".into(),
        ))
    }
    async fn count_trades_by_status(&self, _status: &str) -> AppResult<i64> {
        Err(AppError::Internal(
            "count_trades_by_status not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_closed_trade_count_for_wallet(&self, _addr: &str) -> AppResult<i64> {
        Err(AppError::Internal(
            "get_closed_trade_count_for_wallet not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_wallet_copy_performance(
        &self,
        _addr: &str,
    ) -> AppResult<Option<WalletCopyPerformance>> {
        Err(AppError::Internal(
            "get_wallet_copy_performance not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_trade_latency_stats(&self, _hours: i32) -> AppResult<TradeLatencyStats> {
        Err(AppError::Internal(
            "get_trade_latency_stats not implemented for PostgreSQL".into(),
        ))
    }
    async fn get_trade_latency_histogram(
        &self,
        _hours: i32,
        _buckets: &[f64],
    ) -> AppResult<Vec<LatencyBucket>> {
        Err(AppError::Internal(
            "get_trade_latency_histogram not implemented for PostgreSQL".into(),
        ))
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
                    FROM positions WHERE state = $1 ORDER BY last_updated DESC"#,
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
            .map(|row| {
                use sqlx::Row;
                PositionDetail {
                    id: row.try_get("id").unwrap_or(0),
                    trade_uuid: row.try_get("trade_uuid").unwrap_or_default(),
                    wallet_address: row.try_get("wallet_address").unwrap_or_default(),
                    token_address: row.try_get("token_address").unwrap_or_default(),
                    token_symbol: row.try_get("token_symbol").ok(),
                    strategy: row.try_get("strategy").unwrap_or_default(),
                    entry_amount_sol: f64_to_decimal(
                        row.try_get::<f64, _>("entry_amount_sol").unwrap_or(0.0),
                    ),
                    entry_price: f64_to_decimal(
                        row.try_get::<f64, _>("entry_price").unwrap_or(0.0),
                    ),
                    entry_tx_signature: row.try_get("entry_tx_signature").unwrap_or_default(),
                    current_price: opt_f64_to_decimal(row.try_get::<f64, _>("current_price").ok()),
                    unrealized_pnl_sol: opt_f64_to_decimal(
                        row.try_get::<f64, _>("unrealized_pnl_sol").ok(),
                    ),
                    unrealized_pnl_percent: opt_f64_to_decimal(
                        row.try_get::<f64, _>("unrealized_pnl_percent").ok(),
                    ),
                    state: row.try_get("state").unwrap_or_default(),
                    exit_price: opt_f64_to_decimal(row.try_get::<f64, _>("exit_price").ok()),
                    exit_tx_signature: row.try_get("exit_tx_signature").ok(),
                    realized_pnl_sol: opt_f64_to_decimal(
                        row.try_get::<f64, _>("realized_pnl_sol").ok(),
                    ),
                    realized_pnl_usd: opt_f64_to_decimal(
                        row.try_get::<f64, _>("realized_pnl_usd").ok(),
                    ),
                    opened_at: row
                        .try_get("opened_at")
                        .map(|dt: chrono::DateTime<chrono::Utc>| dt.to_rfc3339())
                        .unwrap_or_default(),
                    last_updated: row
                        .try_get("last_updated")
                        .map(|dt: chrono::DateTime<chrono::Utc>| dt.to_rfc3339())
                        .unwrap_or_default(),
                    closed_at: row
                        .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("closed_at")
                        .ok()
                        .flatten()
                        .map(|dt| dt.to_rfc3339()),
                }
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
                    WHERE status = $1
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
            .map(|row| {
                use sqlx::Row;
                WalletDetail {
                    id: row.try_get("id").unwrap_or(0),
                    address: row.try_get("address").unwrap_or_default(),
                    status: row.try_get("status").unwrap_or_default(),
                    wqs_score: opt_f64_to_decimal(row.try_get::<f64, _>("wqs_score").ok()),
                    roi_7d: opt_f64_to_decimal(row.try_get::<f64, _>("roi_7d").ok()),
                    roi_30d: opt_f64_to_decimal(row.try_get::<f64, _>("roi_30d").ok()),
                    trade_count_30d: row.try_get("trade_count_30d").ok(),
                    win_rate: opt_f64_to_decimal(row.try_get::<f64, _>("win_rate").ok()),
                    max_drawdown_30d: opt_f64_to_decimal(
                        row.try_get::<f64, _>("max_drawdown_30d").ok(),
                    ),
                    avg_trade_size_sol: opt_f64_to_decimal(
                        row.try_get::<f64, _>("avg_trade_size_sol").ok(),
                    ),
                    avg_win_sol: opt_f64_to_decimal(row.try_get::<f64, _>("avg_win_sol").ok()),
                    avg_loss_sol: opt_f64_to_decimal(row.try_get::<f64, _>("avg_loss_sol").ok()),
                    profit_factor: opt_f64_to_decimal(row.try_get::<f64, _>("profit_factor").ok()),
                    realized_pnl_30d_sol: opt_f64_to_decimal(
                        row.try_get::<f64, _>("realized_pnl_30d_sol").ok(),
                    ),
                    last_trade_at: row.try_get("last_trade_at").ok(),
                    promoted_at: row.try_get("promoted_at").ok(),
                    ttl_expires_at: row.try_get("ttl_expires_at").ok(),
                    notes: row.try_get("notes").ok(),
                    archetype: row.try_get("archetype").ok(),
                    avg_entry_delay_seconds: opt_f64_to_decimal(
                        row.try_get::<f64, _>("avg_entry_delay_seconds").ok(),
                    ),
                    created_at: row
                        .try_get("created_at")
                        .map(|dt: chrono::DateTime<chrono::Utc>| dt.to_rfc3339())
                        .unwrap_or_default(),
                    updated_at: row
                        .try_get("updated_at")
                        .map(|dt: chrono::DateTime<chrono::Utc>| dt.to_rfc3339())
                        .unwrap_or_default(),
                }
            })
            .collect();
        Ok(wallets)
    }

    fn pool(&self) -> DbPool {
        DbPool::PostgreSQL(self.pool.clone())
    }

    async fn get_evaluation_data(&self) -> AppResult<(Decimal, Decimal, Decimal, Decimal)> {
        // PostgreSQL uses NUMERIC columns — query_scalar maps directly to Decimal
        let unrealized_sol: Decimal = sqlx::query_scalar::<_, Decimal>(
            r#"SELECT COALESCE(SUM(unrealized_pnl_sol), 0.0) FROM positions WHERE state IN ('ACTIVE', 'EXITING')"#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::Database)?;

        let realized_pnl_sol: Decimal = sqlx::query_scalar::<_, Decimal>(
            r#"SELECT COALESCE(SUM(realized_pnl_sol), 0.0) FROM positions WHERE state = 'CLOSED' AND closed_at >= NOW() - INTERVAL '24 hours'"#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::Database)?;

        let realized_usd: Decimal = sqlx::query_scalar::<_, Decimal>(
            r#"SELECT COALESCE(SUM(realized_pnl_usd), 0.0) FROM positions WHERE state = 'CLOSED' AND closed_at >= NOW() - INTERVAL '24 hours' AND realized_pnl_usd IS NOT NULL"#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::Database)?;

        let null_price_pnl_sol: Decimal = sqlx::query_scalar::<_, Decimal>(
            r#"SELECT COALESCE(SUM(realized_pnl_sol), 0.0) FROM positions WHERE state = 'CLOSED' AND closed_at >= NOW() - INTERVAL '24 hours' AND realized_pnl_usd IS NULL"#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::Database)?;

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

impl PostgresBackend {
    fn row_to_trade(&self, row: sqlx::postgres::PgRow) -> AppResult<Trade> {
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
            created_at: row
                .try_get("created_at")
                .unwrap_or_else(|_| chrono::Utc::now()),
            updated_at: row
                .try_get("updated_at")
                .unwrap_or_else(|_| chrono::Utc::now()),
        })
    }

    fn row_to_position(&self, row: sqlx::postgres::PgRow) -> AppResult<Position> {
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
            opened_at: row
                .try_get("opened_at")
                .unwrap_or_else(|_| chrono::Utc::now()),
            last_updated: row
                .try_get("last_updated")
                .unwrap_or_else(|_| chrono::Utc::now()),
            closed_at: row.try_get("closed_at").ok(),
            token_amount: opt_f64_to_decimal(row.try_get("token_amount").ok()),
        })
    }

    fn row_to_wallet(&self, row: sqlx::postgres::PgRow) -> AppResult<Wallet> {
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
            last_trade_at: row.try_get("last_trade_at").ok(),
            promoted_at: row.try_get("promoted_at").ok(),
            ttl_expires_at: row.try_get("ttl_expires_at").ok(),
            notes: row.try_get("notes").ok(),
            archetype: row.try_get("archetype").ok(),
            avg_entry_delay_seconds: opt_f64_to_decimal(
                row.try_get("avg_entry_delay_seconds").ok(),
            ),
            created_at: row
                .try_get("created_at")
                .unwrap_or_else(|_| chrono::Utc::now()),
            updated_at: row
                .try_get("updated_at")
                .unwrap_or_else(|_| chrono::Utc::now()),
        })
    }
}
