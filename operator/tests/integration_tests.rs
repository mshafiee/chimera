//! Integration tests for Chimera Operator
//!
//! Tests database operations and system behavior using an in-memory test DB.

use chimera_operator::db_abstraction::{Database, InsertTrade, UpdateTradeStatus};
use rust_decimal::prelude::*;
use sqlx::Pool;
use sqlx::Sqlite;
use std::sync::Arc;

mod common;

fn sqlite_pool(db: &Arc<dyn Database>) -> Pool<Sqlite> {
    common::sqlite_pool(db)
}

/// Setup test database
async fn setup_test_db() -> (Arc<dyn Database>, tempfile::TempDir) {
    common::create_test_db().await
}

#[tokio::test]
async fn test_health_check_db_connectivity() {
    // Verifies that the test DB can be set up and migrations applied successfully.
    let (db, _dir) = setup_test_db().await;
    let pool = sqlite_pool(&db);
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
    let (db, _dir) = setup_test_db().await;
    let pool = sqlite_pool(&db);

    let uuid = "idempotency-test-uuid-1234";
    let wallet = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
    let token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263";

    // First insert should succeed
    db.insert_trade(&InsertTrade {
        trade_uuid: uuid.to_string(),
        wallet_address: wallet.to_string(),
        token_address: token.to_string(),
        token_symbol: Some("BONK".to_string()),
        strategy: "SHIELD".to_string(),
        side: "BUY".to_string(),
        amount_sol: Decimal::from_str("0.1").unwrap(),
        status: "PENDING".to_string(),
    })
    .await
    .expect("First insert should succeed");

    // Second insert with same UUID should fail
    let second = db
        .insert_trade(&InsertTrade {
            trade_uuid: uuid.to_string(),
            wallet_address: wallet.to_string(),
            token_address: token.to_string(),
            token_symbol: Some("BONK".to_string()),
            strategy: "SHIELD".to_string(),
            side: "BUY".to_string(),
            amount_sol: Decimal::from_str("0.1").unwrap(),
            status: "PENDING".to_string(),
        })
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
    let (db, _dir) = setup_test_db().await;
    let pool = sqlite_pool(&db);

    let uuid = "circuit-test-uuid-5678";
    let wallet = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
    let token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263";

    db.insert_trade(&InsertTrade {
        trade_uuid: uuid.to_string(),
        wallet_address: wallet.to_string(),
        token_address: token.to_string(),
        token_symbol: Some("BONK".to_string()),
        strategy: "SHIELD".to_string(),
        side: "BUY".to_string(),
        amount_sol: Decimal::from_str("1.0").unwrap(),
        status: "CLOSED".to_string(),
    })
    .await
    .unwrap();

    let big_loss = Decimal::from_str("-2.5").unwrap();
    db.update_trade_net_pnl(uuid, big_loss).await.unwrap();

    // Verify the loss is stored correctly
    let row: (String,) = sqlx::query_as("SELECT net_pnl_sol FROM trades WHERE trade_uuid = ?")
        .bind(uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    let net_pnl: f64 = row.0.parse().unwrap_or(0.0);
    assert!(
        net_pnl < 0.0,
        "net_pnl_sol should be negative for a losing trade"
    );
    assert!((net_pnl + 2.5).abs() < 0.0001, "net_pnl_sol should be -2.5");
}

#[tokio::test]
async fn test_trade_status_update() {
    // Verify that a trade's status can be updated from PENDING to CLOSED.
    let (db, _dir) = setup_test_db().await;
    let pool = sqlite_pool(&db);

    let uuid = "status-update-uuid-9012";
    let wallet = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
    let token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263";

    db.insert_trade(&InsertTrade {
        trade_uuid: uuid.to_string(),
        wallet_address: wallet.to_string(),
        token_address: token.to_string(),
        token_symbol: Some("BONK".to_string()),
        strategy: "SHIELD".to_string(),
        side: "BUY".to_string(),
        amount_sol: Decimal::from_str("0.5").unwrap(),
        status: "PENDING".to_string(),
    })
    .await
    .unwrap();

    db.update_trade_status(&UpdateTradeStatus {
        trade_uuid: uuid.to_string(),
        status: "CLOSED".to_string(),
        tx_signature: Some("tx_signature_abc".to_string()),
        error_message: None,
        network_fee_sol: None,
    })
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
    let (db, _dir) = setup_test_db().await;
    let pool = sqlite_pool(&db);

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

// =============================================================================
// Backend-Agnostic Test (Phase 0 Validation)
// =============================================================================

#[tokio::test]
async fn test_backend_agnostic_wallet_insert() {
    // This test demonstrates the new backend-agnostic test harness.
    // When TEST_DATABASE_URL is set and the postgres feature is enabled,
    // it runs against PostgreSQL. Otherwise, it runs against SQLite.
    
    let (db, _temp_dir, _backend) = common::create_test_db_from_env().await;
    
    // This operation should work on both SQLite and PostgreSQL
    // because we're using the Database trait abstraction
    let result = db.upsert_wallet(
        "test-wallet-backend-agnostic",
        Some(Decimal::from_str("55.0").unwrap()),
        Some(Decimal::from_str("12.0").unwrap()),
        Some(Decimal::from_str("30.0").unwrap()),
        Some(25),
        Some(Decimal::from_str("0.65").unwrap()),
        Some(Decimal::from_str("10.0").unwrap()),
        Some(Decimal::from_str("0.5").unwrap()),
        None,
    )
    .await;

    assert!(
        result.is_ok(),
        "upsert_wallet should work on both backends"
    );
    
    // The test passes on both backends, proving the harness works
    println!("Backend-agnostic wallet insert test passed");
}
