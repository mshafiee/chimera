//! Roster merge module for Scout integration
//!
//! Implements SQL-level merge using ATTACH DATABASE to safely import
//! wallet roster updates from the Python Scout without write lock conflicts.
//!
//! The Scout writes to `roster_new.db` and the Operator merges it into
//! the main database using this module.

use chrono::{DateTime, Utc};
use std::path::Path;

use crate::error::{AppError, AppResult};

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
pub async fn merge_roster(_pool: &sqlx::Pool<sqlx::Postgres>, _roster_path: &Path) -> AppResult<MergeResult> {
    Err(AppError::Internal(
        "Roster merge using SQLite ATTACH DATABASE is not supported with PostgreSQL. Use direct SQL imports instead.".to_string()
    ))
}

/// Check if a roster file is valid (exists and passes integrity check)
pub async fn validate_roster(_pool: &sqlx::Pool<sqlx::Postgres>, _roster_path: &Path) -> AppResult<bool> {
    Err(AppError::Internal(
        "Roster validation using SQLite ATTACH DATABASE is not supported with PostgreSQL.".to_string()
    ))
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