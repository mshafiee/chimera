//! Roster merge module for Scout integration
//!
//! Implements SQL-level merge using ATTACH DATABASE to safely import
//! wallet roster updates from the Python Scout without write lock conflicts.
//!
//! The Scout writes to `roster_new.db` and the Operator merges it into
//! the main database using this module.
//!
//! # Schema Consistency
//!
//! **CRITICAL**: The `wallets` table schema in `roster_new.db` MUST match
//! the schema defined in `database/schema/wallets.sql`. This file is the
//! shared source of truth used by both:
//! - Rust (Operator): Used by sqlx migrations and this merge function
//! - Python (Scout): Used by `scout/core/db_writer.py::RosterWriter`
//!
//! When updating the schema:
//! 1. Update `database/schema/wallets.sql` (source of truth)
//! 2. Update Rust migrations in `operator/migrations/`
//! 3. Update Python `RosterWriter.WALLETS_SCHEMA` to match
//! 4. Test merge operation to ensure compatibility
//!
//! Schema validation: This function checks for table existence but does NOT
//! validate column structure. Column mismatches will cause merge failures.

use crate::db::DbPool;
use crate::error::AppResult;
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tracing::{error, info, warn};

/// Expected wallets table schema (must match database/schema/wallets.sql)
/// This is validated at runtime before merge to prevent silent failures
const EXPECTED_WALLETS_COLUMNS: &[(&str, &str)] = &[
    ("id", "INTEGER"),
    ("address", "TEXT"),
    ("status", "TEXT"),
    ("wqs_score", "REAL"),
    ("roi_7d", "REAL"),
    ("roi_30d", "REAL"),
    ("trade_count_30d", "INTEGER"),
    ("win_rate", "REAL"),
    ("max_drawdown_30d", "REAL"),
    ("avg_trade_size_sol", "REAL"),
    ("avg_win_sol", "REAL"),
    ("avg_loss_sol", "REAL"),
    ("profit_factor", "REAL"),
    ("realized_pnl_30d_sol", "REAL"),
    ("last_trade_at", "TIMESTAMP"),
    ("promoted_at", "TIMESTAMP"),
    ("ttl_expires_at", "TIMESTAMP"),
    ("notes", "TEXT"),
    ("archetype", "TEXT"),
    ("avg_entry_delay_seconds", "REAL"),
    ("created_at", "TIMESTAMP"),
    ("updated_at", "TIMESTAMP"),
];

/// Normalize SQLite types for comparison
/// SQLite is flexible with type names (TIMESTAMP, DATETIME, etc.)
fn normalize_sqlite_type(ty: &str) -> &str {
    let upper = ty.to_uppercase();
    if upper.contains("INT") {
        "INTEGER"
    } else if upper.contains("REAL") || upper.contains("FLOAT") || upper.contains("DOUBLE") {
        "REAL"
    } else if upper.contains("TEXT") || upper.contains("CHAR") || upper.contains("VARCHAR") {
        "TEXT"
    } else if upper.contains("TIMESTAMP") || upper.contains("DATETIME") || upper.contains("DATE") {
        "TIMESTAMP"
    } else {
        ty
    }
}

/// Validate that the attached roster database's wallets table schema
/// matches the expected schema from database/schema/wallets.sql
async fn validate_wallets_schema(
    conn: &mut sqlx::SqliteConnection,
) -> AppResult<()> {
    // Query table_info for the attached database
    // PRAGMA table_info returns: (cid, name, type, notnull, dflt_value, pk)
    let columns: Vec<(i32, String, String, Option<i32>, Option<String>, i32)> =
        sqlx::query_as("PRAGMA new_roster.table_info(wallets)")
            .fetch_all(&mut *conn)
            .await?;
    
    // Build map of actual columns
    let mut actual_columns: HashMap<String, String> = HashMap::new();
    for (_, name, col_type, _, _, _) in columns {
        actual_columns.insert(name.to_lowercase(), col_type.to_uppercase());
    }
    
    // Validate each expected column exists with correct type
    let mut missing = Vec::new();
    let mut type_mismatches = Vec::new();
    
    for (expected_name, expected_type) in EXPECTED_WALLETS_COLUMNS {
        let key = expected_name.to_lowercase();
        match actual_columns.get(&key) {
            None => missing.push(*expected_name),
            Some(actual_type) => {
                // SQLite type normalization: REAL, INTEGER, TEXT, TIMESTAMP
                let normalized_expected = normalize_sqlite_type(expected_type);
                let normalized_actual = normalize_sqlite_type(actual_type);
                if normalized_expected != normalized_actual {
                    type_mismatches.push((
                        *expected_name,
                        *expected_type,
                        actual_type.clone(),
                    ));
                }
            }
        }
    }
    
    // Check for extra columns (warn but don't fail)
    let expected_names: HashSet<String> = EXPECTED_WALLETS_COLUMNS
        .iter()
        .map(|(n, _)| n.to_lowercase())
        .collect();
    let extra: Vec<String> = actual_columns
        .keys()
        .filter(|k| !expected_names.contains(*k))
        .cloned()
        .collect();
    
    if !missing.is_empty() || !type_mismatches.is_empty() {
        let mut error_msg = String::from("Schema mismatch detected in roster database:\n");
        if !missing.is_empty() {
            error_msg.push_str(&format!("  Missing columns: {}\n", missing.join(", ")));
        }
        if !type_mismatches.is_empty() {
            error_msg.push_str("  Type mismatches:\n");
            for (name, expected, actual) in type_mismatches {
                error_msg.push_str(&format!("    {}: expected {}, got {}\n", name, expected, actual));
            }
        }
        error_msg.push_str("\nExpected schema is defined in database/schema/wallets.sql");
        return Err(crate::error::AppError::Internal(error_msg));
    }
    
    if !extra.is_empty() {
        warn!(
            extra_columns = ?extra,
            "Roster database has extra columns not in expected schema"
        );
    }
    
    Ok(())
}

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
    // Retry on SQLITE_BUSY errors (can occur during heavy concurrent load)
    let attach_sql = format!("ATTACH DATABASE '{}' AS new_roster", roster_path_str);
    let mut retries = 3;
    loop {
        match sqlx::query(&attach_sql).execute(&mut *conn).await {
            Ok(_) => break,
            Err(e) if retries > 0 => {
                if let Some(sqlite_err) = e.as_database_error() {
                    if sqlite_err.message().contains("database is locked") || 
                       sqlite_err.message().contains("SQLITE_BUSY") {
                        retries -= 1;
                        warn!(
                            retries_left = retries,
                            "Database busy during ATTACH, retrying..."
                        );
                        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                        continue;
                    }
                }
                return Err(crate::error::AppError::Database(e));
            }
            Err(e) => return Err(crate::error::AppError::Database(e)),
        }
    }

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

    // Step 2.5: Check if new_roster has wallets table
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
            "Roster database missing 'wallets' table. Ensure Scout's RosterWriter \
            creates the table using the schema from database/schema/wallets.sql".to_string(),
        ));
    }

    // Step 3: Validate schema compatibility
    info!("Validating wallets table schema");
    if let Err(e) = validate_wallets_schema(&mut *conn).await {
        // Detach before returning error
        let _ = sqlx::query("DETACH DATABASE new_roster")
            .execute(&mut *conn)
            .await;
        return Err(e);
    }
    info!("Schema validation passed");

    // Step 4: Count wallets in new roster
    let new_wallet_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM new_roster.wallets")
        .fetch_one(&mut *conn)
        .await?;

    let new_count = new_wallet_count.0 as u32;

    if new_count == 0 {
        warnings.push("New roster contains zero wallets".to_string());
        warn!("New roster contains zero wallets - proceeding with merge anyway");
    }

    // Step 5: With upsert strategy, we don't remove wallets that aren't in Scout's roster.
    // Wallets are only updated/inserted, not deleted. This preserves Operator's ability
    // to ban/demote wallets without Scout immediately reviving them.
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
