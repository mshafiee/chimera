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
        match std::env::var("CHIMERA_DB_BACKEND")
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
    pub path: std::path::PathBuf,     // For SQLite
    pub url: Option<String>,           // For PostgreSQL
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
