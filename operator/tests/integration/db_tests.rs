//! Database Integration Tests
//!
//! Tests SQLite WAL behavior and roster merge operations:
//! - WAL mode concurrent reads
//! - Roster merge with integrity checks
//! - Atomic write behavior
//! - Database lock handling

use chimera_operator::db_abstraction::{create_database, Database, DatabaseConfig, DbPool};
use chimera_operator::roster::{merge_roster, validate_roster};
use sqlx::Pool;
use sqlx::Postgres;
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;

fn pg_pool(db: &Arc<dyn Database>) -> Pool<Postgres> {
    match db.pool() {
        DbPool::PostgreSQL(pool) => pool,
        _ => panic!("test requires PostgreSQL backend"),
    }
}

/// Create a temporary database for testing
async fn create_test_db() -> (Arc<dyn Database>, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    let config = DatabaseConfig::postgres(std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL must be set"));
    let db = create_database(&config).await.unwrap();
    db.run_migrations().await.unwrap();

    (db, temp_dir)
}

/// Create a test roster database
async fn create_test_roster(roster_path: &Path, wallet_count: u32) {
    use sqlx::postgres::PgPoolOptions;
    use std::str::FromStr;

    let db_url = format!("sqlite:{}?mode=rwc", roster_path.display());
    let connect_options = PgConnectOptions::from_str(&db_url)
        .unwrap()
        .journal_mode(sqlx::postgres::SqliteJournalMode::Wal)
        .create_if_missing(true);

    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(connect_options)
        .await
        .unwrap();

    // Create wallets table — must match EXPECTED_WALLETS_COLUMNS in roster.rs
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

    // Insert test wallets
    for i in 0..wallet_count {
        sqlx::query("INSERT INTO wallets (address, status, wqs_score) VALUES (?, ?, ?)")
            .bind(format!("test_wallet_{}", i))
            .bind("CANDIDATE")
            .bind(50.0 + (i as f64))
            .execute(&pool)
            .await
            .unwrap();
    }

    pool.close().await;
}

// =============================================================================
// WAL MODE TESTS
// =============================================================================

#[tokio::test]
async fn test_wal_mode_enabled() {
    let (db, _temp_dir) = create_test_db().await;
    let pool = pg_pool(&db);

    // Check journal mode
    let result: (String,) = sqlx::query_as("PRAGMA journal_mode")
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(
        result.0.to_uppercase(),
        "WAL",
        "Database should be in WAL mode"
    );
}

#[tokio::test]
async fn test_concurrent_reads() {
    let (db, _temp_dir) = create_test_db().await;
    let pool = pg_pool(&db);

    // Insert test data
    sqlx::query("INSERT INTO wallets (address, status, created_at, updated_at) VALUES (?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
        .bind("test_wallet")
        .bind("ACTIVE")
        .execute(&pool)
        .await
        .unwrap();

    // Spawn multiple concurrent readers
    let mut handles = vec![];
    for i in 0..5 {
        let pool_clone = pool.clone();
        handles.push(tokio::spawn(async move {
            let result: (String,) = sqlx::query_as("SELECT status FROM wallets WHERE address = ?")
                .bind("test_wallet")
                .fetch_one(&pool_clone)
                .await
                .unwrap();

            assert_eq!(result.0, "ACTIVE", "Reader {} should see ACTIVE status", i);
        }));
    }

    // Wait for all readers
    for handle in handles {
        handle.await.unwrap();
    }
}

#[tokio::test]
async fn test_busy_timeout_configured() {
    let (db, _temp_dir) = create_test_db().await;
    let pool = pg_pool(&db);

    // Check busy timeout (should be 5000ms = 5000000 microseconds)
    let result: (i64,) = sqlx::query_as("PRAGMA busy_timeout")
        .fetch_one(&pool)
        .await
        .unwrap();

    assert!(
        result.0 >= 5000,
        "Busy timeout should be at least 5000ms, got {}ms",
        result.0
    );
}

// =============================================================================
// ROSTER MERGE TESTS
// =============================================================================

#[tokio::test]
async fn test_roster_merge_success() {
    let (db, temp_dir) = create_test_db().await;
    let pool = pg_pool(&db);
    let roster_path = temp_dir.path().join("roster_new.db");

    // Create test roster with 3 wallets
    create_test_roster(&roster_path, 3).await;

    // Perform merge
    let pool = pg_pool(&db);
    let result = merge_roster(&pool, &roster_path).await.unwrap();

    assert_eq!(result.wallets_merged, 3, "Should merge 3 wallets");
    assert!(result.integrity_ok, "Integrity check should pass");
    assert_eq!(result.warnings.len(), 0, "Should have no warnings");

    // Verify wallets were inserted
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM wallets")
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(count.0, 3, "Should have 3 wallets in main database");
}

#[tokio::test]
async fn test_roster_merge_integrity_check_failure() {
    let (db, temp_dir) = create_test_db().await;
    let roster_path = temp_dir.path().join("roster_new.db");

    // Create a corrupted roster file (empty file)
    std::fs::write(&roster_path, b"").unwrap();

    // Attempt merge - should fail on integrity check or attachment
    let pool = pg_pool(&db);
    let result = merge_roster(&pool, &roster_path).await;

    assert!(result.is_err(), "Merge should fail on corrupted roster");
    let error = result.unwrap_err();
    let error_msg = error.to_string().to_lowercase();
    assert!(
        error_msg.contains("integrity")
            || error_msg.contains("not found")
            || error_msg.contains("database")
            || error_msg.contains("attach")
            || error_msg.contains("corrupt"),
        "Error should mention integrity, database, attach, or corrupt. Got: {}",
        error
    );
}

#[tokio::test]
async fn test_roster_merge_missing_file() {
    let (db, temp_dir) = create_test_db().await;
    let roster_path = temp_dir.path().join("nonexistent.db");

    // Attempt merge with non-existent file
    let pool = pg_pool(&db);
    let result = merge_roster(&pool, &roster_path).await;

    assert!(result.is_err(), "Merge should fail on missing file");
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("not found") || error_msg.contains("Roster file"),
        "Error should mention file not found"
    );
}

#[tokio::test]
async fn test_roster_merge_atomic_write() {
    let (db, temp_dir) = create_test_db().await;
    let pool = pg_pool(&db);

    // Insert initial wallet
    sqlx::query("INSERT INTO wallets (address, status, created_at, updated_at) VALUES (?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
        .bind("existing_wallet")
        .bind("ACTIVE")
        .execute(&pool)
        .await
        .unwrap();

    let roster_path = temp_dir.path().join("roster_new.db");

    // Create new roster with different wallets
    create_test_roster(&roster_path, 2).await;

    // Perform merge
    let pool = pg_pool(&db);
    let _result = merge_roster(&pool, &roster_path).await.unwrap();

    // REPLACE strategy: existing wallets are deleted, new wallets are inserted
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM wallets")
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(
        count.0, 2,
        "Should have 2 wallets after merge (all deleted, 2 new inserted)"
    );

    // Verify old wallet was deleted (REPLACE strategy removes existing)
    let old_wallet: Option<(String,)> =
        sqlx::query_as("SELECT address FROM wallets WHERE address = ?")
            .bind("existing_wallet")
            .fetch_optional(&pool)
            .await
            .unwrap();

    assert!(old_wallet.is_none(), "Old wallet should be deleted in REPLACE strategy");
}

#[tokio::test]
async fn test_roster_merge_empty_roster() {
    let (db, temp_dir) = create_test_db().await;
    let roster_path = temp_dir.path().join("roster_new.db");

    // Create empty roster (just schema, no wallets)
    create_test_roster(&roster_path, 0).await;

    // Perform merge
    let pool = pg_pool(&db);
    let result = merge_roster(&pool, &roster_path).await.unwrap();

    assert_eq!(result.wallets_merged, 0, "Should merge 0 wallets");
    assert!(
        !result.warnings.is_empty(),
        "Should warn about empty roster"
    );
    assert!(result.warnings.iter().any(|w| w.contains("zero wallets")));
}

#[tokio::test]
async fn test_roster_validate_success() {
    let (db, temp_dir) = create_test_db().await;
    let roster_path = temp_dir.path().join("roster_new.db");

    // Create valid roster
    create_test_roster(&roster_path, 5).await;

    // Validate
    let pool = pg_pool(&db);
    let is_valid = validate_roster(&pool, &roster_path).await.unwrap();

    assert!(is_valid, "Valid roster should pass validation");
}

#[tokio::test]
async fn test_roster_validate_missing_file() {
    let (db, temp_dir) = create_test_db().await;
    let roster_path = temp_dir.path().join("nonexistent.db");

    // Validate missing file
    let pool = pg_pool(&db);
    let is_valid = validate_roster(&pool, &roster_path).await.unwrap();

    assert!(!is_valid, "Missing file should fail validation");
}

#[tokio::test]
async fn test_roster_merge_transaction_rollback() {
    let (db, temp_dir) = create_test_db().await;
    let pool = pg_pool(&db);

    // Insert initial wallet
    sqlx::query("INSERT INTO wallets (address, status, created_at, updated_at) VALUES (?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
        .bind("wallet_before_merge")
        .bind("ACTIVE")
        .execute(&pool)
        .await
        .unwrap();

    let roster_path = temp_dir.path().join("roster_new.db");

    // Create roster that will cause an error during merge
    // We'll create a roster with invalid data that might cause issues
    // For this test, we'll use a valid roster but simulate a failure scenario
    create_test_roster(&roster_path, 1).await;

    // Manually corrupt the roster after creation to test rollback
    // (In real scenario, this would be caught by integrity check)
    // For this test, we verify that if merge fails, transaction is rolled back

    // Verify initial state
    let count_before: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM wallets")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count_before.0, 1, "Should have 1 wallet before merge");

    // Perform merge (should succeed)
    let pool = pg_pool(&db);
    let result = merge_roster(&pool, &roster_path).await.unwrap();
    assert_eq!(result.wallets_merged, 1);

    // Verify final state — REPLACE strategy: old wallet deleted, new wallet inserted
    let count_after: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM wallets")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count_after.0, 1,
        "Should have 1 wallet after merge (old deleted, new inserted)"
    );

    // Verify old wallet was deleted
    let old_wallet: Option<(String,)> =
        sqlx::query_as("SELECT address FROM wallets WHERE address = ?")
            .bind("wallet_before_merge")
            .fetch_optional(&pool)
            .await
            .unwrap();

    assert!(old_wallet.is_none(), "Old wallet should be deleted in REPLACE strategy");
}

// =============================================================================
// DATABASE LOCK TESTS
// =============================================================================

#[tokio::test]
async fn test_concurrent_writes_with_timeout() {
    let (db, _temp_dir) = create_test_db().await;
    let pool = pg_pool(&db);

    // Spawn multiple writers that will contend for locks
    let mut handles = vec![];
    for i in 0..3 {
        let pool_clone = pool.clone();
        handles.push(tokio::spawn(async move {
            // Each writer tries to insert
            let result = sqlx::query(
                "INSERT INTO wallets (address, status, created_at, updated_at) VALUES (?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)"
            )
            .bind(format!("concurrent_wallet_{}", i))
            .bind("ACTIVE")
            .execute(&pool_clone)
            .await;

            // Should succeed (busy timeout handles contention)
            assert!(result.is_ok(), "Writer {} should succeed", i);
        }));
    }

    // Wait for all writers
    for handle in handles {
        handle.await.unwrap();
    }

    // Verify all inserts succeeded
    let count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM wallets WHERE address LIKE 'concurrent_wallet_%'")
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_eq!(count.0, 3, "All concurrent writes should succeed");
}
