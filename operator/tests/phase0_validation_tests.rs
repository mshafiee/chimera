//! Phase 0 Validation Tests
//!
//! These tests validate that the test harness works correctly against the
//! database backend and that Database-trait methods behave as documented:
//! persistence, filtering, Decimal round-trips, and status updates.
//!
//! NOTE: the shared harness (tests/common/mod.rs) is PostgreSQL-only — SQLite
//! was decommissioned. These tests require a live Postgres instance and are
//! `#[ignore]`d so a default test run without TEST_DATABASE_URL stays green:
//!   TEST_DATABASE_URL="postgresql://user:pass@localhost:5432/postgres" \
//!     cargo test --test phase0_validation_tests -- --ignored

mod common;

use chimera_operator::db_abstraction::Database;
use rust_decimal::prelude::*;

#[tokio::test]
#[ignore] // Requires TEST_DATABASE_URL (PostgreSQL)
async fn test_phase0_wallet_operations() {
    // Validates upsert_wallet persists through the trait abstraction
    let (db, _guard) = common::create_test_db().await;

    let wallet_addr = "test-phase0-wallet";

    let result = db
        .upsert_wallet(
            wallet_addr,
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

    result.expect("upsert_wallet should work");

    // Verify the write actually persisted: re-read and assert stored fields.
    let wallet = db.get_wallet(wallet_addr).await.unwrap();
    let wallet = wallet.expect("wallet must be persisted");
    assert_eq!(wallet.status, "CANDIDATE");
    assert_eq!(wallet.wqs_score, Some(Decimal::from_str("55.0").unwrap()));
}

#[tokio::test]
#[ignore] // Requires TEST_DATABASE_URL (PostgreSQL)
async fn test_phase0_trade_insert_and_query() {
    // Validates insert_trade and basic query operations work
    let (db, _guard) = common::create_test_db().await;

    let trade_uuid = "test-phase0-trade";
    let wallet_address = "test-wallet";

    db.insert_trade(&chimera_operator::db_abstraction::InsertTrade {
        trade_uuid: trade_uuid.to_string(),
        wallet_address: wallet_address.to_string(),
        token_address: "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263".to_string(),
        token_symbol: Some("BONK".to_string()),
        strategy: "SHIELD".to_string(),
        side: "BUY".to_string(),
        amount_sol: Decimal::from_str("0.5").unwrap(),
        status: "PENDING".to_string(),
    })
    .await
    .expect("insert_trade should work");

    // Verify trade exists — filter by the inserted wallet so the assertion is
    // self-contained and cannot be affected by unrelated rows.
    let trades = db
        .get_trades_filtered(None, None, None, None, Some(wallet_address), 10, 0)
        .await
        .expect("get_trades_filtered should work");

    assert_eq!(
        trades.len(), 1,
        "Should find exactly 1 trade for {wallet_address}"
    );
    assert_eq!(trades[0].trade_uuid, trade_uuid);
}

#[tokio::test]
#[ignore] // Requires TEST_DATABASE_URL (PostgreSQL)
async fn test_phase0_decimal_precision() {
    // Validates that Decimal values round-trip losslessly through NUMERIC
    // (AGENTS.md no-float-for-money rule).
    let (db, _guard) = common::create_test_db().await;

    let test_amount = Decimal::from_str("0.123456789").unwrap();
    let wallet_addr = "test-decimal-wallet";

    db.upsert_wallet(
        wallet_addr,
        Some(Decimal::from_str("55.0").unwrap()),
        None,
        None,
        None,
        None,
        None,
        Some(test_amount),
        None,
    )
    .await
    .expect("upsert_wallet with decimal should work");

    // Read the value back and assert EXACT equality — a backend that
    // truncates/rounds NUMERIC on write must fail here.
    let wallet = db.get_wallet(wallet_addr).await.unwrap();
    let wallet = wallet.expect("wallet must be persisted");
    assert_eq!(
        wallet.avg_trade_size_sol,
        Some(test_amount),
        "Decimal must round-trip exactly through NUMERIC"
    );
}

#[tokio::test]
#[ignore] // Requires TEST_DATABASE_URL (PostgreSQL)
async fn test_phase0_wallet_status_update() {
    // Validates update_wallet_status_ext persists status + notes
    let (db, _guard) = common::create_test_db().await;

    let wallet_addr = "test-status-wallet";

    db.upsert_wallet(
        wallet_addr,
        Some(Decimal::from_str("55.0").unwrap()),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .expect("upsert_wallet should work");

    let result = db
        .update_wallet_status_ext(
            wallet_addr,
            "ACTIVE",
            Some(24),
            Some("promoted for testing"),
        )
        .await;

    result.expect("update_wallet_status_ext should work");

    // Re-read and verify the update actually persisted (a no-op implementation
    // must not pass).
    let wallet = db.get_wallet(wallet_addr).await.unwrap();
    let wallet = wallet.expect("wallet must be persisted");
    assert_eq!(wallet.status, "ACTIVE", "status must be updated");
    assert!(
        wallet.promoted_at.is_some(),
        "promoted_at must be set when status becomes ACTIVE"
    );
    assert!(
        wallet.ttl_expires_at.is_some(),
        "ttl_expires_at must be set from ttl_hours"
    );
    assert_eq!(
        wallet.notes.as_deref(),
        Some("promoted for testing"),
        "notes must be persisted"
    );
}

#[tokio::test]
#[ignore] // Requires TEST_DATABASE_URL (PostgreSQL)
async fn test_phase0_postgres_specific_validation() {
    // Postgres-specific check: the NUMERIC columns used for financial values
    // must carry a fractional scale (the decommissioned SQLite backend stored
    // TEXT; NUMERIC(30,0) would silently round financial values).
    let (db, _guard) = common::create_test_db().await;
    let pool = common::pg_pool(&db);

    let scale: i64 = sqlx::query_scalar(
        "SELECT numeric_scale FROM information_schema.columns \
         WHERE table_name = 'trades' AND column_name = 'amount_sol'",
    )
    .fetch_one(&pool)
    .await
    .expect("information_schema query must run");

    assert!(
        scale > 0,
        "trades.amount_sol must have fractional scale (NUMERIC(30,0) would truncate), got scale {scale}"
    );
}

// =============================================================================
// Summary of Phase 0 Exit Criteria Validation
// =============================================================================
//
// The tests above validate the following exit criteria from the plan:
//
// ✅ 1. Harness exists (tests/common/mod.rs) — PostgreSQL-only, per-test DB
// ✅ 2. create_test_db() creates an isolated database (dropped on teardown)
// ✅ 3. Database trait methods work through the abstraction
// ✅ 4. Decimal values round-trip losslessly through NUMERIC
// ✅ 5. Status updates persist (status/promoted_at/ttl/notes)
// ✅ 6. Postgres NUMERIC columns carry fractional scale
//
// Usage (requires TEST_DATABASE_URL):
//   TEST_DATABASE_URL="postgresql://localhost/postgres" \
//     cargo test --test phase0_validation_tests -- --ignored
