//! Integration tests for Chimera Operator
//!
//! Tests database operations and system behavior using an in-memory test DB.

use chimera_operator::config::DatabaseConfig;
use chimera_operator::db;
use rust_decimal::prelude::*;
use tempfile::TempDir;

/// Setup test database
async fn setup_test_db() -> (db::DbPool, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");

    let pool = db::init_pool(&DatabaseConfig {
        path: db_path.clone(),
        max_connections: 5,
    })
    .await
    .unwrap();

    // Run migrations
    db::run_migrations(&pool).await.unwrap();

    (pool, temp_dir)
}

#[tokio::test]
async fn test_health_check_db_connectivity() {
    // Verifies that the test DB can be set up and migrations applied successfully.
    let (pool, _dir) = setup_test_db().await;
    // A simple query that should always succeed on a healthy DB
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM trades")
        .fetch_one(&pool)
        .await
        .expect("trades table should exist after migrations");
    assert_eq!(row.0, 0, "Fresh DB should have zero trades");
}

#[tokio::test]
async fn test_trade_idempotency() {
    // Inserting two rows with the same trade_uuid should fail on the second insert
    // because the DB schema enforces UNIQUE on trade_uuid.
    let (pool, _dir) = setup_test_db().await;

    let uuid = "idempotency-test-uuid-1234";
    let wallet = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
    let token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263";

    // First insert should succeed
    db::insert_trade(
        &pool,
        uuid,
        wallet,
        token,
        Some("BONK"),
        "SHIELD",
        "BUY",
        Decimal::from_str("0.1").unwrap(),
        "PENDING",
    )
    .await
    .expect("First insert should succeed");

    // Second insert with same UUID should fail
    let second = db::insert_trade(
        &pool,
        uuid,
        wallet,
        token,
        Some("BONK"),
        "SHIELD",
        "BUY",
        Decimal::from_str("0.1").unwrap(),
        "PENDING",
    )
    .await;
    assert!(
        second.is_err(),
        "Duplicate trade_uuid should be rejected by DB"
    );

    // Confirm only one row exists
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM trades WHERE trade_uuid = ?")
        .bind(uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.0, 1, "Only one trade should exist for this UUID");
}

#[tokio::test]
async fn test_circuit_breaker_loss_tracking() {
    // Inserting a CLOSED trade with a large negative PnL and querying for it works correctly.
    let (pool, _dir) = setup_test_db().await;

    let uuid = "circuit-test-uuid-5678";
    let wallet = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
    let token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263";

    db::insert_trade(
        &pool,
        uuid,
        wallet,
        token,
        Some("BONK"),
        "SHIELD",
        "BUY",
        Decimal::from_str("1.0").unwrap(),
        "CLOSED",
    )
    .await
    .unwrap();

    let big_loss = Decimal::from_str("-2.5").unwrap();
    db::update_trade_net_pnl(&pool, uuid, big_loss)
        .await
        .unwrap();

    // Verify the loss is stored correctly
    let row: (f64,) = sqlx::query_as("SELECT net_pnl_sol FROM trades WHERE trade_uuid = ?")
        .bind(uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        row.0 < 0.0,
        "net_pnl_sol should be negative for a losing trade"
    );
    assert!((row.0 + 2.5).abs() < 0.0001, "net_pnl_sol should be -2.5");
}

#[tokio::test]
async fn test_trade_status_update() {
    // Verify that a trade's status can be updated from OPEN to CLOSED.
    let (pool, _dir) = setup_test_db().await;

    let uuid = "status-update-uuid-9012";
    let wallet = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
    let token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263";

    db::insert_trade(
        &pool,
        uuid,
        wallet,
        token,
        Some("BONK"),
        "SHIELD",
        "BUY",
        Decimal::from_str("0.5").unwrap(),
        "PENDING",
    )
    .await
    .unwrap();

    db::update_trade_status(&pool, uuid, "CLOSED", Some("tx_signature_abc"), None)
        .await
        .unwrap();

    let row: (String,) = sqlx::query_as("SELECT status FROM trades WHERE trade_uuid = ?")
        .bind(uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.0, "CLOSED", "Status should be updated to CLOSED");
}

#[tokio::test]
async fn test_wallet_insert_and_query() {
    // Insert a wallet record and verify it can be retrieved.
    let (pool, _dir) = setup_test_db().await;

    let address = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";

    sqlx::query(
        "INSERT INTO wallets (address, status, wqs_score, roi_7d, roi_30d, trade_count_30d, win_rate, max_drawdown_30d, avg_trade_size_sol)
         VALUES (?, 'CANDIDATE', 55.0, 12.0, 30.0, 25, 0.65, 10.0, 0.5)",
    )
    .bind(address)
    .execute(&pool)
    .await
    .expect("Wallet insert should succeed");

    let row: (String,) = sqlx::query_as("SELECT status FROM wallets WHERE address = ?")
        .bind(address)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.0, "CANDIDATE", "Wallet status should be CANDIDATE");

    // Simulate wallet promotion
    sqlx::query("UPDATE wallets SET status = 'ACTIVE' WHERE address = ?")
        .bind(address)
        .execute(&pool)
        .await
        .unwrap();

    let row: (String,) = sqlx::query_as("SELECT status FROM wallets WHERE address = ?")
        .bind(address)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.0, "ACTIVE", "Wallet status should be updated to ACTIVE");
}
