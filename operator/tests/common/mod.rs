//! Common test utilities for database testing
//!
//! Provides PostgreSQL-only test harness. Tests use `create_test_pg_db()` which requires
//! the `TEST_DATABASE_URL` env var.

use chimera_operator::db_abstraction::{create_database, Database, DatabaseConfig, DbPool};
use sqlx::postgres::PgPoolOptions;
use sqlx::{Pool, Postgres};
use std::sync::Arc;
use tempfile::TempDir;

/// Create a PostgreSQL test database with migrations applied
///
/// This requires `TEST_DATABASE_URL` env var to be set.
///
/// # Panics
/// - If `TEST_DATABASE_URL` environment variable is not set
/// - If database connection or migration fails
pub async fn create_test_db() -> (Arc<dyn Database>, TempDir) {
    create_test_pg_db().await
}

/// Create a PostgreSQL test database with migrations applied
///
/// This requires `TEST_DATABASE_URL` env var to be set.
///
/// # Panics
/// - If `TEST_DATABASE_URL` environment variable is not set
/// - If database connection or migration fails
pub async fn create_test_pg_db() -> (Arc<dyn Database>, TempDir) {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must be set for tests");

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
    let config = DatabaseConfig::postgres(std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL must be set"));
    let db = create_database(&config).await.unwrap();
    db.run_migrations().await.unwrap();

    (db, temp_dir)
}

/// Extract PostgreSQL pool from a generic database
///
/// # Panics
/// - If the database is not a PostgreSQL backend
pub fn pg_pool(db: &Arc<dyn Database>) -> Pool<Postgres> {
    match db.pool() {
        DbPool::PostgreSQL(pool) => pool,
        _ => panic!("test requires PostgreSQL backend"),
    }
}

/// Create a database for testing based on environment
///
/// Always uses PostgreSQL with `TEST_DATABASE_URL` env var.
pub async fn create_test_db_from_env() -> (Arc<dyn Database>, TempDir, String) {
    let (db, temp_dir) = create_test_pg_db().await;
    (db, temp_dir, "postgres".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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