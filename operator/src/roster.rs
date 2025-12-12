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

    // Step 5: Get current wallet count (for removed count)
    let current_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM wallets")
        .fetch_one(&mut *conn)
        .await?;

    let wallets_removed = current_count.0 as u32;

    // Step 6: Merge in a transaction
    // Using a simple replace strategy: delete all, insert all from new
    // This preserves the last-known-good state if merge fails mid-way
    sqlx::query("BEGIN TRANSACTION")
        .execute(&mut *conn)
        .await?;

    // Delete existing wallets
    let delete_result = sqlx::query("DELETE FROM wallets")
        .execute(&mut *conn)
        .await;

    if let Err(e) = delete_result {
        let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
        let _ = sqlx::query("DETACH DATABASE new_roster")
            .execute(&mut *conn)
            .await;
        return Err(e.into());
    }

    // Insert from new roster
    // Note: Column list must match the wallets table schema
    let insert_result = sqlx::query(
        r#"
        INSERT INTO wallets (
            address, status, wqs_score, roi_7d, roi_30d,
            trade_count_30d, win_rate, max_drawdown_30d,
            avg_trade_size_sol, avg_win_sol, avg_loss_sol, profit_factor, realized_pnl_30d_sol,
            last_trade_at, promoted_at,
            ttl_expires_at, notes, created_at, updated_at
        )
        SELECT 
            address, status, wqs_score, roi_7d, roi_30d,
            trade_count_30d, win_rate, max_drawdown_30d,
            avg_trade_size_sol, avg_win_sol, avg_loss_sol, profit_factor, realized_pnl_30d_sol,
            last_trade_at, promoted_at,
            ttl_expires_at, notes, created_at, CURRENT_TIMESTAMP
        FROM new_roster.wallets
        "#,
    )
    .execute(&mut *conn)
    .await;

    match insert_result {
        Ok(_) => {
            sqlx::query("COMMIT").execute(&mut *conn).await?;
            info!(
                wallets_merged = new_count,
                wallets_removed = wallets_removed,
                "Roster merge committed"
            );
        }
        Err(e) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            let _ = sqlx::query("DETACH DATABASE new_roster")
                .execute(&mut *conn)
                .await;
            error!(error = %e, "Roster merge failed during insert");
            return Err(e.into());
        }
    }

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
