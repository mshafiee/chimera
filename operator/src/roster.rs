//! Roster merge module for Scout integration
//!
//! Implements SQL-level merge using ATTACH DATABASE to safely import
//! wallet roster updates from the Python Scout without write lock conflicts.
//!
//! The Scout writes to `roster_new.db` and the Operator merges it into
//! the main database using this module.

use crate::db::DbPool;
use crate::error::AppResult;
use chrono::{DateTime, Utc};
use std::path::Path;
use tracing::{error, info, warn};

/// Result of a roster merge operation
#[derive(Debug, Clone)]
pub struct MergeResult {
    /// Number of wallets inserted/updated
    pub wallets_merged: u32,
    /// Number of wallets removed (if any)
    pub wallets_removed: u32,
    /// Whether integrity check passed
    pub integrity_ok: bool,
    /// Timestamp of merge
    pub merged_at: DateTime<Utc>,
    /// Any warnings during merge
    pub warnings: Vec<String>,
}

/// Merge roster from external database file
///
/// This function:
/// 1. Attaches the roster_new.db file
/// 2. Runs integrity check on attached DB
/// 3. Merges wallets in a transaction
/// 4. Detaches the database
///
/// # Arguments
/// * `pool` - Database connection pool
/// * `roster_path` - Path to roster_new.db file
///
/// # Returns
/// * `MergeResult` with statistics about the merge
pub async fn merge_roster(pool: &DbPool, roster_path: &Path) -> AppResult<MergeResult> {
    let mut warnings = Vec::new();

    // Check if roster file exists
    if !roster_path.exists() {
        return Err(crate::error::AppError::Internal(format!(
            "Roster file not found: {}",
            roster_path.display()
        )));
    }

    let roster_path_str = roster_path.to_string_lossy().to_string();

    info!(path = %roster_path_str, "Starting roster merge");

    // Get a connection from the pool
    let mut conn = pool.acquire().await?;

    // Step 1: Attach the new roster database
    let attach_sql = format!("ATTACH DATABASE '{}' AS new_roster", roster_path_str);
    sqlx::query(&attach_sql).execute(&mut *conn).await?;

    info!("Attached roster database");

    // Step 2: Run integrity check on attached database
    let integrity_result: Vec<(String,)> =
        sqlx::query_as("PRAGMA new_roster.integrity_check")
            .fetch_all(&mut *conn)
            .await?;

    let integrity_ok = integrity_result
        .first()
        .map(|(s,)| s == "ok")
        .unwrap_or(false);

    if !integrity_ok {
        // Detach and abort
        let _ = sqlx::query("DETACH DATABASE new_roster")
            .execute(&mut *conn)
            .await;

        error!("Roster integrity check failed: {:?}", integrity_result);
        return Err(crate::error::AppError::Internal(
            "Roster integrity check failed".to_string(),
        ));
    }

    info!("Roster integrity check passed");

    // Step 3: Check if new_roster has wallets table
    let table_check: Result<(i32,), _> = sqlx::query_as(
        "SELECT COUNT(*) FROM new_roster.sqlite_master WHERE type='table' AND name='wallets'"
    )
    .fetch_one(&mut *conn)
    .await;

    if table_check.map(|(c,)| c).unwrap_or(0) == 0 {
        let _ = sqlx::query("DETACH DATABASE new_roster")
            .execute(&mut *conn)
            .await;

        return Err(crate::error::AppError::Internal(
            "Roster database missing 'wallets' table".to_string(),
        ));
    }

    // Step 4: Count wallets in new roster
    let new_wallet_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM new_roster.wallets")
        .fetch_one(&mut *conn)
        .await?;

    let new_count = new_wallet_count.0 as u32;

    if new_count == 0 {
        warnings.push("New roster contains zero wallets".to_string());
        warn!("New roster contains zero wallets - proceeding with merge anyway");
    }

    // Step 5: Get current wallet count (for statistics)
    // Note: With upsert strategy, we don't remove wallets that aren't in Scout's roster.
    // Wallets are only updated/inserted, not deleted. This preserves Operator's ability
    // to ban/demote wallets without Scout immediately reviving them.
    let current_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM wallets")
        .fetch_one(&mut *conn)
        .await?;

    // With upsert, we don't actually remove wallets, so this is 0
    // (wallets not in Scout's roster remain in the DB)
    let wallets_removed = 0u32;

    let batch_size = 500;
    let mut offset = 0;
    
    // Step 6: Merge in batches to prevent locking the DB for too long
    // Using a limit/offset strategy
    
    // Define a struct to hold the row data (sqlx only supports tuples up to 9-16 elements)
    #[derive(sqlx::FromRow)]
    struct RosterTransferRow {
        address: String,
        status: String,
        wqs_score: Option<f64>,
        roi_7d: Option<f64>,
        roi_30d: Option<f64>,
        trade_count_30d: Option<i64>,
        win_rate: Option<f64>,
        max_drawdown_30d: Option<f64>,
        avg_trade_size_sol: Option<f64>,
        avg_win_sol: Option<f64>,
        avg_loss_sol: Option<f64>,
        profit_factor: Option<f64>,
        realized_pnl_30d_sol: Option<f64>,
        last_trade_at: Option<String>, // Read as string to avoid parsing issues during transfer
        promoted_at: Option<String>,
        ttl_expires_at: Option<String>,
        notes: Option<String>,
        archetype: Option<String>,
        avg_entry_delay_seconds: Option<f64>,
        created_at: Option<String>,
        updated_at: Option<String>, // Scout's updated_at timestamp
    }

    loop {
        // Read batch from attached roster
        let rows: Vec<RosterTransferRow> = sqlx::query_as(
            &format!(r#"
                SELECT 
                    address, status, wqs_score, roi_7d, roi_30d,
                    trade_count_30d, win_rate, max_drawdown_30d,
                    avg_trade_size_sol, avg_win_sol, avg_loss_sol, profit_factor, realized_pnl_30d_sol,
                    last_trade_at, promoted_at,
                    ttl_expires_at, notes, archetype, avg_entry_delay_seconds, created_at, updated_at
                FROM new_roster.wallets 
                LIMIT {} OFFSET {}
            "#, batch_size, offset)
        )
        .fetch_all(&mut *conn)
        .await?;

        if rows.is_empty() {
            break;
        }

        // Upsert batch into main DB with updated_at preservation logic
        // This prevents Scout from "reviving" wallets the Operator just banned/demoted
        let mut tx = pool.begin().await?;
        for row in rows {
            // Use INSERT with ON CONFLICT to upsert, preserving Operator's updated_at
            // and status if they're newer than Scout's. This prevents race conditions
            // where Operator bans/demotes a wallet just before Scout writes its roster.
            sqlx::query(
                r#"
                INSERT INTO wallets (
                    address, status, wqs_score, roi_7d, roi_30d,
                    trade_count_30d, win_rate, max_drawdown_30d,
                    avg_trade_size_sol, avg_win_sol, avg_loss_sol, profit_factor, realized_pnl_30d_sol,
                    last_trade_at, promoted_at,
                    ttl_expires_at, notes, archetype, avg_entry_delay_seconds, created_at, updated_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, COALESCE(?, CURRENT_TIMESTAMP))
                ON CONFLICT(address) DO UPDATE SET
                    status = CASE 
                        WHEN wallets.updated_at > COALESCE(excluded.updated_at, '1970-01-01') 
                        THEN wallets.status  -- Preserve Operator's status if newer
                        ELSE excluded.status
                    END,
                    wqs_score = excluded.wqs_score,
                    roi_7d = excluded.roi_7d,
                    roi_30d = excluded.roi_30d,
                    trade_count_30d = excluded.trade_count_30d,
                    win_rate = excluded.win_rate,
                    max_drawdown_30d = excluded.max_drawdown_30d,
                    avg_trade_size_sol = excluded.avg_trade_size_sol,
                    avg_win_sol = excluded.avg_win_sol,
                    avg_loss_sol = excluded.avg_loss_sol,
                    profit_factor = excluded.profit_factor,
                    realized_pnl_30d_sol = excluded.realized_pnl_30d_sol,
                    last_trade_at = excluded.last_trade_at,
                    promoted_at = excluded.promoted_at,
                    ttl_expires_at = excluded.ttl_expires_at,
                    notes = excluded.notes,
                    archetype = excluded.archetype,
                    avg_entry_delay_seconds = excluded.avg_entry_delay_seconds,
                    updated_at = CASE 
                        WHEN wallets.updated_at > COALESCE(excluded.updated_at, '1970-01-01') 
                        THEN wallets.updated_at  -- Preserve Operator's updated_at if newer
                        ELSE excluded.updated_at
                    END
                "#
            )
            .bind(&row.address).bind(&row.status).bind(row.wqs_score).bind(row.roi_7d).bind(row.roi_30d)
            .bind(row.trade_count_30d).bind(row.win_rate).bind(row.max_drawdown_30d)
            .bind(row.avg_trade_size_sol).bind(row.avg_win_sol).bind(row.avg_loss_sol).bind(row.profit_factor).bind(row.realized_pnl_30d_sol)
            .bind(&row.last_trade_at).bind(&row.promoted_at)
            .bind(&row.ttl_expires_at).bind(&row.notes).bind(&row.archetype).bind(row.avg_entry_delay_seconds).bind(&row.created_at)
            .bind(&row.updated_at)
            .execute(&mut *tx)
            .await?;
        }
        
        tx.commit().await?;
        
        offset += batch_size;
        
        // Yield to allow other readers/writers
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        
        info!(
            merged = offset.min(new_count),
            total = new_count,
            "Roster merge progress"
        );
    }
    
    // Log success (no single result object anymore, since we batched)
    info!(
        wallets_merged = new_count,
        wallets_removed = wallets_removed,
        "Roster merge completed"
    );

    // Step 7: Detach the database
    sqlx::query("DETACH DATABASE new_roster")
        .execute(&mut *conn)
        .await?;

    info!("Roster database detached");

    // Log the merge to config_audit
    crate::db::log_config_change(
        pool,
        "roster_merge",
        Some(&format!("{} wallets", wallets_removed)),
        &format!("{} wallets", new_count),
        "SYSTEM_SCOUT",
        Some(&format!("Merged from {}", roster_path_str)),
    )
    .await?;

    Ok(MergeResult {
        wallets_merged: new_count,
        wallets_removed,
        integrity_ok: true,
        merged_at: Utc::now(),
        warnings,
    })
}

/// Check if a roster file is valid (exists and passes integrity check)
pub async fn validate_roster(pool: &DbPool, roster_path: &Path) -> AppResult<bool> {
    if !roster_path.exists() {
        return Ok(false);
    }

    let roster_path_str = roster_path.to_string_lossy().to_string();
    let mut conn = pool.acquire().await?;

    // Attach
    let attach_sql = format!("ATTACH DATABASE '{}' AS check_roster", roster_path_str);
    if sqlx::query(&attach_sql).execute(&mut *conn).await.is_err() {
        return Ok(false);
    }

    // Check integrity
    let integrity_result: Vec<(String,)> =
        sqlx::query_as("PRAGMA check_roster.integrity_check")
            .fetch_all(&mut *conn)
            .await?;

    let is_valid = integrity_result
        .first()
        .map(|(s,)| s == "ok")
        .unwrap_or(false);

    // Detach
    let _ = sqlx::query("DETACH DATABASE check_roster")
        .execute(&mut *conn)
        .await;

    Ok(is_valid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_result_debug() {
        let result = MergeResult {
            wallets_merged: 10,
            wallets_removed: 5,
            integrity_ok: true,
            merged_at: Utc::now(),
            warnings: vec![],
        };
        assert!(format!("{:?}", result).contains("10"));
    }
}
