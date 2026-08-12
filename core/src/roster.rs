//! Roster merge module for Scout integration
//!
//! Intentionally DECOMMISSIONED: the original design used SQLite
//! `ATTACH DATABASE` to safely import wallet roster updates written by the
//! Python Scout (`roster_new.db`) without write lock conflicts.
//!
//! The operator is PostgreSQL-only since 2026-07 (SQLite was decommissioned),
//! and PostgreSQL has no `ATTACH DATABASE` equivalent, so both functions in
//! this module unconditionally reject the operation with a clear error.
//! Callers must use direct SQL imports instead.

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
/// NOT SUPPORTED on PostgreSQL: SQLite `ATTACH DATABASE`-based ingestion was
/// decommissioned with the SQLite backend. Returns an error pointing callers
/// at direct SQL imports.
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
///
/// NOT SUPPORTED on PostgreSQL (see [`merge_roster`]); always rejected.
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

    /// `connect_lazy` creates a Pool handle without opening any connection, so
    /// the pool argument can be supplied even though no database is reachable
    /// in unit tests. Both roster functions ignore the pool and reject the
    /// operation unconditionally.
    fn dummy_pool() -> sqlx::Pool<sqlx::Postgres> {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy("postgres://localhost:5432/does_not_exist")
            .expect("connect_lazy builds a lazy pool without connecting")
    }

    #[tokio::test]
    async fn test_merge_roster_rejected_on_postgres() {
        let pool = dummy_pool();
        let err = merge_roster(&pool, Path::new("roster_new.db"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not supported with PostgreSQL"));
    }

    #[tokio::test]
    async fn test_validate_roster_rejected_on_postgres() {
        let pool = dummy_pool();
        let err = validate_roster(&pool, Path::new("roster_new.db"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not supported with PostgreSQL"));
    }
}