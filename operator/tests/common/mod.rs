//! Common test utilities for database testing
//!
//! Provides a backend-agnostic test harness that supports both SQLite and PostgreSQL.
//! Tests can use `create_test_db()` for SQLite by default, or `create_test_pg_db()` for
//! PostgreSQL when the `postgres` feature is enabled and `TEST_DATABASE_URL` is set.

use chimera_operator::db_abstraction::{create_database, Database, DatabaseConfig, DbPool};
#[cfg(feature = "postgres")]
use sqlx::postgres::PgPoolOptions;
use sqlx::{Pool, Sqlite};
#[cfg(feature = "postgres")]
use sqlx::Postgres;
use std::sync::Arc;
use tempfile::TempDir;

/// Create a SQLite test database with migrations applied
///
/// Returns an `Arc<dyn Database>` and a `TempDir` that will clean up when dropped.
pub async fn create_test_db() -> (Arc<dyn Database>, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let config = DatabaseConfig::sqlite(temp_dir.path().join("test.db"));
    let db = create_database(&config).await.unwrap();
    db.run_migrations().await.unwrap();
    (db, temp_dir)
}

/// Create a PostgreSQL test database with migrations applied
///
/// This is gated behind the `postgres` feature and requires `TEST_DATABASE_URL` env var.
/// Use for tests that need to verify Postgres backend parity with SQLite.
///
/// # Panics
/// - If `postgres` feature is not enabled
/// - If `TEST_DATABASE_URL` environment variable is not set
/// - If database connection or migration fails
#[cfg(feature = "postgres")]
pub async fn create_test_pg_db() -> (Arc<dyn Database>, TempDir) {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must be set for Postgres tests");

    let temp_dir = TempDir::new().unwrap();
    
    // Create a unique database name to avoid conflicts between concurrent tests
    let db_name = format!("test_{}", uuid::Uuid::new_v4().to_string().replace('-', "_"));
    
    // Connect to postgres (not the specific DB) to create the test database
    let (base_url, _) = database_url
        .rsplit_once('/')
        .unwrap_or((&database_url, ""));
    
    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&format!("{}/postgres", base_url))
        .await
        .expect("Failed to connect to Postgres server");

    // Create the test database
    sqlx::query(&format!("CREATE DATABASE {}", db_name))
        .execute(&admin_pool)
        .await
        .expect("Failed to create test database");

    admin_pool.close().await;

    // Connect to the new test database
    let test_db_url = format!("{}/{}", base_url, db_name);
    let config = DatabaseConfig::postgres(&test_db_url);
    let db = create_database(&config).await.unwrap();
    db.run_migrations().await.unwrap();

    (db, temp_dir)
}

/// Extract SQLite pool from a generic database
///
/// # Panics
/// If the database is not a SQLite backend
pub fn sqlite_pool(db: &Arc<dyn Database>) -> Pool<Sqlite> {
    match db.pool() {
        DbPool::SQLite(pool) => pool,
        _ => panic!("test requires SQLite backend"),
    }
}

/// Extract PostgreSQL pool from a generic database
///
/// # Panics
/// - If `postgres` feature is not enabled
/// - If the database is not a PostgreSQL backend
#[cfg(feature = "postgres")]
pub fn pg_pool(db: &Arc<dyn Database>) -> Pool<Postgres> {
    match db.pool() {
        DbPool::Postgres(pool) => pool,
        _ => panic!("test requires PostgreSQL backend"),
    }
}

/// Create a database for testing based on environment
///
/// Returns SQLite by default, or PostgreSQL if `TEST_DATABASE_URL` is set and
/// the `postgres` feature is enabled.
///
/// This helper is useful for parameterized tests that should run against both
/// backends when available.
pub async fn create_test_db_from_env() -> (Arc<dyn Database>, TempDir, String) {
    #[cfg(feature = "postgres")]
    {
        if std::env::var("TEST_DATABASE_URL").is_ok() {
            let (db, temp_dir) = create_test_pg_db().await;
            return (db, temp_dir, "postgres".to_string());
        }
    }
    
    let (db, temp_dir) = create_test_db().await;
    (db, temp_dir, "sqlite".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_sqlite_db_basic() {
        let (db, _temp_dir) = create_test_db().await;
        
        // Verify basic functionality works
        let pool = sqlite_pool(&db);
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM trades")
            .fetch_one(&pool)
            .await
            .unwrap();
        
        assert_eq!(count.0, 0, "Fresh DB should have zero trades");
    }

    #[tokio::test]
    async fn test_create_sqlite_db_runs_migrations() {
        let (db, _temp_dir) = create_test_db().await;
        
        // Verify migrations ran by checking for expected tables
        let pool = sqlite_pool(&db);
        let result: (String,) = sqlx::query_as(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='wallets'"
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        
        assert_eq!(result.0, "wallets", "wallets table should exist after migrations");
    }

    #[cfg(feature = "postgres")]
    #[tokio::test]
    #[ignore] // Requires TEST_DATABASE_URL to be set
    async fn test_create_postgres_db_basic() {
        let (db, _temp_dir) = create_test_pg_db().await;
        
        // Verify basic functionality works
        let pool = pg_pool(&db);
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM trades")
            .fetch_one(&pool)
            .await
            .unwrap();
        
        assert_eq!(count.0, 0, "Fresh DB should have zero trades");
    }

    #[cfg(feature = "postgres")]
    #[tokio::test]
    #[ignore] // Requires TEST_DATABASE_URL to be set
    async fn test_create_postgres_db_runs_migrations() {
        let (db, _temp_dir) = create_test_pg_db().await;
        
        // Verify migrations ran by checking for expected tables
        let pool = pg_pool(&db);
        let result: (String,) = sqlx::query_as(
            "SELECT tablename FROM pg_tables WHERE tablename='wallets'"
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        
        assert_eq!(result.0, "wallets", "wallets table should exist after migrations");
    }
}