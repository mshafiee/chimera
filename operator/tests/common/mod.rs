//! Common test utilities for database testing
//!
//! Provides PostgreSQL-only test harness. Tests use `create_test_pg_db()` which requires
//! the `TEST_DATABASE_URL` env var.
//!
//! Each call creates a unique `test_<uuid>` database that is automatically dropped
//! when the returned [`TestDbGuard`] is dropped at the end of the test.

use chimera_operator::db_abstraction::{create_database, Database, DatabaseConfig, DbPool};
use sqlx::postgres::PgPoolOptions;
use sqlx::{Pool, Postgres};
use std::sync::Arc;
use tempfile::TempDir;

/// Owns the teardown of a per-test database: dropping the guard drops the
/// `test_<uuid>` database so repeated/CI runs do not leak databases on the
/// Postgres server.
pub struct TestDbGuard {
    db_name: String,
    server_url: String,
    _temp_dir: TempDir,
}

impl Drop for TestDbGuard {
    fn drop(&mut self) {
        let db_name = self.db_name.clone();
        let server_url = self.server_url.clone();
        // Fire-and-forget cleanup on a dedicated thread with its own runtime:
        // Drop runs inside the test's tokio runtime, so we cannot block_on here
        // (sqlx 0.8 also removed the blocking connection API). WITH (FORCE)
        // terminates any lingering connections to the test database.
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(_) => return,
            };
            let _ = rt.block_on(async move {
                let Ok(pool) = PgPoolOptions::new()
                    .max_connections(1)
                    .connect(&server_url)
                    .await
                else {
                    return;
                };
                let _ = sqlx::query(&format!(
                    "DROP DATABASE IF EXISTS {db_name} WITH (FORCE)"
                ))
                .execute(&pool)
                .await;
                pool.close().await;
            });
        });
    }
}

/// Split a Postgres URL into (server_base, db_name, query_string) preserving any
/// query parameters (e.g. `?sslmode=require`) so they are not silently dropped
/// when the database name is swapped.
fn split_database_url(database_url: &str) -> (&str, &str, Option<&str>) {
    let (url_part, query) = match database_url.split_once('?') {
        Some((u, q)) => (u, Some(q)),
        None => (database_url, None),
    };
    let (base_url, db_name) = url_part.rsplit_once('/').unwrap_or((database_url, ""));
    (base_url, db_name, query)
}

/// Strict SQL-identifier guard: db names are interpolated into DDL, so validate
/// the pattern instead of trusting the caller.
fn assert_valid_db_name(db_name: &str) {
    let valid = !db_name.is_empty()
        && db_name.len() <= 63
        && db_name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && db_name.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c == '_');
    assert!(valid, "invalid generated test database name: {db_name}");
}

/// Create a PostgreSQL test database with migrations applied
///
/// This requires `TEST_DATABASE_URL` env var to be set.
///
/// # Panics
/// - If `TEST_DATABASE_URL` environment variable is not set
/// - If database connection or migration fails
pub async fn create_test_db() -> (Arc<dyn Database>, TestDbGuard) {
    create_test_pg_db().await
}

/// Create a PostgreSQL test database with migrations applied
///
/// This requires `TEST_DATABASE_URL` env var to be set.
///
/// # Panics
/// - If `TEST_DATABASE_URL` environment variable is not set
/// - If database connection or migration fails
pub async fn create_test_pg_db() -> (Arc<dyn Database>, TestDbGuard) {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must be set for tests");

    // Create a unique database name to avoid conflicts between concurrent tests
    let db_name = format!("test_{}", uuid::Uuid::new_v4().to_string().replace('-', "_"));
    assert_valid_db_name(&db_name);

    let (base_url, _original_db, query) = split_database_url(&database_url);

    // The maintenance database is assumed to be named `postgres` (the default);
    // managed providers with a different maintenance DB must point
    // TEST_DATABASE_URL at a server where `postgres` exists.
    let query_suffix = query.map(|q| format!("?{q}")).unwrap_or_default();
    let server_url = format!("{base_url}/postgres{query_suffix}");
    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&server_url)
        .await
        .expect("Failed to connect to Postgres server");

    // Create the test database
    sqlx::query(&format!("CREATE DATABASE {db_name}"))
        .execute(&admin_pool)
        .await
        .expect("Failed to create test database");

    admin_pool.close().await;

    // Connect to the NEW test database (not the original TEST_DATABASE_URL,
    // which would cause concurrent tests to share a single database).
    let test_db_url = format!("{base_url}/{db_name}{query_suffix}");
    let config = DatabaseConfig::postgres(test_db_url);
    let db = create_database(&config).await.unwrap();
    db.run_migrations().await.unwrap();

    let temp_dir = TempDir::new().unwrap();
    (db, TestDbGuard {
        db_name,
        server_url,
        _temp_dir: temp_dir,
    })
}

/// Extract PostgreSQL pool from a generic database
///
/// # Panics
/// - If the database is not a PostgreSQL backend
pub fn pg_pool(db: &Arc<dyn Database>) -> Pool<Postgres> {
    match db.pool() {
        DbPool::PostgreSQL(pool) => pool,
    }
}

/// Create a database for testing based on environment
///
/// Always uses PostgreSQL with `TEST_DATABASE_URL` env var.
pub async fn create_test_db_from_env() -> (Arc<dyn Database>, TestDbGuard, String) {
    let (db, guard) = create_test_pg_db().await;
    (db, guard, "postgres".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires TEST_DATABASE_URL to be set
    async fn test_create_postgres_db_basic() {
        let (db, _guard) = create_test_pg_db().await;

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
        let (db, _guard) = create_test_pg_db().await;

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
