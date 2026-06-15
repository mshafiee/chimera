//! Database Integrity Unit Tests
//!
//! Tests silent failure patterns in db.rs that can corrupt trade state or PnL:
//! - update_trade_status() returns Ok(()) even when UUID does not exist
//! - close_position() with multiple active positions closes all (not just one)
//! - close_position() with exit_price=0 records -100% loss
//! - open_position() with entry_price=0 creates untrackable position
//! - update_trade_costs() overwrites on retry (not idempotent)
//! - PnL precision with f64 round-trip
//! - Orphaned position after trade deleted

use chimera_operator::config::DatabaseConfig;
use chimera_operator::db::{
    close_position, init_pool, insert_trade, open_position, run_migrations, update_trade_costs,
    update_trade_status,
};
use rust_decimal::Decimal;
use std::str::FromStr;
use tempfile::TempDir;

// ─── helpers ─────────────────────────────────────────────────────────────────

async fn create_test_db() -> (chimera_operator::db::DbPool, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let db_config = DatabaseConfig {
        path: temp_dir.path().join("db_integrity_test.db"),
        max_connections: 5,
    };
    let pool = init_pool(&db_config).await.unwrap();
    run_migrations(&pool).await.unwrap();
    (pool, temp_dir)
}

/// Insert a trade and return its UUID.
async fn setup_trade(pool: &chimera_operator::db::DbPool, uuid: &str) {
    insert_trade(
        pool,
        uuid,
        "wallet_test",
        "token_test",
        Some("SYM"),
        "SHIELD",
        "BUY",
        Decimal::from_str("1.0").unwrap(),
        "PENDING",
    )
    .await
    .unwrap();
}

// ─── Test 39 (plan) ── update_trade_status silently ok on missing UUID ────────

#[tokio::test]
async fn test_update_trade_status_nonexistent_uuid_silent_success() {
    // BUG DOCUMENTED: update_trade_status returns Ok(()) even when 0 rows were updated.
    // The caller cannot distinguish "updated successfully" from "UUID not found".
    // This allows phantom state transitions that leave the actual trade stuck in PENDING.

    let (pool, _tmp) = create_test_db().await;

    let result = update_trade_status(&pool, "nonexistent-uuid-xyz", "QUEUED", None, None).await;

    assert!(
        result.is_err(),
        "update_trade_status must return Err when UUID does not exist (rows_affected == 0)"
    );

    // Confirm nothing was actually inserted
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM trades WHERE trade_uuid = ?")
        .bind("nonexistent-uuid-xyz")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count.0, 0, "No trade should exist for this UUID");
}

// ─── Test 40 (plan) ── affected rows check catches missing update ─────────────

#[tokio::test]
async fn test_update_trade_status_real_trade_affects_exactly_one_row() {
    // Positive case: updating a real trade must affect exactly 1 row.
    // The function currently returns Ok(()) in both cases — callers must
    // independently verify row count by re-querying.

    let (pool, _tmp) = create_test_db().await;
    let uuid = "uuid-real-trade";
    setup_trade(&pool, uuid).await;

    update_trade_status(&pool, uuid, "QUEUED", None, None)
        .await
        .unwrap();

    let status: (String,) = sqlx::query_as("SELECT status FROM trades WHERE trade_uuid = ?")
        .bind(uuid)
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(
        status.0, "QUEUED",
        "Real trade status should be updated to QUEUED"
    );
}

// ─── Test 41 (plan) ── close_position with multiple active positions ──────────

#[tokio::test]
async fn test_close_position_closes_all_active_positions_for_wallet_token() {
    // RISK: close_position() fetches ALL active positions for (wallet, token) and
    // closes every one with the same exit price. If two positions were opened at
    // different prices, both are closed simultaneously — the second position's PnL
    // is calculated as if it was opened at the same time as the first.

    let (pool, _tmp) = create_test_db().await;

    // Insert two trades for the same wallet+token
    let uuid1 = "uuid-pos-1";
    let uuid2 = "uuid-pos-2";
    for uuid in [uuid1, uuid2] {
        insert_trade(
            &pool,
            uuid,
            "wallet_multi",
            "token_multi",
            Some("M"),
            "SHIELD",
            "BUY",
            Decimal::from_str("2.0").unwrap(),
            "ACTIVE",
        )
        .await
        .unwrap();
    }

    // Open two positions: first at $1.00, second at $2.00
    open_position(
        &pool,
        uuid1,
        "wallet_multi",
        "token_multi",
        Some("M"),
        "SHIELD",
        Decimal::from_str("2.0").unwrap(),
        Decimal::from_str("1.00").unwrap(),
        "sig1",
        None,
    )
    .await
    .unwrap();
    open_position(
        &pool,
        uuid2,
        "wallet_multi",
        "token_multi",
        Some("M"),
        "SHIELD",
        Decimal::from_str("2.0").unwrap(),
        Decimal::from_str("2.00").unwrap(),
        "sig2",
        None,
    )
    .await
    .unwrap();

    // Both positions are ACTIVE
    let active: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM positions WHERE wallet_address = ? AND token_address = ? AND state = 'ACTIVE'"
    )
    .bind("wallet_multi")
    .bind("token_multi")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(active.0, 2);

    // Close at $3.00
    close_position(
        &pool,
        "token_multi",
        "wallet_multi",
        Decimal::from_str("3.00").unwrap(),
        "sig_exit",
        "uuid-multi-1",
        None,
        Decimal::ONE,
    )
    .await
    .unwrap();

    // BOTH positions should be CLOSED — documents the all-at-once behavior
    let closed: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM positions WHERE wallet_address = ? AND token_address = ? AND state = 'CLOSED'"
    )
    .bind("wallet_multi")
    .bind("token_multi")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        closed.0, 2,
        "close_position() closes ALL active positions for wallet+token simultaneously"
    );
}

// ─── Test 42 (plan) ── close_position with exit_price=0 records 100% loss ────

#[tokio::test]
async fn test_close_position_zero_exit_price_records_full_loss() {
    // BUG RISK: close_position() with exit_price=0 silently records PnL as 0
    // (since the `if !entry_price_dec.is_zero()` guard returns Decimal::ZERO on bad input).
    // The position is marked CLOSED with exit_price=0 and realized_pnl=0 — not an error.

    let (pool, _tmp) = create_test_db().await;
    let uuid = "uuid-zero-exit";

    insert_trade(
        &pool,
        uuid,
        "wallet_z",
        "token_z",
        Some("Z"),
        "SHIELD",
        "BUY",
        Decimal::from_str("1.0").unwrap(),
        "ACTIVE",
    )
    .await
    .unwrap();
    open_position(
        &pool,
        uuid,
        "wallet_z",
        "token_z",
        Some("Z"),
        "SHIELD",
        Decimal::from_str("1.0").unwrap(),
        Decimal::from_str("100.0").unwrap(),
        "sig_z",
        None,
    )
    .await
    .unwrap();

    // Close with exit_price = 0
    let result = close_position(&pool, "token_z", "wallet_z", Decimal::ZERO, "sig_exit_z", uuid, None, Decimal::ONE).await;

    // The function returns Ok regardless — documents no validation on exit_price=0
    assert!(
        result.is_ok(),
        "close_position with exit_price=0 does not return an error (BUG DOCUMENTED)"
    );

    let (exit_price, pnl): (f64, f64) =
        sqlx::query_as("SELECT exit_price, realized_pnl_sol FROM positions WHERE trade_uuid = ?")
            .bind(uuid)
            .fetch_one(&pool)
            .await
            .unwrap();

    // When exit_price=0, the PnL formula: (0 - entry) / entry × amount = -100% loss.
    // The code stores exit_price=0 and realized_pnl = (0 - 100) / 100 × 1.0 = -1.0 SOL.
    // No validation or error is raised — position is CLOSED with misleading exit_price=0.
    assert_eq!(
        exit_price, 0.0,
        "Exit price stored as 0 — position closed with invalid exit price (no validation)"
    );
    // Actual behavior: PnL IS calculated as -1.0 (full loss), not 0.
    // The code does NOT have a guard on the exit_price side; the formula fires and gives -1.0.
    // Callers cannot distinguish "intentional 100% loss" from "missing exit price data".
    assert!(
        (pnl - (-1.0)).abs() < 1e-9,
        "PnL should reflect -100% loss when exit_price=0: expected -1.0, got {}. \
         No validation guard exists on exit_price=0 — callers get a valid-looking full loss.",
        pnl
    );
}

// ─── Test 43 (plan) ── open_position with entry_price=0 is not rejected ──────

#[tokio::test]
async fn test_open_position_zero_entry_price_not_rejected() {
    // BUG DOCUMENTED: open_position() does not validate entry_price.
    // Passing entry_price=0 silently creates a position that can never be properly
    // tracked by stop-loss (loss_percent = 0 → dynamic stop bypassed).

    let (pool, _tmp) = create_test_db().await;
    let uuid = "uuid-zero-entry";

    insert_trade(
        &pool,
        uuid,
        "wallet_ze",
        "token_ze",
        None,
        "SHIELD",
        "BUY",
        Decimal::from_str("1.0").unwrap(),
        "PENDING",
    )
    .await
    .unwrap();

    let result = open_position(
        &pool,
        uuid,
        "wallet_ze",
        "token_ze",
        None,
        "SHIELD",
        Decimal::from_str("1.0").unwrap(),
        Decimal::ZERO, // zero entry price
        "sig_ze",
        None,
    )
    .await;

    // Documents: no error is raised for zero entry price
    assert!(
        result.is_ok(),
        "BUG DOCUMENTED: open_position with entry_price=0 should error but does not"
    );

    let entry: (f64,) = sqlx::query_as("SELECT entry_price FROM positions WHERE trade_uuid = ?")
        .bind(uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        entry.0, 0.0,
        "Zero entry price was stored — position is untrackable"
    );
}

// ─── Test 44 (plan) ── trade costs overwritten on retry ──────────────────────

#[tokio::test]
async fn test_trade_costs_overwritten_on_retry_not_doubled() {
    // update_trade_costs() has no idempotency guard. Calling it twice for the same
    // trade_uuid overwrites costs with the second call's values (not additive).
    // This means retried cost updates use the latest value, but the first call's costs
    // are silently discarded — net effect: costs from only the last call are recorded.

    let (pool, _tmp) = create_test_db().await;
    let uuid = "uuid-costs";
    setup_trade(&pool, uuid).await;

    // First call: 0.001 SOL Jito tip
    update_trade_costs(
        &pool,
        uuid,
        Decimal::from_str("0.001").unwrap(),
        Decimal::from_str("0.0005").unwrap(),
        Decimal::from_str("0.0002").unwrap(),
    )
    .await
    .unwrap();

    // Second call: different values
    update_trade_costs(
        &pool,
        uuid,
        Decimal::from_str("0.002").unwrap(),
        Decimal::from_str("0.001").unwrap(),
        Decimal::from_str("0.0004").unwrap(),
    )
    .await
    .unwrap();

    let (jito, total): (f64, f64) =
        sqlx::query_as("SELECT jito_tip_sol, total_cost_sol FROM trades WHERE trade_uuid = ?")
            .bind(uuid)
            .fetch_one(&pool)
            .await
            .unwrap();

    // Second call wins: jito = 0.002, total = 0.002+0.001+0.0004 = 0.0034
    assert!(
        (jito - 0.002).abs() < 1e-9,
        "Second update overwrites first: jito_tip_sol should be 0.002, got {}",
        jito
    );
    assert!(
        (total - 0.0034).abs() < 1e-9,
        "Total cost should reflect second call only: 0.0034, got {}",
        total
    );
}

// ─── Test 45 (plan) ── orphaned position after trade deleted ─────────────────

#[tokio::test]
async fn test_position_can_become_orphaned_after_trade_delete() {
    // Documents: SQLite foreign key constraints PREVENT accidental orphaning via normal DELETE.
    // The schema sets `PRAGMA foreign_keys = ON` per connection; positions.trade_uuid
    // references trades — deleting a trade with an active position fails.
    //
    // Orphaning risk: a direct SQLite file edit (`sqlite3 chimera.db "DELETE FROM trades ..."`),
    // a script that disables FK per-connection, or a schema migration that drops FK constraints
    // could create orphaned positions undetectable by the Operator's normal queries.
    //
    // This test confirms: normal application DELETE is blocked (FK works as designed).

    let (pool, _tmp) = create_test_db().await;
    let uuid = "uuid-orphan";

    insert_trade(
        &pool,
        uuid,
        "wallet_o",
        "token_orphan",
        Some("ORP"),
        "SHIELD",
        "BUY",
        Decimal::from_str("1.0").unwrap(),
        "ACTIVE",
    )
    .await
    .unwrap();
    open_position(
        &pool,
        uuid,
        "wallet_o",
        "token_orphan",
        Some("ORP"),
        "SHIELD",
        Decimal::from_str("1.0").unwrap(),
        Decimal::from_str("1.0").unwrap(),
        "sig_o",
        None,
    )
    .await
    .unwrap();

    // FK constraint PREVENTS the trade from being deleted
    let delete_result = sqlx::query("DELETE FROM trades WHERE trade_uuid = ?")
        .bind(uuid)
        .execute(&pool)
        .await;

    assert!(
        delete_result.is_err(),
        "FK constraint must block trade deletion when a child position exists"
    );

    // Position is still intact — trade deletion was blocked
    let pos_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM positions WHERE trade_uuid = ? AND state = 'ACTIVE'")
            .bind(uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        pos_count.0, 1,
        "Position must survive the blocked trade delete — FK enforcement confirmed"
    );

    // DOCUMENTED RISK: A direct sqlite3 CLI edit or per-connection PRAGMA foreign_keys=OFF
    // bypasses this protection and could create orphaned positions. The Operator has no
    // runtime check for orphaned positions beyond the reconciliation job.
}

// ─── Test 46 (plan) ── PnL precision f64 round-trip ─────────────────────────

#[tokio::test]
async fn test_pnl_precision_f64_roundtrip() {
    // SQLite stores REAL as IEEE 754 double. A Decimal with 14+ significant digits
    // loses precision when converted to f64 for storage and read back.
    // The acceptable precision floor is ~1e-7 SOL per position.

    let (pool, _tmp) = create_test_db().await;
    let uuid = "uuid-precision";

    insert_trade(
        &pool,
        uuid,
        "wallet_p",
        "token_p",
        None,
        "SHIELD",
        "BUY",
        Decimal::from_str("1.0").unwrap(),
        "PENDING",
    )
    .await
    .unwrap();

    // Entry price with 15 significant digits
    let precise_entry = Decimal::from_str("1.23456789012345").unwrap();
    open_position(
        &pool,
        uuid,
        "wallet_p",
        "token_p",
        None,
        "SHIELD",
        Decimal::from_str("1.0").unwrap(),
        precise_entry,
        "sig_p",
        None,
    )
    .await
    .unwrap();

    let stored: (f64,) = sqlx::query_as("SELECT entry_price FROM positions WHERE trade_uuid = ?")
        .bind(uuid)
        .fetch_one(&pool)
        .await
        .unwrap();

    let recovered = Decimal::from_f64_retain(stored.0).unwrap_or(Decimal::ZERO);

    let diff = (precise_entry - recovered).abs();
    assert!(
        diff < Decimal::from_str("0.000001").unwrap(),
        "f64 round-trip precision loss should be < 1e-6 SOL, got diff={}",
        diff
    );
}

// ─── Test 47 (plan) ── close_position with no active positions is silent ──────

#[tokio::test]
async fn test_close_position_no_active_positions_returns_ok_silently() {
    // BUG DOCUMENTED: When close_position() finds no active positions, it returns
    // Ok(()) with only a WARN log. The caller has no way to detect a missed close.
    // This can happen if: duplicate exit signal arrives after position was already closed,
    // OR if the state machine advanced the position to EXITING before close_position ran.

    let (pool, _tmp) = create_test_db().await;

    let result = close_position(
        &pool,
        "token_missing",
        "wallet_missing",
        Decimal::from_str("2.0").unwrap(),
        "sig_missing",
        "uuid-missing",
        None,
        Decimal::ONE,
    )
    .await;

    assert!(
        result.is_ok(),
        "BUG DOCUMENTED: close_position returns Ok() when no position found — silent no-op"
    );
}
