//! Integration tests for Chimera Operator
//!
//! Tests database operations and system behavior using an in-memory test DB.

use chimera_operator::db_abstraction::{Database, InsertTrade, UpdateTradeStatus};
use rust_decimal::prelude::*;
use sqlx::Pool;
use sqlx::Postgres;
use std::sync::Arc;

mod common;

fn pg_pool(db: &Arc<dyn Database>) -> Pool<Postgres> {
    common::pg_pool(db)
}

/// Setup test database (drops the created database on teardown via the guard)
async fn setup_test_db() -> (Arc<dyn Database>, common::TestDbGuard) {
    common::create_test_db().await
}

#[tokio::test]
async fn test_health_check_db_connectivity() {
    // Verifies that the test DB can be set up and migrations applied successfully.
    let (db, _dir) = setup_test_db().await;
    let pool = pg_pool(&db);
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
    let pool = pg_pool(&db);

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
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM trades WHERE trade_uuid = $1")
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
    let pool = pg_pool(&db);

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
    sqlx::query("UPDATE trades SET net_pnl_sol = $1 WHERE trade_uuid = $2")
        .bind(big_loss)
        .bind(uuid)
        .execute(&pool)
        .await
        .unwrap();

    // Verify the loss is stored exactly (decode NUMERIC as Decimal — sqlx
    // cannot decode NUMERIC into String, and f64 round-tripping would be lossy
    // for a 30-digit financial value).
    let (net_pnl,): (Decimal,) =
        sqlx::query_as("SELECT net_pnl_sol FROM trades WHERE trade_uuid = $1")
            .bind(uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(net_pnl, big_loss, "net_pnl_sol should be exactly -2.5");
}

#[tokio::test]
async fn test_trade_status_update() {
    // Verify that a trade's status can be updated from PENDING to CLOSED.
    let (db, _dir) = setup_test_db().await;
    let pool = pg_pool(&db);

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

    let row: (String,) = sqlx::query_as("SELECT status FROM trades WHERE trade_uuid = $1")
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
    let pool = pg_pool(&db);

    let address = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";

    sqlx::query(
        "INSERT INTO wallets (address, status, wqs_score, roi_7d, roi_30d, trade_count_30d, win_rate, max_drawdown_30d, avg_trade_size_sol)
         VALUES ($1, 'CANDIDATE', 55.0, 12.0, 30.0, 25, 0.65, 10.0, 0.5)",
    )
    .bind(address)
    .execute(&pool)
    .await
    .expect("Wallet insert should succeed");

    let row: (String,) = sqlx::query_as("SELECT status FROM wallets WHERE address = $1")
        .bind(address)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.0, "CANDIDATE", "Wallet status should be CANDIDATE");

    // Simulate wallet promotion
    sqlx::query("UPDATE wallets SET status = 'ACTIVE' WHERE address = $1")
        .bind(address)
        .execute(&pool)
        .await
        .unwrap();

    let row: (String,) = sqlx::query_as("SELECT status FROM wallets WHERE address = $1")
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
    // NOTE: despite the name, the harness is PostgreSQL-only — the Database
    // trait abstracts the backend, but create_test_db_from_env() always
    // creates a Postgres database and requires TEST_DATABASE_URL.
    let (db, _guard, _backend) = common::create_test_db_from_env().await;

    let address = "test-wallet-backend-agnostic";

    // This operation should work through the Database trait abstraction
    let result = db
        .upsert_wallet(
            address,
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

    result.expect("upsert_wallet should work through the Database trait");

    // Verify the write actually persisted: re-read the row and assert the
    // stored fields instead of trusting the upsert result alone.
    let pool = pg_pool(&db);
    let (status,): (String,) = sqlx::query_as("SELECT status FROM wallets WHERE address = $1")
        .bind(address)
        .fetch_one(&pool)
        .await
        .expect("wallet should have been persisted");
    assert_eq!(status, "CANDIDATE", "upserted wallet must be persisted as CANDIDATE");

    println!("Backend-agnostic wallet insert test passed");
}
