//! End-to-End Position Lifecycle Integration Tests
//!
//! Validates that the critical financial flows complete correctly:
//! - Duplicate BUY creates only one trade row + one position (UNIQUE constraint)
//! - SELL with no matching position is a no-op (not an error)
//! - PnL accuracy when fees are included (A1 cost model)
//! - Status transition correctness and FAILED → RETRY → EXECUTING

use chimera_operator::db_abstraction::{Database, InsertTrade, UpdateTradeStatus};
use rust_decimal::Decimal;
use sqlx::Pool;
use sqlx::Postgres;
use std::str::FromStr;
use std::sync::Arc;
use tempfile::TempDir;

fn pg_pool(db: &Arc<dyn Database>) -> Pool<Postgres> {
    crate::common::pg_pool(db)
}

// ─── helpers ─────────────────────────────────────────────────────────────────

async fn create_test_db() -> (Arc<dyn Database>, crate::common::TestDbGuard) {
    crate::common::create_test_pg_db().await
}

// ─── Test 90 (plan) ── duplicate BUY creates only one position ────────────────

#[tokio::test]
async fn test_duplicate_buy_uuid_idempotency() {
    // Two BUY signals with the same trade_uuid: the UNIQUE constraint on
    // trades.trade_uuid rejects the second insert with an error (callers must
    // swallow the error on retry — this is NOT an idempotent no-op), and
    // exactly one trade row + one position must exist.

    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    let uuid = "uuid-dup-buy";

    // First insert succeeds
    db.insert_trade(&InsertTrade {
        trade_uuid: uuid.to_string(),
        wallet_address: "wallet".to_string(),
        token_address: "token".to_string(),
        token_symbol: Some("T".to_string()),
        strategy: "SHIELD".to_string(),
        side: "BUY".to_string(),
        amount_sol: Decimal::from_str("1.0").unwrap(),
        status: "PENDING".to_string(),
    })
    .await
    .unwrap();
    db.activate_trade_and_open_position(
        uuid,
        "wallet",
        "token",
        Some("T"),
        "SHIELD",
        Decimal::from_str("1.0").unwrap(),
        Decimal::from_str("1.0").unwrap(),
        "sig1",
        None,
        None,
    )
    .await
    .unwrap();

    // Second insert for the same UUID must fail (UNIQUE violation)
    let second_insert = db
        .insert_trade(&InsertTrade {
            trade_uuid: uuid.to_string(),
            wallet_address: "wallet".to_string(),
            token_address: "token".to_string(),
            token_symbol: Some("T".to_string()),
            strategy: "SHIELD".to_string(),
            side: "BUY".to_string(),
            amount_sol: Decimal::from_str("1.0").unwrap(),
            status: "PENDING".to_string(),
        })
        .await;
    assert!(
        second_insert.is_err(),
        "Duplicate trade_uuid must be rejected by UNIQUE constraint"
    );

    // Only one position should exist
    let pos_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM positions WHERE trade_uuid = $1")
        .bind(uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        pos_count.0, 1,
        "Exactly one position must exist for a duplicated trade_uuid"
    );

    // And exactly one trade row
    let trade_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM trades WHERE trade_uuid = $1")
        .bind(uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        trade_count.0, 1,
        "Exactly one trade row must exist for a duplicated trade_uuid"
    );
}

// ─── Test 88 (plan) ── SELL with no active position is a no-op ───────────────

#[tokio::test]
async fn test_close_position_no_active_position_is_noop() {
    // close_position() on a token with no ACTIVE positions returns Ok with a WARN log.
    // No trade record is created. No position is modified.

    let (db, _tmp) = create_test_db().await;

    let result = db
        .close_position_full(
            "uuid-nosell",
            "wallet_nosell",
            "token_nosell",
            Decimal::from_str("2.0").unwrap(),
            "sig_exit",
            None,
            Decimal::ONE,
            true,
        )
        .await;

    assert!(
        result.is_ok(),
        "Closing non-existent position should not error"
    );
    assert!(
        !result.unwrap(),
        "No active position was closed"
    );

    let pool = pg_pool(&db);
    let pos_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM positions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        pos_count.0, 0,
        "No positions should exist after close on empty DB"
    );

    // The documented invariant also covers trades: no trade record is created
    // by the no-op close path.
    let trade_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM trades")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        trade_count.0, 0,
        "No trade rows should exist after close on empty DB"
    );
}

// ─── Test 92 (plan) ── PnL accuracy with fees ─────────────────────────────────

#[tokio::test]
async fn test_pnl_calculation_accuracy_with_fees() {
    // Scenario: BUY 1 SOL at $100. SELL at $110.
    // Gross PnL: (110 - 100) / 100 × 1 SOL = +0.1 SOL
    //
    // Costs are recorded on the trade rows BEFORE the close (the production
    // order), so `close_position_full` computes and persists the net PnL
    // itself using the A1 cost model: net = gross − entry tips − entry network
    // fees − exit tips − exit network fees (DEX fee and slippage are
    // attribution-only and are NOT subtracted a second time).
    //
    // Entry tip 0.001 + exit tip 0.001, no network fees: net = 0.1 − 0.002 = 0.098.

    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    let entry_uuid = "uuid-pnl-fees";
    let exit_uuid = "uuid-pnl-fees-exit";

    db.insert_trade(&InsertTrade {
        trade_uuid: entry_uuid.to_string(),
        wallet_address: "wallet_f".to_string(),
        token_address: "token_f".to_string(),
        token_symbol: Some("F".to_string()),
        strategy: "SHIELD".to_string(),
        side: "BUY".to_string(),
        amount_sol: Decimal::from_str("1.0").unwrap(),
        status: "ACTIVE".to_string(),
    })
    .await
    .unwrap();
    db.activate_trade_and_open_position(
        entry_uuid,
        "wallet_f",
        "token_f",
        Some("F"),
        "SHIELD",
        Decimal::from_str("1.0").unwrap(),
        Decimal::from_str("100.0").unwrap(), // entry $100
        "sig_buy_f",
        None,
        None,
    )
    .await
    .unwrap();

    // Entry costs (on the BUY trade row).
    db.update_trade_costs(
        entry_uuid,
        Decimal::from_str("0.001").unwrap(),  // Jito tip
        Decimal::from_str("0.0005").unwrap(), // DEX fee (attribution only)
        Decimal::from_str("0.0002").unwrap(), // slippage (attribution only)
    )
    .await
    .unwrap();

    // The SELL trade row carries the exit-side costs (the production flow).
    db.insert_trade(&InsertTrade {
        trade_uuid: exit_uuid.to_string(),
        wallet_address: "wallet_f".to_string(),
        token_address: "token_f".to_string(),
        token_symbol: Some("F".to_string()),
        strategy: "EXIT".to_string(),
        side: "SELL".to_string(),
        amount_sol: Decimal::from_str("1.0").unwrap(),
        status: "EXITING".to_string(),
    })
    .await
    .unwrap();
    db.update_trade_costs(
        exit_uuid,
        Decimal::from_str("0.001").unwrap(), // exit Jito tip
        Decimal::from_str("0.0005").unwrap(), // exit DEX fee (attribution only)
        Decimal::from_str("0.0002").unwrap(), // exit slippage (attribution only)
    )
    .await
    .unwrap();

    // Sell at $110 (passing the EXIT trade uuid — its row supplies the
    // exit-side costs).
    db.close_position_full(
        exit_uuid,
        "wallet_f",
        "token_f",
        Decimal::from_str("110.0").unwrap(),
        "sig_sell_f",
        None,
        Decimal::ONE,
        true,
    )
    .await
    .unwrap();

    let (realized_pnl, realized_net_pnl): (Decimal, Decimal) = sqlx::query_as(
        "SELECT COALESCE(realized_pnl_sol, 0), COALESCE(realized_net_pnl_sol, 0) \
         FROM positions WHERE trade_uuid = $1",
    )
    .bind(entry_uuid)
    .fetch_one(&pool)
    .await
    .unwrap();

    // Expected: (110 - 100) / 100 × 1.0 = +0.1 SOL gross, exactly.
    assert_eq!(
        realized_pnl, Decimal::from_str("0.1").unwrap(),
        "Gross PnL should be exactly +0.1 SOL"
    );
    // A1: net = gross − entry tip − exit tip = 0.1 − 0.002.
    assert_eq!(
        realized_net_pnl, Decimal::from_str("0.098").unwrap(),
        "Net PnL after fees should be exactly +0.098 SOL (A1 cost model)"
    );

    // The trades row's own net_pnl_sol is also persisted by the close path.
    let (trade_net,): (Decimal,) =
        sqlx::query_as("SELECT COALESCE(net_pnl_sol, 0) FROM trades WHERE trade_uuid = $1")
            .bind(exit_uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        trade_net, Decimal::from_str("0.098").unwrap(),
        "trades.net_pnl_sol must match the position's realized net PnL"
    );
}

// ─── Test: trade_uuid_exists checks both tables ───────────────────────────────

#[tokio::test]
async fn test_trade_uuid_exists_checks_dead_letter_queue() {
    // trade_uuid_exists() checks both `trades` and `dead_letter_queue` tables.
    // A UUID in the DLQ should be detected as existing to prevent re-processing.

    let (db, _tmp) = create_test_db().await;
    let uuid = "uuid-dlq-check";

    // Not in any table
    let exists_before = db.trade_uuid_exists(uuid).await.unwrap();
    assert!(!exists_before, "UUID must not exist before insertion");

    // Insert into dead_letter_queue
    db.insert_dlq(Some(uuid), "{}", "test reason", None, None)
        .await
        .unwrap();

    let exists_dlq = db.trade_uuid_exists(uuid).await.unwrap();
    assert!(
        exists_dlq,
        "UUID in DLQ must be detected by trade_uuid_exists()"
    );
}

// ─── Test: status transition correctness ─────────────────────────────────────

#[tokio::test]
async fn test_full_trade_status_progression() {
    // A successful trade should flow: PENDING → QUEUED → EXECUTING → ACTIVE → CLOSED.

    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    let uuid = "uuid-full-flow";

    db.insert_trade(&InsertTrade {
        trade_uuid: uuid.to_string(),
        wallet_address: "wallet".to_string(),
        token_address: "token".to_string(),
        token_symbol: None,
        strategy: "SHIELD".to_string(),
        side: "BUY".to_string(),
        amount_sol: Decimal::from_str("1.0").unwrap(),
        status: "PENDING".to_string(),
    })
    .await
    .unwrap();

    for (status, sig) in [
        ("QUEUED", None),
        ("EXECUTING", None),
        ("ACTIVE", Some("sig123")),
    ] {
        db.update_trade_status(&UpdateTradeStatus {
            trade_uuid: uuid.to_string(),
            status: status.to_string(),
            tx_signature: sig.map(|s| s.to_string()),
            error_message: None,
            network_fee_sol: None,
        })
        .await
        .unwrap();
        let (s,): (String,) = sqlx::query_as("SELECT status FROM trades WHERE trade_uuid = $1")
            .bind(uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(s, status, "Status should be {}", status);
    }

    // Open position
    db.activate_trade_and_open_position(
        uuid,
        "wallet",
        "token",
        None,
        "SHIELD",
        Decimal::from_str("1.0").unwrap(),
        Decimal::from_str("50.0").unwrap(),
        "sig123",
        None,
        None,
    )
    .await
    .unwrap();

    // Close position
    db.close_position_full(
        uuid,
        "wallet",
        "token",
        Decimal::from_str("60.0").unwrap(),
        "sig_exit",
        None,
        Decimal::ONE,
        true,
    )
    .await
    .unwrap();

    db.update_trade_status(&UpdateTradeStatus {
        trade_uuid: uuid.to_string(),
        status: "CLOSED".to_string(),
        tx_signature: Some("sig_exit".to_string()),
        error_message: None,
        network_fee_sol: None,
    })
    .await
    .unwrap();

    let (final_status,): (String,) =
        sqlx::query_as("SELECT status FROM trades WHERE trade_uuid = $1")
            .bind(uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(final_status, "CLOSED");

    let (pos_state,): (String,) =
        sqlx::query_as("SELECT state FROM positions WHERE trade_uuid = $1")
            .bind(uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(pos_state, "CLOSED");
}

// ─── Test: FAILED → RETRY → EXECUTING ────────────────────────────────────────

#[tokio::test]
async fn test_failed_trade_can_retry() {
    // A FAILED trade should be retryable: FAILED → RETRY → EXECUTING.

    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    let uuid = "uuid-retry";

    db.insert_trade(&InsertTrade {
        trade_uuid: uuid.to_string(),
        wallet_address: "wallet".to_string(),
        token_address: "token".to_string(),
        token_symbol: None,
        strategy: "SHIELD".to_string(),
        side: "BUY".to_string(),
        amount_sol: Decimal::from_str("1.0").unwrap(),
        status: "PENDING".to_string(),
    })
    .await
    .unwrap();

    db.update_trade_status(&UpdateTradeStatus {
        trade_uuid: uuid.to_string(),
        status: "FAILED".to_string(),
        tx_signature: None,
        error_message: Some("RPC timeout".to_string()),
        network_fee_sol: None,
    })
    .await
    .unwrap();
    db.update_trade_status(&UpdateTradeStatus {
        trade_uuid: uuid.to_string(),
        status: "RETRY".to_string(),
        tx_signature: None,
        error_message: None,
        network_fee_sol: None,
    })
    .await
    .unwrap();
    db.update_trade_status(&UpdateTradeStatus {
        trade_uuid: uuid.to_string(),
        status: "EXECUTING".to_string(),
        tx_signature: None,
        error_message: None,
        network_fee_sol: None,
    })
    .await
    .unwrap();

    let (status,): (String,) = sqlx::query_as("SELECT status FROM trades WHERE trade_uuid = $1")
        .bind(uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "EXECUTING", "Retried trade should be EXECUTING");
}
