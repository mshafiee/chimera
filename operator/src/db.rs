//! Database module for Chimera Operator
//!
//! Manages SQLite connection pool with WAL mode and provides
//! database operations for trades, positions, and system tables.

use crate::config::DatabaseConfig;
use crate::error::{AppError, AppResult};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Pool, Sqlite};
use std::path::Path;
use std::str::FromStr;
use tracing::{info, warn};

/// Type alias for the SQLite connection pool
pub type DbPool = Pool<Sqlite>;

/// Initialize the database connection pool
pub async fn init_pool(config: &DatabaseConfig) -> AppResult<DbPool> {
    // Ensure data directory exists
    if let Some(parent) = config.path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AppError::Database(sqlx::Error::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to create database directory: {}", e),
                )))
            })?;
            info!("Created database directory: {:?}", parent);
        }
    }

    let db_url = format!("sqlite:{}?mode=rwc", config.path.display());

    let connect_options = SqliteConnectOptions::from_str(&db_url)
        .map_err(|e| AppError::Database(e))?
        // Enable WAL mode for concurrent reads
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        // Set busy timeout to 5 seconds
        .busy_timeout(std::time::Duration::from_secs(5))
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
        "Database pool initialized: {:?} (max {} connections)",
        config.path, config.max_connections
    );

    Ok(pool)
}

/// Run database migrations (apply schema)
pub async fn run_migrations(pool: &DbPool) -> AppResult<()> {
    // Read and execute schema file
    let schema_path = Path::new("database/schema.sql");

    if !schema_path.exists() {
        warn!("Schema file not found at {:?}, skipping migrations", schema_path);
        return Ok(());
    }

    let schema = std::fs::read_to_string(schema_path).map_err(|e| {
        AppError::Internal(format!("Failed to read schema file: {}", e))
    })?;

    // Split schema into individual statements and execute
    // SQLite doesn't support multiple statements in one query
    for statement in schema.split(';') {
        let stmt = statement.trim();
        if stmt.is_empty() || stmt.starts_with("--") {
            continue;
        }

        // Skip PRAGMA statements that might conflict with connection settings
        if stmt.to_uppercase().starts_with("PRAGMA JOURNAL_MODE")
            || stmt.to_uppercase().starts_with("PRAGMA BUSY_TIMEOUT")
        {
            continue;
        }

        sqlx::query(stmt)
            .execute(pool)
            .await
            .map_err(|e| {
                // Log but don't fail on "already exists" errors
                if e.to_string().contains("already exists") {
                    warn!("Table/index already exists, skipping: {}", e);
                    return e;
                }
                e
            })
            .ok(); // Continue on error (table already exists)
    }

    info!("Database schema applied successfully");
    Ok(())
}

/// Check if a trade_uuid already exists (for idempotency)
pub async fn trade_uuid_exists(pool: &DbPool, trade_uuid: &str) -> AppResult<bool> {
    // Check trades table
    let trade_exists: (i32,) = sqlx::query_as(
        "SELECT COUNT(*) FROM trades WHERE trade_uuid = ?"
    )
    .bind(trade_uuid)
    .fetch_one(pool)
    .await?;

    if trade_exists.0 > 0 {
        return Ok(true);
    }

    // Check dead letter queue
    let dlq_exists: (i32,) = sqlx::query_as(
        "SELECT COUNT(*) FROM dead_letter_queue WHERE trade_uuid = ?"
    )
    .bind(trade_uuid)
    .fetch_one(pool)
    .await?;

    Ok(dlq_exists.0 > 0)
}

/// Insert a trade record
pub async fn insert_trade(
    pool: &DbPool,
    trade_uuid: &str,
    wallet_address: &str,
    token_address: &str,
    token_symbol: Option<&str>,
    strategy: &str,
    side: &str,
    amount_sol: f64,
    status: &str,
) -> AppResult<i64> {
    let result = sqlx::query(
        r#"
        INSERT INTO trades (
            trade_uuid, wallet_address, token_address, token_symbol,
            strategy, side, amount_sol, status
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(trade_uuid)
    .bind(wallet_address)
    .bind(token_address)
    .bind(token_symbol)
    .bind(strategy)
    .bind(side)
    .bind(amount_sol)
    .bind(status)
    .execute(pool)
    .await?;

    Ok(result.last_insert_rowid())
}

/// Update trade status
pub async fn update_trade_status(
    pool: &DbPool,
    trade_uuid: &str,
    status: &str,
    tx_signature: Option<&str>,
    error_message: Option<&str>,
) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE trades 
        SET status = ?, tx_signature = ?, error_message = ?
        WHERE trade_uuid = ?
        "#,
    )
    .bind(status)
    .bind(tx_signature)
    .bind(error_message)
    .bind(trade_uuid)
    .execute(pool)
    .await?;

    Ok(())
}

/// Insert into dead letter queue
pub async fn insert_dead_letter(
    pool: &DbPool,
    trade_uuid: Option<&str>,
    payload: &str,
    reason: &str,
    error_details: Option<&str>,
    source_ip: Option<&str>,
) -> AppResult<i64> {
    let result = sqlx::query(
        r#"
        INSERT INTO dead_letter_queue (
            trade_uuid, payload, reason, error_details, source_ip
        ) VALUES (?, ?, ?, ?, ?)
        "#,
    )
    .bind(trade_uuid)
    .bind(payload)
    .bind(reason)
    .bind(error_details)
    .bind(source_ip)
    .execute(pool)
    .await?;

    Ok(result.last_insert_rowid())
}

/// Log a configuration change
pub async fn log_config_change(
    pool: &DbPool,
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
    .execute(pool)
    .await?;

    Ok(())
}

/// Get count of trades in a specific status
pub async fn count_trades_by_status(pool: &DbPool, status: &str) -> AppResult<i64> {
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM trades WHERE status = ?"
    )
    .bind(status)
    .fetch_one(pool)
    .await?;

    Ok(count.0)
}

/// Get total PnL for the last 24 hours
pub async fn get_pnl_24h(pool: &DbPool) -> AppResult<f64> {
    let result: (Option<f64>,) = sqlx::query_as(
        r#"
        SELECT COALESCE(SUM(pnl_usd), 0.0)
        FROM trades
        WHERE status = 'CLOSED'
        AND created_at >= datetime('now', '-24 hours')
        "#,
    )
    .fetch_one(pool)
    .await?;

    Ok(result.0.unwrap_or(0.0))
}

/// Get count of consecutive losses
pub async fn get_consecutive_losses(pool: &DbPool) -> AppResult<u32> {
    // Get the most recent trades and count consecutive losses
    let trades: Vec<(f64,)> = sqlx::query_as(
        r#"
        SELECT COALESCE(pnl_usd, 0.0)
        FROM trades
        WHERE status = 'CLOSED'
        ORDER BY created_at DESC
        LIMIT 20
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut consecutive = 0u32;
    for (pnl,) in trades {
        if pnl < 0.0 {
            consecutive += 1;
        } else {
            break;
        }
    }

    Ok(consecutive)
}

// =============================================================================
// JITO TIP HISTORY
// =============================================================================

/// Insert a Jito tip record
pub async fn insert_jito_tip(
    pool: &DbPool,
    tip_amount_sol: f64,
    bundle_signature: Option<&str>,
    strategy: &str,
    success: bool,
) -> AppResult<i64> {
    let result = sqlx::query(
        r#"
        INSERT INTO jito_tip_history (tip_amount_sol, bundle_signature, strategy, success)
        VALUES (?, ?, ?, ?)
        "#,
    )
    .bind(tip_amount_sol)
    .bind(bundle_signature)
    .bind(strategy)
    .bind(if success { 1 } else { 0 })
    .execute(pool)
    .await?;

    Ok(result.last_insert_rowid())
}

/// Get recent successful tips for percentile calculation
/// Returns tip amounts in descending order (most recent first)
pub async fn get_recent_tips(pool: &DbPool, limit: u32) -> AppResult<Vec<f64>> {
    let tips: Vec<(f64,)> = sqlx::query_as(
        r#"
        SELECT tip_amount_sol
        FROM jito_tip_history
        WHERE success = 1
        ORDER BY created_at DESC
        LIMIT ?
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(tips.into_iter().map(|(t,)| t).collect())
}

/// Get count of successful tips (for cold start detection)
pub async fn get_tip_count(pool: &DbPool) -> AppResult<u32> {
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM jito_tip_history WHERE success = 1"
    )
    .fetch_one(pool)
    .await?;

    Ok(count.0 as u32)
}

/// Clean up old tip history (keep only last 7 days)
pub async fn prune_old_tips(pool: &DbPool) -> AppResult<u64> {
    let result = sqlx::query(
        "DELETE FROM jito_tip_history WHERE created_at < datetime('now', '-7 days')"
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

// =============================================================================
// POSITIONS & STUCK STATE RECOVERY
// =============================================================================

/// Position record from database
#[derive(Debug, Clone)]
pub struct PositionRecord {
    pub id: i64,
    pub trade_uuid: String,
    pub wallet_address: String,
    pub token_address: String,
    pub strategy: String,
    pub state: String,
    pub entry_tx_signature: String,
    pub last_updated: chrono::DateTime<chrono::Utc>,
}

/// Get positions stuck in EXITING state for too long
pub async fn get_stuck_positions(pool: &DbPool, stuck_seconds: i64) -> AppResult<Vec<PositionRecord>> {
    let positions: Vec<(i64, String, String, String, String, String, String, String)> = sqlx::query_as(
        r#"
        SELECT id, trade_uuid, wallet_address, token_address, strategy, state, 
               entry_tx_signature, last_updated
        FROM positions
        WHERE state = 'EXITING'
        AND last_updated < datetime('now', ? || ' seconds')
        "#,
    )
    .bind(-stuck_seconds)
    .fetch_all(pool)
    .await?;

    positions
        .into_iter()
        .map(|(id, trade_uuid, wallet_address, token_address, strategy, state, entry_tx_signature, last_updated)| {
            Ok(PositionRecord {
                id,
                trade_uuid,
                wallet_address,
                token_address,
                strategy,
                state,
                entry_tx_signature,
                last_updated: chrono::DateTime::parse_from_rfc3339(&last_updated)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now()),
            })
        })
        .collect()
}

/// Update position state
pub async fn update_position_state(
    pool: &DbPool,
    trade_uuid: &str,
    new_state: &str,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE positions SET state = ? WHERE trade_uuid = ?"
    )
    .bind(new_state)
    .bind(trade_uuid)
    .execute(pool)
    .await?;

    Ok(())
}

/// Insert reconciliation log entry
pub async fn insert_reconciliation_log(
    pool: &DbPool,
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
    .execute(pool)
    .await?;

    Ok(result.last_insert_rowid())
}

// =============================================================================
// CIRCUIT BREAKER SUPPORT
// =============================================================================

/// Get maximum drawdown from peak (for circuit breaker)
pub async fn get_max_drawdown_percent(pool: &DbPool) -> AppResult<f64> {
    // Calculate drawdown from highest cumulative PnL to current
    let result: (Option<f64>,) = sqlx::query_as(
        r#"
        WITH cumulative_pnl AS (
            SELECT 
                created_at,
                SUM(COALESCE(pnl_usd, 0)) OVER (ORDER BY created_at) as running_pnl
            FROM trades
            WHERE status = 'CLOSED'
        ),
        peaks AS (
            SELECT 
                MAX(running_pnl) as peak_pnl,
                (SELECT running_pnl FROM cumulative_pnl ORDER BY created_at DESC LIMIT 1) as current_pnl
            FROM cumulative_pnl
        )
        SELECT 
            CASE 
                WHEN peak_pnl > 0 THEN ((peak_pnl - current_pnl) / peak_pnl) * 100
                ELSE 0
            END as drawdown_percent
        FROM peaks
        "#,
    )
    .fetch_one(pool)
    .await?;

    Ok(result.0.unwrap_or(0.0).max(0.0))
}

/// Get active positions count
pub async fn get_active_positions_count(pool: &DbPool) -> AppResult<u32> {
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM positions WHERE state = 'ACTIVE'"
    )
    .fetch_one(pool)
    .await?;

    Ok(count.0 as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_pool_creation() {
        let config = DatabaseConfig {
            path: PathBuf::from(":memory:"),
            max_connections: 1,
        };

        let pool = init_pool(&config).await;
        assert!(pool.is_ok());
    }
}
