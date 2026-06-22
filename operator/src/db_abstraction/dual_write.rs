//! Dual-write database backend for zero-downtime migration
//!
//! This implementation writes to both SQLite and PostgreSQL backends simultaneously,
//! allowing for gradual migration with minimal disruption.

use super::{
    postgres::PostgresBackend, sqlite::SqliteBackend, Database, InsertPosition, InsertTrade,
    Position, Trade, UpdatePosition, UpdateTradeStatus, Wallet,
};
use crate::error::{AppError, AppResult};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{error, warn};

/// Configuration for dual-write backend
#[derive(Debug, Clone)]
pub struct DualWriteConfig {
    /// Whether to enable dual writes (writes to both backends)
    pub enable_dual_writes: bool,
    /// Whether to read from PostgreSQL (if false, reads from SQLite)
    pub read_from_postgres: bool,
    /// Whether to stop on PostgreSQL write errors (if false, logs and continues)
    pub stop_on_pg_error: bool,
}

impl Default for DualWriteConfig {
    fn default() -> Self {
        Self {
            enable_dual_writes: true,
            read_from_postgres: false, // Start reading from SQLite
            stop_on_pg_error: false,   // Don't stop on PG errors during migration
        }
    }
}

impl DualWriteConfig {
    /// Load configuration from environment variables
    pub fn from_env() -> Self {
        Self {
            enable_dual_writes: std::env::var("CHIMERA_DUAL_WRITE_ENABLE")
                .as_deref()
                .unwrap_or("true")
                .parse()
                .unwrap_or(true),
            read_from_postgres: std::env::var("CHIMERA_READ_FROM_POSTGRES")
                .as_deref()
                .unwrap_or("false")
                .parse()
                .unwrap_or(false),
            stop_on_pg_error: std::env::var("CHIMERA_STOP_ON_PG_ERROR")
                .as_deref()
                .unwrap_or("false")
                .parse()
                .unwrap_or(false),
        }
    }
}

/// Dual-write database backend
///
/// Writes to both SQLite and PostgreSQL simultaneously.
/// Reads from either based on configuration.
pub struct DualWriteBackend {
    sqlite: Arc<SqliteBackend>,
    postgres: Arc<PostgresBackend>,
    config: DualWriteConfig,
    /// Track whether PostgreSQL is healthy
    postgres_healthy: Arc<AtomicBool>,
}

impl DualWriteBackend {
    /// Create a new dual-write backend
    pub async fn new(
        sqlite: Arc<SqliteBackend>,
        postgres: Arc<PostgresBackend>,
        config: DualWriteConfig,
    ) -> Self {
        // Verify PostgreSQL connectivity
        let pg_healthy = postgres.startup_integrity_check().await.is_ok();

        Self {
            sqlite,
            postgres,
            config,
            postgres_healthy: Arc::new(AtomicBool::new(pg_healthy)),
        }
    }

    /// Get SQLite backend reference
    pub fn sqlite(&self) -> &Arc<SqliteBackend> {
        &self.sqlite
    }

    /// Get PostgreSQL backend reference
    pub fn postgres(&self) -> &Arc<PostgresBackend> {
        &self.postgres
    }

    /// Check if PostgreSQL backend is healthy
    pub fn is_postgres_healthy(&self) -> bool {
        self.postgres_healthy.load(Ordering::Relaxed)
    }

    /// Execute a write operation on both backends
    async fn dual_write<F, Fut, T>(
        &self,
        operation_name: &str,
        sqlite_op: F,
        postgres_op: F,
    ) -> AppResult<T>
    where
        F: Fn(Arc<dyn Database>) -> Fut,
        Fut: std::future::Future<Output = AppResult<T>>,
    {
        // Always write to SQLite (primary backend)
        let sqlite_result = sqlite_op(Arc::clone(&self.sqlite) as Arc<dyn Database>).await;

        // Write to PostgreSQL if dual-writes are enabled and PG is healthy
        if self.config.enable_dual_writes && self.is_postgres_healthy() {
            let pg_result = postgres_op(Arc::clone(&self.postgres) as Arc<dyn Database>).await;

            // Handle PostgreSQL write result
            match &pg_result {
                Ok(_) => {
                    // Success - both backends written
                    tracing::debug!(
                        operation = operation_name,
                        "Dual-write succeeded for both backends"
                    );
                }
                Err(e) => {
                    // PostgreSQL write failed
                    self.postgres_healthy.store(false, Ordering::Relaxed);
                    error!(
                        operation = operation_name,
                        error = %e,
                        "PostgreSQL write failed, marking unhealthy"
                    );

                    if self.config.stop_on_pg_error {
                        return Err(e.clone());
                    }
                }
            }
        }

        sqlite_result
    }

    /// Execute a read operation from the appropriate backend
    async fn read<F, Fut, T>(&self, op: F) -> AppResult<T>
    where
        F: Fn(Arc<dyn Database>) -> Fut,
        Fut: std::future::Future<Output = AppResult<T>>,
    {
        if self.config.read_from_postgres && self.is_postgres_healthy() {
            op(Arc::clone(&self.postgres) as Arc<dyn Database>).await
        } else {
            op(Arc::clone(&self.sqlite) as Arc<dyn Database>).await
        }
    }

    /// Periodic health check for PostgreSQL
    pub async fn health_check(&self) {
        let result = self.postgres.startup_integrity_check().await;
        self.postgres_healthy.store(result.is_ok(), Ordering::Relaxed);

        if !self.is_postgres_healthy() {
            warn!("PostgreSQL health check failed");
        }
    }

    /// Enable reading from PostgreSQL
    pub fn enable_postgres_reads(&self) {
        self.config.read_from_postgres = true;
        info!("Enabled reading from PostgreSQL");
    }

    /// Disable reading from PostgreSQL (fallback to SQLite)
    pub fn disable_postgres_reads(&self) {
        self.config.read_from_postgres = false;
        warn!("Disabled reading from PostgreSQL, using SQLite");
    }
}

#[async_trait::async_trait]
impl Database for DualWriteBackend {
    // ========================================================================
    // CONNECTION LIFECYCLE
    // ========================================================================

    async fn close(&self) -> AppResult<()> {
        // Close both backends
        let sqlite_result = self.sqlite.close().await;
        let postgres_result = self.postgres.close().await;

        // Return error if either failed
        sqlite_result?;
        postgres_result?;
        Ok(())
    }

    // ========================================================================
    // MIGRATION & STARTUP
    // ========================================================================

    async fn run_migrations(&self) -> AppResult<()> {
        // Run migrations on both backends
        self.dual_write(
            "run_migrations",
            |db| async move { db.run_migrations().await },
            |db| async move { db.run_migrations().await },
        )
        .await
    }

    async fn startup_integrity_check(&self) -> AppResult<()> {
        // Check SQLite integrity
        self.sqlite.startup_integrity_check().await?;

        // Check PostgreSQL connectivity (don't fail if PG is down)
        if self.config.enable_dual_writes {
            let _ = self.postgres.startup_integrity_check().await;
        }

        Ok(())
    }

    async fn recover_executing_trades(&self) -> AppResult<u32> {
        // Only recover on SQLite (avoid duplicate recovery)
        self.sqlite.recover_executing_trades().await
    }

    // ========================================================================
    // TRADE OPERATIONS
    // ========================================================================

    async fn trade_uuid_exists(&self, trade_uuid: &str) -> AppResult<bool> {
        self.read(|db| async move { db.trade_uuid_exists(trade_uuid).await })
            .await
    }

    async fn insert_trade(&self, trade: &InsertTrade) -> AppResult<i64> {
        self.dual_write(
            "insert_trade",
            |db| async move { db.insert_trade(trade).await },
            |db| async move { db.insert_trade(trade).await },
        )
        .await
    }

    async fn update_trade_status(&self, update: &UpdateTradeStatus) -> AppResult<()> {
        self.dual_write(
            "update_trade_status",
            |db| async move {
                db.update_trade_status(update).await?;
                Ok(())
            },
            |db| async move {
                db.update_trade_status(update).await?;
                Ok(())
            },
        )
        .await
    }

    async fn get_trade_by_uuid(&self, trade_uuid: &str) -> AppResult<Option<Trade>> {
        self.read(|db| async move { db.get_trade_by_uuid(trade_uuid).await })
            .await
    }

    async fn get_queued_trades(&self, limit: i32) -> AppResult<Vec<Trade>> {
        self.read(|db| async move { db.get_queued_trades(limit).await })
            .await
    }

    async fn get_trades_by_status(&self, status: &str, limit: i32) -> AppResult<Vec<Trade>> {
        self.read(|db| async move { db.get_trades_by_status(status, limit).await })
            .await
    }

    async fn update_trade_execution(
        &self,
        trade_uuid: &str,
        tx_signature: &str,
        jito_tip_sol: rust_decimal::Decimal,
        dex_fee_sol: rust_decimal::Decimal,
        slippage_cost_sol: rust_decimal::Decimal,
    ) -> AppResult<()> {
        self.dual_write(
            "update_trade_execution",
            |db| async move {
                db.update_trade_execution(
                    trade_uuid,
                    tx_signature,
                    jito_tip_sol,
                    dex_fee_sol,
                    slippage_cost_sol,
                )
                .await
            },
            |db| async move {
                db.update_trade_execution(
                    trade_uuid,
                    tx_signature,
                    jito_tip_sol,
                    dex_fee_sol,
                    slippage_cost_sol,
                )
                .await
            },
        )
        .await
    }

    async fn update_trade_pnl(
        &self,
        trade_uuid: &str,
        pnl_sol: rust_decimal::Decimal,
        pnl_usd: rust_decimal::Decimal,
    ) -> AppResult<()> {
        self.dual_write(
            "update_trade_pnl",
            |db| async move { db.update_trade_pnl(trade_uuid, pnl_sol, pnl_usd).await },
            |db| async move { db.update_trade_pnl(trade_uuid, pnl_sol, pnl_usd).await },
        )
        .await
    }

    // ========================================================================
    // POSITION OPERATIONS
    // ========================================================================

    async fn insert_position(&self, position: &InsertPosition) -> AppResult<i64> {
        self.dual_write(
            "insert_position",
            |db| async move { db.insert_position(position).await },
            |db| async move { db.insert_position(position).await },
        )
        .await
    }

    async fn update_position(&self, update: &UpdatePosition) -> AppResult<()> {
        self.dual_write(
            "update_position",
            |db| async move {
                db.update_position(update).await?;
                Ok(())
            },
            |db| async move {
                db.update_position(update).await?;
                Ok(())
            },
        )
        .await
    }

    async fn get_active_positions(&self) -> AppResult<Vec<Position>> {
        self.read(|db| async move { db.get_active_positions().await })
            .await
    }

    async fn get_position_by_trade_uuid(&self, trade_uuid: &str) -> AppResult<Option<Position>> {
        self.read(|db| async move { db.get_position_by_trade_uuid(trade_uuid).await })
            .await
    }

    async fn close_position(
        &self,
        trade_uuid: &str,
        exit_price: rust_decimal::Decimal,
        exit_tx_signature: &str,
        realized_pnl_sol: rust_decimal::Decimal,
        realized_pnl_usd: rust_decimal::Decimal,
    ) -> AppResult<()> {
        self.dual_write(
            "close_position",
            |db| async move {
                db.close_position(
                    trade_uuid,
                    exit_price,
                    exit_tx_signature,
                    realized_pnl_sol,
                    realized_pnl_usd,
                )
                .await
            },
            |db| async move {
                db.close_position(
                    trade_uuid,
                    exit_price,
                    exit_tx_signature,
                    realized_pnl_sol,
                    realized_pnl_usd,
                )
                .await
            },
        )
        .await
    }

    // ========================================================================
    // WALLET OPERATIONS
    // ========================================================================

    async fn get_wallet(&self, address: &str) -> AppResult<Option<Wallet>> {
        self.read(|db| async move { db.get_wallet(address).await })
            .await
    }

    async fn get_active_wallets(&self) -> AppResult<Vec<Wallet>> {
        self.read(|db| async move { db.get_active_wallets().await })
            .await
    }

    async fn update_wallet_status(&self, address: &str, status: &str) -> AppResult<()> {
        self.dual_write(
            "update_wallet_status",
            |db| async move {
                db.update_wallet_status(address, status).await?;
                Ok(())
            },
            |db| async move {
                db.update_wallet_status(address, status).await?;
                Ok(())
            },
        )
        .await
    }

    async fn merge_roster(&self, roster_db_path: &str) -> AppResult<u32> {
        // Merge to both backends
        self.dual_write(
            "merge_roster",
            |db| async move { db.merge_roster(roster_db_path).await },
            |db| async move { db.merge_roster(roster_db_path).await },
        )
        .await
    }

    async fn get_wallets_by_status(&self, status: &str) -> AppResult<Vec<Wallet>> {
        self.read(|db| async move { db.get_wallets_by_status(status).await })
            .await
    }

    // ========================================================================
    // SYSTEM OPERATIONS
    // ========================================================================

    async fn get_circuit_breaker_state(&self) -> AppResult<super::CircuitBreakerState> {
        self.read(|db| async move { db.get_circuit_breaker_state().await })
            .await
    }

    async fn update_circuit_breaker_state(
        &self,
        state: &str,
        tripped_at: Option<&str>,
        trip_reason: Option<&str>,
    ) -> AppResult<()> {
        self.dual_write(
            "update_circuit_breaker_state",
            |db| async move {
                db.update_circuit_breaker_state(state, tripped_at, trip_reason)
                    .await?;
                Ok(())
            },
            |db| async move {
                db.update_circuit_breaker_state(state, tripped_at, trip_reason)
                    .await?;
                Ok(())
            },
        )
        .await
    }

    async fn get_kill_switch_state(&self) -> AppResult<super::KillSwitchState> {
        self.read(|db| async move { db.get_kill_switch_state().await })
            .await
    }

    async fn set_kill_switch_state(&self, state: &str, reason: Option<&str>) -> AppResult<()> {
        self.dual_write(
            "set_kill_switch_state",
            |db| async move {
                db.set_kill_switch_state(state, reason).await?;
                Ok(())
            },
            |db| async move {
                db.set_kill_switch_state(state, reason).await?;
                Ok(())
            },
        )
        .await
    }

    async fn insert_dlq(
        &self,
        trade_uuid: Option<&str>,
        payload: &str,
        reason: &str,
        error_details: Option<&str>,
        source_ip: Option<&str>,
    ) -> AppResult<i64> {
        self.dual_write(
            "insert_dlq",
            |db| async move { db.insert_dlq(trade_uuid, payload, reason, error_details, source_ip).await },
            |db| async move { db.insert_dlq(trade_uuid, payload, reason, error_details, source_ip).await },
        )
        .await
    }

    async fn get_admin_wallet_role(&self, wallet_address: &str) -> AppResult<Option<String>> {
        self.read(|db| async move { db.get_admin_wallet_role(wallet_address).await })
            .await
    }

    // ========================================================================
    // STATISTICS & REPORTING
    // ========================================================================

    async fn get_trade_statistics(&self) -> AppResult<super::TradeStatistics> {
        self.read(|db| async move { db.get_trade_statistics().await })
            .await
    }

    async fn get_recent_trades(&self, limit: i64, offset: i64) -> AppResult<Vec<Trade>> {
        self.read(|db| async move { db.get_recent_trades(limit, offset).await })
            .await
    }

    async fn get_wallet_performance(
        &self,
        wallet_address: &str,
    ) -> AppResult<Option<super::WalletPerformance>> {
        self.read(|db| async move { db.get_wallet_performance(wallet_address).await })
            .await
    }
}
