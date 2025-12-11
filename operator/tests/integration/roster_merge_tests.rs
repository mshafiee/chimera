//! Integration tests for roster merge functionality
//!
//! Tests ATTACH DATABASE pattern, integrity checks, and atomic writes

use chimera_operator::roster;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{Pool, Sqlite};
use std::time::Duration;
use tempfile::TempDir;

/// Create a test database pool
async fn create_test_pool() -> (Pool<Sqlite>, TempDir) {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");
    
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&db_path)
                .create_if_missing(true)
                .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
                .busy_timeout(std::time::Duration::from_secs(5)),
        )
        .await
        .unwrap();
    
    // Run schema
    let schema = include_str!("../../../database/schema.sql");
    sqlx::raw_sql(schema).execute(&pool).await.unwrap();
    
    (pool, temp_dir)
}

/// Test roster merge with valid database
#[tokio::test]
async fn test_roster_merge_valid() {
    let (pool, temp_dir) = create_test_pool().await;
    
    // Create a test roster_new.db file
    let roster_path = temp_dir.path().join("roster_new.db");
    let roster_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&roster_path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
    
    // Create wallets table in roster
    sqlx::query(
        r#"
        CREATE TABLE wallets (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            address TEXT NOT NULL UNIQUE,
            status TEXT NOT NULL DEFAULT 'CANDIDATE',
            wqs_score REAL,
            roi_7d REAL,
            roi_30d REAL,
            trade_count_30d INTEGER,
            win_rate REAL,
            max_drawdown_30d REAL,
            avg_trade_size_sol REAL,
            last_trade_at TIMESTAMP,
            promoted_at TIMESTAMP,
            ttl_expires_at TIMESTAMP,
            notes TEXT,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )
        "#,
    )
    .execute(&roster_pool)
    .await
    .unwrap();
    
    // Insert test wallet
    sqlx::query(
        "INSERT INTO wallets (address, status, wqs_score) VALUES (?, ?, ?)"
    )
    .bind("test_wallet_123")
    .bind("ACTIVE")
    .bind(85.5)
    .execute(&roster_pool)
    .await
    .unwrap();
    
    drop(roster_pool);
    
    // Merge roster
    let result = roster::merge_roster(&pool, &roster_path).await;
    assert!(result.is_ok(), "Merge should succeed");
    
    let merge_result = result.unwrap();
    assert_eq!(merge_result.wallets_merged, 1);
    
    // Verify wallet was merged
    let wallet: (String, String, Option<f64>) = sqlx::query_as(
        "SELECT address, status, wqs_score FROM wallets WHERE address = ?"
    )
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
    let (pool, temp_dir) = create_test_pool().await;
    
    // Create a corrupted roster file (empty file)
    let roster_path = temp_dir.path().join("roster_new.db");
    std::fs::write(&roster_path, b"corrupted").unwrap();
    
    // Merge should fail integrity check
    let result = roster::merge_roster(&pool, &roster_path).await;
    assert!(result.is_err(), "Merge should fail on corrupted database");
}

/// Test roster merge with missing wallets table
#[tokio::test]
async fn test_roster_merge_missing_table() {
    let (pool, temp_dir) = create_test_pool().await;
    
    // Create empty database (no wallets table)
    let roster_path = temp_dir.path().join("roster_new.db");
    let roster_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&roster_path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
    
    drop(roster_pool);
    
    // Merge should fail - no wallets table
    let result = roster::merge_roster(&pool, &roster_path).await;
    assert!(result.is_err(), "Merge should fail when wallets table missing");
}

use std::time::Duration;



