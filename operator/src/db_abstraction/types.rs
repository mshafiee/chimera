//! Shared types for database abstraction layer

use serde::{Deserialize, Serialize};
use sqlx::{Pool, Postgres, Sqlite};
use std::fmt::{self, Display, Formatter};

/// Database backend selection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseBackend {
    SQLite,
    PostgreSQL,
}

impl DatabaseBackend {
    /// Get database backend from environment variable
    /// Defaults to SQLite for development
    pub fn from_env() -> Self {
        match std::env::var("CHIMERA_DB_MODE")
            .as_deref()
            .unwrap_or("sqlite")
            .to_lowercase()
            .as_str()
        {
            "postgres" | "postgresql" => DatabaseBackend::PostgreSQL,
            _ => DatabaseBackend::SQLite,
        }
    }

    /// Get the default port for this database backend
    pub fn default_port(&self) -> u16 {
        match self {
            DatabaseBackend::SQLite => 0, // File-based, no port
            DatabaseBackend::PostgreSQL => 5432,
        }
    }
}

impl Display for DatabaseBackend {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            DatabaseBackend::SQLite => write!(f, "sqlite"),
            DatabaseBackend::PostgreSQL => write!(f, "postgresql"),
        }
    }
}

/// Type alias for SQLite connection pool
pub type SqlitePool = Pool<Sqlite>;

/// Type alias for PostgreSQL connection pool
pub type PostgresPool = Pool<Postgres>;

/// Database configuration
#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    pub backend: DatabaseBackend,
    pub path: std::path::PathBuf, // For SQLite
    pub url: Option<String>,      // For PostgreSQL
    pub max_connections: u32,
    pub acquire_timeout_seconds: u64,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            backend: DatabaseBackend::from_env(),
            path: std::path::PathBuf::from("data/chimera.db"),
            url: std::env::var("DATABASE_URL").ok(),
            max_connections: 10,
            acquire_timeout_seconds: 30,
        }
    }
}

impl DatabaseConfig {
    /// Create SQLite config with custom path
    pub fn sqlite(path: std::path::PathBuf) -> Self {
        Self {
            backend: DatabaseBackend::SQLite,
            path,
            ..Default::default()
        }
    }

    /// Create PostgreSQL config with URL
    pub fn postgres(url: String) -> Self {
        Self {
            backend: DatabaseBackend::PostgreSQL,
            url: Some(url),
            path: std::path::PathBuf::new(), // Not used for PostgreSQL
            ..Default::default()
        }
    }
}

/// Database pool enum (for type erasure)
pub enum DbPool {
    SQLite(SqlitePool),
    PostgreSQL(PostgresPool),
}

impl DbPool {
    /// Close the database pool
    pub async fn close(self) {
        match self {
            DbPool::SQLite(pool) => pool.close().await,
            DbPool::PostgreSQL(pool) => pool.close().await,
        }
    }

    /// Get pool size (total connections)
    pub fn size(&self) -> u32 {
        match self {
            DbPool::SQLite(pool) => pool.size(),
            DbPool::PostgreSQL(pool) => pool.size(),
        }
    }

    /// Get number of idle connections
    pub fn num_idle(&self) -> u32 {
        match self {
            DbPool::SQLite(pool) => pool.num_idle() as u32,
            DbPool::PostgreSQL(pool) => pool.num_idle() as u32,
        }
    }

    /// Get pool utilization as percentage (0.0-1.0)
    pub fn utilization(&self) -> f64 {
        let size = self.size() as f64;
        let idle = self.num_idle() as f64;
        if size > 0.0 {
            (size - idle) / size
        } else {
            0.0
        }
    }
}

/// Connection pool statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolStats {
    pub active_connections: u32,
    pub idle_connections: u32,
    pub max_connections: u32,
    pub utilization_percent: f64,
}

/// Trade insertion data
#[derive(Debug, Clone)]
pub struct InsertTrade {
    pub trade_uuid: String,
    pub wallet_address: String,
    pub token_address: String,
    pub token_symbol: Option<String>,
    pub strategy: String,
    pub side: String,
    pub amount_sol: rust_decimal::Decimal,
    pub status: String,
}

/// Trade status update data
#[derive(Debug, Clone)]
pub struct UpdateTradeStatus {
    pub trade_uuid: String,
    pub status: String,
    pub tx_signature: Option<String>,
    pub error_message: Option<String>,
    pub network_fee_sol: Option<rust_decimal::Decimal>,
}

/// Position insertion data
#[derive(Debug, Clone)]
pub struct InsertPosition {
    pub trade_uuid: String,
    pub wallet_address: String,
    pub token_address: String,
    pub token_symbol: Option<String>,
    pub strategy: String,
    pub entry_amount_sol: rust_decimal::Decimal,
    pub entry_price: rust_decimal::Decimal,
    pub entry_tx_signature: String,
}

/// Position update data
#[derive(Debug, Clone)]
pub struct UpdatePosition {
    pub trade_uuid: String,
    pub current_price: Option<rust_decimal::Decimal>,
    pub unrealized_pnl_sol: Option<rust_decimal::Decimal>,
    pub unrealized_pnl_percent: Option<rust_decimal::Decimal>,
    pub state: Option<String>,
    pub exit_price: Option<rust_decimal::Decimal>,
    pub exit_tx_signature: Option<String>,
    pub realized_pnl_sol: Option<rust_decimal::Decimal>,
    pub realized_pnl_usd: Option<rust_decimal::Decimal>,
}

/// Position record for stuck state recovery
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PositionRecord {
    pub id: i64,
    pub trade_uuid: String,
    pub wallet_address: String,
    pub token_address: String,
    pub strategy: String,
    pub state: String,
    pub entry_tx_signature: String,
    pub exit_tx_signature: Option<String>,
    pub last_updated: chrono::DateTime<chrono::Utc>,
}

/// Active position enriched with entry data for monitoring loop
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ActivePositionEntry {
    pub trade_uuid: String,
    pub wallet_address: String,
    pub token_address: String,
    pub token_symbol: String,
    pub strategy: String,
    pub entry_price: rust_decimal::Decimal,
    pub entry_amount_sol: rust_decimal::Decimal,
    pub entry_time: chrono::DateTime<chrono::Utc>,
}

/// Position with full details for API response
#[derive(Debug, Clone, serde::Serialize)]
pub struct PositionDetail {
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
    pub opened_at: String,
    pub last_updated: String,
    pub closed_at: Option<String>,
}

/// Lightweight summary for PnL refresh background task
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ActivePositionSummary {
    pub trade_uuid: String,
    pub token_address: String,
    pub entry_price: rust_decimal::Decimal,
    pub entry_amount_sol: rust_decimal::Decimal,
    pub entry_sol_price_usd: Option<rust_decimal::Decimal>,
}

/// Wallet with full details for API response
#[derive(Debug, Clone, serde::Serialize)]
pub struct WalletDetail {
    pub id: i64,
    pub address: String,
    pub status: String,
    pub wqs_score: Option<rust_decimal::Decimal>,
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
    pub last_trade_at: Option<String>,
    pub promoted_at: Option<String>,
    pub ttl_expires_at: Option<String>,
    pub notes: Option<String>,
    pub archetype: Option<String>,
    pub avg_entry_delay_seconds: Option<rust_decimal::Decimal>,
    pub created_at: String,
    pub updated_at: String,
}

/// Wallet copy performance metrics from database
#[derive(Debug, Clone, serde::Serialize)]
pub struct WalletCopyPerformance {
    pub wallet_address: String,
    pub copy_pnl_7d: rust_decimal::Decimal,
    pub copy_pnl_30d: rust_decimal::Decimal,
    pub signal_success_rate: rust_decimal::Decimal,
    pub avg_return_per_trade: rust_decimal::Decimal,
    pub total_trades: i32,
    pub winning_trades: i32,
    pub last_updated: String,
}

/// Wallet monitoring information from database
#[derive(Debug, Clone, serde::Serialize)]
pub struct WalletMonitoring {
    pub wallet_address: String,
    pub helius_webhook_id: Option<String>,
    pub rpc_polling_active: i32,
    pub last_transaction_signature: Option<String>,
    pub last_monitored_at: Option<String>,
    pub monitoring_enabled: i32,
    pub created_at: String,
    pub updated_at: String,
    pub webhook_status: Option<String>,
    pub webhook_registered_at: Option<String>,
    pub webhook_last_health_check: Option<String>,
    pub webhook_health_status: Option<String>,
    pub registration_attempts: i32,
    pub last_registration_error: Option<String>,
    pub last_updated_url: Option<String>,
}

/// Webhook monitoring record for comprehensive tracking
#[derive(Debug, Clone, serde::Serialize)]
pub struct WalletMonitoringExtended {
    pub wallet_address: String,
    pub helius_webhook_id: Option<String>,
    pub monitoring_enabled: i32,
    pub webhook_status: String,
    pub webhook_registered_at: Option<String>,
    pub webhook_last_health_check: Option<String>,
    pub webhook_health_status: String,
    pub registration_attempts: i32,
    pub last_registration_error: Option<String>,
    pub last_updated_url: Option<String>,
}

/// A retryable DLQ item (trade_uuid, payload, retry_count)
#[derive(Debug, Clone, serde::Serialize)]
pub struct RetryableDlqItem {
    pub trade_uuid: String,
    pub payload: String,
    pub retry_count: i64,
}

/// Parameters for batch updating DLQ items
#[derive(Debug, Clone)]
pub struct UpdateDlqItemParams {
    pub trade_uuid: String,
    pub retry_count: i64,
    pub can_retry: bool,
    pub mark_processed: bool,
}

/// Dead letter queue item
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeadLetterItem {
    pub id: i64,
    pub trade_uuid: Option<String>,
    pub payload: String,
    pub reason: String,
    pub error_details: Option<String>,
    pub source_ip: Option<String>,
    pub retry_count: i32,
    pub can_retry: bool,
    pub received_at: String,
    pub processed_at: Option<String>,
}

/// Config audit log entry
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConfigAuditItem {
    pub id: i64,
    pub key: String,
    pub old_value: Option<String>,
    pub new_value: String,
    pub changed_by: String,
    pub change_reason: Option<String>,
    pub changed_at: String,
}

/// Trade record for API response
#[derive(Debug, Clone, serde::Serialize)]
pub struct TradeDetail {
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
    pub jito_tip_sol: Option<rust_decimal::Decimal>,
    pub dex_fee_sol: Option<rust_decimal::Decimal>,
    pub slippage_cost_sol: Option<rust_decimal::Decimal>,
    pub total_cost_sol: Option<rust_decimal::Decimal>,
    pub net_pnl_sol: Option<rust_decimal::Decimal>,
    pub created_at: String,
    pub updated_at: String,
}

/// Webhook audit log record
#[derive(Debug, Clone, serde::Serialize)]
pub struct WebhookAuditLog {
    pub id: i64,
    pub wallet_address: String,
    pub action: String,
    pub status: String,
    pub webhook_id: Option<String>,
    pub details: Option<String>,
    pub error_message: Option<String>,
    pub duration_ms: Option<i32>,
    pub created_at: String,
}

/// Webhook statistics for monitoring
#[derive(Debug, Clone, serde::Serialize)]
pub struct WebhookStats {
    pub total_webhooks: usize,
    pub active_webhooks: usize,
    pub stale_webhooks: usize,
    pub failed_registrations: usize,
}

/// Wallet webhook eligibility result with profitability assessment
#[derive(Debug, Clone, serde::Serialize)]
pub struct WebhookEligibility {
    pub eligible: bool,
    pub wqs_score: Option<rust_decimal::Decimal>,
    pub confidence: rust_decimal::Decimal,
    pub status: String,
    pub archetype: String,
    pub trade_count: i64,
    pub roi_7d: Option<rust_decimal::Decimal>,
    pub roi_30d: Option<rust_decimal::Decimal>,
    pub reason: String,
}

/// Reconciliation discrepancy entry
#[derive(Debug, Clone, serde::Serialize)]
pub struct DiscrepancyRow {
    pub id: i64,
    pub trade_uuid: String,
    pub discrepancy_type: String,
    pub severity: String,
    pub description: String,
    pub db_value: Option<String>,
    pub on_chain_value: Option<String>,
    pub detected_at: String,
    pub resolved: bool,
    pub resolved_at: Option<String>,
}

/// Reconciliation status response
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReconciliationStatus {
    pub last_reconciliation_at: Option<String>,
    pub next_reconciliation_at: Option<String>,
    pub status: String,
    pub checked_count: i64,
    pub discrepancy_count: i64,
    pub unresolved_count: i64,
    pub duration_seconds: Option<f64>,
    pub recent_discrepancies: Vec<DiscrepancyRow>,
}

/// Reconciliation history entry
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReconciliationRun {
    pub id: i64,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub status: String,
    pub checked_count: i64,
    pub discrepancy_count: i64,
    pub unresolved_count: i64,
    pub duration_seconds: Option<f64>,
}

/// Discrepancy type statistics for reconciliation stats
#[derive(Debug, Clone, serde::Serialize)]
pub struct DiscrepancyTypeStats {
    pub discrepancy_type: String,
    pub count: i64,
    pub percentage: f64,
}

/// Reconciliation statistics
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReconciliationStats {
    pub total_reconciliations: i64,
    pub successful_reconciliations: i64,
    pub failed_reconciliations: i64,
    pub total_checked: i64,
    pub total_discrepancies: i64,
    pub total_unresolved: i64,
    pub avg_discrepancies_per_run: f64,
    pub most_common_discrepancy_types: Vec<DiscrepancyTypeStats>,
}

/// Trade latency statistics
#[derive(Debug, serde::Serialize)]
pub struct TradeLatencyStats {
    pub count: u32,
    pub avg_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
}

/// Latency histogram bucket
#[derive(Debug, serde::Serialize)]
pub struct LatencyBucket {
    pub range: String,
    pub count: u32,
    pub percentage: f64,
}

// =============================================================================
// CONVERSIONS: Internal types → API detail types
// =============================================================================

use super::{Position, Trade, Wallet};

impl From<Wallet> for WalletDetail {
    fn from(w: Wallet) -> Self {
        WalletDetail {
            id: w.id,
            address: w.address,
            status: w.status,
            wqs_score: w.wqs_score,
            roi_7d: w.roi_7d,
            roi_30d: w.roi_30d,
            trade_count_30d: w.trade_count_30d,
            win_rate: w.win_rate,
            max_drawdown_30d: w.max_drawdown_30d,
            avg_trade_size_sol: w.avg_trade_size_sol,
            avg_win_sol: w.avg_win_sol,
            avg_loss_sol: w.avg_loss_sol,
            profit_factor: w.profit_factor,
            realized_pnl_30d_sol: w.realized_pnl_30d_sol,
            last_trade_at: w.last_trade_at.map(|t| t.to_rfc3339()),
            promoted_at: w.promoted_at.map(|t| t.to_rfc3339()),
            ttl_expires_at: w.ttl_expires_at.map(|t| t.to_rfc3339()),
            notes: w.notes,
            archetype: w.archetype,
            avg_entry_delay_seconds: w.avg_entry_delay_seconds,
            created_at: w.created_at.to_rfc3339(),
            updated_at: w.updated_at.to_rfc3339(),
        }
    }
}

impl From<Position> for PositionDetail {
    fn from(p: Position) -> Self {
        PositionDetail {
            id: p.id,
            trade_uuid: p.trade_uuid,
            wallet_address: p.wallet_address,
            token_address: p.token_address,
            token_symbol: p.token_symbol,
            strategy: p.strategy,
            entry_amount_sol: p.entry_amount_sol,
            entry_price: p.entry_price,
            entry_tx_signature: p.entry_tx_signature,
            current_price: p.current_price,
            unrealized_pnl_sol: p.unrealized_pnl_sol,
            unrealized_pnl_percent: p.unrealized_pnl_percent,
            state: p.state,
            exit_price: p.exit_price,
            exit_tx_signature: p.exit_tx_signature,
            realized_pnl_sol: p.realized_pnl_sol,
            realized_pnl_usd: p.realized_pnl_usd,
            opened_at: p.opened_at.to_rfc3339(),
            last_updated: p.last_updated.to_rfc3339(),
            closed_at: p.closed_at.map(|t| t.to_rfc3339()),
        }
    }
}

impl From<Trade> for TradeDetail {
    fn from(t: Trade) -> Self {
        TradeDetail {
            id: t.id,
            trade_uuid: t.trade_uuid,
            wallet_address: t.wallet_address,
            token_address: t.token_address,
            token_symbol: t.token_symbol,
            strategy: t.strategy,
            side: t.side,
            amount_sol: t.amount_sol,
            price_at_signal: t.price_at_signal,
            tx_signature: t.tx_signature,
            status: t.status,
            retry_count: t.retry_count,
            error_message: t.error_message,
            pnl_sol: t.pnl_sol,
            pnl_usd: t.pnl_usd,
            jito_tip_sol: Some(t.jito_tip_sol),
            dex_fee_sol: Some(t.dex_fee_sol),
            slippage_cost_sol: Some(t.slippage_cost_sol),
            total_cost_sol: Some(t.total_cost_sol),
            net_pnl_sol: t.net_pnl_sol,
            created_at: t.created_at.to_rfc3339(),
            updated_at: t.updated_at.to_rfc3339(),
        }
    }
}

/// Exit target data for profit target state persistence
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExitTargetData {
    pub entry_price: rust_decimal::Decimal,
    pub entry_amount_sol: rust_decimal::Decimal,
    pub peak_price: rust_decimal::Decimal,
    pub peak_profit_percent: rust_decimal::Decimal,
    pub targets_hit: String,
    pub trailing_stop_active: bool,
    pub trailing_stop_price: rust_decimal::Decimal,
    pub remaining_fraction: rust_decimal::Decimal,
}
