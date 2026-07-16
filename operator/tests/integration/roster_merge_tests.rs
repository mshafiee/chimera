//! Integration tests for roster merge functionality
//!
//! Tests ATTACH DATABASE pattern, integrity checks, and atomic writes

use chimera_operator::db_abstraction::{create_database, Database, DatabaseConfig, DbPool};
use chimera_operator::roster;
use sqlx::postgres::PgPoolOptions;
use sqlx::{Pool, Postgres};
use std::sync::Arc;
use tempfile::TempDir;

fn pg_pool(db: &Arc<dyn Database>) -> Pool<Postgres> {
    match db.pool() {
        DbPool::PostgreSQL(pool) => pool,
        _ => panic!("test requires PostgreSQL backend"),
    }
}

/// Create a test database pool
async fn create_test_pool() -> (Arc<dyn Database>, TempDir) {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");

    let config = DatabaseConfig::postgres(std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL must be set"));
    let db = create_database(&config).await.unwrap();
    let pool = pg_pool(&db);

    // Create wallets table (must match database/schema/wallets.sql)
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS wallets (
            id SERIAL PRIMARY KEY AUTOINCREMENT,
            address TEXT NOT NULL UNIQUE,
            status TEXT NOT NULL DEFAULT 'CANDIDATE',
            wqs_score REAL,
            wqs_confidence REAL,
            roi_7d REAL,
            roi_30d REAL,
            trade_count_30d INTEGER,
            win_rate REAL,
            max_drawdown_30d REAL,
            avg_trade_size_sol REAL,
            avg_win_sol REAL,
            avg_loss_sol REAL,
            profit_factor REAL,
            realized_pnl_30d_sol REAL,
            last_trade_at TIMESTAMP,
            promoted_at TIMESTAMP,
            ttl_expires_at TIMESTAMP,
            notes TEXT,
            archetype TEXT,
            avg_entry_delay_seconds REAL,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();

    // config_audit is required by merge_roster for audit logging
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS config_audit (
            id SERIAL PRIMARY KEY AUTOINCREMENT,
            key TEXT NOT NULL,
            old_value TEXT,
            new_value TEXT,
            changed_by TEXT NOT NULL,
            change_reason TEXT,
            changed_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )
        "#,
    )
    .execute(&pool)
    .await
    .unwrap();

    (db, temp_dir)
}

/// Test roster merge with valid database
#[tokio::test]
async fn test_roster_merge_valid() {
    let (db, temp_dir) = create_test_pool().await;

    // Create a test roster_new.db file
    let roster_path = temp_dir.path().join("roster_new.db");
    let roster_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(
            sqlx::postgres::PgConnectOptions::new()
                .filename(&roster_path)
                .create_if_missing(true),
        )
        .await
        .unwrap();

    // Create wallets table in roster (must match database/schema/wallets.sql)
    sqlx::query(
        r#"
        CREATE TABLE wallets (
            id SERIAL PRIMARY KEY AUTOINCREMENT,
            address TEXT NOT NULL UNIQUE,
            status TEXT NOT NULL DEFAULT 'CANDIDATE',
            wqs_score REAL,
            wqs_confidence REAL,
            roi_7d REAL,
            roi_30d REAL,
            trade_count_30d INTEGER,
            win_rate REAL,
            max_drawdown_30d REAL,
            avg_trade_size_sol REAL,
            avg_win_sol REAL,
            avg_loss_sol REAL,
            profit_factor REAL,
            realized_pnl_30d_sol REAL,
            last_trade_at TIMESTAMP,
            promoted_at TIMESTAMP,
            ttl_expires_at TIMESTAMP,
            notes TEXT,
            archetype TEXT,
            avg_entry_delay_seconds REAL,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )
        "#,
    )
    .execute(&roster_pool)
    .await
    .unwrap();

    // Insert test wallet
    sqlx::query("INSERT INTO wallets (address, status, wqs_score) VALUES (?, ?, ?)")
        .bind("test_wallet_123")
        .bind("ACTIVE")
        .bind(85.5)
        .execute(&roster_pool)
        .await
        .unwrap();

    drop(roster_pool);

    // Merge roster
    let pool = pg_pool(&db);
    let result = roster::merge_roster(&pool, &roster_path).await;
    assert!(result.is_ok(), "Merge should succeed");

    let merge_result = result.unwrap();
    assert_eq!(merge_result.wallets_merged, 1);

    // Verify wallet was merged
    let pool = pg_pool(&db);
    let wallet: (String, String, Option<f64>) =
        sqlx::query_as("SELECT address, status, wqs_score FROM wallets WHERE address = ?")
            .bind("test_wallet_123")
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_eq!(wallet.0, "test_wallet_123");
    assert_eq!(wallet.1, "ACTIVE");
    assert!((wallet.2.unwrap() - 85.5).abs() < 0.01);
}

/// Test roster merge with integrity check failure
#[tokio::test]
async fn test_roster_merge_integrity_failure() {
    let (db, temp_dir) = create_test_pool().await;

    // Create a corrupted roster file (empty file)
    let roster_path = temp_dir.path().join("roster_new.db");
    std::fs::write(&roster_path, b"corrupted").unwrap();

    // Merge should fail integrity check
    let pool = pg_pool(&db);
    let result = roster::merge_roster(&pool, &roster_path).await;
    assert!(result.is_err(), "Merge should fail on corrupted database");
}

/// Test roster merge with missing wallets table
#[tokio::test]
async fn test_roster_merge_missing_table() {
    let (db, temp_dir) = create_test_pool().await;

    // Create empty database (no wallets table)
    let roster_path = temp_dir.path().join("roster_new.db");
    let roster_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(
            sqlx::postgres::PgConnectOptions::new()
                .filename(&roster_path)
                .create_if_missing(true),
        )
        .await
        .unwrap();

    drop(roster_pool);

    // Merge should fail - no wallets table
    let pool = pg_pool(&db);
    let result = roster::merge_roster(&pool, &roster_path).await;
    assert!(
        result.is_err(),
        "Merge should fail when wallets table missing"
    );
}

/// Test roster merge with schema mismatch
#[tokio::test]
async fn test_roster_merge_schema_mismatch() {
    let (db, temp_dir) = create_test_pool().await;

    // Create roster with missing column
    let roster_path = temp_dir.path().join("roster_new.db");
    let roster_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(
            sqlx::postgres::PgConnectOptions::new()
                .filename(&roster_path)
                .create_if_missing(true),
        )
        .await
        .unwrap();

    // Create wallets table missing a required column (e.g., wqs_score)
    sqlx::query(
        r#"
        CREATE TABLE wallets (
            id SERIAL PRIMARY KEY AUTOINCREMENT,
            address TEXT NOT NULL UNIQUE,
            status TEXT NOT NULL DEFAULT 'CANDIDATE'
        )
        "#,
    )
    .execute(&roster_pool)
    .await
    .unwrap();

    drop(roster_pool);

    // Merge should fail with schema validation error
    let pool = pg_pool(&db);
    let result = roster::merge_roster(&pool, &roster_path).await;
    assert!(result.is_err(), "Merge should fail on schema mismatch");

    let error_msg = format!("{}", result.unwrap_err());
    assert!(
        error_msg.contains("schema") || error_msg.contains("column") || error_msg.contains("no such column"),
        "Error should mention schema mismatch, got: {}",
        error_msg
    );
}
