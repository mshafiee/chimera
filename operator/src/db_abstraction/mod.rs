//! Database abstraction layer
//!
//! Provides a unified interface for both SQLite and PostgreSQL backends.
//! Select backend via CHIMERA_DB_BACKEND environment variable.

pub mod dual_write;
pub mod postgres;
pub mod sqlite;
pub mod types;

pub use types::{
    DatabaseBackend, DatabaseConfig, DbPool, InsertPosition, InsertTrade,
    UpdatePosition, UpdateTradeStatus,
};
pub use dual_write::{DualWriteBackend, DualWriteConfig};

use crate::error::{AppError, AppResult};
use std::sync::Arc;

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
    async fn get_wallet_performance(&self, wallet_address: &str) -> AppResult<Option<WalletPerformance>>;
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
    /// PostgreSQL only (production after cutover)
    PostgreSQLOnly,
    /// Dual-write mode (migration period - writes to both, reads from SQLite)
    DualWrite,
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
            "dual-write" | "dual_write" | "dual" => DatabaseMode::DualWrite,
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
        DatabaseMode::DualWrite => {
            tracing::info!("Using dual-write mode (SQLite + PostgreSQL)");

            // Create both backends
            let sqlite_config = DatabaseConfig::sqlite(config.path.clone());
            let postgres_config = DatabaseConfig::postgres(
                config
                    .url
                    .clone()
                    .ok_or_else(|| AppError::Internal("PostgreSQL URL required for dual-write mode".to_string()))?,
            );

            let sqlite = Arc::new(sqlite::SqliteBackend::new(&sqlite_config).await?);
            let postgres = Arc::new(postgres::PostgresBackend::new(&postgres_config).await?);

            // Create dual-write backend
            let dual_config = DualWriteConfig::from_env();
            Ok(Arc::new(DualWriteBackend::new(sqlite, postgres, dual_config).await))
        }
    }
}

// ========================================================================
// HELPERS
// ========================================================================

/// Convert f64 to Decimal safely
pub fn f64_to_decimal(val: f64) -> rust_decimal::Decimal {
    rust_decimal::Decimal::from_f64(val).unwrap_or(rust_decimal::Decimal::ZERO)
}

/// Convert Decimal to f64 safely
pub fn decimal_to_f64(val: rust_decimal::Decimal) -> f64 {
    val.to_string().parse::<f64>().unwrap_or(0.0)
}

/// Convert Option<f64> to Option<Decimal>
pub fn opt_f64_to_decimal(val: Option<f64>) -> Option<rust_decimal::Decimal> {
    val.map(f64_to_decimal)
}

/// Convert Option<Decimal> to Option<f64>
pub fn opt_decimal_to_f64(val: Option<rust_decimal::Decimal>) -> Option<f64> {
    val.map(decimal_to_f64)
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
