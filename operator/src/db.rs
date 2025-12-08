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
                    AppError::Database(sqlx::Error::Io(std::io::Error::other(
                        format!("Failed to create database directory: {}", e),
                    )))
                })?;
                info!("Created database directory: {:?}", parent);
            }
        }

        let db_url = format!("sqlite:{}?mode=rwc", config.path.display());

        let connect_options = SqliteConnectOptions::from_str(&db_url)
        .map_err(AppError::Database)?
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

// =============================================================================
// INCIDENTS API (Dead Letter Queue & Config Audit)
// =============================================================================

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

/// Get dead letter queue items
pub async fn get_dead_letter_queue(
    pool: &DbPool,
    limit: Option<i64>,
    offset: Option<i64>,
) -> AppResult<Vec<DeadLetterItem>> {
    let mut query = String::from(
        "SELECT id, trade_uuid, payload, reason, error_details, source_ip, retry_count, can_retry, received_at, processed_at FROM dead_letter_queue ORDER BY received_at DESC"
    );

    if let Some(lim) = limit {
        query.push_str(&format!(" LIMIT {}", lim));
    }

    if let Some(off) = offset {
        query.push_str(&format!(" OFFSET {}", off));
    }

    // Query as tuple and map to struct (can_retry is INTEGER in DB, need to convert to bool)
    let rows: Vec<(i64, Option<String>, String, String, Option<String>, Option<String>, i32, i64, String, Option<String>)> = 
        sqlx::query_as(&query)
        .fetch_all(pool)
        .await?;

    let items = rows
        .into_iter()
        .map(|(id, trade_uuid, payload, reason, error_details, source_ip, retry_count, can_retry_int, received_at, processed_at)| {
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
        })
        .collect();

    Ok(items)
}

/// Count dead letter queue items
pub async fn count_dead_letter_queue(pool: &DbPool) -> AppResult<i64> {
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM dead_letter_queue")
        .fetch_one(pool)
        .await?;

    Ok(count.0)
}

/// Config audit log entry
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct ConfigAuditItem {
    pub id: i64,
    pub key: String,
    pub old_value: Option<String>,
    pub new_value: String,
    pub changed_by: String,
    pub change_reason: Option<String>,
    pub changed_at: String,
}

/// Get config audit log
pub async fn get_config_audit(
    pool: &DbPool,
    limit: Option<i64>,
    offset: Option<i64>,
) -> AppResult<Vec<ConfigAuditItem>> {
    let mut query = String::from(
        "SELECT id, key, old_value, new_value, changed_by, change_reason, changed_at FROM config_audit ORDER BY changed_at DESC"
    );

    if let Some(lim) = limit {
        query.push_str(&format!(" LIMIT {}", lim));
    }

    if let Some(off) = offset {
        query.push_str(&format!(" OFFSET {}", off));
    }

    let items = sqlx::query_as::<_, ConfigAuditItem>(&query)
        .fetch_all(pool)
        .await?;

    Ok(items)
}

/// Count config audit entries
pub async fn count_config_audit(pool: &DbPool) -> AppResult<i64> {
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM config_audit")
        .fetch_one(pool)
        .await?;

    Ok(count.0)
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

/// Get total PnL for the last 7 days
pub async fn get_pnl_7d(pool: &DbPool) -> AppResult<f64> {
    let result: (Option<f64>,) = sqlx::query_as(
        r#"
        SELECT COALESCE(SUM(pnl_usd), 0.0)
        FROM trades
        WHERE status = 'CLOSED'
        AND created_at >= datetime('now', '-7 days')
        "#,
    )
    .fetch_one(pool)
    .await?;

    Ok(result.0.unwrap_or(0.0))
}

/// Get total PnL for the last 30 days
pub async fn get_pnl_30d(pool: &DbPool) -> AppResult<f64> {
    let result: (Option<f64>,) = sqlx::query_as(
        r#"
        SELECT COALESCE(SUM(pnl_usd), 0.0)
        FROM trades
        WHERE status = 'CLOSED'
        AND created_at >= datetime('now', '-30 days')
        "#,
    )
    .fetch_one(pool)
    .await?;

    Ok(result.0.unwrap_or(0.0))
}

/// Get strategy performance metrics (win rate, avg return, trade count)
pub async fn get_strategy_performance(
    pool: &DbPool,
    strategy: &str,
    days: i64,
) -> AppResult<(f64, f64, u32)> {
    // Get trades for the strategy in the time period
    // Use parameterized query to avoid SQL injection
    let query_str = format!(
        r#"
        SELECT pnl_usd
        FROM trades
        WHERE status = 'CLOSED'
        AND strategy = ?
        AND created_at >= datetime('now', '-{} days')
        ORDER BY created_at DESC
        "#,
        days
    );
    
    let trades: Vec<(Option<f64>,)> = sqlx::query_as(&query_str)
        .bind(strategy)
        .fetch_all(pool)
        .await?;

    if trades.is_empty() {
        return Ok((0.0, 0.0, 0));
    }

    let mut total_pnl = 0.0;
    let mut winning_trades = 0u32;
    let mut total_trades = 0u32;

    for (pnl_opt,) in trades {
        if let Some(pnl) = pnl_opt {
            total_trades += 1;
            total_pnl += pnl;
            if pnl > 0.0 {
                winning_trades += 1;
            }
        }
    }

    let win_rate = if total_trades > 0 {
        (winning_trades as f64 / total_trades as f64) * 100.0
    } else {
        0.0
    };

    let avg_return = if total_trades > 0 {
        total_pnl / total_trades as f64
    } else {
        0.0
    };

    Ok((win_rate, avg_return, total_trades))
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
    pub exit_tx_signature: Option<String>,
    pub last_updated: chrono::DateTime<chrono::Utc>,
}

/// Get positions stuck in EXITING state for too long
pub async fn get_stuck_positions(pool: &DbPool, stuck_seconds: i64) -> AppResult<Vec<PositionRecord>> {
    let positions: Vec<(i64, String, String, String, String, String, String, Option<String>, String)> = sqlx::query_as(
        r#"
        SELECT id, trade_uuid, wallet_address, token_address, strategy, state, 
               entry_tx_signature, exit_tx_signature, last_updated
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
        .map(|(id, trade_uuid, wallet_address, token_address, strategy, state, entry_tx_signature, exit_tx_signature, last_updated)| {
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

// =============================================================================
// POSITIONS API
// =============================================================================

/// Position with full details for API response
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct PositionDetail {
    pub id: i64,
    pub trade_uuid: String,
    pub wallet_address: String,
    pub token_address: String,
    pub token_symbol: Option<String>,
    pub strategy: String,
    pub entry_amount_sol: f64,
    pub entry_price: f64,
    pub entry_tx_signature: String,
    pub current_price: Option<f64>,
    pub unrealized_pnl_sol: Option<f64>,
    pub unrealized_pnl_percent: Option<f64>,
    pub state: String,
    pub exit_price: Option<f64>,
    pub exit_tx_signature: Option<String>,
    pub realized_pnl_sol: Option<f64>,
    pub realized_pnl_usd: Option<f64>,
    pub opened_at: String,
    pub last_updated: String,
    pub closed_at: Option<String>,
}

/// Get all positions with optional state filter
pub async fn get_positions(pool: &DbPool, state_filter: Option<&str>) -> AppResult<Vec<PositionDetail>> {
    let positions = match state_filter {
        Some(state) => {
            sqlx::query_as::<_, PositionDetail>(
                r#"
                SELECT id, trade_uuid, wallet_address, token_address, token_symbol, strategy,
                       entry_amount_sol, entry_price, entry_tx_signature, current_price,
                       unrealized_pnl_sol, unrealized_pnl_percent, state, exit_price,
                       exit_tx_signature, realized_pnl_sol, realized_pnl_usd,
                       opened_at, last_updated, closed_at
                FROM positions
                WHERE state = ?
                ORDER BY last_updated DESC
                "#
            )
            .bind(state)
            .fetch_all(pool)
            .await?
        }
        None => {
            sqlx::query_as::<_, PositionDetail>(
                r#"
                SELECT id, trade_uuid, wallet_address, token_address, token_symbol, strategy,
                       entry_amount_sol, entry_price, entry_tx_signature, current_price,
                       unrealized_pnl_sol, unrealized_pnl_percent, state, exit_price,
                       exit_tx_signature, realized_pnl_sol, realized_pnl_usd,
                       opened_at, last_updated, closed_at
                FROM positions
                ORDER BY last_updated DESC
                "#
            )
            .fetch_all(pool)
            .await?
        }
    };

    Ok(positions)
}

/// Count active positions
pub async fn count_active_positions(pool: &DbPool) -> AppResult<i64> {
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM positions WHERE state = 'ACTIVE'"
    )
    .fetch_one(pool)
    .await?;

    Ok(count.0)
}

/// Count total trades
pub async fn count_total_trades(pool: &DbPool) -> AppResult<i64> {
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM trades"
    )
    .fetch_one(pool)
    .await?;

    Ok(count.0)
}

/// Get a single position by trade_uuid
pub async fn get_position_by_uuid(pool: &DbPool, trade_uuid: &str) -> AppResult<Option<PositionDetail>> {
    let position = sqlx::query_as::<_, PositionDetail>(
        r#"
        SELECT id, trade_uuid, wallet_address, token_address, token_symbol, strategy,
               entry_amount_sol, entry_price, entry_tx_signature, current_price,
               unrealized_pnl_sol, unrealized_pnl_percent, state, exit_price,
               exit_tx_signature, realized_pnl_sol, realized_pnl_usd,
               opened_at, last_updated, closed_at
        FROM positions
        WHERE trade_uuid = ?
        "#
    )
    .bind(trade_uuid)
    .fetch_optional(pool)
    .await?;

    Ok(position)
}

// =============================================================================
// WALLETS API
// =============================================================================

/// Wallet with full details for API response
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct WalletDetail {
    pub id: i64,
    pub address: String,
    pub status: String,
    pub wqs_score: Option<f64>,
    pub roi_7d: Option<f64>,
    pub roi_30d: Option<f64>,
    pub trade_count_30d: Option<i32>,
    pub win_rate: Option<f64>,
    pub max_drawdown_30d: Option<f64>,
    pub avg_trade_size_sol: Option<f64>,
    pub last_trade_at: Option<String>,
    pub promoted_at: Option<String>,
    pub ttl_expires_at: Option<String>,
    pub notes: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Get all wallets with optional status filter
pub async fn get_wallets(pool: &DbPool, status_filter: Option<&str>) -> AppResult<Vec<WalletDetail>> {
    let wallets = match status_filter {
        Some(status) => {
            sqlx::query_as::<_, WalletDetail>(
                r#"
                SELECT id, address, status, wqs_score, roi_7d, roi_30d, trade_count_30d,
                       win_rate, max_drawdown_30d, avg_trade_size_sol, last_trade_at,
                       promoted_at, ttl_expires_at, notes, created_at, updated_at
                FROM wallets
                WHERE status = ?
                ORDER BY wqs_score DESC NULLS LAST
                "#
            )
            .bind(status)
            .fetch_all(pool)
            .await?
        }
        None => {
            sqlx::query_as::<_, WalletDetail>(
                r#"
                SELECT id, address, status, wqs_score, roi_7d, roi_30d, trade_count_30d,
                       win_rate, max_drawdown_30d, avg_trade_size_sol, last_trade_at,
                       promoted_at, ttl_expires_at, notes, created_at, updated_at
                FROM wallets
                ORDER BY wqs_score DESC NULLS LAST
                "#
            )
            .fetch_all(pool)
            .await?
        }
    };

    Ok(wallets)
}

/// Get a single wallet by address
pub async fn get_wallet_by_address(pool: &DbPool, address: &str) -> AppResult<Option<WalletDetail>> {
    let wallet = sqlx::query_as::<_, WalletDetail>(
        r#"
        SELECT id, address, status, wqs_score, roi_7d, roi_30d, trade_count_30d,
               win_rate, max_drawdown_30d, avg_trade_size_sol, last_trade_at,
               promoted_at, ttl_expires_at, notes, created_at, updated_at
        FROM wallets
        WHERE address = ?
        "#
    )
    .bind(address)
    .fetch_optional(pool)
    .await?;

    Ok(wallet)
}

/// Update wallet status with optional TTL
pub async fn update_wallet_status(
    pool: &DbPool,
    address: &str,
    status: &str,
    ttl_hours: Option<i64>,
    reason: Option<&str>,
) -> AppResult<bool> {
    let ttl_expires_at = ttl_hours.map(|hours| {
        chrono::Utc::now() + chrono::Duration::hours(hours)
    });

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
        "#
    )
    .bind(status)
    .bind(promoted_at)
    .bind(ttl_expires_at.map(|dt| dt.to_rfc3339()))
    .bind(reason)
    .bind(address)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Get wallets with expired TTL that need to be demoted
pub async fn get_expired_ttl_wallets(pool: &DbPool) -> AppResult<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        r#"
        SELECT address FROM wallets
        WHERE status = 'ACTIVE'
        AND ttl_expires_at IS NOT NULL
        AND ttl_expires_at < datetime('now')
        "#
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|(addr,)| addr).collect())
}

/// Demote a wallet from ACTIVE to CANDIDATE (for TTL expiration)
pub async fn demote_wallet(pool: &DbPool, address: &str, reason: &str) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE wallets
        SET status = 'CANDIDATE',
            ttl_expires_at = NULL,
            notes = ?
        WHERE address = ?
        "#
    )
    .bind(reason)
    .bind(address)
    .execute(pool)
    .await?;

    Ok(())
}

// =============================================================================
// TRADES API / EXPORT
// =============================================================================

/// Trade record for API response
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct TradeDetail {
    pub id: i64,
    pub trade_uuid: String,
    pub wallet_address: String,
    pub token_address: String,
    pub token_symbol: Option<String>,
    pub strategy: String,
    pub side: String,
    pub amount_sol: f64,
    pub price_at_signal: Option<f64>,
    pub tx_signature: Option<String>,
    pub status: String,
    pub retry_count: i32,
    pub error_message: Option<String>,
    pub pnl_sol: Option<f64>,
    pub pnl_usd: Option<f64>,
    pub created_at: String,
    pub updated_at: String,
}

/// Get trades with optional filters for API and export
pub async fn get_trades(
    pool: &DbPool,
    from_date: Option<&str>,
    to_date: Option<&str>,
    status_filter: Option<&str>,
    strategy_filter: Option<&str>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> AppResult<Vec<TradeDetail>> {
    // Build query dynamically based on filters
    let mut query = String::from(
        r#"
        SELECT id, trade_uuid, wallet_address, token_address, token_symbol, strategy,
               side, amount_sol, price_at_signal, tx_signature, status, retry_count,
               error_message, pnl_sol, pnl_usd, created_at, updated_at
        FROM trades
        WHERE 1=1
        "#
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

    query.push_str(" ORDER BY created_at DESC");

    if let Some(lim) = limit {
        query.push_str(&format!(" LIMIT {}", lim));
    }

    if let Some(off) = offset {
        query.push_str(&format!(" OFFSET {}", off));
    }

    // Execute with bindings
    let mut q = sqlx::query_as::<_, TradeDetail>(&query);

    for binding in bindings {
        q = q.bind(binding);
    }

    let trades = q.fetch_all(pool).await?;
    Ok(trades)
}

/// Count total trades (for pagination)
pub async fn count_trades(
    pool: &DbPool,
    from_date: Option<&str>,
    to_date: Option<&str>,
    status_filter: Option<&str>,
    strategy_filter: Option<&str>,
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

    let mut q = sqlx::query_as::<_, (i64,)>(&query);

    for binding in bindings {
        q = q.bind(binding);
    }

    let (count,) = q.fetch_one(pool).await?;
    Ok(count)
}

/// Generate CSV content from trades
pub fn trades_to_csv(trades: &[TradeDetail]) -> String {
    let mut csv = String::new();
    
    // Header
    csv.push_str("id,trade_uuid,wallet_address,token_address,token_symbol,strategy,side,amount_sol,price_at_signal,tx_signature,status,pnl_sol,pnl_usd,created_at\n");
    
    // Data rows
    for trade in trades {
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            trade.id,
            trade.trade_uuid,
            trade.wallet_address,
            trade.token_address,
            trade.token_symbol.as_deref().unwrap_or(""),
            trade.strategy,
            trade.side,
            trade.amount_sol,
            trade.price_at_signal.map(|p| p.to_string()).unwrap_or_default(),
            trade.tx_signature.as_deref().unwrap_or(""),
            trade.status,
            trade.pnl_sol.map(|p| p.to_string()).unwrap_or_default(),
            trade.pnl_usd.map(|p| p.to_string()).unwrap_or_default(),
            trade.created_at,
        ));
    }
    
    csv
}

/// Generate PDF content from trades
pub fn trades_to_pdf(trades: &[TradeDetail]) -> AppResult<Vec<u8>> {
    use printpdf::*;
    
    let mut doc = PdfDocument::new("Chimera Trade History");
    
    // Build PDF operations
    let mut ops = Vec::new();
    
    // Title
    ops.extend_from_slice(&[
        Op::StartTextSection,
        Op::SetTextCursor {
            pos: Point::new(Mm(10.0), Mm(280.0)),
        },
        Op::SetFontSizeBuiltinFont {
            font: BuiltinFont::HelveticaBold,
            size: Pt(16.0),
        },
        Op::SetLineHeight { lh: Pt(16.0) },
        Op::WriteTextBuiltinFont {
            font: BuiltinFont::HelveticaBold,
            items: vec![TextItem::Text("Chimera Trade History Report".to_string())],
        },
        Op::EndTextSection,
    ]);
    
    // Generated time
    let generated_time = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
    ops.extend_from_slice(&[
        Op::StartTextSection,
        Op::SetTextCursor {
            pos: Point::new(Mm(10.0), Mm(270.0)),
        },
        Op::SetFontSizeBuiltinFont {
            font: BuiltinFont::Helvetica,
            size: Pt(10.0),
        },
        Op::SetLineHeight { lh: Pt(10.0) },
        Op::WriteTextBuiltinFont {
            font: BuiltinFont::Helvetica,
            items: vec![TextItem::Text(format!("Generated: {}", generated_time))],
        },
        Op::EndTextSection,
    ]);
    
    // Table header
    let mut y_pos = 260.0;
    ops.extend_from_slice(&[
        Op::StartTextSection,
        Op::SetTextCursor {
            pos: Point::new(Mm(10.0), Mm(y_pos)),
        },
        Op::SetFontSizeBuiltinFont {
            font: BuiltinFont::Helvetica,
            size: Pt(8.0),
        },
        Op::SetLineHeight { lh: Pt(8.0) },
        Op::WriteTextBuiltinFont {
            font: BuiltinFont::Helvetica,
            items: vec![TextItem::Text("ID | Trade UUID | Wallet | Token | Strategy | Side | Amount SOL | Status | PnL USD | Created".to_string())],
        },
        Op::EndTextSection,
    ]);
    
    // Draw line under header
    y_pos -= 5.0;
    let line_y = y_pos;
    ops.push(Op::DrawPolygon {
        polygon: Polygon {
            rings: vec![PolygonRing {
                points: vec![
                    LinePoint {
                        p: Point::new(Mm(10.0), Mm(line_y)),
                        bezier: false,
                    },
                    LinePoint {
                        p: Point::new(Mm(200.0), Mm(line_y)),
                        bezier: false,
                    },
                ],
            }],
            mode: PaintMode::Stroke,
            winding_order: WindingOrder::NonZero,
        },
    });
    
    y_pos -= 5.0;
    
    // Add trade rows (limit to prevent PDF from being too large)
    let max_rows = 1000;
    let display_trades = if trades.len() > max_rows {
        &trades[..max_rows]
    } else {
        trades
    };
    
    for trade in display_trades {
        if y_pos < 20.0 {
            // Would need to create new page here - for now, just stop
            break;
        }
        
        let row = format!(
            "{} | {}... | {}... | {} | {} | {} | {:.4} | {} | {:.2} | {}",
            trade.id,
            &trade.trade_uuid[..12.min(trade.trade_uuid.len())],
            &trade.wallet_address[..8.min(trade.wallet_address.len())],
            trade.token_symbol.as_deref()
                .map(|s| s.chars().take(8).collect::<String>())
                .unwrap_or_else(|| trade.token_address.chars().take(8).collect()),
            trade.strategy,
            trade.side,
            trade.amount_sol,
            trade.status,
            trade.pnl_usd.map(|p| p).unwrap_or(0.0),
            &trade.created_at[..10.min(trade.created_at.len())], // Just date part
        );
        
        ops.extend_from_slice(&[
            Op::StartTextSection,
            Op::SetTextCursor {
                pos: Point::new(Mm(10.0), Mm(y_pos)),
            },
            Op::SetFontSizeBuiltinFont {
                font: BuiltinFont::Helvetica,
                size: Pt(7.0),
            },
            Op::SetLineHeight { lh: Pt(7.0) },
            Op::WriteTextBuiltinFont {
                font: BuiltinFont::Helvetica,
                items: vec![TextItem::Text(row)],
            },
            Op::EndTextSection,
        ]);
        
        y_pos -= 4.0;
    }
    
    if trades.len() > max_rows {
        ops.extend_from_slice(&[
            Op::StartTextSection,
            Op::SetTextCursor {
                pos: Point::new(Mm(10.0), Mm(y_pos)),
            },
            Op::SetFontSizeBuiltinFont {
                font: BuiltinFont::Helvetica,
                size: Pt(8.0),
            },
            Op::SetLineHeight { lh: Pt(8.0) },
            Op::WriteTextBuiltinFont {
                font: BuiltinFont::Helvetica,
                items: vec![TextItem::Text(format!("... and {} more trades", trades.len() - max_rows))],
            },
            Op::EndTextSection,
        ]);
    }
    
    // Create page with operations
    let page = PdfPage::new(Mm(210.0), Mm(297.0), ops);
    
    // Add page to document and save
    let mut warnings = Vec::new();
    let bytes = doc.with_pages(vec![page]).save(&PdfSaveOptions::default(), &mut warnings);
    
    Ok(bytes)
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
