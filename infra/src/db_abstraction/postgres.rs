//! PostgreSQL backend implementation for Database trait

use super::types::DatabaseConfig;
use super::types::PostgresPool;
use super::{
    datetime_to_string, ActivePositionEntry, ActivePositionSummary, CircuitBreakerState,
    ConfigAuditItem, Database, DbPool, DeadLetterItem, DiscrepancyRow, DiscrepancyTypeStats,
    ExitTargetData, InsertPosition, InsertTrade, KillSwitchState, LatencyBucket, Position,
    PositionDetail, PositionRecord, ReconciliationRun, ReconciliationStats, ReconciliationStatus,
    RetryableDlqItem, Trade, TradeDetail, TradeLatencyStats, TradeStatistics, UpdateDlqItemParams,
    UpdatePosition, UpdateTradeStatus, Wallet, WalletCopyPerformance, WalletDetail,
    WalletMonitoring, WalletPerformance, WebhookAuditLog,
};
use chimera_core::error::{AppError, AppResult};
use rust_decimal::prelude::*;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::Row;
use std::str::FromStr;
use tracing::info;

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
            .application_name("chimera-operator")
            // Bounded prepared-statement cache. Historically the cache was
            // disabled (capacity 0) to dodge `cached plan must not change
            // result type` errors when migrations ALTER columns while cached
            // statements are live. But capacity 0 made every query prepare a
            // NEW server-side statement that postgres retains per session,
            // growing each backend ~75MB/min (~2GB within an hour) until the
            // container OOM-killed backends every ~30 min. A bounded cache
            // (1000) fixes the leak; the migration concern is handled by
            // sqlx deallocating evicted statements and migrations running at
            // startup before heavy query load.
            .statement_cache_capacity(1000);

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

/// Append the shared `trades` WHERE-filter fragment (created_at range, status, strategy,
/// wallet_address) to `sql`, numbering bind placeholders `$1..` and returning the next
/// available placeholder index. Used by BOTH `get_trades_filtered` and
/// `count_trades_filtered` so their predicates cannot drift (a drift would make the
/// pagination total mismatch the returned rows). Callers must bind the `Some` filters in
/// the same canonical order: from, to, status, strategy, wallet.
fn append_trade_filter_clauses(
    sql: &mut String,
    from_date: Option<&str>,
    to_date: Option<&str>,
    status_filter: Option<&str>,
    strategy_filter: Option<&str>,
    wallet_address_filter: Option<&str>,
) -> usize {
    let mut n = 1usize;
    if from_date.is_some() {
        sql.push_str(&format!(" AND created_at >= ${n}::timestamptz"));
        n += 1;
    }
    if to_date.is_some() {
        sql.push_str(&format!(" AND created_at <= ${n}::timestamptz"));
        n += 1;
    }
    if status_filter.is_some() {
        sql.push_str(&format!(" AND status = ${n}"));
        n += 1;
    }
    if strategy_filter.is_some() {
        sql.push_str(&format!(" AND strategy = ${n}"));
        n += 1;
    }
    if wallet_address_filter.is_some() {
        sql.push_str(&format!(" AND wallet_address = ${n}"));
        n += 1;
    }
    n
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

    async fn get_pool_stats(&self) -> AppResult<super::PoolStats> {
        use super::PoolStats;

        let max_connections = self.pool.options().get_max_connections();
        let idle_connections = self.pool.num_idle();
        let active_connections = max_connections.saturating_sub(idle_connections as u32);

        let utilization_percent = if max_connections > 0 {
            (active_connections as f64 / max_connections as f64) * 100.0
        } else {
            0.0
        };

        Ok(PoolStats {
            active_connections,
            idle_connections: idle_connections as u32,
            max_connections,
            utilization_percent,
        })
    }

    // ========================================================================
    // MIGRATION & STARTUP
    // ========================================================================

    async fn run_migrations(&self) -> AppResult<()> {
        // 0001_full_schema.sql issues `COMMENT ON TABLE schema_migrations`
        // BEFORE its own CREATE TABLE (historical ordering bug). On a fresh
        // database that COMMENT fails and the whole migration chain aborts.
        // The applied-migration file cannot be edited (sqlx checksum), so
        // pre-create the table here — 0001's CREATE IF NOT EXISTS is then a
        // no-op and its COMMENT succeeds. Harmless on existing databases.
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS schema_migrations (
                version TEXT PRIMARY KEY,
                applied_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(AppError::Database)?;

        sqlx::migrate!("./migrations_postgres")
            .run(&self.pool)
            .await
            .map_err(|e| AppError::Database(e.into()))?;
        info!("PostgreSQL migrations applied");
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
        .bind(trade.amount_sol)
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
                    network_fee_sol = COALESCE($5, network_fee_sol),
                    closed_at = CASE WHEN $1 = 'CLOSED' THEN CURRENT_TIMESTAMP ELSE closed_at END
                WHERE trade_uuid = $4
                "#,
            )
            .bind(&update.status)
            .bind(sig)
            .bind(&update.error_message)
            .bind(&update.trade_uuid)
            .bind(update.network_fee_sol)
            .execute(&self.pool)
            .await?
        } else {
            sqlx::query(
                r#"
                UPDATE trades
                SET status = $1, error_message = COALESCE($2, error_message),
                    network_fee_sol = COALESCE($4, network_fee_sol),
                    closed_at = CASE WHEN $1 = 'CLOSED' THEN CURRENT_TIMESTAMP ELSE closed_at END
                WHERE trade_uuid = $3
                "#,
            )
            .bind(&update.status)
            .bind(&update.error_message)
            .bind(&update.trade_uuid)
            .bind(update.network_fee_sol)
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
                net_pnl_sol, pnl_data_valid, created_at, updated_at
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
                net_pnl_sol, pnl_data_valid, created_at, updated_at
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
                net_pnl_sol, pnl_data_valid, created_at, updated_at
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

    // ========================================================================
    // POSITION OPERATIONS
    // ========================================================================

    async fn insert_position(&self, position: &InsertPosition) -> AppResult<i64> {
        // Guard against fake seed positions — reject positions with placeholder signatures
        if position.entry_tx_signature == "seed-position-init"
            || position.entry_tx_signature == "SEED_POSITION_INIT"
            || position.entry_tx_signature.starts_with("seed-position")
            || position.entry_tx_signature.starts_with("SEED_POSITION")
        {
            return Err(AppError::Validation(format!(
                "Position has placeholder entry_tx_signature '{}'. Seed positions are not allowed in production.",
                position.entry_tx_signature
            )));
        }

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
        .bind(position.entry_amount_sol)
        .bind(position.entry_price)
        .bind(&position.entry_tx_signature)
        .fetch_one(&self.pool)
        .await?;

        Ok(result.try_get("id").unwrap_or(0))
    }

    async fn update_position(&self, update: &UpdatePosition) -> AppResult<()> {
        // Bind values are accumulated in a SINGLE ordered container so that the
        // `$N` placeholder numbering always matches the bind order. Splitting
        // decimals and strings into separate vectors desyncs from `$N` (see the
        // SQLite reference, which uses one ordered `binds` vec). Each field is
        // pushed in the same order its `SET` clause is emitted.
        enum SetBind {
            Dec(Decimal),
            Str(String),
        }

        let mut set_clauses: Vec<String> = Vec::new();
        let mut binds: Vec<SetBind> = Vec::new();
        let mut param_idx = 1;

        if let Some(price) = update.current_price {
            set_clauses.push(format!("current_price = ${}", param_idx));
            binds.push(SetBind::Dec(price));
            param_idx += 1;
        }
        if let Some(pnl) = update.unrealized_pnl_sol {
            set_clauses.push(format!("unrealized_pnl_sol = ${}", param_idx));
            binds.push(SetBind::Dec(pnl));
            param_idx += 1;
        }
        if let Some(pnl_pct) = update.unrealized_pnl_percent {
            set_clauses.push(format!("unrealized_pnl_percent = ${}", param_idx));
            binds.push(SetBind::Dec(pnl_pct));
            param_idx += 1;
        }
        if let Some(state) = &update.state {
            set_clauses.push(format!("state = ${}", param_idx));
            binds.push(SetBind::Str(state.clone()));
            param_idx += 1;
        }
        if let Some(exit_price) = update.exit_price {
            set_clauses.push(format!("exit_price = ${}", param_idx));
            binds.push(SetBind::Dec(exit_price));
            param_idx += 1;
        }
        if let Some(exit_sig) = &update.exit_tx_signature {
            set_clauses.push(format!("exit_tx_signature = ${}", param_idx));
            binds.push(SetBind::Str(exit_sig.clone()));
            param_idx += 1;
        }
        if let Some(pnl) = update.realized_pnl_sol {
            set_clauses.push(format!("realized_pnl_sol = ${}", param_idx));
            binds.push(SetBind::Dec(pnl));
            param_idx += 1;
        }
        if let Some(pnl_usd) = update.realized_pnl_usd {
            set_clauses.push(format!("realized_pnl_usd = ${}", param_idx));
            binds.push(SetBind::Dec(pnl_usd));
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
        for bind in binds {
            query = match bind {
                SetBind::Dec(d) => query.bind(d),
                SetBind::Str(s) => query.bind(s),
            };
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

    async fn get_active_position_by_wallet_token(
        &self,
        wallet_address: &str,
        token_address: &str,
    ) -> AppResult<Option<Position>> {
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
            WHERE wallet_address = $1 AND token_address = $2 AND state IN ('ACTIVE', 'EXITING')
            ORDER BY opened_at DESC
            LIMIT 1
            "#,
        )
        .bind(wallet_address)
        .bind(token_address)
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::Database)?;

        match row {
            Some(r) => Ok(Some(self.row_to_position(r)?)),
            None => Ok(None),
        }
    }

    async fn get_unresolved_trade_by_wallet_token(
        &self,
        wallet_address: &str,
        token_address: &str,
    ) -> AppResult<Option<String>> {
        let row = sqlx::query(
            r#"
            SELECT trade_uuid
            FROM trades
            WHERE wallet_address = $1
              AND token_address = $2
              AND status IN ('PENDING', 'QUEUED', 'EXECUTING', 'PENDING_CONFIRMATION')
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(wallet_address)
        .bind(token_address)
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::Database)?;

        Ok(row.map(|r| r.get::<String, _>("trade_uuid")))
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
        .bind(exit_price)
        .bind(exit_tx_signature)
        .bind(realized_pnl_sol)
        .bind(realized_pnl_usd)
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
        // Roster merging via SQLite's `ATTACH DATABASE` was a scout -> operator
        // ETL step from the SQLite era (decommissioned 2026-07). Scout roster
        // data is ingested through the shared PostgreSQL `wallets` table now.
        // If this is reached, the caller is using a retired ingestion path.
        Err(AppError::Internal(
            "merge_roster is not supported: SQLite ATTACH DATABASE roster ingestion \
             was decommissioned. Scout roster data flows through the shared \
             PostgreSQL wallets table."
                .to_string(),
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

    async fn get_wallets_by_conviction_tier(
        &self,
        tier: chimera_core::config::ConvictionTier,
    ) -> AppResult<Vec<Wallet>> {
        use chimera_core::config::ConvictionTier;

        let (status, min_wqs, max_wqs) = match tier {
            ConvictionTier::High => (Some("ACTIVE"), Some(80), None),
            ConvictionTier::Regular => (Some("ACTIVE"), Some(60), Some(79)),
            ConvictionTier::Emerging => (Some("ACTIVE"), Some(0), Some(59)),
        };

        let mut wallets = self.get_wallets_with_wqs(status, min_wqs, max_wqs).await?;

        if matches!(tier, ConvictionTier::Emerging) {
            wallets.retain(|w| w.status != "REJECTED");
        }

        Ok(wallets)
    }

    async fn get_wallets_with_wqs(
        &self,
        status: Option<&str>,
        min_wqs: Option<i32>,
        max_wqs: Option<i32>,
    ) -> AppResult<Vec<Wallet>> {
        let mut query = String::from(
            r#"
            SELECT
                id, address, status, wqs_score, wqs_confidence,
                roi_7d, roi_30d, trade_count_30d, win_rate, max_drawdown_30d,
                avg_trade_size_sol, avg_win_sol, avg_loss_sol, profit_factor,
                realized_pnl_30d_sol, last_trade_at, promoted_at, ttl_expires_at,
                notes, archetype, avg_entry_delay_seconds, created_at, updated_at
            FROM wallets
            WHERE 1=1
            "#,
        );

        let mut conditions = Vec::new();
        let mut bind_count = 0;

        if status.is_some() {
            bind_count += 1;
            conditions.push(format!("status = ${}", bind_count));
        }
        if min_wqs.is_some() {
            bind_count += 1;
            conditions.push(format!("wqs_score >= ${}", bind_count));
        }
        if max_wqs.is_some() {
            bind_count += 1;
            conditions.push(format!("wqs_score <= ${}", bind_count));
        }

        if !conditions.is_empty() {
            query.push_str(" AND ");
            query.push_str(&conditions.join(" AND "));
        }

        query.push_str(" ORDER BY wqs_score DESC");

        let mut query_builder = sqlx::query(&query);

        if let Some(s) = status {
            query_builder = query_builder.bind(s);
        }
        if let Some(min) = min_wqs {
            query_builder = query_builder.bind(min);
        }
        if let Some(max) = max_wqs {
            query_builder = query_builder.bind(max);
        }

        let rows = query_builder
            .fetch_all(&self.pool)
            .await
            .map_err(AppError::Database)?;

        rows.into_iter().map(|r| self.row_to_wallet(r)).collect()
    }

    async fn get_promotion_candidates(
        &self,
        min_wqs: f64,
        max_age_days: i64,
        limit: i64,
    ) -> AppResult<Vec<Wallet>> {
        // Recency-first: require a trade within max_age_days so promotion
        // surfaces wallets that actually trade, and order by last_trade_at DESC
        // (most recent first) so the freshest active wallets fill the roster.
        let rows = sqlx::query(
            r#"
            SELECT
                id, address, status, wqs_score, wqs_confidence,
                roi_7d, roi_30d, trade_count_30d, win_rate, max_drawdown_30d,
                avg_trade_size_sol, avg_win_sol, avg_loss_sol, profit_factor,
                realized_pnl_30d_sol, last_trade_at, promoted_at, ttl_expires_at,
                notes, archetype, avg_entry_delay_seconds, created_at, updated_at
            FROM wallets
            WHERE status = 'CANDIDATE'
              AND wqs_score >= $1
              AND last_trade_at > NOW() - make_interval(days => $2::integer)
            ORDER BY last_trade_at DESC NULLS LAST, wqs_score DESC
            LIMIT $3
            "#,
        )
        .bind(min_wqs)
        .bind(max_age_days)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::Database)?;

        rows.into_iter().map(|r| self.row_to_wallet(r)).collect()
    }

    async fn demote_dormant_active_wallets(&self, max_age_days: i64) -> AppResult<u64> {
        // Demote ACTIVE wallets that haven't traded on-chain within max_age_days
        // back to CANDIDATE. Frees roster slots for active candidates and removes
        // dead-weight wallets that generate no copy signals. Skips wallets with
        // unknown last_trade_at (NULL) — those are handled by the inactivity
        // rotation's own logic.
        let result = sqlx::query(
            r#"
            UPDATE wallets
            SET status = 'CANDIDATE',
                updated_at = CURRENT_TIMESTAMP
            WHERE status = 'ACTIVE'
              AND last_trade_at IS NOT NULL
              AND last_trade_at <= NOW() - make_interval(days => $1::integer)
            "#,
        )
        .bind(max_age_days)
        .execute(&self.pool)
        .await
        .map_err(AppError::Database)?;
        Ok(result.rows_affected())
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
                .map(datetime_to_string)
                .unwrap_or_else(|_| datetime_to_string(chrono::Utc::now())),
        })
    }

    async fn update_circuit_breaker_state(
        &self,
        state: &str,
        tripped_at: Option<chrono::DateTime<chrono::Utc>>,
        trip_reason: Option<&str>,
    ) -> AppResult<()> {
        let mut sql = "UPDATE circuit_breaker_state SET state = $1, updated_at = CURRENT_TIMESTAMP"
            .to_string();
        let mut param_idx = 2;

        if tripped_at.is_some() {
            sql.push_str(&format!(", tripped_at = ${}", param_idx));
            param_idx += 1;
        }
        if trip_reason.is_some() {
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
                .map(datetime_to_string)
                .unwrap_or_else(|_| datetime_to_string(chrono::Utc::now())),
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
                COALESCE(SUM(net_pnl_sol) FILTER (WHERE pnl_data_valid), 0) as total_pnl_sol,
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
            total_pnl_sol: row
                .try_get::<Decimal, _>("total_pnl_sol")
                .unwrap_or(Decimal::ZERO),
            total_volume_sol: row
                .try_get::<Decimal, _>("total_volume_sol")
                .unwrap_or(Decimal::ZERO),
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
                net_pnl_sol, pnl_data_valid, created_at, updated_at
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
                copy_pnl_7d: r
                    .try_get::<Decimal, _>("copy_pnl_7d")
                    .unwrap_or(Decimal::ZERO),
                copy_pnl_30d: r
                    .try_get::<Decimal, _>("copy_pnl_30d")
                    .unwrap_or(Decimal::ZERO),
                signal_success_rate: r
                    .try_get::<Decimal, _>("signal_success_rate")
                    .unwrap_or(Decimal::ZERO),
                total_trades: r.try_get("total_trades").unwrap_or(0),
                winning_trades: r.try_get("winning_trades").unwrap_or(0),
            })),
            None => Ok(None),
        }
    }

    // ========================================================================
    // JITO TIPS
    // ========================================================================

    async fn insert_jito_tip(
        &self,
        tip: &Decimal,
        bsig: Option<&str>,
        strat: Option<&str>,
        success: bool,
    ) -> AppResult<i64> {
        let row = sqlx::query(
            r#"
            INSERT INTO jito_tip_history (tip_amount_sol, bundle_signature, strategy, success)
            VALUES ($1, $2, $3, $4)
            RETURNING id
            "#,
        )
        .bind(tip)
        .bind(bsig)
        .bind(strat)
        .bind(success)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.try_get("id").unwrap_or(0))
    }

    async fn get_recent_jito_tips(&self, limit: i32) -> AppResult<Vec<Decimal>> {
        let rows = sqlx::query_scalar::<_, Decimal>(
            r#"
            SELECT tip_amount_sol
            FROM jito_tip_history
            WHERE success = TRUE
            ORDER BY created_at DESC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::Database)?;

        Ok(rows)
    }

    async fn get_jito_tip_count(&self) -> AppResult<u32> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM jito_tip_history WHERE success = TRUE")
                .fetch_one(&self.pool)
                .await
                .map_err(AppError::Database)?;

        Ok(count as u32)
    }

    async fn prune_old_jito_tips(&self) -> AppResult<u64> {
        let result = sqlx::query(
            "DELETE FROM jito_tip_history WHERE created_at < NOW() - INTERVAL '7 days'",
        )
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    // ========================================================================
    // PnL / PERFORMANCE
    // ========================================================================

    async fn get_pnl_window(&self, from: &str, to: Option<&str>) -> AppResult<Decimal> {
        let total: Decimal = if let Some(t) = to {
            sqlx::query_scalar::<_, Decimal>(
                r#"SELECT COALESCE(SUM(realized_pnl_sol), 0.0) FROM positions
                   WHERE state = 'CLOSED' AND pnl_data_valid AND closed_at >= NOW() - make_interval(hours => $1::int)
                   AND closed_at < NOW() - make_interval(hours => $2::int)"#,
            )
            .bind(from)
            .bind(t)
            .fetch_one(&self.pool)
            .await
            .map_err(AppError::Database)?
        } else {
            sqlx::query_scalar::<_, Decimal>(
                r#"SELECT COALESCE(SUM(realized_pnl_sol), 0.0) FROM positions
                   WHERE state = 'CLOSED' AND pnl_data_valid AND closed_at >= NOW() - make_interval(hours => $1::int)"#,
            )
            .bind(from)
            .fetch_one(&self.pool)
            .await
            .map_err(AppError::Database)?
        };

        Ok(total)
    }

    async fn get_pnl_24h(&self) -> AppResult<Decimal> {
        let total: Decimal = sqlx::query_scalar::<_, Decimal>(
            r#"SELECT COALESCE(SUM(realized_pnl_sol), 0.0) FROM positions
               WHERE state = 'CLOSED' AND pnl_data_valid AND closed_at >= NOW() - INTERVAL '24 hours'"#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::Database)?;

        Ok(total)
    }

    async fn get_pnl_7d(&self) -> AppResult<Decimal> {
        let total: Decimal = sqlx::query_scalar::<_, Decimal>(
            r#"SELECT COALESCE(SUM(realized_pnl_sol), 0.0) FROM positions
               WHERE state = 'CLOSED' AND pnl_data_valid AND closed_at >= NOW() - INTERVAL '7 days'"#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::Database)?;

        Ok(total)
    }

    async fn get_pnl_30d(&self) -> AppResult<Decimal> {
        let total: Decimal = sqlx::query_scalar::<_, Decimal>(
            r#"SELECT COALESCE(SUM(realized_pnl_sol), 0.0) FROM positions
               WHERE state = 'CLOSED' AND pnl_data_valid AND closed_at >= NOW() - INTERVAL '30 days'"#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::Database)?;

        Ok(total)
    }

    async fn get_total_realized_pnl(&self) -> AppResult<Decimal> {
        let total: Decimal = sqlx::query_scalar::<_, Decimal>(
            r#"SELECT COALESCE(SUM(realized_pnl_sol), 0.0) FROM positions
               WHERE state = 'CLOSED' AND pnl_data_valid"#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::Database)?;

        Ok(total)
    }

    async fn record_portfolio_snapshot(
        &self,
        nav_sol: Decimal,
        capital_sol: Decimal,
        realized_pnl_sol: Decimal,
        unrealized_pnl_sol: Decimal,
        open_positions: i32,
        sol_price_usd: Option<Decimal>,
        trade_mode: Option<String>,
    ) -> AppResult<()> {
        sqlx::query(
            r#"INSERT INTO portfolio_snapshots
                 (nav_sol, capital_sol, realized_pnl_sol, unrealized_pnl_sol,
                  open_positions, sol_price_usd, trade_mode)
               VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
        )
        .bind(nav_sol)
        .bind(capital_sol)
        .bind(realized_pnl_sol)
        .bind(unrealized_pnl_sol)
        .bind(open_positions)
        .bind(sol_price_usd)
        .bind(trade_mode)
        .execute(&self.pool)
        .await
        .map_err(AppError::Database)?;
        Ok(())
    }

    async fn get_portfolio_nav_history(
        &self,
        days: u32,
    ) -> AppResult<Vec<crate::db_abstraction::types::PortfolioSnapshot>> {
        let rows: Vec<crate::db_abstraction::types::PortfolioSnapshot> =
            sqlx::query_as::<_, crate::db_abstraction::types::PortfolioSnapshot>(
                r#"SELECT recorded_at, nav_sol, capital_sol, realized_pnl_sol,
                      unrealized_pnl_sol, open_positions, sol_price_usd, trade_mode
               FROM portfolio_snapshots
               WHERE recorded_at >= NOW() - INTERVAL '1 day' * $1
               ORDER BY recorded_at ASC"#,
            )
            .bind(days as i32)
            .fetch_all(&self.pool)
            .await
            .map_err(AppError::Database)?;
        Ok(rows)
    }

    async fn delete_portfolio_snapshots_before(&self, days: i32) -> AppResult<u64> {
        let res = sqlx::query(
            r#"DELETE FROM portfolio_snapshots
               WHERE recorded_at < NOW() - INTERVAL '1 day' * $1"#,
        )
        .bind(days)
        .execute(&self.pool)
        .await
        .map_err(AppError::Database)?;
        Ok(res.rows_affected())
    }

    async fn get_capital_deployed_30d(&self) -> AppResult<Decimal> {
        let total: Decimal = sqlx::query_scalar::<_, Decimal>(
            r#"SELECT COALESCE(SUM(entry_amount_sol), 0.0) FROM positions
               WHERE state = 'CLOSED' AND pnl_data_valid AND closed_at >= NOW() - INTERVAL '30 days'"#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::Database)?;

        Ok(total)
    }

    async fn cancel_stale_trades(&self, max_age_minutes: i32) -> AppResult<u64> {
        let result = sqlx::query(
            r#"UPDATE trades SET status = 'DEAD_LETTER', updated_at = NOW()
               WHERE status IN ('PENDING', 'QUEUED')
               AND created_at < NOW() - make_interval(mins => $1::int)"#,
        )
        .bind(max_age_minutes)
        .execute(&self.pool)
        .await
        .map_err(AppError::Database)?;

        Ok(result.rows_affected() as u64)
    }

    async fn get_strategy_performance(
        &self,
        strat: &str,
        days: i32,
    ) -> AppResult<(f64, Decimal, u32)> {
        let days_clamped = days.clamp(1, 365);

        let rows: Vec<Decimal> = sqlx::query_scalar::<_, Decimal>(
            r#"
            SELECT COALESCE(net_pnl_sol, 0.0)
            FROM trades
            WHERE status = 'CLOSED'
            AND pnl_data_valid
            AND strategy = $1
            AND created_at >= NOW() - ($2 || ' days')::interval
            ORDER BY created_at DESC
            "#,
        )
        .bind(strat)
        .bind(days_clamped.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::Database)?;

        if rows.is_empty() {
            return Ok((0.0, Decimal::ZERO, 0));
        }

        let mut total_pnl = Decimal::ZERO;
        let mut winning_trades = 0u32;
        let total_trades = rows.len() as u32;

        for pnl in rows {
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

    async fn get_consecutive_losses(&self) -> AppResult<u32> {
        let rows: Vec<Decimal> = sqlx::query_scalar::<_, Decimal>(
            r#"
            SELECT COALESCE(p.realized_pnl_sol, 0.0)
            FROM trades t
            LEFT JOIN positions p ON p.trade_uuid = t.trade_uuid
            WHERE t.status = 'CLOSED'
            AND t.pnl_data_valid
            ORDER BY t.created_at DESC
            LIMIT 20
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::Database)?;

        let mut consecutive = 0u32;
        for pnl in rows {
            if pnl < Decimal::ZERO {
                consecutive += 1;
            } else {
                break;
            }
        }

        Ok(consecutive)
    }

    async fn get_consecutive_losses_since(
        &self,
        since: Option<chrono::DateTime<chrono::Utc>>,
    ) -> AppResult<u32> {
        // Same logic as get_consecutive_losses but optionally restricted to
        // trades created after `since` (the circuit-breaker reset baseline),
        // so a reset clears the streak instead of re-tripping on history.
        let rows: Vec<Decimal> = if let Some(since) = since {
            sqlx::query_scalar::<_, Decimal>(
                r#"
                SELECT COALESCE(p.realized_pnl_sol, 0.0)
                FROM trades t
                LEFT JOIN positions p ON p.trade_uuid = t.trade_uuid
                WHERE t.status = 'CLOSED'
                AND t.pnl_data_valid
                AND t.created_at > $1
                ORDER BY t.created_at DESC
                LIMIT 20
                "#,
            )
            .bind(since)
            .fetch_all(&self.pool)
            .await
            .map_err(AppError::Database)?
        } else {
            sqlx::query_scalar::<_, Decimal>(
                r#"
                SELECT COALESCE(p.realized_pnl_sol, 0.0)
                FROM trades t
                LEFT JOIN positions p ON p.trade_uuid = t.trade_uuid
                WHERE t.status = 'CLOSED'
                AND t.pnl_data_valid
                ORDER BY t.created_at DESC
                LIMIT 20
                "#,
            )
            .fetch_all(&self.pool)
            .await
            .map_err(AppError::Database)?
        };

        let mut consecutive = 0u32;
        for pnl in rows {
            if pnl < Decimal::ZERO {
                consecutive += 1;
            } else {
                break;
            }
        }

        Ok(consecutive)
    }

    async fn get_max_drawdown_percent(&self, cap: Decimal) -> AppResult<(Decimal, Decimal)> {
        // Query all closed positions to find the all-time peak
        let closed_rows: Vec<Decimal> = sqlx::query_scalar::<_, Decimal>(
            r#"
            SELECT COALESCE(realized_pnl_sol, 0.0)
            FROM positions
            WHERE state = 'CLOSED' AND pnl_data_valid
            ORDER BY closed_at ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::Database)?;

        let mut peak_pnl = Decimal::ZERO;
        let mut running_pnl = Decimal::ZERO;
        let mut max_drawdown = Decimal::ZERO;
        for pnl in &closed_rows {
            running_pnl += pnl;
            if running_pnl > peak_pnl {
                peak_pnl = running_pnl;
            }
            // Track the historical worst peak-to-trough drawdown
            let denominator = cap + peak_pnl;
            if denominator > Decimal::ZERO {
                let dd = ((peak_pnl - running_pnl) / denominator) * Decimal::from(100);
                if dd > max_drawdown {
                    max_drawdown = dd;
                }
            }
        }

        let unrealized_pnl: Decimal = sqlx::query_scalar::<_, Decimal>(
            r#"SELECT COALESCE(SUM(unrealized_pnl_sol), 0.0) FROM positions WHERE state IN ('ACTIVE', 'EXITING')"#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::Database)?;

        let current_pnl = running_pnl + unrealized_pnl;

        let denominator = cap + peak_pnl;
        let current_drawdown = if denominator > Decimal::ZERO {
            let drawdown = ((peak_pnl - current_pnl) / denominator) * Decimal::from(100);
            drawdown.max(Decimal::ZERO)
        } else {
            Decimal::ZERO
        };

        Ok((current_drawdown, max_drawdown))
    }

    // ========================================================================
    // POSITION LIFECYCLE
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

        // A2: serialize concurrent opens for the same wallet+token across ALL
        // callers (not just the in-process pipeline lock). Under READ
        // COMMITTED, concurrent transactions would otherwise both pass the
        // duplicate check before either commits (write-skew), creating
        // duplicate ACTIVE positions. The advisory lock is held until this
        // transaction commits/rolls back, so the second opener observes the
        // first opener's committed row and is rejected by the duplicate check.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
            .bind(token_address)
            .execute(&mut *tx)
            .await?;

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
            let exposure_values: Vec<Decimal> = sqlx::query_scalar(
                "SELECT COALESCE(entry_amount_sol, 0) FROM positions WHERE state IN ('ACTIVE', 'EXITING')",
            )
            .fetch_all(&mut *tx)
            .await?;
            let current: Decimal = exposure_values.into_iter().sum();
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
            "SELECT COUNT(*) FROM positions WHERE token_address = $1 AND state IN ('ACTIVE','EXITING')",
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
            SET status = 'ACTIVE', tx_signature = $1
            WHERE trade_uuid = $2
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
                state, unrealized_pnl_sol, unrealized_pnl_percent
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'ACTIVE', 0, 0)
            "#,
        )
        .bind(trade_uuid)
        .bind(wallet_address)
        .bind(token_address)
        .bind(token_symbol)
        .bind(strategy)
        .bind(amount_sol)
        .bind(entry_price)
        .bind(tx_signature)
        .bind(entry_sol_price_usd)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
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
                    if (db_err.code().as_deref() == Some("40001")
                        || db_err.code().as_deref() == Some("40P01"))
                        && attempt < MAX_RETRIES =>
                {
                    let backoff = std::time::Duration::from_millis(50 * (1 << (attempt - 1)));
                    tracing::debug!(
                        attempt = attempt,
                        backoff_ms = backoff.as_millis(),
                        trade_uuid = %trade_uuid,
                        "Serialization conflict, retrying portfolio heat check"
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
    ) -> AppResult<bool> {
        if exit_price.is_zero() {
            return Err(AppError::Validation(
                "exit_price cannot be zero — PnL calculations would produce -100% loss".to_string(),
            ));
        }

        let exit_fraction = exit_fraction.max(Decimal::ZERO).min(Decimal::ONE);

        let mut tx = self.pool.begin().await?;

        #[allow(clippy::type_complexity)]
        let active_positions: Vec<(i64, Decimal, Decimal, String, Option<Decimal>)> =
            sqlx::query_as(
                r#"
                SELECT id, entry_price, entry_amount_sol, trade_uuid, entry_sol_price_usd
                FROM positions
                WHERE wallet_address = $1 AND token_address = $2 AND state IN ('ACTIVE', 'EXITING')
                FOR UPDATE
                "#,
            )
            .bind(wallet_address)
            .bind(token_address)
            .fetch_all(&mut *tx)
            .await?;

        if active_positions.is_empty() {
            tracing::warn!(
                wallet = %wallet_address,
                token = %token_address,
                "No active positions found to close"
            );
            tx.commit().await?;
            return Ok(false);
        }

        #[allow(clippy::type_complexity)]
        let exit_costs: Option<(Option<Decimal>, Option<Decimal>, Option<Decimal>, Option<Decimal>)> =
            sqlx::query_as(
                "SELECT jito_tip_sol, dex_fee_sol, slippage_cost_sol, network_fee_sol FROM trades WHERE trade_uuid = $1",
            )
            .bind(trade_uuid)
            .fetch_optional(&mut *tx)
            .await?;

        // A1 canonical cost model: net PnL = gross(fill-price cash flows)
        // − network fees − Jito tips. DEX route fees and price impact are
        // ATTRIBUTION ONLY — the executable Jupiter quote already embeds
        // them in outAmount, so subtracting them here would double-count.
        let exit_tip = exit_costs
            .and_then(|(t, _, _, _)| t)
            .unwrap_or(Decimal::ZERO);
        let exit_network_fee = exit_costs
            .and_then(|(_, _, _, nf)| nf)
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

        #[allow(clippy::type_complexity)]
        let mut entry_costs_map: std::collections::HashMap<
            String,
            (
                Option<Decimal>,
                Option<Decimal>,
                Option<Decimal>,
                Decimal,
                Option<Decimal>,
            ),
        > = std::collections::HashMap::new();
        if !entry_uuids.is_empty() {
            let placeholders = entry_uuids
                .iter()
                .enumerate()
                .map(|(i, _)| format!("${}", i + 1))
                .collect::<Vec<_>>()
                .join(", ");
            let bulk_sql = format!(
                "SELECT trade_uuid, jito_tip_sol, dex_fee_sol, slippage_cost_sol, amount_sol, network_fee_sol FROM trades WHERE trade_uuid IN ({})",
                placeholders
            );
            let mut bulk_q = sqlx::query_as::<
                _,
                (
                    String,
                    Option<Decimal>,
                    Option<Decimal>,
                    Option<Decimal>,
                    Decimal,
                    Option<Decimal>,
                ),
            >(&bulk_sql);
            for uuid in &entry_uuids {
                bulk_q = bulk_q.bind(uuid);
            }
            let cost_rows: Vec<(
                String,
                Option<Decimal>,
                Option<Decimal>,
                Option<Decimal>,
                Decimal,
                Option<Decimal>,
            )> = bulk_q.fetch_all(&mut *tx).await?;
            for (uuid, tip, dex, slip, amount, nf) in cost_rows {
                entry_costs_map.insert(uuid, (tip, dex, slip, amount, nf));
            }
        }

        let mut gross_pnl = Decimal::ZERO;
        let mut entry_total_costs = Decimal::ZERO;

        let is_full_close = exit_fraction >= Decimal::ONE;

        let mut fully_closed_entry_uuids: Vec<String> = Vec::new();

        // A1: the exit transaction's tip and network fee belong to THIS close
        // event and are allocated once across all positions being closed
        // (normally one; duplicates share the allocation).
        let position_count = Decimal::from(active_positions.len() as u64);
        let exit_tip_alloc = exit_tip
            .checked_div(position_count)
            .unwrap_or(Decimal::ZERO);
        let exit_network_fee_alloc = exit_network_fee
            .checked_div(position_count)
            .unwrap_or(Decimal::ZERO);

        for (id, entry_price_dec, entry_amount_dec, entry_trade_uuid, entry_sol_price_opt) in
            active_positions.iter()
        {
            let id = *id;
            let entry_price_dec = *entry_price_dec;
            let entry_amount_dec = *entry_amount_dec;
            let entry_sol_price_dec = *entry_sol_price_opt;
            let mut net_pnl_opt: Option<Decimal> = None;

            let exited_amount = entry_amount_dec * exit_fraction;

            // A1 canonical gross PnL: every branch computes
            //   (exit_price / entry_price − 1) × exited_amount
            // i.e. return-times-capital in SOL. When both SOL/USD rates are
            // known the USD prices are converted to SOL-per-token first (the
            // SOL/USD rate cancels in the ratio); otherwise the ratio is
            // computed directly from USD prices — no per-token diff is ever
            // divided by a SOL price without exposure scaling.
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
                    } else {
                        tracing::warn!(
                            trade_uuid = %trade_uuid,
                            "SOL/USD conversion price zero at close; using direct USD return ratio for PnL"
                        );
                        let ratio = (exit_price - entry_price_dec) / entry_price_dec;
                        ratio * exited_amount
                    }
                } else {
                    if sol_price_usd.is_none() || entry_sol_price_dec.is_none() {
                        tracing::warn!(
                            trade_uuid = %trade_uuid,
                            "SOL/USD price data incomplete; using direct USD return ratio for PnL"
                        );
                    }
                    let ratio = (exit_price - entry_price_dec) / entry_price_dec;
                    ratio * exited_amount
                }
            } else {
                Decimal::ZERO
            };

            if let Some((et, _ed, _es, orig_amount, entry_nf)) =
                entry_costs_map.get(entry_trade_uuid.as_str())
            {
                // A1: only tips + network fees are real out-of-pocket costs.
                // dex_fee_sol / slippage_cost_sol columns stay populated for
                // attribution but are never subtracted (embedded in fills).
                let entry_tip = (*et).unwrap_or(Decimal::ZERO);
                let entry_network_fee = (*entry_nf).unwrap_or(Decimal::ZERO);
                let exited_fraction_of_original = if !orig_amount.is_zero() {
                    exited_amount
                        .checked_div(*orig_amount)
                        .unwrap_or(exit_fraction)
                } else {
                    exit_fraction
                };
                let proportional_entry_tip = entry_tip * exited_fraction_of_original;
                let proportional_entry_network_fee =
                    entry_network_fee * exited_fraction_of_original;
                entry_total_costs += proportional_entry_tip + proportional_entry_network_fee;
                let net_pnl_sol = pnl_sol
                    - proportional_entry_tip
                    - proportional_entry_network_fee
                    - exit_tip_alloc
                    - exit_network_fee_alloc;
                net_pnl_opt = Some(net_pnl_sol);
            }

            let pnl_usd_opt: Option<Decimal> = sol_price_usd.map(|sol_usd| pnl_sol * sol_usd);

            if is_full_close {
                // A1: accumulate — earlier tiered partial closes already
                // recorded realized PnL on this row; the final tranche must
                // ADD to it, never overwrite it.
                let rows = sqlx::query(
                    r#"
                    UPDATE positions
                    SET
                        exit_price = $1,
                        exit_tx_signature = $2,
                        realized_pnl_sol = COALESCE(realized_pnl_sol, 0) + $3,
                        realized_pnl_usd = COALESCE(realized_pnl_usd, 0) + $4,
                        realized_net_pnl_sol = COALESCE(realized_net_pnl_sol, 0) + $5,
                        closed_at = CASE WHEN $6 THEN NOW() ELSE NULL END,
                        state = $7
                    WHERE id = $8 AND state IN ('ACTIVE', 'EXITING')
                    "#,
                )
                .bind(exit_price)
                .bind(signature)
                .bind(pnl_sol)
                .bind(pnl_usd_opt.unwrap_or(Decimal::ZERO))
                .bind(net_pnl_opt.unwrap_or(Decimal::ZERO))
                .bind(confirmed)
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

                fully_closed_entry_uuids.push(entry_trade_uuid.clone());
            } else {
                let remaining_amount = entry_amount_dec - exited_amount;

                let current_realized_sol: Decimal = sqlx::query_scalar(
                    "SELECT COALESCE(realized_pnl_sol, 0) FROM positions WHERE id = $1",
                )
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?
                .unwrap_or(Decimal::ZERO);
                let new_realized_sol = current_realized_sol + pnl_sol;

                let new_realized_usd = if let Some(pnl_usd) = pnl_usd_opt {
                    let current_realized_usd: Decimal = sqlx::query_scalar(
                        "SELECT COALESCE(realized_pnl_usd, 0) FROM positions WHERE id = $1",
                    )
                    .bind(id)
                    .fetch_optional(&mut *tx)
                    .await?
                    .unwrap_or(Decimal::ZERO);
                    Some(current_realized_usd + pnl_usd)
                } else {
                    None
                };

                let rows = sqlx::query(
                    r#"
                    UPDATE positions
                    SET
                        entry_amount_sol = $1,
                        exit_price = $2,
                        exit_tx_signature = $3,
                        realized_pnl_sol = $4,
                        realized_pnl_usd = $5,
                        realized_net_pnl_sol = COALESCE(realized_net_pnl_sol, 0) + $6,
                        token_amount = token_amount * (1 - $7),
                        state = $8,
                        last_updated = NOW()
                    WHERE id = $9 AND state IN ('ACTIVE', 'EXITING')
                    "#,
                )
                .bind(remaining_amount)
                .bind(exit_price)
                .bind(signature)
                .bind(new_realized_sol)
                .bind(new_realized_usd)
                .bind(net_pnl_opt.unwrap_or(Decimal::ZERO))
                .bind(exit_fraction)
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

        // Mark fully-closed entry (BUY) trades as CLOSED so they don't stay ACTIVE forever.
        // Without this, the trades table accumulates orphaned ACTIVE rows whose positions are CLOSED,
        // which corrupts circuit-breaker loss counts and dashboard stats.
        for entry_uuid in &fully_closed_entry_uuids {
            let rows = sqlx::query(
                "UPDATE trades SET status = 'CLOSED', closed_at = CURRENT_TIMESTAMP WHERE trade_uuid = $1 AND status = 'ACTIVE'",
            )
            .bind(entry_uuid)
            .execute(&mut *tx)
            .await?;
            if rows.rows_affected() > 0 {
                tracing::info!(
                    entry_trade_uuid = %entry_uuid,
                    "Marked BUY trade CLOSED after position fully closed"
                );
            }
        }

        // A1: aggregate net PnL = gross − entry (tips + network fees)
        // − exit tip − exit network fee. Previously the exit network fee was
        // omitted here even though position-level math included it.
        let net_pnl = gross_pnl - entry_total_costs - exit_tip - exit_network_fee;
        let current_net: Decimal =
            sqlx::query_scalar("SELECT COALESCE(net_pnl_sol, 0) FROM trades WHERE trade_uuid = $1")
                .bind(trade_uuid)
                .fetch_optional(&mut *tx)
                .await?
                .unwrap_or(Decimal::ZERO);
        let new_net = current_net + net_pnl;

        let pnl_usd = sol_price_usd
            .map(|price| net_pnl * price)
            .unwrap_or(Decimal::ZERO);

        sqlx::query(
            r#"
            UPDATE trades SET pnl_sol = $1, pnl_usd = $2 WHERE trade_uuid = $3
            "#,
        )
        .bind(net_pnl)
        .bind(pnl_usd)
        .bind(trade_uuid)
        .execute(&mut *tx)
        .await?;

        sqlx::query("UPDATE trades SET net_pnl_sol = $1 WHERE trade_uuid = $2")
            .bind(new_net)
            .bind(trade_uuid)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(true)
    }

    async fn update_position_token_amount(&self, uuid: &str, token_amount: u64) -> AppResult<()> {
        // Bind as i64, not string: sqlx encodes i64 as INT8 which Postgres casts
        // cleanly to NUMERIC. Binding the u64's string form against a NUMERIC column
        // was silently failing to persist (no error, zero rows affected).
        sqlx::query("UPDATE positions SET token_amount = $1 WHERE trade_uuid = $2")
            .bind(token_amount as i64)
            .bind(uuid)
            .execute(&self.pool)
            .await
            .map_err(AppError::Database)?;
        Ok(())
    }

    async fn force_close_orphan_position(&self, trade_uuid: &str, reason: &str) -> AppResult<()> {
        // Guard on state='ACTIVE' AND token_amount IS NULL for idempotency:
        // once closed (or once token_amount is populated) the row no longer
        // matches, so this is safe to call repeatedly and from multiple paths.
        // Non-destructive — sets state + timestamps + zero PnL, never deletes.
        sqlx::query(
            r#"
            UPDATE positions
            SET state = 'CLOSED',
                closed_at = CURRENT_TIMESTAMP,
                realized_pnl_sol = 0,
                realized_pnl_usd = 0,
                unrealized_pnl_sol = 0,
                exit_tx_signature = $2
            WHERE trade_uuid = $1
              AND state = 'ACTIVE'
              AND token_amount IS NULL
            "#,
        )
        .bind(trade_uuid)
        .bind(reason)
        .execute(&self.pool)
        .await
        .map_err(AppError::Database)?;
        Ok(())
    }

    async fn revert_position_exit(&self, position_trade_uuid: &str) -> AppResult<()> {
        let mut tx = self.pool.begin().await?;

        #[allow(clippy::type_complexity)]
        let pos: Option<(
            Decimal,
            Decimal,
            Option<String>,
            Option<Decimal>,
            Option<Decimal>,
            String,
            String,
        )> = sqlx::query_as(
            r#"
            SELECT entry_price, entry_amount_sol, exit_tx_signature, realized_pnl_sol, realized_pnl_usd, wallet_address, token_address
            FROM positions WHERE trade_uuid = $1
            "#,
        )
        .bind(position_trade_uuid)
        .fetch_optional(&mut *tx)
        .await?;

        if let Some((
            _entry_price,
            _entry_amount,
            Some(ref exit_sig),
            realized_pnl_sol_opt,
            realized_pnl_usd_opt,
            ref wallet_address,
            ref token_address,
        )) = pos
        {
            if !exit_sig.is_empty() {
                let exit_trade: Option<(String, Decimal)> = sqlx::query_as(
                    "SELECT trade_uuid, amount_sol FROM trades WHERE tx_signature = $1 AND side = 'SELL'",
                )
                .bind(exit_sig)
                .fetch_optional(&mut *tx)
                .await?;

                if let Some((ref exit_trade_uuid, _exit_amount)) = exit_trade {
                    let buy_signal_amount_sol: Decimal =
                        sqlx::query_scalar("SELECT amount_sol FROM trades WHERE trade_uuid = $1")
                            .bind(position_trade_uuid)
                            .fetch_one(&mut *tx)
                            .await?;

                    let confirmed_exit_values: Vec<Decimal> = sqlx::query_scalar(
                        "SELECT amount_sol FROM trades WHERE wallet_address = $1 AND token_address = $2 AND side = 'SELL' AND status = 'CLOSED' AND tx_signature <> $3",
                    )
                    .bind(wallet_address)
                    .bind(token_address)
                    .bind(exit_sig)
                    .fetch_all(&mut *tx)
                    .await?;
                    let confirmed_exit_amount: Decimal = confirmed_exit_values.into_iter().sum();

                    let reverted_amount =
                        (buy_signal_amount_sol - confirmed_exit_amount).max(Decimal::ZERO);

                    let mut new_realized_pnl_sol: Option<Decimal> = None;
                    let mut new_realized_pnl_usd: Option<Decimal> = None;

                    if confirmed_exit_amount > Decimal::ZERO {
                        #[allow(clippy::type_complexity)]
                        let (failed_net, failed_tip, failed_dex, failed_slip): (
                            Option<Decimal>,
                            Option<Decimal>,
                            Option<Decimal>,
                            Option<Decimal>,
                        ) = sqlx::query_as(
                            "SELECT net_pnl_sol, jito_tip_sol, dex_fee_sol, slippage_cost_sol FROM trades WHERE trade_uuid = $1",
                        )
                        .bind(exit_trade_uuid)
                        .fetch_one(&mut *tx)
                        .await?;

                        let failed_gross = match (failed_net, failed_tip, failed_dex, failed_slip) {
                            (Some(net), Some(tip), Some(dex), Some(slip)) => net + tip + dex + slip,
                            _ => Decimal::ZERO,
                        };

                        let current_pnl_sol = realized_pnl_sol_opt.unwrap_or(Decimal::ZERO);
                        let reverted_pnl = current_pnl_sol - failed_gross;
                        new_realized_pnl_sol = Some(reverted_pnl);

                        if realized_pnl_usd_opt.is_some() {
                            tracing::warn!(
                                exit_trade_uuid = %exit_trade_uuid,
                                "Reverting position with prior confirmed exits — setting realized_pnl_usd to NULL"
                            );
                            new_realized_pnl_usd = None;
                        }
                    }

                    if reverted_amount <= Decimal::ZERO {
                        tracing::warn!(
                            trade_uuid = %position_trade_uuid,
                            "Revert would leave zero/negative exposure; keeping position CLOSED"
                        );
                    } else {
                        sqlx::query(
                            r#"
                            UPDATE positions
                            SET
                                state = 'ACTIVE',
                                entry_amount_sol = $1,
                                exit_price = NULL,
                                exit_tx_signature = NULL,
                                realized_pnl_sol = $2,
                                realized_pnl_usd = $3,
                                closed_at = NULL,
                                last_updated = NOW()
                            WHERE trade_uuid = $4
                            "#,
                        )
                        .bind(reverted_amount)
                        .bind(new_realized_pnl_sol)
                        .bind(new_realized_pnl_usd)
                        .bind(position_trade_uuid)
                        .execute(&mut *tx)
                        .await?;
                    }

                    sqlx::query(
                        r#"
                        UPDATE trades
                        SET
                            status = 'FAILED',
                            net_pnl_sol = NULL,
                            error_message = 'Exit transaction failed to confirm on-chain (reverted by recovery manager)'
                        WHERE trade_uuid = $1
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

    async fn count_shadow_positions_by_token(&self, token_address: &str) -> AppResult<i64> {
        let row = sqlx::query("SELECT COUNT(*)::bigint AS count FROM shadow_positions WHERE token_address = $1")
            .bind(token_address)
            .fetch_one(&self.pool)
            .await?;
        Ok(row.get::<i64, _>("count"))
    }

    async fn get_stuck_positions(&self, stuck_seconds: i64) -> AppResult<Vec<PositionRecord>> {
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
            chrono::DateTime<chrono::Utc>,
        )> = sqlx::query_as(
            r#"
            SELECT id, trade_uuid, wallet_address, token_address, strategy, state,
                   entry_tx_signature, exit_tx_signature, last_updated
            FROM positions
            WHERE state = 'EXITING'
            AND last_updated < NOW() - make_interval(secs => $1::double precision)
            "#,
        )
        .bind(stuck_seconds as f64)
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
                        last_updated,
                    })
                },
            )
            .collect()
    }

    async fn update_position_state(&self, trade_uuid: &str, new_state: &str) -> AppResult<()> {
        sqlx::query("UPDATE positions SET state = $1 WHERE trade_uuid = $2")
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
            SET current_price = $1,
                unrealized_pnl_sol = $2,
                unrealized_pnl_percent = $3,
                last_updated = NOW()
            WHERE trade_uuid = $4
              AND state IN ('ACTIVE', 'EXITING')
            "#,
        )
        .bind(current_price)
        .bind(pnl_sol)
        .bind(pnl_pct)
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
            Decimal,
            Decimal,
            chrono::DateTime<chrono::Utc>,
        )> = sqlx::query_as(
            r#"
            SELECT
                p.trade_uuid,
                p.wallet_address,
                p.token_address,
                t.token_symbol,
                p.strategy,
                COALESCE(p.entry_price, 0),
                COALESCE(p.entry_amount_sol, 0),
                COALESCE(p.opened_at, NOW())
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
                    entry_price,
                    entry_amount_sol,
                    entry_time,
                )| ActivePositionEntry {
                    token_symbol: token_opt.unwrap_or_else(|| token_address.clone()),
                    trade_uuid,
                    wallet_address,
                    token_address,
                    strategy,
                    entry_price,
                    entry_amount_sol,
                    entry_time,
                },
            )
            .collect();

        Ok(entries)
    }

    async fn get_active_position_tokens(&self) -> AppResult<Vec<ActivePositionSummary>> {
        #[allow(clippy::type_complexity)]
        let rows: Vec<(String, String, Decimal, Decimal, Option<Decimal>)> = sqlx::query_as(
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
                |(
                    trade_uuid,
                    token_address,
                    entry_price,
                    entry_amount_sol,
                    entry_sol_price_usd,
                )| {
                    ActivePositionSummary {
                        trade_uuid,
                        token_address,
                        entry_price,
                        entry_amount_sol,
                        entry_sol_price_usd,
                    }
                },
            )
            .collect())
    }

    async fn get_position_peak_price(&self, trade_uuid: &str) -> AppResult<Option<String>> {
        let row: Option<Option<Decimal>> =
            sqlx::query_scalar("SELECT peak_price FROM exit_targets WHERE trade_uuid = $1")
                .bind(trade_uuid)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.flatten().map(|d| d.to_string()))
    }

    // ========================================================================
    // WALLETS / MONITORING
    // ========================================================================

    #[allow(clippy::too_many_arguments)]
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
            VALUES ($1, 'CANDIDATE', $2, $3, $4, $5, $6, $7, $8, $9, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
            ON CONFLICT(address) DO UPDATE SET
                wqs_score          = COALESCE(EXCLUDED.wqs_score, wallets.wqs_score),
                roi_7d             = COALESCE(EXCLUDED.roi_7d, wallets.roi_7d),
                roi_30d            = COALESCE(EXCLUDED.roi_30d, wallets.roi_30d),
                trade_count_30d    = COALESCE(EXCLUDED.trade_count_30d, wallets.trade_count_30d),
                win_rate           = COALESCE(EXCLUDED.win_rate, wallets.win_rate),
                max_drawdown_30d   = COALESCE(EXCLUDED.max_drawdown_30d, wallets.max_drawdown_30d),
                avg_trade_size_sol = COALESCE(EXCLUDED.avg_trade_size_sol, wallets.avg_trade_size_sol),
                notes              = COALESCE(EXCLUDED.notes, wallets.notes),
                updated_at         = CURRENT_TIMESTAMP
            RETURNING (xmax = 0) AS inserted
            "#,
        )
        .bind(address)
        .bind(wqs_score)
        .bind(roi_7d)
        .bind(roi_30d)
        .bind(trade_count_30d)
        .bind(win_rate)
        .bind(max_drawdown_30d)
        .bind(avg_trade_size_sol)
        .bind(notes)
        .fetch_one(&self.pool)
        .await?;

        Ok(result.try_get::<bool, _>("inserted").unwrap_or(false))
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
            Some(chrono::Utc::now())
        } else {
            None
        };

        let result = sqlx::query(
            r#"
            UPDATE wallets
            SET status = $1,
                promoted_at = COALESCE($2, promoted_at),
                ttl_expires_at = $3,
                notes = COALESCE($4, notes),
                demoted_at = CASE WHEN $1 = 'ACTIVE' THEN NULL ELSE demoted_at END
            WHERE address = $5
            "#,
        )
        .bind(status)
        .bind(promoted_at)
        .bind(ttl_expires_at)
        .bind(reason)
        .bind(address)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    async fn get_expired_ttl_wallets(&self) -> AppResult<Vec<String>> {
        let wallets: Vec<String> = sqlx::query_scalar(
            r#"
            SELECT address FROM wallets
            WHERE status = 'ACTIVE'
            AND ttl_expires_at IS NOT NULL
            AND ttl_expires_at < NOW()
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(wallets)
    }

    async fn demote_wallet(&self, address: &str, reason: &str) -> AppResult<()> {
        sqlx::query(
            r#"
            UPDATE wallets
            SET status = 'CANDIDATE',
                ttl_expires_at = NULL,
                demoted_at = NOW(),
                notes = $1
            WHERE address = $2
            "#,
        )
        .bind(reason)
        .bind(address)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn has_recent_token_loss(
        &self,
        token_address: &str,
        within_minutes: i64,
    ) -> AppResult<bool> {
        let count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*) FROM trades
            WHERE token_address = $1
              AND status = 'CLOSED'
              AND pnl_sol < 0
              AND pnl_sol / amount_sol * 100 < -3.0
              AND closed_at > NOW() - ($2 || ' minutes')::interval
            "#,
        )
        .bind(token_address)
        .bind(within_minutes)
        .fetch_one(&self.pool)
        .await?;
        Ok(count > 0)
    }

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
            WHERE wallet_address = $1
            "#,
        )
        .bind(wallet_address)
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::Database)?;

        match row {
            Some(r) => Ok(Some(Self::row_to_wallet_monitoring(&r)?)),
            None => Ok(None),
        }
    }

    async fn find_webhook_with_capacity(&self, max_wallets: i64) -> AppResult<Option<String>> {
        let webhook_id: Option<String> = sqlx::query_scalar(
            r#"
            SELECT helius_webhook_id
            FROM wallet_monitoring
            WHERE helius_webhook_id IS NOT NULL
            GROUP BY helius_webhook_id
            HAVING COUNT(*) < $1
            ORDER BY COUNT(*) DESC
            LIMIT 1
            "#,
        )
        .bind(max_wallets)
        .fetch_optional(&self.pool)
        .await?;
        Ok(webhook_id)
    }

    async fn clear_webhook_id_for_webhook(&self, webhook_id: &str) -> AppResult<()> {
        sqlx::query(
            "UPDATE wallet_monitoring SET helius_webhook_id = NULL WHERE helius_webhook_id = $1",
        )
        .bind(webhook_id)
        .execute(&self.pool)
        .await?;
        Ok(())
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
                wallet_address, helius_webhook_id, monitoring_enabled, last_monitored_at,
                webhook_status, webhook_registered_at
            )
            VALUES ($1, $2, $3, CURRENT_TIMESTAMP,
                    CASE WHEN $2 IS NOT NULL THEN 'active' ELSE 'orphaned' END,
                    CASE WHEN $2 IS NOT NULL THEN CURRENT_TIMESTAMP ELSE NULL END)
            ON CONFLICT(wallet_address) DO UPDATE SET
                helius_webhook_id = $2,
                monitoring_enabled = $3,
                rpc_polling_active = $3,
                webhook_status = CASE
                    WHEN $2 IS NOT NULL THEN 'active'
                    ELSE 'orphaned'
                END,
                webhook_registered_at = CASE
                    WHEN $2 IS NOT NULL THEN CURRENT_TIMESTAMP
                    ELSE wallet_monitoring.webhook_registered_at
                END,
                last_monitored_at = CURRENT_TIMESTAMP,
                updated_at = CURRENT_TIMESTAMP,
                registration_attempts = CASE
                    WHEN $2 IS NOT NULL THEN 0
                    ELSE wallet_monitoring.registration_attempts
                END,
                last_registration_error = CASE
                    WHEN $2 IS NOT NULL THEN NULL
                    ELSE wallet_monitoring.last_registration_error
                END,
                webhook_health_status = CASE
                    WHEN $2 IS NOT NULL THEN 'unknown'
                    ELSE wallet_monitoring.webhook_health_status
                END
            "#,
        )
        .bind(wallet_address)
        .bind(helius_webhook_id)
        .bind(monitoring_enabled)
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
            SET last_transaction_signature = $1,
                last_monitored_at = CURRENT_TIMESTAMP
            WHERE wallet_address = $2
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

    async fn get_active_wallets_with_webhook_ids(&self) -> AppResult<Vec<(String, String)>> {
        let rows = sqlx::query(
            r#"
            SELECT w.address, wm.helius_webhook_id
            FROM wallets w
            JOIN wallet_monitoring wm ON w.address = wm.wallet_address
            WHERE w.status = 'ACTIVE'
              AND wm.helius_webhook_id IS NOT NULL
              AND wm.helius_webhook_id != ''
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::Database)?;

        let result: Vec<(String, String)> = rows
            .iter()
            .filter_map(|row| {
                let address: Option<String> = row.try_get("address").ok()?;
                let webhook_id: Option<String> = row.try_get("helius_webhook_id").ok()?;
                Some((address?, webhook_id?))
            })
            .collect();

        Ok(result)
    }

    async fn clear_webhook_id(&self, wallet_address: &str) -> AppResult<()> {
        sqlx::query(
            r#"
            UPDATE wallet_monitoring
            SET helius_webhook_id = NULL,
                webhook_status = 'orphaned',
                webhook_health_status = 'unknown',
                updated_at = NOW()
            WHERE wallet_address = $1
            "#,
        )
        .bind(wallet_address)
        .execute(&self.pool)
        .await
        .map_err(AppError::Database)?;
        Ok(())
    }

    async fn get_stale_webhook_wallets(&self, threshold_days: i32) -> AppResult<Vec<String>> {
        let wallets = sqlx::query_scalar(
            r#"
            SELECT wallet_address
            FROM wallet_monitoring
            WHERE webhook_status = 'active'
              AND helius_webhook_id IS NOT NULL
              AND (
                  webhook_last_health_check IS NULL
                  OR webhook_last_health_check < NOW() - make_interval(days => $1)
                  OR webhook_health_status IN ('error', 'unhealthy', 'unknown')
              )
            "#,
        )
        .bind(threshold_days)
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
            .map(Self::row_to_wallet_monitoring)
            .collect::<AppResult<Vec<_>>>()?;

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
            SET webhook_health_status = $1,
                webhook_last_health_check = CURRENT_TIMESTAMP,
                webhook_status = CASE
                    WHEN $1 = 'healthy' THEN 'active'
                    WHEN $1 = 'unhealthy' THEN 'paused'
                    ELSE webhook_status
                END
            WHERE wallet_address = $2
            "#,
        )
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
            SET webhook_status = $1
            WHERE wallet_address = $2
            "#,
        )
        .bind(webhook_status)
        .bind(wallet_address)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_last_speculative_signal(
        &self,
        wallet_address: &str,
        timestamp: chrono::DateTime<chrono::Utc>,
    ) -> AppResult<()> {
        sqlx::query(
            r#"
            UPDATE wallet_monitoring
            SET last_speculative_signal_at = $1,
                updated_at = NOW()
            WHERE wallet_address = $2
            "#,
        )
        .bind(timestamp)
        .bind(wallet_address)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_inactivity_demotion_count(&self, wallet_address: &str) -> AppResult<i32> {
        let count = sqlx::query_scalar(
            r#"
            SELECT COALESCE(inactivity_demotion_count, 0)
            FROM wallet_monitoring
            WHERE wallet_address = $1
            "#,
        )
        .bind(wallet_address)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        Ok(count)
    }

    async fn increment_inactivity_demotion_count(&self, wallet_address: &str) -> AppResult<()> {
        sqlx::query(
            r#"
            UPDATE wallet_monitoring
            SET inactivity_demotion_count = COALESCE(inactivity_demotion_count, 0) + 1,
                updated_at = NOW()
            WHERE wallet_address = $1
            "#,
        )
        .bind(wallet_address)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn reset_inactivity_demotion_count(&self, wallet_address: &str) -> AppResult<()> {
        sqlx::query(
            r#"
            UPDATE wallet_monitoring
            SET inactivity_demotion_count = 0,
                updated_at = NOW()
            WHERE wallet_address = $1
            "#,
        )
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
        let audit_result = sqlx::query(
            r#"
            INSERT INTO webhook_lifecycle_audit
            (wallet_address, action, status, webhook_id, details, error_message, duration_ms)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
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

        if let Err(e) = audit_result {
            tracing::error!(
                wallet = %wallet_address,
                action = %action,
                status = %status,
                error = %e,
                "Failed to record webhook lifecycle audit event"
            );
        }

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
        let mut idx = 1usize;

        if let Some(wa) = wallet_address {
            sql.push_str(&format!(" AND wallet_address = ${idx}"));
            idx += 1;
            binds.push(wa.to_string());
        }
        if let Some(a) = action {
            sql.push_str(&format!(" AND action = ${idx}"));
            idx += 1;
            binds.push(a.to_string());
        }
        if let Some(s) = status {
            sql.push_str(&format!(" AND status = ${idx}"));
            idx += 1;
            binds.push(s.to_string());
        }

        sql.push_str(" ORDER BY created_at DESC");

        let limit_val = limit.unwrap_or(100).clamp(1, 1000);
        sql.push_str(&format!(" LIMIT ${idx}"));

        let mut query = sqlx::query(&sql);
        for b in binds {
            query = query.bind(b);
        }
        query = query.bind(limit_val);

        let rows = query.fetch_all(&self.pool).await?;

        let logs = rows
            .into_iter()
            .map(|r| WebhookAuditLog {
                id: r.try_get("id").unwrap_or_default(),
                wallet_address: r.try_get("wallet_address").unwrap_or_default(),
                action: r.try_get("action").unwrap_or_default(),
                status: r.try_get("status").unwrap_or_default(),
                webhook_id: r.try_get("webhook_id").ok(),
                details: r.try_get("details").ok(),
                error_message: r.try_get("error_message").ok(),
                duration_ms: r.try_get("duration_ms").ok(),
                created_at: r
                    .try_get::<chrono::DateTime<chrono::Utc>, _>("created_at")
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_else(|_| chrono::Utc::now().to_rfc3339()),
            })
            .collect();

        Ok(logs)
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
                last_registration_error = $1,
                webhook_status = CASE
                    WHEN registration_attempts + 1 >= 2 THEN 'failed'
                    ELSE webhook_status
                END
            WHERE wallet_address = $2
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
            SELECT config_value FROM webhook_configuration WHERE config_key = $1
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
            INSERT INTO webhook_configuration (config_key, config_value, last_updated_at, updated_by)
            VALUES ($1, $2, NOW(), $3)
            ON CONFLICT (config_key) DO UPDATE SET
                config_value = EXCLUDED.config_value,
                updated_by = EXCLUDED.updated_by,
                last_updated_at = NOW()
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

        let placeholders: Vec<String> = (1..=helius_webhook_ids.len())
            .map(|i| format!("${i}"))
            .collect();

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
            ) VALUES ($1, $2, $3, $4, $5, $6::jsonb, $7, $8, $9)
            ON CONFLICT (trade_uuid) DO UPDATE SET
                peak_price = EXCLUDED.peak_price,
                peak_profit_percent = EXCLUDED.peak_profit_percent,
                targets_hit = EXCLUDED.targets_hit,
                trailing_stop_active = EXCLUDED.trailing_stop_active,
                trailing_stop_price = EXCLUDED.trailing_stop_price,
                remaining_fraction = EXCLUDED.remaining_fraction,
                last_updated = NOW()
            "#,
        )
        .bind(trade_uuid)
        .bind(entry_price)
        .bind(entry_amount_sol)
        .bind(peak_price)
        .bind(peak_profit_percent)
        .bind(targets_hit_json)
        .bind(trailing_stop_active)
        .bind(trailing_stop_price)
        .bind(remaining_fraction)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn load_exit_target(&self, trade_uuid: &str) -> AppResult<Option<ExitTargetData>> {
        let row = sqlx::query(
            r#"
            SELECT
                entry_price,
                entry_amount_sol,
                peak_price,
                peak_profit_percent,
                COALESCE(targets_hit, '[]'::jsonb)::TEXT AS targets_hit,
                trailing_stop_active,
                COALESCE(trailing_stop_price, 0) AS trailing_stop_price,
                COALESCE(remaining_fraction, 1) AS remaining_fraction
            FROM exit_targets
            WHERE trade_uuid = $1
            "#,
        )
        .bind(trade_uuid)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|row| ExitTargetData {
            entry_price: row
                .try_get::<Decimal, _>("entry_price")
                .unwrap_or(Decimal::ZERO),
            entry_amount_sol: row
                .try_get::<Decimal, _>("entry_amount_sol")
                .unwrap_or(Decimal::ZERO),
            peak_price: row
                .try_get::<Decimal, _>("peak_price")
                .unwrap_or(Decimal::ZERO),
            peak_profit_percent: row
                .try_get::<Decimal, _>("peak_profit_percent")
                .unwrap_or(Decimal::ZERO),
            targets_hit: row
                .try_get::<String, _>("targets_hit")
                .unwrap_or_else(|_| "[]".to_string()),
            trailing_stop_active: row
                .try_get::<bool, _>("trailing_stop_active")
                .unwrap_or(false),
            trailing_stop_price: row
                .try_get::<Decimal, _>("trailing_stop_price")
                .unwrap_or(Decimal::ZERO),
            remaining_fraction: row
                .try_get::<Decimal, _>("remaining_fraction")
                .unwrap_or(Decimal::ONE),
        }))
    }

    async fn delete_exit_target(&self, trade_uuid: &str) -> AppResult<()> {
        sqlx::query("DELETE FROM exit_targets WHERE trade_uuid = $1")
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
        let row = sqlx::query(
            r#"
            INSERT INTO reconciliation_log (
                trade_uuid, expected_state, actual_on_chain, discrepancy,
                on_chain_tx_signature, notes
            ) VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id
            "#,
        )
        .bind(trade_uuid)
        .bind(expected_state)
        .bind(actual_on_chain)
        .bind(discrepancy)
        .bind(on_chain_tx_signature)
        .bind(notes)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.try_get("id").unwrap_or(0))
    }

    async fn get_reconciliation_status(
        &self,
        discrepancies_limit: i32,
    ) -> AppResult<ReconciliationStatus> {
        let limit = discrepancies_limit.clamp(1, 100) as i64;

        let latest_row = sqlx::query(
            r#"
            SELECT
                created_at,
                EXTRACT(EPOCH FROM created_at)::BIGINT AS created_ts
            FROM reconciliation_log
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .fetch_optional(&self.pool)
        .await?;

        let (last_at, last_ts) = match latest_row {
            Some(row) => {
                let at = row
                    .try_get::<chrono::DateTime<chrono::Utc>, _>("created_at")
                    .ok()
                    .map(|dt| dt.to_rfc3339());
                let ts = row.try_get::<i64, _>("created_ts").ok();
                (at, ts)
            }
            None => (None, None),
        };

        let next_at = last_ts.map(|ts| {
            let next = ts + 86400 - (ts % 86400) + 14400;
            next.to_string()
        });

        let (checked_count, discrepancy_count, unresolved_count): (i64, i64, i64) =
            sqlx::query_as(
                r#"
                SELECT
                    COALESCE(COUNT(*), 0) AS checked,
                    COALESCE(SUM(CASE WHEN discrepancy != 'NONE' THEN 1 ELSE 0 END), 0) AS discrepancies,
                    COALESCE(SUM(CASE WHEN discrepancy != 'NONE' AND resolved_at IS NULL THEN 1 ELSE 0 END), 0) AS unresolved
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
                created_at AS detected_at,
                resolved_at IS NOT NULL AS resolved,
                resolved_at
            FROM reconciliation_log
            WHERE discrepancy != 'NONE'
            ORDER BY created_at DESC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let recent_discrepancies = recent_rows
            .iter()
            .map(|row| {
                let discrepancy: String = row.try_get("discrepancy").unwrap_or_default();
                let notes: Option<String> = row.try_get("notes").ok();

                DiscrepancyRow {
                    id: row.try_get("id").unwrap_or(0),
                    trade_uuid: row.try_get("trade_uuid").unwrap_or_default(),
                    discrepancy_type: match discrepancy.as_str() {
                        "NONE" => "none".to_string(),
                        "MISSING_TX" => "missing_position".to_string(),
                        "AMOUNT_MISMATCH" => "pnl_mismatch".to_string(),
                        "STATE_MISMATCH" => "state_mismatch".to_string(),
                        "COST_MISMATCH" => "cost_mismatch".to_string(),
                        other => other.to_lowercase(),
                    },
                    severity: match discrepancy.as_str() {
                        "NONE" => "low".to_string(),
                        "MISSING_TX" => "critical".to_string(),
                        "AMOUNT_MISMATCH" => "high".to_string(),
                        "STATE_MISMATCH" => "medium".to_string(),
                        "COST_MISMATCH" => "medium".to_string(),
                        _ => "low".to_string(),
                    },
                    description: notes.unwrap_or_else(|| discrepancy.clone()),
                    db_value: row.try_get::<String, _>("expected_state").ok(),
                    on_chain_value: row.try_get::<String, _>("actual_on_chain").ok(),
                    detected_at: row
                        .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("detected_at")
                        .ok()
                        .flatten()
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                    resolved: row.try_get::<bool, _>("resolved").unwrap_or(false),
                    resolved_at: row
                        .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("resolved_at")
                        .ok()
                        .flatten()
                        .map(|dt| dt.to_rfc3339()),
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
                    created_at::DATE AS run_date,
                    MIN(id) AS id,
                    MIN(created_at) AS started_at,
                    MAX(created_at) AS completed_at,
                    'completed' AS status,
                    COUNT(*) AS checked_count,
                    SUM(CASE WHEN discrepancy != 'NONE' THEN 1 ELSE 0 END) AS discrepancy_count,
                    SUM(CASE WHEN discrepancy != 'NONE' AND resolved_at IS NULL THEN 1 ELSE 0 END) AS unresolved_count,
                    EXTRACT(EPOCH FROM (MAX(created_at) - MIN(created_at)))::FLOAT8 AS duration_seconds
                FROM reconciliation_log
                GROUP BY created_at::DATE
                ORDER BY run_date DESC
                LIMIT $1
            )
            SELECT
                id,
                started_at,
                completed_at,
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
                id: row.try_get("id").unwrap_or(0),
                started_at: row
                    .try_get::<chrono::DateTime<chrono::Utc>, _>("started_at")
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
                completed_at: row
                    .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("completed_at")
                    .ok()
                    .flatten()
                    .map(|dt| dt.to_rfc3339()),
                status: row
                    .try_get("status")
                    .unwrap_or_else(|_| "completed".to_string()),
                checked_count: row.try_get("checked_count").unwrap_or(0),
                discrepancy_count: row.try_get("discrepancy_count").unwrap_or(0),
                unresolved_count: row.try_get("unresolved_count").unwrap_or(0),
                duration_seconds: row
                    .try_get::<Option<f64>, _>("duration_seconds")
                    .ok()
                    .flatten(),
            })
            .collect();

        Ok(runs)
    }

    async fn count_reconciliation_runs(&self) -> AppResult<i64> {
        let result: (i64,) = sqlx::query_as(
            r#"
            SELECT COUNT(DISTINCT created_at::DATE) AS count
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
                    COUNT(DISTINCT created_at::DATE) AS total_runs,
                    COUNT(*) AS total_checked,
                    SUM(CASE WHEN discrepancy != 'NONE' THEN 1 ELSE 0 END) AS total_discrepancies,
                    SUM(CASE WHEN discrepancy != 'NONE' AND resolved_at IS NULL THEN 1 ELSE 0 END) AS total_unresolved
                FROM reconciliation_log
            )
            SELECT
                total_runs,
                total_checked,
                COALESCE(total_discrepancies, 0) AS total_discrepancies,
                COALESCE(total_unresolved, 0) AS total_unresolved
            FROM stats
            "#,
        )
        .fetch_one(&self.pool)
        .await?;

        let successful_reconciliations = sqlx::query_as::<_, (i64,)>(
            r#"
            SELECT COUNT(DISTINCT created_at::DATE)
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
                COUNT(*) AS count
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
                let discrepancy_type: String = row.try_get("discrepancy").unwrap_or_default();
                let count: i64 = row.try_get("count").unwrap_or(0);
                let percentage = if total_discrepancies > 0 {
                    (count as f64 / total_discrepancies as f64) * 100.0
                } else {
                    0.0
                };

                DiscrepancyTypeStats {
                    discrepancy_type: match discrepancy_type.as_str() {
                        "NONE" => "none".to_string(),
                        "MISSING_TX" => "missing_position".to_string(),
                        "AMOUNT_MISMATCH" => "pnl_mismatch".to_string(),
                        "STATE_MISMATCH" => "state_mismatch".to_string(),
                        "COST_MISMATCH" => "cost_mismatch".to_string(),
                        other => other.to_lowercase(),
                    },
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
                resolved_by = $1,
                notes = COALESCE(notes || '; ', '') || $2
            WHERE id = $3 AND resolved_at IS NULL
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
    // TRADE QUERIES
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
                   total_cost_sol, net_pnl_sol, pnl_data_valid, created_at, updated_at
            FROM trades
            WHERE 1=1
            "#,
        );

        let next = append_trade_filter_clauses(
            &mut query,
            from_date,
            to_date,
            status_filter,
            strategy_filter,
            wallet_address_filter,
        );

        let limit_n = next;
        let offset_n = if limit < 0 { next } else { next + 1 };
        if limit < 0 {
            query.push_str(&format!(
                " ORDER BY created_at DESC LIMIT ALL OFFSET ${offset_n}"
            ));
        } else {
            query.push_str(&format!(
                " ORDER BY created_at DESC LIMIT ${limit_n} OFFSET ${offset_n}"
            ));
        }

        let mut q = sqlx::query(&query);
        if let Some(from) = from_date {
            q = q.bind(from);
        }
        if let Some(to) = to_date {
            q = q.bind(to);
        }
        if let Some(status) = status_filter {
            q = q.bind(status);
        }
        if let Some(strategy) = strategy_filter {
            q = q.bind(strategy);
        }
        if let Some(wallet) = wallet_address_filter {
            q = q.bind(wallet);
        }
        if limit >= 0 {
            q = q.bind(limit);
        }
        q = q.bind(offset);

        let rows = q.fetch_all(&self.pool).await?;
        let trades: Vec<TradeDetail> = rows
            .into_iter()
            .map(|row| TradeDetail {
                id: row.try_get("id").unwrap_or(0),
                trade_uuid: row.try_get("trade_uuid").unwrap_or_default(),
                wallet_address: row.try_get("wallet_address").unwrap_or_default(),
                token_address: row.try_get("token_address").unwrap_or_default(),
                token_symbol: row.try_get("token_symbol").ok(),
                strategy: row.try_get("strategy").unwrap_or_default(),
                side: row.try_get("side").unwrap_or_default(),
                amount_sol: row
                    .try_get::<Decimal, _>("amount_sol")
                    .unwrap_or(Decimal::ZERO),
                price_at_signal: row
                    .try_get::<Option<Decimal>, _>("price_at_signal")
                    .ok()
                    .flatten(),
                tx_signature: row.try_get("tx_signature").ok(),
                status: row.try_get("status").unwrap_or_default(),
                retry_count: row.try_get("retry_count").unwrap_or(0),
                error_message: row.try_get("error_message").ok(),
                pnl_sol: row.try_get::<Option<Decimal>, _>("pnl_sol").ok().flatten(),
                pnl_usd: row.try_get::<Option<Decimal>, _>("pnl_usd").ok().flatten(),
                jito_tip_sol: row
                    .try_get::<Option<Decimal>, _>("jito_tip_sol")
                    .ok()
                    .flatten(),
                dex_fee_sol: row
                    .try_get::<Option<Decimal>, _>("dex_fee_sol")
                    .ok()
                    .flatten(),
                slippage_cost_sol: row
                    .try_get::<Option<Decimal>, _>("slippage_cost_sol")
                    .ok()
                    .flatten(),
                total_cost_sol: row
                    .try_get::<Option<Decimal>, _>("total_cost_sol")
                    .ok()
                    .flatten(),
                net_pnl_sol: row
                    .try_get::<Option<Decimal>, _>("net_pnl_sol")
                    .ok()
                    .flatten(),
                pnl_data_valid: row.try_get::<bool, _>("pnl_data_valid").unwrap_or(true),
                created_at: row
                    .try_get::<chrono::DateTime<chrono::Utc>, _>("created_at")
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_else(|_| chrono::Utc::now().to_rfc3339()),
                updated_at: row
                    .try_get::<chrono::DateTime<chrono::Utc>, _>("updated_at")
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_else(|_| chrono::Utc::now().to_rfc3339()),
            })
            .collect();
        Ok(trades)
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

        let _next = append_trade_filter_clauses(
            &mut query,
            from_date,
            to_date,
            status_filter,
            strategy_filter,
            wallet_address_filter,
        );

        let mut q = sqlx::query_as::<_, (i64,)>(&query);
        if let Some(from) = from_date {
            q = q.bind(from);
        }
        if let Some(to) = to_date {
            q = q.bind(to);
        }
        if let Some(status) = status_filter {
            q = q.bind(status);
        }
        if let Some(strategy) = strategy_filter {
            q = q.bind(strategy);
        }
        if let Some(wallet) = wallet_address_filter {
            q = q.bind(wallet);
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
        // Single atomic UPDATE: each column accumulates against the latest
        // committed value, so concurrent callers cannot lose each other's
        // increments (a read-modify-write would).
        let result = sqlx::query(
            r#"
            UPDATE trades
            SET jito_tip_sol = COALESCE(jito_tip_sol, 0) + $1,
                dex_fee_sol = COALESCE(dex_fee_sol, 0) + $2,
                slippage_cost_sol = COALESCE(slippage_cost_sol, 0) + $3,
                total_cost_sol = COALESCE(total_cost_sol, 0) + ($1 + $2 + $3)
            WHERE trade_uuid = $4
            "#,
        )
        .bind(jito_tip_sol)
        .bind(dex_fee_sol)
        .bind(slippage_cost_sol)
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
            SET status = 'DEAD_LETTER', error_message = $1
            WHERE trade_uuid = $2
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
            INSERT INTO dead_letter_queue (trade_uuid, payload, reason, error_details, can_retry)
            VALUES ($1, $2, 'DEAD_LETTER', $3,
                NOT ($3 ILIKE '%liquidity%'
                  OR $3 ILIKE '%No active position%'
                  OR $3 ILIKE '%rug%'
                  OR $3 ILIKE '%honeypot%'
                  OR $3 ILIKE '%rejected%'
                  OR $3 ILIKE '%below minimum%'
                  OR $3 ILIKE '%mint authority%'
                  OR $3 ILIKE '%freeze authority%'
                  OR $3 ILIKE '%not speculative%'
                  OR $3 ILIKE '%Amount%below minimum%'
                  OR $3 ILIKE '%allocation limit%'
                  OR $3 ILIKE '%Cost efficiency%'
                  OR $3 ILIKE '%total exposure%'
                  OR $3 ILIKE '%portfolio heat%'
                  OR $3 ILIKE '%Missing token_address%'))
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

    async fn count_trades_by_status(&self, status: &str) -> AppResult<i64> {
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM trades WHERE status = $1")
            .bind(status)
            .fetch_one(&self.pool)
            .await?;
        Ok(count.0)
    }

    async fn get_closed_trade_count_for_wallet(&self, wallet_address: &str) -> AppResult<i64> {
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM trades WHERE wallet_address = $1 AND status = 'CLOSED'",
        )
        .bind(wallet_address)
        .fetch_one(&self.pool)
        .await?;
        Ok(count.0)
    }

    async fn get_wallet_copy_stats(
        &self,
        wallet_address: &str,
    ) -> AppResult<(i64, rust_decimal::Decimal)> {
        let row = sqlx::query(
            "SELECT COUNT(*) FILTER (WHERE status = 'CLOSED')::bigint AS closed_trades, \
                    COALESCE(SUM(net_pnl_sol) FILTER (WHERE status = 'CLOSED'), 0) AS net_pnl_sol \
             FROM trades WHERE wallet_address = $1",
        )
        .bind(wallet_address)
        .fetch_one(&self.pool)
        .await?;
        let closed_trades: i64 = row.try_get("closed_trades")?;
        let net_pnl_sol: rust_decimal::Decimal = row.try_get("net_pnl_sol")?;
        Ok((closed_trades, net_pnl_sol))
    }

    async fn get_token_mirror_avg_pnl(
        &self,
        token_address: &str,
        window_hours: i32,
        min_samples: i32,
    ) -> AppResult<Option<rust_decimal::Decimal>> {
        let avg: Option<(rust_decimal::Decimal,)> = sqlx::query_as(
            // Dedup (2026-08-14): keep one exit per (wallet, hour).
            // Repeat whale BUY signals opened up to 14 shadow positions per
            // token in under a minute, multiplying every moonshot exit N
            // times and inflating this gate 7-14x. no_price exits book zero
            // PnL and are excluded so they neither dilute the average nor
            // shadow a real position in the same hour bucket.
            r#"WITH dedup AS (
                 SELECT DISTINCT ON (sp.wallet_address, date_trunc('hour', sp.opened_at))
                        se.pnl_pct
                 FROM shadow_exits se
                 JOIN shadow_positions sp ON sp.shadow_id = se.shadow_id
                 WHERE sp.token_address = $1
                   AND se.exit_strategy = 'mirror_main'
                   AND se.exit_reason IS DISTINCT FROM 'no_price'
                   AND sp.opened_at > NOW() - ($2 || ' hours')::interval
                 ORDER BY sp.wallet_address, date_trunc('hour', sp.opened_at), sp.opened_at
               )
               SELECT AVG(pnl_pct) FROM dedup HAVING COUNT(*) >= $3"#,
        )
        .bind(token_address)
        .bind(window_hours)
        .bind(min_samples)
        .fetch_optional(&self.pool)
        .await?;
        Ok(avg.map(|a| a.0))
    }

    async fn get_wallet_pnl_statistics(
        &self,
        wallet_address: &str,
        window_days: i32,
    ) -> AppResult<Option<(i64, rust_decimal::Decimal, rust_decimal::Decimal)>> {
        let row: Option<(i64, rust_decimal::Decimal, rust_decimal::Decimal)> = sqlx::query_as(
            // Dedup (2026-08-14): one exit per (token, strategy, hour) — see
            // get_token_mirror_avg_pnl. This feeds the t-stat gate and the
            // shadow total-PnL proven branch; duplicate positions multiplied
            // both mean×n totals and sample counts, admitting wallets whose
            // only "edge" was re-signaling a pumping token. The strategy is
            // part of the key: mirror_main and dune_wallet rows on the same
            // token+hour are independent evidence, not duplicates.
            // no_price exits are excluded (zero-PnL distortions of
            // mean/stddev).
            r#"WITH dedup AS (
                 SELECT DISTINCT ON (sp.token_address, se.exit_strategy, date_trunc('hour', sp.opened_at))
                        se.pnl_pct
                 FROM shadow_exits se
                 JOIN shadow_positions sp ON sp.shadow_id = se.shadow_id
                 WHERE sp.wallet_address = $1
                   AND se.exit_strategy IN ('mirror_main', 'dune_wallet')
                   AND se.exit_reason IS DISTINCT FROM 'no_price'
                   AND sp.opened_at > NOW() - ($2 || ' days')::interval
                 ORDER BY sp.token_address, se.exit_strategy, date_trunc('hour', sp.opened_at), sp.opened_at
               )
               SELECT COUNT(*)::bigint AS n,
                      COALESCE(AVG(pnl_pct), 0) AS mean,
                      COALESCE(STDDEV(pnl_pct), 0) AS stddev
               FROM dedup
               HAVING COUNT(*) > 0"#,
        )
        .bind(wallet_address)
        .bind(window_days)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
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
            WHERE wallet_address = $1
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
                    .unwrap_or_else(|_| wallet_address.to_string()),
                copy_pnl_7d: r.try_get::<Decimal, _>("copy_pnl_7d").unwrap_or_default(),
                copy_pnl_30d: r.try_get::<Decimal, _>("copy_pnl_30d").unwrap_or_default(),
                signal_success_rate: r
                    .try_get::<Decimal, _>("signal_success_rate")
                    .unwrap_or_default(),
                avg_return_per_trade: r
                    .try_get::<Decimal, _>("avg_return_per_trade")
                    .unwrap_or_default(),
                total_trades: r.try_get("total_trades").unwrap_or(0),
                winning_trades: r.try_get("winning_trades").unwrap_or(0),
                last_updated: r
                    .try_get::<chrono::DateTime<chrono::Utc>, _>("last_updated")
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            })),
            None => Ok(None),
        }
    }

    // ========================================================================
    // DLQ / CONFIG AUDIT
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
            VALUES ($1, $2, $3, $4, $5)
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

    async fn get_dead_letter_entries(
        &self,
        limit: i32,
        offset: i32,
    ) -> AppResult<Vec<DeadLetterItem>> {
        let rows = sqlx::query(
            "SELECT id, trade_uuid, payload, reason, error_details, source_ip, retry_count, can_retry, received_at, processed_at FROM dead_letter_queue ORDER BY received_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        let items = rows
            .into_iter()
            .map(|row| DeadLetterItem {
                id: row.try_get("id").unwrap_or(0),
                trade_uuid: row.try_get("trade_uuid").ok(),
                payload: row.try_get("payload").unwrap_or_default(),
                reason: row.try_get("reason").unwrap_or_default(),
                error_details: row.try_get("error_details").ok(),
                source_ip: row.try_get("source_ip").ok(),
                retry_count: row.try_get::<i32, _>("retry_count").unwrap_or(0),
                can_retry: row.try_get::<bool, _>("can_retry").unwrap_or(true),
                received_at: row
                    .try_get::<chrono::DateTime<chrono::Utc>, _>("received_at")
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
                processed_at: row
                    .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("processed_at")
                    .ok()
                    .flatten()
                    .map(|dt| dt.to_rfc3339()),
            })
            .collect();

        Ok(items)
    }

    async fn count_dead_letter_entries(&self) -> AppResult<i64> {
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM dead_letter_queue")
            .fetch_one(&self.pool)
            .await?;
        Ok(count.0)
    }

    async fn get_dead_letter_entry(&self, trade_uuid: &str) -> AppResult<Option<DeadLetterItem>> {
        let row = sqlx::query(
            "SELECT id, trade_uuid, payload, reason, error_details, source_ip, retry_count, can_retry, received_at, processed_at FROM dead_letter_queue WHERE trade_uuid = $1",
        )
        .bind(trade_uuid)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => Ok(Some(DeadLetterItem {
                id: row.try_get("id").unwrap_or(0),
                trade_uuid: row.try_get("trade_uuid").ok(),
                payload: row.try_get("payload").unwrap_or_default(),
                reason: row.try_get("reason").unwrap_or_default(),
                error_details: row.try_get("error_details").ok(),
                source_ip: row.try_get("source_ip").ok(),
                retry_count: row.try_get::<i32, _>("retry_count").unwrap_or(0),
                can_retry: row.try_get::<bool, _>("can_retry").unwrap_or(true),
                received_at: row
                    .try_get::<chrono::DateTime<chrono::Utc>, _>("received_at")
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
                processed_at: row
                    .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("processed_at")
                    .ok()
                    .flatten()
                    .map(|dt| dt.to_rfc3339()),
            })),
            None => Ok(None),
        }
    }

    async fn get_retryable_dlq_items(&self, limit: i64) -> AppResult<Vec<RetryableDlqItem>> {
        let rows = sqlx::query(
            "SELECT trade_uuid, payload, retry_count FROM dead_letter_queue WHERE can_retry = TRUE AND processed_at IS NULL LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| RetryableDlqItem {
                trade_uuid: row.try_get("trade_uuid").unwrap_or_default(),
                payload: row.try_get("payload").unwrap_or_default(),
                retry_count: row.try_get::<i32, _>("retry_count").unwrap_or(0) as i64,
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
                "UPDATE dead_letter_queue SET retry_count = $1, can_retry = $2, processed_at = CURRENT_TIMESTAMP WHERE trade_uuid = $3 AND processed_at IS NULL",
            )
            .bind(retry_count)
            .bind(can_retry)
            .bind(trade_uuid)
            .execute(&self.pool)
            .await?;
        } else {
            sqlx::query(
                "UPDATE dead_letter_queue SET retry_count = $1, can_retry = $2 WHERE trade_uuid = $3 AND processed_at IS NULL",
            )
            .bind(retry_count)
            .bind(can_retry)
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
                    "UPDATE dead_letter_queue SET retry_count = $1, can_retry = $2, processed_at = CURRENT_TIMESTAMP WHERE trade_uuid = $3 AND processed_at IS NULL",
                )
                .bind(item.retry_count)
                .bind(item.can_retry)
                .bind(&item.trade_uuid)
                .execute(&mut *tx)
                .await?
            } else {
                sqlx::query(
                    "UPDATE dead_letter_queue SET retry_count = $1, can_retry = $2 WHERE trade_uuid = $3 AND processed_at IS NULL",
                )
                .bind(item.retry_count)
                .bind(item.can_retry)
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
            "SELECT id, key, old_value, new_value, changed_by, change_reason, changed_at FROM config_audit ORDER BY changed_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        let items = rows
            .into_iter()
            .map(|row| ConfigAuditItem {
                id: row.try_get("id").unwrap_or(0),
                key: row.try_get("key").unwrap_or_default(),
                old_value: row.try_get("old_value").ok(),
                new_value: row.try_get("new_value").unwrap_or_default(),
                changed_by: row.try_get("changed_by").unwrap_or_default(),
                change_reason: row.try_get("change_reason").ok(),
                changed_at: row
                    .try_get::<chrono::DateTime<chrono::Utc>, _>("changed_at")
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            })
            .collect();

        Ok(items)
    }

    async fn get_config_audit_entries_by_key_prefix(
        &self,
        prefix: &str,
        limit: i32,
    ) -> AppResult<Vec<ConfigAuditItem>> {
        let rows = sqlx::query(
            "SELECT id, key, old_value, new_value, changed_by, change_reason, changed_at FROM config_audit WHERE key LIKE $1 || '%' ORDER BY changed_at DESC LIMIT $2",
        )
        .bind(prefix)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let items = rows
            .into_iter()
            .map(|row| ConfigAuditItem {
                id: row.try_get("id").unwrap_or(0),
                key: row.try_get("key").unwrap_or_default(),
                old_value: row.try_get("old_value").ok(),
                new_value: row.try_get("new_value").unwrap_or_default(),
                changed_by: row.try_get("changed_by").unwrap_or_default(),
                change_reason: row.try_get("change_reason").ok(),
                changed_at: row
                    .try_get::<chrono::DateTime<chrono::Utc>, _>("changed_at")
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
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
    // TRADE LATENCY
    // ========================================================================

    async fn get_trade_latency_stats(&self, hours: i32) -> AppResult<TradeLatencyStats> {
        let latencies: Vec<f64> = sqlx::query_scalar(
            r#"
            SELECT EXTRACT(EPOCH FROM (updated_at - created_at)) * 1000.0
             FROM trades
             WHERE status = 'CLOSED'
             AND created_at >= NOW() - make_interval(hours => $1)
             AND updated_at IS NOT NULL
             AND updated_at > created_at
            "#,
        )
        .bind(hours)
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
        if bucket_bounds.is_empty() {
            return Ok(vec![]);
        }

        // Single-pass bucketing: scan the filtered trades ONCE, compute the latency
        // per row in a subquery, then aggregate every bucket with COUNT(*) FILTER in
        // the same pass. Previously this issued one COUNT query per bucket bound (N+1),
        // re-scanning and recomputing the latency expression each time.
        //
        // Param layout: $1 = hours; $2..$(n+1) = bucket_bounds (upper bounds, in order).
        // bucket i covers [lower_i, upper_i): lower_0 = 0.0, lower_i = bound_{i-1}.
        // The final bucket is open-ended (>= last bound) so the highest latencies
        // are never left uncounted.
        let mut select_parts: Vec<String> = Vec::with_capacity(bucket_bounds.len() + 1);
        select_parts.push("COUNT(*) AS total".to_string());
        for (i, _upper) in bucket_bounds.iter().enumerate() {
            let lower_sql = if i == 0 {
                "0.0".to_string()
            } else {
                format!("${}", i + 1)
            };
            let filter = if i == bucket_bounds.len() - 1 {
                format!("COUNT(*) FILTER (WHERE lat >= {lower_sql})")
            } else {
                let upper_sql = format!("${}", i + 2);
                format!("COUNT(*) FILTER (WHERE lat >= {lower_sql} AND lat < {upper_sql})")
            };
            select_parts.push(filter);
        }

        let sql = format!(
            r#"
            SELECT {select}
            FROM (
                SELECT EXTRACT(EPOCH FROM (updated_at - created_at)) * 1000.0 AS lat
                FROM trades
                WHERE status = 'CLOSED'
                  AND created_at >= NOW() - make_interval(hours => $1)
                  AND updated_at IS NOT NULL
            ) t
            "#,
            select = select_parts.join(",\n    ")
        );

        let mut query = sqlx::query(&sql).bind(hours);
        for &bound in bucket_bounds {
            query = query.bind(bound);
        }

        let row = query
            .fetch_one(&self.pool)
            .await
            .map_err(AppError::Database)?;
        let total: i64 = row.try_get("total").unwrap_or(0);
        if total == 0 {
            return Ok(vec![]);
        }

        let mut buckets = Vec::with_capacity(bucket_bounds.len());
        let mut lower_bound = 0.0;
        for (i, &upper_bound) in bucket_bounds.iter().enumerate() {
            // Column index 0 is `total`; bucket counts start at index 1.
            let count: i64 = row.try_get(i + 1).unwrap_or(0);
            let percentage = (count as f64 / total as f64) * 100.0;
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
                    entry_amount_sol: row
                        .try_get::<Decimal, _>("entry_amount_sol")
                        .unwrap_or(Decimal::ZERO),
                    entry_price: row
                        .try_get::<Decimal, _>("entry_price")
                        .unwrap_or(Decimal::ZERO),
                    entry_tx_signature: row.try_get("entry_tx_signature").unwrap_or_default(),
                    current_price: row
                        .try_get::<Option<Decimal>, _>("current_price")
                        .ok()
                        .flatten(),
                    unrealized_pnl_sol: row
                        .try_get::<Option<Decimal>, _>("unrealized_pnl_sol")
                        .ok()
                        .flatten(),
                    unrealized_pnl_percent: row
                        .try_get::<Option<Decimal>, _>("unrealized_pnl_percent")
                        .ok()
                        .flatten(),
                    state: row.try_get("state").unwrap_or_default(),
                    exit_price: row
                        .try_get::<Option<Decimal>, _>("exit_price")
                        .ok()
                        .flatten(),
                    exit_tx_signature: row.try_get("exit_tx_signature").ok(),
                    realized_pnl_sol: row
                        .try_get::<Option<Decimal>, _>("realized_pnl_sol")
                        .ok()
                        .flatten(),
                    realized_pnl_usd: row
                        .try_get::<Option<Decimal>, _>("realized_pnl_usd")
                        .ok()
                        .flatten(),
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

    async fn get_whale_buy_prices(
        &self,
        wallet_address: &str,
        token_address: &str,
        window_hours: i64,
    ) -> AppResult<Vec<Decimal>> {
        let rows = sqlx::query(
            r#"SELECT entry_price_usd
               FROM shadow_positions
               WHERE wallet_address = $1
                 AND token_address = $2
                 AND entry_price_usd IS NOT NULL
                 AND entry_price_usd > 0
                 AND opened_at > NOW() - (INTERVAL '1 hour' * $3)
               ORDER BY opened_at ASC"#,
        )
        .bind(wallet_address)
        .bind(token_address)
        .bind(window_hours)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| row.try_get("entry_price_usd").unwrap_or(Decimal::ZERO))
            .filter(|p| *p > Decimal::ZERO)
            .collect())
    }

    async fn has_recent_net_loss(
        &self,
        token_address: &str,
        window_hours: i64,
        loss_threshold_pct: Decimal,
    ) -> AppResult<bool> {
        let exists: bool = sqlx::query_scalar(
            r#"SELECT EXISTS(
                 SELECT 1 FROM positions
                 WHERE token_address = $1
                   AND state = 'CLOSED'
                   AND closed_at > NOW() - (INTERVAL '1 hour' * $2)
                   AND COALESCE(realized_net_pnl_sol, realized_pnl_sol, 0)
                       < -entry_amount_sol * $3 / 100
               )"#,
        )
        .bind(token_address)
        .bind(window_hours)
        .bind(loss_threshold_pct)
        .fetch_one(&self.pool)
        .await?;
        Ok(exists)
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
                    wqs_score: row
                        .try_get::<Option<Decimal>, _>("wqs_score")
                        .ok()
                        .flatten(),
                    roi_7d: row.try_get::<Option<Decimal>, _>("roi_7d").ok().flatten(),
                    roi_30d: row.try_get::<Option<Decimal>, _>("roi_30d").ok().flatten(),
                    trade_count_30d: row.try_get("trade_count_30d").ok(),
                    win_rate: row.try_get::<Option<Decimal>, _>("win_rate").ok().flatten(),
                    max_drawdown_30d: row
                        .try_get::<Option<Decimal>, _>("max_drawdown_30d")
                        .ok()
                        .flatten(),
                    avg_trade_size_sol: row
                        .try_get::<Option<Decimal>, _>("avg_trade_size_sol")
                        .ok()
                        .flatten(),
                    avg_win_sol: row
                        .try_get::<Option<Decimal>, _>("avg_win_sol")
                        .ok()
                        .flatten(),
                    avg_loss_sol: row
                        .try_get::<Option<Decimal>, _>("avg_loss_sol")
                        .ok()
                        .flatten(),
                    profit_factor: row
                        .try_get::<Option<Decimal>, _>("profit_factor")
                        .ok()
                        .flatten(),
                    realized_pnl_30d_sol: row
                        .try_get::<Option<Decimal>, _>("realized_pnl_30d_sol")
                        .ok()
                        .flatten(),
                    last_trade_at: row.try_get("last_trade_at").ok(),
                    promoted_at: row.try_get("promoted_at").ok(),
                    ttl_expires_at: row.try_get("ttl_expires_at").ok(),
                    notes: row.try_get("notes").ok(),
                    archetype: row.try_get("archetype").ok(),
                    avg_entry_delay_seconds: row
                        .try_get::<Option<Decimal>, _>("avg_entry_delay_seconds")
                        .ok()
                        .flatten(),
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
            r#"SELECT COALESCE(SUM(realized_pnl_sol), 0.0) FROM positions WHERE state = 'CLOSED' AND pnl_data_valid AND closed_at >= NOW() - INTERVAL '24 hours'"#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::Database)?;

        let realized_usd: Decimal = sqlx::query_scalar::<_, Decimal>(
            r#"SELECT COALESCE(SUM(realized_pnl_usd), 0.0) FROM positions WHERE state = 'CLOSED' AND pnl_data_valid AND closed_at >= NOW() - INTERVAL '24 hours' AND realized_pnl_usd IS NOT NULL"#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::Database)?;

        let null_price_pnl_sol: Decimal = sqlx::query_scalar::<_, Decimal>(
            r#"SELECT COALESCE(SUM(realized_pnl_sol), 0.0) FROM positions WHERE state = 'CLOSED' AND pnl_data_valid AND closed_at >= NOW() - INTERVAL '24 hours' AND realized_pnl_usd IS NULL"#,
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

    /// Atomic operation: Insert trade and create position in a single transaction
    async fn insert_trade_and_create_position(
        &self,
        trade: &InsertTrade,
        position: &InsertPosition,
    ) -> AppResult<i64> {
        // Use PostgreSQL transaction for atomicity
        let mut tx = self.pool.begin().await?;

        // Insert trade
        let trade_id = sqlx::query(
            "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP) RETURNING id"
        )
        .bind(&trade.trade_uuid)
        .bind(&trade.wallet_address)
        .bind(&trade.token_address)
        .bind(&trade.strategy)
        .bind(&trade.side)
        .bind(trade.amount_sol)
        .bind(&trade.status)
        .fetch_one(&mut *tx)
        .await
        .map_err(AppError::Database)?
        .try_get("id")
        .map_err(AppError::Database)?;

        // Insert position
        sqlx::query(
            "INSERT INTO positions (trade_uuid, wallet_address, token_address, token_symbol, strategy, entry_amount_sol, entry_price, entry_tx_signature, state) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'ACTIVE')"
        )
        .bind(&trade.trade_uuid)
        .bind(&trade.wallet_address)
        .bind(&trade.token_address)
        .bind(&trade.token_symbol)
        .bind(&trade.strategy)
        .bind(position.entry_amount_sol)
        .bind(position.entry_price)
        .bind(&position.entry_tx_signature)
        .execute(&mut *tx)
        .await
        .map_err(AppError::Database)?;

        // Commit transaction
        tx.commit().await.map_err(|e| {
            AppError::Database(sqlx::Error::Io(std::io::Error::other(format!(
                "Failed to commit transaction: {}",
                e
            ))))
        })?;

        Ok(trade_id)
    }

    /// Atomic operation: Update trade status and position state in a single transaction
    async fn update_trade_status_and_position(
        &self,
        trade_uuid: &str,
        trade_status: &str,
        position_state: Option<&str>,
    ) -> AppResult<()> {
        // Use PostgreSQL transaction for atomicity
        let mut tx = self.pool.begin().await?;

        // Update trade status
        sqlx::query(
            "UPDATE trades SET status = $1, updated_at = CURRENT_TIMESTAMP, \
             closed_at = CASE WHEN $1 = 'CLOSED' THEN CURRENT_TIMESTAMP ELSE closed_at END \
             WHERE trade_uuid = $2",
        )
        .bind(trade_status)
        .bind(trade_uuid)
        .execute(&mut *tx)
        .await
        .map_err(AppError::Database)?;

        // Update position state if provided
        if let Some(state) = position_state {
            sqlx::query(
                "UPDATE positions SET state = $1, last_updated = CURRENT_TIMESTAMP WHERE trade_uuid = $2"
            )
            .bind(state)
            .bind(trade_uuid)
            .execute(&mut *tx)
            .await
            .map_err(AppError::Database)?;
        }

        // Commit transaction
        tx.commit().await.map_err(|e| {
            AppError::Database(sqlx::Error::Io(std::io::Error::other(format!(
                "Failed to commit transaction: {}",
                e
            ))))
        })?;

        Ok(())
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
            amount_sol: row
                .try_get::<Decimal, _>("amount_sol")
                .unwrap_or(Decimal::ZERO),
            price_at_signal: row
                .try_get::<Option<Decimal>, _>("price_at_signal")
                .ok()
                .flatten(),
            tx_signature: row.try_get("tx_signature").ok(),
            status: row.try_get("status").unwrap_or_default(),
            retry_count: row.try_get("retry_count").unwrap_or(0),
            error_message: row.try_get("error_message").ok(),
            pnl_sol: row.try_get::<Option<Decimal>, _>("pnl_sol").ok().flatten(),
            pnl_usd: row.try_get::<Option<Decimal>, _>("pnl_usd").ok().flatten(),
            jito_tip_sol: row
                .try_get::<Decimal, _>("jito_tip_sol")
                .unwrap_or(Decimal::ZERO),
            dex_fee_sol: row
                .try_get::<Decimal, _>("dex_fee_sol")
                .unwrap_or(Decimal::ZERO),
            slippage_cost_sol: row
                .try_get::<Decimal, _>("slippage_cost_sol")
                .unwrap_or(Decimal::ZERO),
            total_cost_sol: row
                .try_get::<Decimal, _>("total_cost_sol")
                .unwrap_or(Decimal::ZERO),
            net_pnl_sol: row
                .try_get::<Option<Decimal>, _>("net_pnl_sol")
                .ok()
                .flatten(),
            pnl_data_valid: row.try_get("pnl_data_valid").unwrap_or(true),
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
            entry_amount_sol: row
                .try_get::<Decimal, _>("entry_amount_sol")
                .unwrap_or(Decimal::ZERO),
            entry_price: row
                .try_get::<Decimal, _>("entry_price")
                .unwrap_or(Decimal::ZERO),
            entry_tx_signature: row.try_get("entry_tx_signature").unwrap_or_default(),
            current_price: row
                .try_get::<Option<Decimal>, _>("current_price")
                .ok()
                .flatten(),
            unrealized_pnl_sol: row
                .try_get::<Option<Decimal>, _>("unrealized_pnl_sol")
                .ok()
                .flatten(),
            unrealized_pnl_percent: row
                .try_get::<Option<Decimal>, _>("unrealized_pnl_percent")
                .ok()
                .flatten(),
            state: row.try_get("state").unwrap_or_default(),
            exit_price: row
                .try_get::<Option<Decimal>, _>("exit_price")
                .ok()
                .flatten(),
            exit_tx_signature: row.try_get("exit_tx_signature").ok(),
            realized_pnl_sol: row
                .try_get::<Option<Decimal>, _>("realized_pnl_sol")
                .ok()
                .flatten(),
            realized_pnl_usd: row
                .try_get::<Option<Decimal>, _>("realized_pnl_usd")
                .ok()
                .flatten(),
            entry_sol_price_usd: row
                .try_get::<Option<Decimal>, _>("entry_sol_price_usd")
                .ok()
                .flatten(),
            opened_at: row
                .try_get("opened_at")
                .unwrap_or_else(|_| chrono::Utc::now()),
            last_updated: row
                .try_get("last_updated")
                .unwrap_or_else(|_| chrono::Utc::now()),
            closed_at: row.try_get("closed_at").ok(),
            token_amount: row
                .try_get::<Option<Decimal>, _>("token_amount")
                .ok()
                .flatten(),
        })
    }

    fn row_to_wallet(&self, row: sqlx::postgres::PgRow) -> AppResult<Wallet> {
        // Helper to safely get Optional Decimal fields that may be stored as DOUBLE PRECISION
        let get_opt_decimal = |col: &str| -> Option<Decimal> {
            // Try as Decimal first (preferred), fall back to f64 -> Decimal conversion
            row.try_get::<Option<Decimal>, _>(col)
                .ok()
                .flatten()
                .or_else(|| {
                    row.try_get::<Option<f64>, _>(col)
                        .ok()
                        .flatten()
                        .map(|f| Decimal::from_f64_retain(f).unwrap_or(Decimal::ZERO))
                })
        };

        Ok(Wallet {
            id: row.try_get("id").unwrap_or(0),
            address: row.try_get("address").unwrap_or_default(),
            status: row.try_get("status").unwrap_or_default(),
            wqs_score: get_opt_decimal("wqs_score"),
            wqs_confidence: get_opt_decimal("wqs_confidence"),
            roi_7d: get_opt_decimal("roi_7d"),
            roi_30d: get_opt_decimal("roi_30d"),
            trade_count_30d: row.try_get("trade_count_30d").ok(),
            win_rate: get_opt_decimal("win_rate"),
            max_drawdown_30d: get_opt_decimal("max_drawdown_30d"),
            avg_trade_size_sol: get_opt_decimal("avg_trade_size_sol"),
            avg_win_sol: get_opt_decimal("avg_win_sol"),
            avg_loss_sol: get_opt_decimal("avg_loss_sol"),
            profit_factor: get_opt_decimal("profit_factor"),
            realized_pnl_30d_sol: get_opt_decimal("realized_pnl_30d_sol"),
            last_trade_at: row.try_get("last_trade_at").ok(),
            promoted_at: row.try_get("promoted_at").ok(),
            ttl_expires_at: row.try_get("ttl_expires_at").ok(),
            notes: row.try_get("notes").ok(),
            archetype: row.try_get("archetype").ok(),
            avg_entry_delay_seconds: get_opt_decimal("avg_entry_delay_seconds"),
            created_at: row
                .try_get("created_at")
                .unwrap_or_else(|_| chrono::Utc::now()),
            updated_at: row
                .try_get("updated_at")
                .unwrap_or_else(|_| chrono::Utc::now()),
        })
    }

    /// Build a `WalletMonitoring` from a query row. Native NUMERIC/BOOLEAN decoding.
    /// Takes a row reference so it can be used with iterator adapters.
    fn row_to_wallet_monitoring(row: &sqlx::postgres::PgRow) -> AppResult<WalletMonitoring> {
        Ok(WalletMonitoring {
            wallet_address: row.try_get("wallet_address").unwrap_or_default(),
            helius_webhook_id: row.try_get("helius_webhook_id").ok(),
            rpc_polling_active: row
                .try_get::<bool, _>("rpc_polling_active")
                .unwrap_or(false),
            last_transaction_signature: row.try_get("last_transaction_signature").ok(),
            last_monitored_at: row
                .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("last_monitored_at")
                .ok()
                .flatten()
                .map(|dt| dt.to_rfc3339()),
            monitoring_enabled: row
                .try_get::<bool, _>("monitoring_enabled")
                .unwrap_or(false),
            created_at: row
                .try_get::<chrono::DateTime<chrono::Utc>, _>("created_at")
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default(),
            updated_at: row
                .try_get::<chrono::DateTime<chrono::Utc>, _>("updated_at")
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default(),
            webhook_status: row.try_get("webhook_status").ok(),
            webhook_registered_at: row
                .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("webhook_registered_at")
                .ok()
                .flatten()
                .map(|dt| dt.to_rfc3339()),
            webhook_last_health_check: row
                .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("webhook_last_health_check")
                .ok()
                .flatten()
                .map(|dt| dt.to_rfc3339()),
            webhook_health_status: row.try_get("webhook_health_status").ok(),
            registration_attempts: row.try_get("registration_attempts").unwrap_or(0),
            last_registration_error: row.try_get("last_registration_error").ok(),
            last_updated_url: row.try_get("last_updated_url").ok(),
            last_speculative_signal_at: row
                .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("last_speculative_signal_at")
                .ok()
                .flatten()
                .map(|dt| dt.to_rfc3339()),
            inactivity_demotion_count: row.try_get("inactivity_demotion_count").unwrap_or(0),
        })
    }
}
