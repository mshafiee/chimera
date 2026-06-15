//! Database module for Chimera Operator
//!
//! Manages SQLite connection pool with WAL mode and provides
//! database operations for trades, positions, and system tables.

use crate::config::DatabaseConfig;
use crate::error::{AppError, AppResult};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Pool, Row, Sqlite};
use std::collections::HashMap;
// Path removed

use rust_decimal::prelude::*;
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
        // Set busy timeout to 60 seconds to cover large roster merges under concurrent writes.
        // ATTACH DATABASE operations can hold a write lock for >30 s on rosters with 50k+ wallets.
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
        "Database pool initialized: {:?} (max {} connections)",
        config.path, config.max_connections
    );

    Ok(pool)
}

/// Run database migrations (apply schema)
pub async fn run_migrations(pool: &DbPool) -> AppResult<()> {
    // Use the macro which embeds the migrations into the binary
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .map_err(|e| AppError::Database(e.into()))?;

    info!("Database migrations applied successfully");
    Ok(())
}

/// Run PRAGMA integrity_check and fail fast if the database is corrupt.
/// Call this at startup before any reads or writes.
pub async fn startup_integrity_check(pool: &DbPool) -> AppResult<()> {
    let result: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Database(e.into()))?;

    if result != "ok" {
        return Err(AppError::Internal(format!(
            "Database integrity check failed: {}",
            result
        )));
    }
    info!("Database integrity check passed");
    Ok(())
}

/// Reset any trades stuck in EXECUTING to FAILED.
///
/// EXECUTING is an ephemeral in-flight state. If the process crashed after
/// setting a trade to EXECUTING but before the on-chain result was written,
/// the trade is orphaned — the signal is gone from the in-memory queue and
/// there is no recovery path. Marking them FAILED surfaces them in the DLQ
/// for manual review rather than leaving them permanently stuck.
pub async fn recover_executing_trades(pool: &DbPool) -> AppResult<u32> {
    let rows_affected = sqlx::query(
        "UPDATE trades SET status = 'FAILED', error_message = 'Recovered from EXECUTING state after restart' WHERE status = 'EXECUTING'"
    )
    .execute(pool)
    .await
    .map_err(|e| AppError::Database(e.into()))?
    .rows_affected();

    if rows_affected > 0 {
        warn!(
            count = rows_affected,
            "Recovered EXECUTING-stuck trades → FAILED (process likely crashed mid-execution)"
        );
    }
    Ok(rows_affected as u32)
}

/// Check if a trade_uuid already exists (for idempotency)
pub async fn trade_uuid_exists(pool: &DbPool, trade_uuid: &str) -> AppResult<bool> {
    // Check trades table
    let trade_exists: (i32,) = sqlx::query_as("SELECT COUNT(*) FROM trades WHERE trade_uuid = ?")
        .bind(trade_uuid)
        .fetch_one(pool)
        .await?;

    if trade_exists.0 > 0 {
        return Ok(true);
    }

    // Check dead letter queue
    let dlq_exists: (i32,) =
        sqlx::query_as("SELECT COUNT(*) FROM dead_letter_queue WHERE trade_uuid = ?")
            .bind(trade_uuid)
            .fetch_one(pool)
            .await?;

    Ok(dlq_exists.0 > 0)
}

/// Insert a trade record
#[allow(clippy::too_many_arguments)]
pub async fn insert_trade(
    pool: &DbPool,
    trade_uuid: &str,
    wallet_address: &str,
    token_address: &str,
    token_symbol: Option<&str>,
    strategy: &str,
    side: &str,
    amount_sol: Decimal,
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
    .bind(amount_sol.to_f64().unwrap_or(0.0))
    .bind(status)
    .execute(pool)
    .await?;

    Ok(result.last_insert_rowid())
}

/// Update trade status
///
/// When `tx_signature` is `None`, the existing value is preserved — this prevents a FAILED
/// status update from destroying a valid signature on a transaction that timed out but may
/// have landed on-chain (critical for reconciliation).
pub async fn update_trade_status(
    pool: &DbPool,
    trade_uuid: &str,
    status: &str,
    tx_signature: Option<&str>,
    error_message: Option<&str>,
) -> AppResult<()> {
    let result = if let Some(sig) = tx_signature {
        sqlx::query(
            r#"
            UPDATE trades
            SET status = ?, tx_signature = ?, error_message = ?
            WHERE trade_uuid = ?
            "#,
        )
        .bind(status)
        .bind(sig)
        .bind(error_message)
        .bind(trade_uuid)
        .execute(pool)
        .await?
    } else {
        // Preserve existing tx_signature — do not overwrite with NULL
        sqlx::query(
            r#"
            UPDATE trades
            SET status = ?, error_message = ?
            WHERE trade_uuid = ?
            "#,
        )
        .bind(status)
        .bind(error_message)
        .bind(trade_uuid)
        .execute(pool)
        .await?
    };

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!(
            "trade_uuid '{}' not found",
            trade_uuid
        )));
    }

    Ok(())
}

/// Update trade costs
pub async fn update_trade_costs(
    pool: &DbPool,
    trade_uuid: &str,
    jito_tip_sol: Decimal,
    dex_fee_sol: Decimal,
    slippage_cost_sol: Decimal,
) -> AppResult<()> {
    let total_cost_sol = jito_tip_sol + dex_fee_sol + slippage_cost_sol;

    let result = sqlx::query(
        r#"
        UPDATE trades
        SET jito_tip_sol = ?, dex_fee_sol = ?, slippage_cost_sol = ?, total_cost_sol = ?
        WHERE trade_uuid = ?
        "#,
    )
    .bind(jito_tip_sol.to_f64().unwrap_or(0.0))
    .bind(dex_fee_sol.to_f64().unwrap_or(0.0))
    .bind(slippage_cost_sol.to_f64().unwrap_or(0.0))
    .bind(total_cost_sol.to_f64().unwrap_or(0.0))
    .bind(trade_uuid)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!(
            "trade_uuid {} not found",
            trade_uuid
        )));
    }

    Ok(())
}

/// Update trade net PnL (after costs)
pub async fn update_trade_net_pnl(
    pool: &DbPool,
    trade_uuid: &str,
    net_pnl_sol: Decimal,
) -> AppResult<()> {
    let result = sqlx::query(
        r#"
        UPDATE trades
        SET net_pnl_sol = ?
        WHERE trade_uuid = ?
        "#,
    )
    .bind(net_pnl_sol.to_f64().unwrap_or(0.0))
    .bind(trade_uuid)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!(
            "trade_uuid {} not found",
            trade_uuid
        )));
    }

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

/// Atomically mark a trade as DEAD_LETTER and insert it into the dead_letter_queue.
///
/// Both the status update on `trades` and the DLQ insert are wrapped in a single
/// BEGIN IMMEDIATE transaction so the two writes are never observed in a partial state.
pub async fn mark_dead_letter(
    pool: &DbPool,
    trade_uuid: &str,
    payload: &str,
    error: &str,
) -> AppResult<()> {
    let mut tx = pool.begin().await?;

    // Update the trade status to DEAD_LETTER (preserve existing tx_signature)
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

    // Insert into dead letter queue
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

/// Write the kill-switch active/inactive state to the dedicated single-row table.
/// Called synchronously (before returning from the API handler) so a crash between
/// this write and the in-memory circuit-breaker trip is safe — the next startup
/// reads this table and re-trips.
pub async fn set_kill_switch_state(
    pool: &DbPool,
    active: bool,
    changed_by: &str,
    reason: Option<&str>,
) -> AppResult<()> {
    let state = if active { "ACTIVE" } else { "INACTIVE" };
    sqlx::query(
        r#"INSERT INTO kill_switch_state (id, state, changed_at, changed_by, reason)
           VALUES (1, ?, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?, ?)
           ON CONFLICT(id) DO UPDATE SET
               state      = excluded.state,
               changed_at = excluded.changed_at,
               changed_by = excluded.changed_by,
               reason     = excluded.reason"#,
    )
    .bind(state)
    .bind(changed_by)
    .bind(reason)
    .execute(pool)
    .await?;
    Ok(())
}

/// Read the persisted kill-switch state. Returns `true` if ACTIVE.
pub async fn is_kill_switch_active(pool: &DbPool) -> bool {
    let row: Option<String> =
        sqlx::query_scalar("SELECT state FROM kill_switch_state WHERE id = 1")
            .fetch_optional(pool)
            .await
            .unwrap_or(None);
    row.as_deref() == Some("ACTIVE")
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
    )> = sqlx::query_as(&query).fetch_all(pool).await?;

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
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM trades WHERE status = ?")
        .bind(status)
        .fetch_one(pool)
        .await?;

    Ok(count.0)
}

/// Count closed trades for a specific wallet (used for Kelly/WQS confidence sizing)
pub async fn get_closed_trade_count(pool: &DbPool, wallet_address: &str) -> AppResult<i64> {
    let result: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM trades WHERE wallet_address = ? AND status = 'CLOSED'",
    )
    .bind(wallet_address)
    .fetch_one(pool)
    .await?;
    Ok(result.0)
}

/// Get PnL for a trailing window offset by a prior period.
///
/// `from_hours` and `to_hours` define the look-back range relative to now:
/// e.g. `(48, 24)` returns the 24-to-48 hour ago window (the "prior 24h").
pub async fn get_pnl_prev_window(
    pool: &DbPool,
    from_hours: u32,
    to_hours: u32,
) -> AppResult<Decimal> {
    let from_modifier = format!("-{} hours", from_hours);
    let to_modifier = format!("-{} hours", to_hours);
    let result: (Option<f64>,) = sqlx::query_as(
        r#"
        SELECT CAST(COALESCE(SUM(realized_pnl_sol), 0) AS REAL)
        FROM positions
        WHERE state = 'CLOSED'
        AND closed_at >= datetime('now', ?)
        AND closed_at < datetime('now', ?)
        "#,
    )
    .bind(&from_modifier)
    .bind(&to_modifier)
    .fetch_one(pool)
    .await?;
    Ok(Decimal::from_f64_retain(result.0.unwrap_or(0.0)).unwrap_or(Decimal::ZERO))
}

/// Get total PnL for the last 24 hours
/// Uses positions.realized_pnl_sol (the field actually populated by close_position)
pub async fn get_pnl_24h(pool: &DbPool) -> AppResult<Decimal> {
    let result: (Option<f64>,) = sqlx::query_as(
        r#"
        SELECT CAST(COALESCE(SUM(realized_pnl_sol), 0) AS REAL)
        FROM positions
        WHERE state = 'CLOSED'
        AND closed_at >= datetime('now', '-24 hours')
        "#,
    )
    .fetch_one(pool)
    .await?;

    Ok(Decimal::from_f64_retain(result.0.unwrap_or(0.0)).unwrap_or(Decimal::ZERO))
}

/// Get total PnL for the last 7 days
pub async fn get_pnl_7d(pool: &DbPool) -> AppResult<Decimal> {
    let result: (Option<f64>,) = sqlx::query_as(
        r#"
        SELECT CAST(COALESCE(SUM(realized_pnl_sol), 0) AS REAL)
        FROM positions
        WHERE state = 'CLOSED'
        AND closed_at >= datetime('now', '-7 days')
        "#,
    )
    .fetch_one(pool)
    .await?;

    Ok(Decimal::from_f64_retain(result.0.unwrap_or(0.0)).unwrap_or(Decimal::ZERO))
}

/// Get total PnL for the last 30 days
pub async fn get_pnl_30d(pool: &DbPool) -> AppResult<Decimal> {
    let result: (Option<f64>,) = sqlx::query_as(
        r#"
        SELECT CAST(COALESCE(SUM(realized_pnl_sol), 0) AS REAL)
        FROM positions
        WHERE state = 'CLOSED'
        AND closed_at >= datetime('now', '-30 days')
        "#,
    )
    .fetch_one(pool)
    .await?;

    Ok(Decimal::from_f64_retain(result.0.unwrap_or(0.0)).unwrap_or(Decimal::ZERO))
}

/// Get strategy performance metrics (win rate, avg return, trade count)
pub async fn get_strategy_performance(
    pool: &DbPool,
    strategy: &str,
    days: i64,
) -> AppResult<(f64, Decimal, u32)> {
    // Validate days to prevent negative or unreasonably large lookback windows
    let days = days.clamp(1, 365);
    let days_interval = format!("-{} days", days);

    let trades: Vec<(f64,)> = sqlx::query_as(
        r#"
        SELECT COALESCE(net_pnl_sol, 0)
        FROM trades
        WHERE status = 'CLOSED'
        AND strategy = ?
        AND created_at >= datetime('now', ?)
        ORDER BY created_at DESC
        "#,
    )
    .bind(strategy)
    .bind(&days_interval)
    .fetch_all(pool)
    .await?;

    if trades.is_empty() {
        return Ok((0.0, Decimal::ZERO, 0));
    }

    let mut total_pnl = Decimal::ZERO;
    let mut winning_trades = 0u32;
    let total_trades = trades.len() as u32;

    for (pnl_f64,) in trades {
        let pnl = Decimal::from_f64_retain(pnl_f64).unwrap_or(Decimal::ZERO);
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

/// Get count of consecutive losses
pub async fn get_consecutive_losses(pool: &DbPool) -> AppResult<u32> {
    // Get the most recent closed trades and count consecutive losses.
    // trades.pnl_usd is never written (inserts omit it); use positions.realized_pnl_sol
    // joined on trade_uuid instead — this is the same column used by get_pnl_24h.
    let trades: Vec<(Option<f64>,)> = sqlx::query_as(
        r#"
        SELECT p.realized_pnl_sol
        FROM trades t
        JOIN positions p ON p.trade_uuid = t.trade_uuid
        WHERE t.status = 'CLOSED'
        ORDER BY t.created_at DESC
        LIMIT 20
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut consecutive = 0u32;
    for (pnl_opt,) in trades {
        // Convert f64 to Decimal for precise financial comparison
        let pnl = if let Some(pnl_f64) = pnl_opt {
            Decimal::from_f64_retain(pnl_f64).unwrap_or(Decimal::ZERO)
        } else {
            Decimal::ZERO
        };

        // Use Decimal comparison to avoid floating point precision issues
        if pnl < Decimal::ZERO {
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
    tip_amount_sol: Decimal,
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
    .bind(tip_amount_sol.to_f64().unwrap_or(0.0))
    .bind(bundle_signature)
    .bind(strategy)
    .bind(if success { 1 } else { 0 })
    .execute(pool)
    .await?;

    Ok(result.last_insert_rowid())
}

/// Get recent successful tips for percentile calculation
/// Returns tip amounts in descending order (most recent first)
pub async fn get_recent_tips(pool: &DbPool, limit: u32) -> AppResult<Vec<Decimal>> {
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

    Ok(tips
        .into_iter()
        .map(|(t,)| Decimal::from_f64_retain(t).unwrap_or(Decimal::ZERO))
        .collect())
}

/// Get count of successful tips (for cold start detection)
pub async fn get_tip_count(pool: &DbPool) -> AppResult<u32> {
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM jito_tip_history WHERE success = 1")
        .fetch_one(pool)
        .await?;

    Ok(count.0 as u32)
}

/// Clean up old tip history (keep only last 7 days)
pub async fn prune_old_tips(pool: &DbPool) -> AppResult<u64> {
    let result =
        sqlx::query("DELETE FROM jito_tip_history WHERE created_at < datetime('now', '-7 days')")
            .execute(pool)
            .await?;

    Ok(result.rows_affected())
}

// =============================================================================
// POSITIONS & STUCK STATE RECOVERY
// =============================================================================

/// Create a new position from a successful buy trade
#[allow(clippy::too_many_arguments)]
pub async fn open_position(
    pool: &DbPool,
    trade_uuid: &str,
    wallet_address: &str,
    token_address: &str,
    token_symbol: Option<&str>,
    strategy: &str,
    amount_sol: Decimal,
    entry_price: Decimal,
    signature: &str,
) -> AppResult<i64> {
    let result = sqlx::query(
        r#"
        INSERT INTO positions (
            trade_uuid, wallet_address, token_address, token_symbol, strategy,
            entry_amount_sol, entry_price, entry_tx_signature, 
            state, unrealized_pnl_sol, unrealized_pnl_percent
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'ACTIVE', 0, 0)
        "#,
    )
    .bind(trade_uuid)
    .bind(wallet_address)
    .bind(token_address)
    .bind(token_symbol)
    .bind(strategy)
    .bind(amount_sol.to_f64().unwrap_or(0.0))
    .bind(entry_price.to_f64().unwrap_or(0.0))
    .bind(signature)
    .execute(pool)
    .await?;

    Ok(result.last_insert_rowid())
}

/// Atomically mark a trade ACTIVE and insert the corresponding position row.
///
/// If either the trade status update or the position insert fails, the whole
/// transaction is rolled back — preventing a dangling ACTIVE trade with no
/// position row (which the position monitor would silently miss).
///
/// `max_heat_sol`: if provided, a final portfolio-heat guard is checked inside
/// the transaction before the INSERT, preventing a race where two concurrent
/// signals both pass the pre-execution heat check then both commit positions
/// that together exceed the limit.
pub async fn activate_trade_and_open_position(
    pool: &DbPool,
    trade_uuid: &str,
    wallet_address: &str,
    token_address: &str,
    token_symbol: Option<&str>,
    strategy: &str,
    amount_sol: Decimal,
    entry_price: Decimal,
    tx_signature: &str,
    max_heat_sol: Option<Decimal>,
) -> AppResult<()> {
    let mut tx = pool.begin().await?;

    // Final heat guard inside the write transaction: serialize concurrent BUY commits.
    if let Some(limit) = max_heat_sol {
        let current_exposure: f64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(entry_amount_sol), 0.0) FROM positions WHERE state IN ('ACTIVE', 'EXITING')",
        )
        .fetch_one(&mut *tx)
        .await?;
        let current = Decimal::from_f64_retain(current_exposure).unwrap_or(Decimal::ZERO);
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
            entry_amount_sol, entry_price, entry_tx_signature,
            state, unrealized_pnl_sol, unrealized_pnl_percent
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'ACTIVE', 0, 0)
        "#,
    )
    .bind(trade_uuid)
    .bind(wallet_address)
    .bind(token_address)
    .bind(token_symbol)
    .bind(strategy)
    .bind(amount_sol.to_f64().unwrap_or(0.0))
    .bind(entry_price.to_f64().unwrap_or(0.0))
    .bind(tx_signature)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

/// Close a position from a successful sell trade.
///
/// Computes realized PnL from entry vs exit price, writes it to the `positions` table,
/// and propagates net PnL (gross PnL minus recorded trade costs) to `trades.net_pnl_sol`.
/// Close (or partially close) open positions for a wallet+token pair.
///
/// `exit_fraction` is in (0, 1]: 1.0 = full close, 0.25 = sell 25% of each position.
/// Partial closes (< 1.0) reduce `entry_amount_sol` in-place and keep the position ACTIVE.
///
/// All position updates are wrapped in a single transaction so concurrent exit signals for the
/// same (wallet, token) pair cannot double-close the same position. Each UPDATE includes a
/// state guard (`AND state IN ('ACTIVE', 'EXITING')`) so that a position already CLOSED by a
/// concurrent call is silently skipped rather than overwritten.
pub async fn close_position(
    pool: &DbPool,
    token_address: &str,
    wallet_address: &str,
    exit_price: Decimal,
    signature: &str,
    trade_uuid: &str,
    sol_price_usd: Option<Decimal>,
    exit_fraction: Decimal,
) -> AppResult<()> {
    if exit_fraction <= Decimal::ZERO || exit_fraction > Decimal::ONE {
        tracing::warn!(
            trade_uuid = %trade_uuid,
            exit_fraction = %exit_fraction,
            "exit_fraction out of range (0, 1] — clamping; check caller"
        );
    }
    let exit_fraction = exit_fraction.max(Decimal::ZERO).min(Decimal::ONE);

    // Begin a transaction so concurrent close_position calls for the same pair serialize.
    let mut tx = pool.begin().await?;

    // Find all ACTIVE (or EXITING) positions for this wallet+token.
    // Include trade_uuid so we can fetch each position's entry-leg costs.
    let active_positions: Vec<(i64, f64, f64, String)> = sqlx::query_as(
        r#"
        SELECT id, entry_price, entry_amount_sol, trade_uuid
        FROM positions
        WHERE wallet_address = ? AND token_address = ? AND state IN ('ACTIVE', 'EXITING')
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
        return Ok(());
    }

    // Fetch exit-leg costs (jito tip, dex fee, slippage for the exit trade)
    let exit_costs: Option<(f64, f64, f64)> = sqlx::query_as(
        "SELECT jito_tip_sol, dex_fee_sol, slippage_cost_sol FROM trades WHERE trade_uuid = ?",
    )
    .bind(trade_uuid)
    .fetch_optional(&mut *tx)
    .await?;

    let (exit_tip, exit_dex_fee, exit_slippage) = exit_costs
        .map(|(t, d, s)| {
            (
                Decimal::from_f64_retain(t).unwrap_or(Decimal::ZERO),
                Decimal::from_f64_retain(d).unwrap_or(Decimal::ZERO),
                Decimal::from_f64_retain(s).unwrap_or(Decimal::ZERO),
            )
        })
        .unwrap_or((Decimal::ZERO, Decimal::ZERO, Decimal::ZERO));

    let exit_total_costs = exit_tip + exit_dex_fee + exit_slippage;

    // When SOL/USD price is unavailable, write NULL for the USD column rather than a stale
    // hardcoded value. The SOL-denominated PnL remains accurate regardless.
    if sol_price_usd.is_none() {
        tracing::warn!(
            trade_uuid = %trade_uuid,
            "SOL/USD price unavailable — realized_pnl_usd will be NULL for this close"
        );
    }

    // Bulk-fetch entry-leg costs for all positions in a single query to avoid N+1.
    // Collect the entry trade_uuids first, then issue one SELECT ... WHERE trade_uuid IN (...).
    let entry_uuids: Vec<String> = active_positions
        .iter()
        .map(|(_, _, _, uuid)| uuid.clone())
        .collect();

    // Build a HashMap<trade_uuid, (jito_tip, dex_fee, slippage)> from one round-trip.
    let mut entry_costs_map: HashMap<String, (f64, f64, f64)> = HashMap::new();
    if !entry_uuids.is_empty() {
        // sqlx does not support dynamic IN-list binding directly, so build the
        // parameterised SQL with the correct number of placeholders.
        let placeholders = entry_uuids
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(", ");
        let bulk_sql = format!(
            "SELECT trade_uuid, jito_tip_sol, dex_fee_sol, slippage_cost_sol FROM trades WHERE trade_uuid IN ({})",
            placeholders
        );
        let mut bulk_q = sqlx::query_as::<_, (String, f64, f64, f64)>(&bulk_sql);
        for uuid in &entry_uuids {
            bulk_q = bulk_q.bind(uuid);
        }
        let cost_rows: Vec<(String, f64, f64, f64)> = bulk_q.fetch_all(&mut *tx).await?;
        for (uuid, tip, dex, slip) in cost_rows {
            entry_costs_map.insert(uuid, (tip, dex, slip));
        }
    }

    let mut gross_pnl = Decimal::ZERO;
    let mut entry_total_costs = Decimal::ZERO;

    let is_full_close = exit_fraction >= Decimal::ONE;

    for (id, entry_price_f64, entry_amount_sol_f64, entry_trade_uuid) in active_positions.iter() {
        let entry_price_dec = Decimal::from_f64_retain(*entry_price_f64).unwrap_or(Decimal::ZERO);
        let entry_amount_dec =
            Decimal::from_f64_retain(*entry_amount_sol_f64).unwrap_or(Decimal::ZERO);

        // Scale exit amount by fraction — partial exits only realise a portion of the position
        let exited_amount = entry_amount_dec * exit_fraction;

        let pnl_sol = if !entry_price_dec.is_zero() {
            let diff = exit_price - entry_price_dec;
            let ratio = diff / entry_price_dec;
            ratio * exited_amount
        } else {
            Decimal::ZERO
        };

        // Scale entry-leg costs proportionally to the fraction being exited (from pre-fetched map)
        if let Some(&(et, ed, es)) = entry_costs_map.get(entry_trade_uuid.as_str()) {
            let proportional_entry_cost = (Decimal::from_f64_retain(et).unwrap_or(Decimal::ZERO)
                + Decimal::from_f64_retain(ed).unwrap_or(Decimal::ZERO)
                + Decimal::from_f64_retain(es).unwrap_or(Decimal::ZERO))
                * exit_fraction;
            entry_total_costs += proportional_entry_cost;
        }

        // USD PnL is None when SOL price unavailable — stored as NULL in DB
        let pnl_usd_opt: Option<f64> = sol_price_usd
            .map(|sol_usd| (pnl_sol * sol_usd).to_f64().unwrap_or(0.0));

        if is_full_close {
            // State guard: skip positions already CLOSED by a concurrent call
            let rows = sqlx::query(
                r#"
                UPDATE positions
                SET
                    exit_price = ?,
                    exit_tx_signature = ?,
                    realized_pnl_sol = ?,
                    realized_pnl_usd = ?,
                    closed_at = CURRENT_TIMESTAMP,
                    state = 'CLOSED'
                WHERE id = ? AND state IN ('ACTIVE', 'EXITING')
                "#,
            )
            .bind(exit_price.to_f64().unwrap_or(0.0))
            .bind(signature)
            .bind(pnl_sol.to_f64().unwrap_or(0.0))
            .bind(pnl_usd_opt)
            .bind(id)
            .execute(&mut *tx)
            .await?;

            if rows.rows_affected() == 0 {
                tracing::warn!(position_id = id, "Position already closed by concurrent call — skipping");
                continue;
            }
        } else {
            // Partial close: reduce remaining amount, keep position ACTIVE.
            // Accumulate realized_pnl_sol so repeated partial exits build up the correct total.
            let remaining_amount = entry_amount_dec - exited_amount;
            let rows = sqlx::query(
                r#"
                UPDATE positions
                SET
                    entry_amount_sol = ?,
                    exit_price = ?,
                    exit_tx_signature = ?,
                    realized_pnl_sol = COALESCE(realized_pnl_sol, 0.0) + ?,
                    realized_pnl_usd = CASE
                        WHEN ? IS NOT NULL THEN COALESCE(realized_pnl_usd, 0.0) + ?
                        ELSE realized_pnl_usd
                    END,
                    last_updated = CURRENT_TIMESTAMP
                WHERE id = ? AND state IN ('ACTIVE', 'EXITING')
                "#,
            )
            .bind(remaining_amount.to_f64().unwrap_or(0.0))
            .bind(exit_price.to_f64().unwrap_or(0.0))
            .bind(signature)
            .bind(pnl_sol.to_f64().unwrap_or(0.0))
            .bind(pnl_usd_opt)
            .bind(pnl_usd_opt)
            .bind(id)
            .execute(&mut *tx)
            .await?;

            if rows.rows_affected() == 0 {
                tracing::warn!(position_id = id, "Position already closed by concurrent call — skipping partial close");
                continue;
            }
        }

        gross_pnl += pnl_sol;
    }

    // Accumulate net_pnl_sol rather than overwriting — each partial exit contributes its
    // portion so the trades row reflects the total realised PnL across all exit legs.
    let net_pnl = gross_pnl - entry_total_costs - exit_total_costs;
    sqlx::query("UPDATE trades SET net_pnl_sol = COALESCE(net_pnl_sol, 0.0) + ? WHERE trade_uuid = ?")
        .bind(net_pnl.to_f64().unwrap_or(0.0))
        .bind(trade_uuid)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    Ok(())
}

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
pub async fn get_stuck_positions(
    pool: &DbPool,
    stuck_seconds: i64,
) -> AppResult<Vec<PositionRecord>> {
    #[allow(clippy::type_complexity)]
    let modifier = format!("-{} seconds", stuck_seconds);
    let positions: Vec<(
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
    .fetch_all(pool)
    .await?;

    positions
        .into_iter()
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

/// Update position state
pub async fn update_position_state(
    pool: &DbPool,
    trade_uuid: &str,
    new_state: &str,
) -> AppResult<()> {
    sqlx::query("UPDATE positions SET state = ? WHERE trade_uuid = ?")
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

/// Get maximum drawdown from peak (for circuit breaker).
/// Includes unrealized losses from open/exiting positions so the circuit breaker
/// trips before a large open loser is closed (not after).
pub async fn get_max_drawdown_percent(pool: &DbPool) -> AppResult<Decimal> {
    let result: (Option<f64>,) = sqlx::query_as(
        r#"
        WITH cumulative_pnl AS (
            SELECT
                closed_at,
                CAST(SUM(COALESCE(realized_pnl_sol, 0)) OVER (ORDER BY closed_at) AS REAL) as running_pnl
            FROM positions
            WHERE state = 'CLOSED'
        ),
        open_unrealized AS (
            -- Sum unrealized PnL for all non-closed positions (negative = loss)
            SELECT COALESCE(SUM(COALESCE(unrealized_pnl_sol, 0)), 0.0) as total_unrealized
            FROM positions
            WHERE state IN ('ACTIVE', 'EXITING')
        ),
        peaks AS (
            SELECT
                COALESCE(MAX(running_pnl), 0.0) as peak_pnl,
                -- current = last realized equity + all open unrealized P&L
                COALESCE((SELECT running_pnl FROM cumulative_pnl ORDER BY closed_at DESC LIMIT 1), 0.0)
                    + (SELECT total_unrealized FROM open_unrealized) as current_pnl
            FROM cumulative_pnl
        )
        SELECT
            CAST(CASE
                WHEN peak_pnl > 0 THEN ((peak_pnl - current_pnl) / peak_pnl) * 100.0
                ELSE 0.0
            END AS REAL) as drawdown_percent
        FROM peaks
        "#,
    )
    .fetch_one(pool)
    .await
    .unwrap_or((Some(0.0),));

    let drawdown = Decimal::from_f64_retain(result.0.unwrap_or(0.0)).unwrap_or(Decimal::ZERO);
    Ok(drawdown.max(Decimal::ZERO))
}

/// Get active positions count
pub async fn get_active_positions_count(pool: &DbPool) -> AppResult<u32> {
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM positions WHERE state = 'ACTIVE'")
        .fetch_one(pool)
        .await?;

    Ok(count.0 as u32)
}

/// Return trade UUIDs for all ACTIVE and EXITING positions.
/// Used by the HWM sweep to prune stale entries from the momentum_exit map.
pub async fn get_active_trade_uuids(pool: &DbPool) -> AppResult<Vec<String>> {
    let uuids: Vec<String> = sqlx::query_scalar(
        "SELECT trade_uuid FROM positions WHERE state IN ('ACTIVE', 'EXITING')",
    )
    .fetch_all(pool)
    .await?;
    Ok(uuids)
}

/// Active position enriched with entry data for the position monitoring loop
#[derive(Debug, Clone)]
pub struct ActivePositionEntry {
    pub trade_uuid: String,
    pub wallet_address: String,
    pub token_address: String,
    pub token_symbol: String,
    pub strategy: String,
    pub entry_price: Decimal,
    pub entry_amount_sol: Decimal,
    pub entry_time: chrono::DateTime<chrono::Utc>,
}

/// Fetch all ACTIVE positions with their entry data for stop-loss / profit-target monitoring
pub async fn get_active_positions_with_entry(pool: &DbPool) -> AppResult<Vec<ActivePositionEntry>> {
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        String,
        String,
        String,
        Option<String>,
        String,
        f64,
        f64,
        String,
    )> = sqlx::query_as(
        r#"
            SELECT
                p.trade_uuid,
                p.wallet_address,
                p.token_address,
                t.token_symbol,
                p.strategy,
                COALESCE(p.entry_price, 0.0),
                COALESCE(p.entry_amount_sol, 0.0),
                COALESCE(p.opened_at, datetime('now'))
            FROM positions p
            LEFT JOIN trades t ON t.trade_uuid = p.trade_uuid
            WHERE p.state = 'ACTIVE'
            "#,
    )
    .fetch_all(pool)
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
                entry_price_f64,
                entry_amount_f64,
                created_at_str,
            )| {
                let entry_price =
                    Decimal::from_f64_retain(entry_price_f64).unwrap_or(Decimal::ZERO);
                let entry_amount_sol =
                    Decimal::from_f64_retain(entry_amount_f64).unwrap_or(Decimal::ZERO);
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

// =============================================================================
// POSITIONS API
// =============================================================================

/// Position with full details for API response
#[derive(Debug, Clone, serde::Serialize)]
pub struct PositionDetail {
    pub id: i64,
    pub trade_uuid: String,
    pub wallet_address: String,
    pub token_address: String,
    pub token_symbol: Option<String>,
    pub strategy: String,
    pub entry_amount_sol: Decimal,
    pub entry_price: Decimal,
    pub entry_tx_signature: String,
    pub current_price: Option<Decimal>,
    pub unrealized_pnl_sol: Option<Decimal>,
    pub unrealized_pnl_percent: Option<Decimal>,
    pub state: String,
    pub exit_price: Option<Decimal>,
    pub exit_tx_signature: Option<String>,
    pub realized_pnl_sol: Option<Decimal>,
    pub realized_pnl_usd: Option<Decimal>,
    pub opened_at: String,
    pub last_updated: String,
    pub closed_at: Option<String>,
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for PositionDetail {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        // Helper function to convert Option<f64> to Option<Decimal>
        fn f64_to_decimal(val: Option<f64>) -> Option<Decimal> {
            val.map(|v| Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO))
        }

        Ok(PositionDetail {
            id: row.try_get("id")?,
            trade_uuid: row.try_get("trade_uuid")?,
            wallet_address: row.try_get("wallet_address")?,
            token_address: row.try_get("token_address")?,
            token_symbol: row.try_get("token_symbol")?,
            strategy: row.try_get("strategy")?,
            entry_amount_sol: f64_to_decimal(row.try_get("entry_amount_sol")?)
                .unwrap_or(Decimal::ZERO),
            entry_price: f64_to_decimal(row.try_get("entry_price")?).unwrap_or(Decimal::ZERO),
            entry_tx_signature: row.try_get("entry_tx_signature")?,
            current_price: f64_to_decimal(row.try_get("current_price")?),
            unrealized_pnl_sol: f64_to_decimal(row.try_get("unrealized_pnl_sol")?),
            unrealized_pnl_percent: f64_to_decimal(row.try_get("unrealized_pnl_percent")?),
            state: row.try_get("state")?,
            exit_price: f64_to_decimal(row.try_get("exit_price")?),
            exit_tx_signature: row.try_get("exit_tx_signature")?,
            realized_pnl_sol: f64_to_decimal(row.try_get("realized_pnl_sol")?),
            realized_pnl_usd: f64_to_decimal(row.try_get("realized_pnl_usd")?),
            opened_at: row.try_get("opened_at")?,
            last_updated: row.try_get("last_updated")?,
            closed_at: row.try_get("closed_at")?,
        })
    }
}

/// Get all positions with optional state filter
pub async fn get_positions(
    pool: &DbPool,
    state_filter: Option<&str>,
) -> AppResult<Vec<PositionDetail>> {
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
                "#,
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
                "#,
            )
            .fetch_all(pool)
            .await?
        }
    };

    Ok(positions)
}

/// Count active positions
pub async fn count_active_positions(pool: &DbPool) -> AppResult<i64> {
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM positions WHERE state = 'ACTIVE'")
        .fetch_one(pool)
        .await?;

    Ok(count.0)
}

/// Count total trades
pub async fn count_total_trades(pool: &DbPool) -> AppResult<i64> {
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM trades")
        .fetch_one(pool)
        .await?;

    Ok(count.0)
}

/// Get a single position by trade_uuid
pub async fn get_position_by_uuid(
    pool: &DbPool,
    trade_uuid: &str,
) -> AppResult<Option<PositionDetail>> {
    let position = sqlx::query_as::<_, PositionDetail>(
        r#"
        SELECT id, trade_uuid, wallet_address, token_address, token_symbol, strategy,
               entry_amount_sol, entry_price, entry_tx_signature, current_price,
               unrealized_pnl_sol, unrealized_pnl_percent, state, exit_price,
               exit_tx_signature, realized_pnl_sol, realized_pnl_usd,
               opened_at, last_updated, closed_at
        FROM positions
        WHERE trade_uuid = ?
        "#,
    )
    .bind(trade_uuid)
    .fetch_optional(pool)
    .await?;

    Ok(position)
}

/// Lightweight summary for the PnL refresh background task
#[derive(Debug, Clone)]
pub struct ActivePositionSummary {
    pub trade_uuid: String,
    pub token_address: String,
    pub entry_price: Decimal,
    pub entry_amount_sol: Decimal,
}

/// Get the peak price recorded for a position (used by trailing stop / profit-target logic).
///
/// Peak price is stored in `exit_targets`, not `positions` — querying `positions` would
/// always return NULL because that column does not exist on the positions table.
pub async fn get_position_peak_price(pool: &DbPool, trade_uuid: &str) -> AppResult<Option<f64>> {
    let row: Option<(f64,)> =
        sqlx::query_as("SELECT peak_price FROM exit_targets WHERE trade_uuid = ?")
            .bind(trade_uuid)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(p,)| p))
}

/// Get trade_uuid, token_address, entry_price, and size for all ACTIVE/EXITING positions (PnL refresh)
pub async fn get_active_position_tokens(pool: &DbPool) -> AppResult<Vec<ActivePositionSummary>> {
    let rows: Vec<(String, String, f64, f64)> = sqlx::query_as(
        r#"
        SELECT trade_uuid, token_address, entry_price, entry_amount_sol
        FROM positions
        WHERE state IN ('ACTIVE', 'EXITING')
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(uuid, token, price, size)| ActivePositionSummary {
            trade_uuid: uuid,
            token_address: token,
            entry_price: Decimal::from_f64_retain(price).unwrap_or(Decimal::ZERO),
            entry_amount_sol: Decimal::from_f64_retain(size).unwrap_or(Decimal::ZERO),
        })
        .collect())
}

/// Update current_price, unrealized_pnl_sol, and unrealized_pnl_percent for active positions
pub async fn update_position_unrealized_pnl(
    pool: &DbPool,
    trade_uuid: &str,
    current_price: Decimal,
    pnl_sol: Decimal,
    pnl_pct: Decimal,
) -> AppResult<()> {
    let current_f64 = current_price.to_f64().unwrap_or(0.0);
    let pnl_sol_f64 = pnl_sol.to_f64().unwrap_or(0.0);
    let pnl_pct_f64 = pnl_pct.to_f64().unwrap_or(0.0);
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
    .bind(current_f64)
    .bind(pnl_sol_f64)
    .bind(pnl_pct_f64)
    .bind(trade_uuid)
    .execute(pool)
    .await?;

    Ok(())
}

// =============================================================================
// WALLETS API
// =============================================================================

/// Wallet with full details for API response
#[derive(Debug, Clone, serde::Serialize)]
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
    pub avg_trade_size_sol: Option<Decimal>,
    pub avg_win_sol: Option<Decimal>,
    pub avg_loss_sol: Option<Decimal>,
    pub profit_factor: Option<f64>,
    pub realized_pnl_30d_sol: Option<Decimal>,
    pub last_trade_at: Option<String>,
    pub promoted_at: Option<String>,
    pub ttl_expires_at: Option<String>,
    pub notes: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for WalletDetail {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        // Helper function to convert Option<f64> to Option<Decimal>
        fn f64_to_decimal(val: Option<f64>) -> Option<Decimal> {
            val.map(|v| Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO))
        }

        Ok(WalletDetail {
            id: row.try_get("id")?,
            address: row.try_get("address")?,
            status: row.try_get("status")?,
            wqs_score: row.try_get("wqs_score")?,
            roi_7d: row.try_get("roi_7d")?,
            roi_30d: row.try_get("roi_30d")?,
            trade_count_30d: row.try_get("trade_count_30d")?,
            win_rate: row.try_get("win_rate")?,
            max_drawdown_30d: row.try_get("max_drawdown_30d")?,
            avg_trade_size_sol: f64_to_decimal(row.try_get("avg_trade_size_sol")?),
            avg_win_sol: f64_to_decimal(row.try_get("avg_win_sol")?),
            avg_loss_sol: f64_to_decimal(row.try_get("avg_loss_sol")?),
            profit_factor: row.try_get("profit_factor")?,
            realized_pnl_30d_sol: f64_to_decimal(row.try_get("realized_pnl_30d_sol")?),
            last_trade_at: row.try_get("last_trade_at")?,
            promoted_at: row.try_get("promoted_at")?,
            ttl_expires_at: row.try_get("ttl_expires_at")?,
            notes: row.try_get("notes")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

/// Get all wallets with optional status filter
pub async fn get_wallets(
    pool: &DbPool,
    status_filter: Option<&str>,
) -> AppResult<Vec<WalletDetail>> {
    let wallets = match status_filter {
        Some(status) => {
            sqlx::query_as::<_, WalletDetail>(
                r#"
                SELECT id, address, status, wqs_score, roi_7d, roi_30d, trade_count_30d,
                       win_rate, max_drawdown_30d, avg_trade_size_sol,
                       avg_win_sol, avg_loss_sol, profit_factor, realized_pnl_30d_sol,
                       last_trade_at,
                       promoted_at, ttl_expires_at, notes, created_at, updated_at
                FROM wallets
                WHERE status = ?
                ORDER BY wqs_score DESC NULLS LAST
                LIMIT 1000
                "#,
            )
            .bind(status)
            .fetch_all(pool)
            .await?
        }
        None => {
            sqlx::query_as::<_, WalletDetail>(
                r#"
                SELECT id, address, status, wqs_score, roi_7d, roi_30d, trade_count_30d,
                       win_rate, max_drawdown_30d, avg_trade_size_sol,
                       avg_win_sol, avg_loss_sol, profit_factor, realized_pnl_30d_sol,
                       last_trade_at,
                       promoted_at, ttl_expires_at, notes, created_at, updated_at
                FROM wallets
                ORDER BY wqs_score DESC NULLS LAST
                LIMIT 1000
                "#,
            )
            .fetch_all(pool)
            .await?
        }
    };

    Ok(wallets)
}

/// Get a single wallet by address
pub async fn get_wallet_by_address(
    pool: &DbPool,
    address: &str,
) -> AppResult<Option<WalletDetail>> {
    let wallet = sqlx::query_as::<_, WalletDetail>(
        r#"
        SELECT id, address, status, wqs_score, roi_7d, roi_30d, trade_count_30d,
               win_rate, max_drawdown_30d, avg_trade_size_sol,
               avg_win_sol, avg_loss_sol, profit_factor, realized_pnl_30d_sol,
               last_trade_at,
               promoted_at, ttl_expires_at, notes, created_at, updated_at
        FROM wallets
        WHERE address = ?
        "#,
    )
    .bind(address)
    .fetch_optional(pool)
    .await?;

    Ok(wallet)
}

/// Add or update a wallet in the database (atomic upsert — no TOCTOU race).
///
/// If the wallet doesn't exist, it will be created with CANDIDATE status.
/// If it exists, all mutable metric fields are updated atomically via
/// INSERT … ON CONFLICT(address) DO UPDATE SET — no separate SELECT required.
#[allow(clippy::too_many_arguments)]
pub async fn upsert_wallet(
    pool: &DbPool,
    address: &str,
    wqs_score: Option<f64>,
    roi_7d: Option<f64>,
    roi_30d: Option<f64>,
    trade_count_30d: Option<i32>,
    win_rate: Option<f64>,
    max_drawdown_30d: Option<f64>,
    avg_trade_size_sol: Option<Decimal>,
    last_trade_at: Option<&str>,
    notes: Option<&str>,
) -> AppResult<bool> {
    let avg_trade_size_f64 = avg_trade_size_sol.map(|d| d.to_f64().unwrap_or(0.0));

    let result = sqlx::query(
        r#"
        INSERT INTO wallets (
            address, status, wqs_score, roi_7d, roi_30d,
            trade_count_30d, win_rate, max_drawdown_30d,
            avg_trade_size_sol, last_trade_at, notes,
            created_at, updated_at
        )
        VALUES (?, 'CANDIDATE', ?, ?, ?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
        ON CONFLICT(address) DO UPDATE SET
            wqs_score        = COALESCE(excluded.wqs_score, wqs_score),
            roi_7d           = COALESCE(excluded.roi_7d, roi_7d),
            roi_30d          = COALESCE(excluded.roi_30d, roi_30d),
            trade_count_30d  = COALESCE(excluded.trade_count_30d, trade_count_30d),
            win_rate         = COALESCE(excluded.win_rate, win_rate),
            max_drawdown_30d = COALESCE(excluded.max_drawdown_30d, max_drawdown_30d),
            avg_trade_size_sol = COALESCE(excluded.avg_trade_size_sol, avg_trade_size_sol),
            last_trade_at    = COALESCE(excluded.last_trade_at, last_trade_at),
            notes            = COALESCE(excluded.notes, notes),
            updated_at       = CURRENT_TIMESTAMP
        "#,
    )
    .bind(address)
    .bind(wqs_score)
    .bind(roi_7d)
    .bind(roi_30d)
    .bind(trade_count_30d)
    .bind(win_rate)
    .bind(max_drawdown_30d)
    .bind(avg_trade_size_f64)
    .bind(last_trade_at)
    .bind(notes)
    .execute(pool)
    .await?;

    // rows_affected == 1 on INSERT (new row), == 2 on conflict UPDATE in SQLite
    Ok(result.rows_affected() != 1)
}

/// Update wallet status with optional TTL
pub async fn update_wallet_status(
    pool: &DbPool,
    address: &str,
    status: &str,
    ttl_hours: Option<i64>,
    reason: Option<&str>,
) -> AppResult<bool> {
    let ttl_expires_at = ttl_hours.map(|hours| chrono::Utc::now() + chrono::Duration::hours(hours));

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
        "#,
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
        "#,
    )
    .bind(reason)
    .bind(address)
    .execute(pool)
    .await?;

    Ok(())
}

/// Get wallet copy performance metrics
pub async fn get_wallet_copy_performance(
    pool: &DbPool,
    wallet_address: &str,
) -> AppResult<Option<WalletCopyPerformance>> {
    let result = sqlx::query_as::<_, WalletCopyPerformance>(
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
    .fetch_optional(pool)
    .await?;

    Ok(result)
}

/// Wallet copy performance metrics from database
#[derive(Debug, Clone, serde::Serialize)]
pub struct WalletCopyPerformance {
    pub wallet_address: String,
    pub copy_pnl_7d: Decimal,
    pub copy_pnl_30d: Decimal,
    pub signal_success_rate: f64,
    pub avg_return_per_trade: Decimal,
    pub total_trades: i32,
    pub winning_trades: i32,
    pub last_updated: String,
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for WalletCopyPerformance {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        // Helper function to convert Option<f64> to Option<Decimal>
        fn f64_to_decimal(val: Option<f64>) -> Option<Decimal> {
            val.map(|v| Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO))
        }

        Ok(WalletCopyPerformance {
            wallet_address: row.try_get("wallet_address")?,
            copy_pnl_7d: f64_to_decimal(row.try_get("copy_pnl_7d")?).unwrap_or(Decimal::ZERO),
            copy_pnl_30d: f64_to_decimal(row.try_get("copy_pnl_30d")?).unwrap_or(Decimal::ZERO),
            signal_success_rate: row.try_get("signal_success_rate")?,
            avg_return_per_trade: f64_to_decimal(row.try_get("avg_return_per_trade")?)
                .unwrap_or(Decimal::ZERO),
            total_trades: row.try_get("total_trades")?,
            winning_trades: row.try_get("winning_trades")?,
            last_updated: row.try_get("last_updated")?,
        })
    }
}

/// Get wallet monitoring information
pub async fn get_wallet_monitoring(
    pool: &DbPool,
    wallet_address: &str,
) -> AppResult<Option<WalletMonitoring>> {
    let result = sqlx::query_as::<_, WalletMonitoring>(
        r#"
        SELECT 
            wallet_address,
            helius_webhook_id,
            rpc_polling_active,
            last_transaction_signature,
            last_monitored_at,
            monitoring_enabled,
            created_at,
            updated_at
        FROM wallet_monitoring
        WHERE wallet_address = ?
        "#,
    )
    .bind(wallet_address)
    .fetch_optional(pool)
    .await?;

    Ok(result)
}

/// Wallet monitoring information from database
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct WalletMonitoring {
    pub wallet_address: String,
    pub helius_webhook_id: Option<String>,
    pub rpc_polling_active: i32,
    pub last_transaction_signature: Option<String>,
    pub last_monitored_at: Option<String>,
    pub monitoring_enabled: i32,
    pub created_at: String,
    pub updated_at: String,
}

/// Update wallet monitoring last transaction signature
pub async fn update_wallet_monitoring_signature(
    pool: &DbPool,
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
    .execute(pool)
    .await?;

    Ok(())
}

/// Insert or update wallet monitoring record
pub async fn upsert_wallet_monitoring(
    pool: &DbPool,
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
    .execute(pool)
    .await?;

    Ok(())
}

/// Get wallet monitoring by address
pub async fn get_wallet_monitoring_by_address(
    pool: &DbPool,
    wallet_address: &str,
) -> AppResult<Option<WalletMonitoring>> {
    let result = sqlx::query_as::<_, WalletMonitoring>(
        r#"
        SELECT 
            wallet_address,
            helius_webhook_id,
            rpc_polling_active,
            last_transaction_signature,
            last_monitored_at,
            monitoring_enabled,
            created_at,
            updated_at
        FROM wallet_monitoring
        WHERE wallet_address = ?
        "#,
    )
    .bind(wallet_address)
    .fetch_optional(pool)
    .await?;

    Ok(result)
}

// =============================================================================
// TRADES API / EXPORT
// =============================================================================

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
    pub amount_sol: Decimal,
    pub price_at_signal: Option<Decimal>,
    pub tx_signature: Option<String>,
    pub status: String,
    pub retry_count: i32,
    pub error_message: Option<String>,
    pub pnl_sol: Option<Decimal>,
    pub pnl_usd: Option<Decimal>,
    pub jito_tip_sol: Option<Decimal>,
    pub dex_fee_sol: Option<Decimal>,
    pub slippage_cost_sol: Option<Decimal>,
    pub total_cost_sol: Option<Decimal>,
    pub net_pnl_sol: Option<Decimal>,
    pub created_at: String,
    pub updated_at: String,
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for TradeDetail {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        // Helper function to convert Option<f64> to Option<Decimal>
        fn f64_to_decimal(val: Option<f64>) -> Option<Decimal> {
            val.map(|v| Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO))
        }

        Ok(TradeDetail {
            id: row.try_get("id")?,
            trade_uuid: row.try_get("trade_uuid")?,
            wallet_address: row.try_get("wallet_address")?,
            token_address: row.try_get("token_address")?,
            token_symbol: row.try_get("token_symbol")?,
            strategy: row.try_get("strategy")?,
            side: row.try_get("side")?,
            amount_sol: f64_to_decimal(row.try_get("amount_sol")?).unwrap_or(Decimal::ZERO),
            price_at_signal: f64_to_decimal(row.try_get("price_at_signal")?),
            tx_signature: row.try_get("tx_signature")?,
            status: row.try_get("status")?,
            retry_count: row.try_get("retry_count")?,
            error_message: row.try_get("error_message")?,
            pnl_sol: f64_to_decimal(row.try_get("pnl_sol")?),
            pnl_usd: f64_to_decimal(row.try_get("pnl_usd")?),
            jito_tip_sol: f64_to_decimal(row.try_get("jito_tip_sol")?),
            dex_fee_sol: f64_to_decimal(row.try_get("dex_fee_sol")?),
            slippage_cost_sol: f64_to_decimal(row.try_get("slippage_cost_sol")?),
            total_cost_sol: f64_to_decimal(row.try_get("total_cost_sol")?),
            net_pnl_sol: f64_to_decimal(row.try_get("net_pnl_sol")?),
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

/// Get trades with optional filters for API and export
#[allow(clippy::too_many_arguments)]
pub async fn get_trades(
    pool: &DbPool,
    from_date: Option<&str>,
    to_date: Option<&str>,
    status_filter: Option<&str>,
    strategy_filter: Option<&str>,
    wallet_address_filter: Option<&str>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> AppResult<Vec<TradeDetail>> {
    // Build query dynamically based on filters
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

    if let Some(wallet_address) = wallet_address_filter {
        query.push_str(" AND wallet_address = ?");
        bindings.push(wallet_address.to_string());
    }

    query.push_str(" ORDER BY created_at DESC");
    query.push_str(" LIMIT ? OFFSET ?");

    // Execute with bindings
    let mut q = sqlx::query_as::<_, TradeDetail>(&query);

    for binding in bindings {
        q = q.bind(binding);
    }

    // Bind LIMIT and OFFSET as parameters to avoid SQL injection via format!()
    let lim = limit.unwrap_or(1000);
    let off = offset.unwrap_or(0);
    q = q.bind(lim).bind(off);

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

    if let Some(wallet_address) = wallet_address_filter {
        query.push_str(" AND wallet_address = ?");
        bindings.push(wallet_address.to_string());
    }

    let mut q = sqlx::query_as::<_, (i64,)>(&query);

    for binding in bindings {
        q = q.bind(binding);
    }

    let (count,) = q.fetch_one(pool).await?;
    Ok(count)
}

/// Generate CSV content from trades
/// Escape a string for CSV output: wrap in quotes if it contains commas, quotes, or newlines.
/// Internal quotes are doubled per RFC 4180.
fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

pub fn trades_to_csv(trades: &[TradeDetail]) -> String {
    let mut csv = String::new();

    // Header
    csv.push_str("id,trade_uuid,wallet_address,token_address,token_symbol,strategy,side,amount_sol,price_at_signal,tx_signature,status,pnl_sol,pnl_usd,jito_tip_sol,dex_fee_sol,slippage_cost_sol,total_cost_sol,net_pnl_sol,created_at\n");

    // Data rows
    for trade in trades {
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            trade.id,
            trade.trade_uuid,
            trade.wallet_address,
            trade.token_address,
            csv_escape(trade.token_symbol.as_deref().unwrap_or("")),
            trade.strategy,
            trade.side,
            trade.amount_sol,
            trade
                .price_at_signal
                .map(|p| p.to_string())
                .unwrap_or_default(),
            trade.tx_signature.as_deref().unwrap_or(""),
            trade.status,
            trade.pnl_sol.map(|p| p.to_string()).unwrap_or_default(),
            trade.pnl_usd.map(|p| p.to_string()).unwrap_or_default(),
            trade
                .jito_tip_sol
                .map(|p| p.to_string())
                .unwrap_or_default(),
            trade.dex_fee_sol.map(|p| p.to_string()).unwrap_or_default(),
            trade
                .slippage_cost_sol
                .map(|p| p.to_string())
                .unwrap_or_default(),
            trade
                .total_cost_sol
                .map(|p| p.to_string())
                .unwrap_or_default(),
            trade.net_pnl_sol.map(|p| p.to_string()).unwrap_or_default(),
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
    // Increased limit to 5000 to support active traders with large trade histories
    // Note: Multi-page PDF support would require additional implementation for page breaks
    let max_rows = 5000;
    let display_trades = if trades.len() > max_rows {
        tracing::warn!(
            total_trades = trades.len(),
            exported_rows = max_rows,
            "PDF export truncated: user has more trades than supported in single page"
        );
        &trades[..max_rows]
    } else {
        trades
    };

    for trade in display_trades {
        if y_pos < 20.0 {
            // Page space exhausted - would require multi-page PDF implementation
            // For now, log warning about incomplete export
            if display_trades.len() > 1 {
                tracing::warn!(
                    rows_attempted = display_trades.len(),
                    "PDF export incomplete: insufficient page space for all rows"
                );
            }
            break;
        }

        let row = format!(
            "{} | {}... | {}... | {} | {} | {} | {:.4} | {} | {:.2} | {}",
            trade.id,
            &trade.trade_uuid[..12.min(trade.trade_uuid.len())],
            &trade.wallet_address[..8.min(trade.wallet_address.len())],
            trade
                .token_symbol
                .as_deref()
                .map(|s| s.chars().take(8).collect::<String>())
                .unwrap_or_else(|| trade.token_address.chars().take(8).collect()),
            trade.strategy,
            trade.side,
            trade.amount_sol.to_f64().unwrap_or(0.0),
            trade.status,
            trade.pnl_usd.and_then(|p| p.to_f64()).unwrap_or(0.0),
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
                items: vec![TextItem::Text(format!(
                    "... and {} more trades",
                    trades.len() - max_rows
                ))],
            },
            Op::EndTextSection,
        ]);
    }

    // Create page with operations
    let page = PdfPage::new(Mm(210.0), Mm(297.0), ops);

    // Add page to document and save
    let mut warnings = Vec::new();
    let bytes = doc
        .with_pages(vec![page])
        .save(&PdfSaveOptions::default(), &mut warnings);

    Ok(bytes)
}

// =============================================================================
// EXIT TARGETS (profit target state persistence)
// =============================================================================

/// Upsert profit target state for a position into exit_targets table
pub async fn upsert_exit_target(
    pool: &DbPool,
    trade_uuid: &str,
    entry_price: f64,
    entry_amount_sol: f64,
    peak_price: f64,
    peak_profit_percent: f64,
    targets_hit_json: &str,
    trailing_stop_active: bool,
    trailing_stop_price: f64,
) -> AppResult<()> {
    sqlx::query(
        r#"
        INSERT INTO exit_targets (
            trade_uuid, entry_price, entry_amount_sol, peak_price,
            peak_profit_percent, targets_hit, trailing_stop_active, trailing_stop_price
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(trade_uuid) DO UPDATE SET
            peak_price = excluded.peak_price,
            peak_profit_percent = excluded.peak_profit_percent,
            targets_hit = excluded.targets_hit,
            trailing_stop_active = excluded.trailing_stop_active,
            trailing_stop_price = excluded.trailing_stop_price,
            last_updated = CURRENT_TIMESTAMP
        "#,
    )
    .bind(trade_uuid)
    .bind(entry_price)
    .bind(entry_amount_sol)
    .bind(peak_price)
    .bind(peak_profit_percent)
    .bind(targets_hit_json)
    .bind(trailing_stop_active as i64)
    .bind(trailing_stop_price)
    .execute(pool)
    .await?;
    Ok(())
}

/// Load saved profit target state for a position
pub async fn load_exit_target(
    pool: &DbPool,
    trade_uuid: &str,
) -> AppResult<Option<(f64, f64, f64, f64, String, bool, f64)>> {
    let row: Option<(f64, f64, f64, f64, String, i64, f64)> = sqlx::query_as(
        r#"
        SELECT entry_price, entry_amount_sol, peak_price, peak_profit_percent,
               COALESCE(targets_hit, '[]'), trailing_stop_active, COALESCE(trailing_stop_price, 0.0)
        FROM exit_targets
        WHERE trade_uuid = ?
        "#,
    )
    .bind(trade_uuid)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|(ep, ea, pp, ppp, th, tsa, tsp)| (ep, ea, pp, ppp, th, tsa != 0, tsp)))
}

/// Delete profit target state for a closed position
pub async fn delete_exit_target(pool: &DbPool, trade_uuid: &str) -> AppResult<()> {
    sqlx::query("DELETE FROM exit_targets WHERE trade_uuid = ?")
        .bind(trade_uuid)
        .execute(pool)
        .await?;
    Ok(())
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
