//! Database Integrity Unit Tests
//!
//! Tests silent failure patterns in db.rs that can corrupt trade state or PnL:
//! - update_trade_status() returns Ok(()) even when UUID does not exist
//! - close_position() with multiple active positions closes all (not just one) [M3 FIXED]
//! - close_position() with exit_price=0 records -100% loss [M11 FIXED]
//! - open_position() with entry_price=0 creates untrackable position [M4 FIXED]
//! - update_trade_costs() accumulates on retry (M10 FIXED)
//! - PnL precision with f64 round-trip
//! - Orphaned position after trade deleted

use chimera_operator::config::DatabaseConfig;
use chimera_operator::db::{
    close_position, init_pool, insert_trade, open_position, revert_position_exit, run_migrations,
    update_trade_costs, update_trade_status,
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

// ─── Test 41 (plan) ── close_position closes only the specified position (M3 fix) ─

#[tokio::test]
async fn test_close_position_closes_only_specified_position() {
    // M3 FIX: close_position() now includes trade_uuid in WHERE clause, so only
    // the specified position is closed. If two positions were opened at different
    // prices, closing one leaves the other ACTIVE — no double-close bug.

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

    // Close ONLY uuid1 at $3.00
    close_position(
        &pool,
        "token_multi",
        "wallet_multi",
        Decimal::from_str("3.00").unwrap(),
        "sig_exit",
        uuid1,  // Only close this specific position
        None,
        Decimal::ONE,
        true,
    )
    .await
    .unwrap();

    // Only ONE position should be CLOSED (uuid1), uuid2 remains ACTIVE
    let closed: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM positions WHERE wallet_address = ? AND token_address = ? AND state = 'CLOSED'"
    )
    .bind("wallet_multi")
    .bind("token_multi")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        closed.0, 1,
        "M3 FIX: close_position() closes only the specified position (by trade_uuid)"
    );

    // Verify uuid2 is still ACTIVE
    let uuid2_state: (String,) = sqlx::query_as("SELECT state FROM positions WHERE trade_uuid = ?")
        .bind(uuid2)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(uuid2_state.0, "ACTIVE", "Second position must remain ACTIVE");
}

// ─── Test 42 (plan) ── close_position with exit_price=0 is rejected (M11 fix) ────

#[tokio::test]
async fn test_close_position_zero_exit_price_is_rejected() {
    // M11 FIX: close_position() now validates exit_price and returns Err when zero.
    // This prevents recording invalid PnL with exit_price=0.

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

    // Close with exit_price = 0 should return Err
    let result = close_position(
        &pool,
        "token_z",
        "wallet_z",
        Decimal::ZERO,
        "sig_exit_z",
        uuid,
        None,
        Decimal::ONE,
        true,
    )
    .await;

    // M11 FIX: Function returns Err when exit_price is zero
    assert!(
        result.is_err(),
        "M11 FIX: close_position with exit_price=0 must return error (validation added)"
    );

    // Position should still be ACTIVE (not closed)
    let state: (String,) = sqlx::query_as("SELECT state FROM positions WHERE trade_uuid = ?")
        .bind(uuid)
        .fetch_one(&pool)
        .await
    .unwrap();
    assert_eq!(state.0, "ACTIVE", "Position must remain ACTIVE when close fails");
}

// ─── Test 43 (plan) ── open_position with entry_price=0 is rejected (M4 fix) ──

#[tokio::test]
async fn test_open_position_zero_entry_price_is_rejected() {
    // M4 FIX: open_position() now validates entry_price and returns Err when zero.
    // This prevents creating untrackable positions that bypass stop-loss.

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

    // M4 FIX: Function returns Err when entry_price is zero
    assert!(
        result.is_err(),
        "M4 FIX: open_position with entry_price=0 must return error (validation added)"
    );

    // No position should have been created
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM positions WHERE trade_uuid = ?")
        .bind(uuid)
        .fetch_one(&pool)
        .await
    .unwrap();
    assert_eq!(count.0, 0, "No position should be created when entry_price is zero");
}

// ─── Test 44 (plan) ── trade costs accumulated on retry (M10 fix) ──────────────

#[tokio::test]
async fn test_trade_costs_accumulate_on_retry() {
    // M10 FIX: update_trade_costs() uses COALESCE accumulation so that retried cost
    // updates add to existing values rather than silently discarding the
    // first call's costs. Net effect: costs from all calls are summed.

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

    // Second call: different values — accumulates on top of first
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

    // Accumulated: jito = 0.001 + 0.002 = 0.003, total = 0.0017 + 0.0034 = 0.0051
    assert!(
        (jito - 0.003).abs() < 1e-9,
        "Accumulated jito_tip_sol should be 0.003, got {}",
        jito
    );
    assert!(
        (total - 0.0051).abs() < 1e-9,
        "Accumulated total cost should be 0.0051, got {}",
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
        true,
    )
    .await;

    assert!(
        result.is_ok(),
        "BUG DOCUMENTED: close_position returns Ok() when no position found — silent no-op"
    );
}

// ─── Test 48 ── close_position with confirmed=false sets state to EXITING ──────

#[tokio::test]
async fn test_close_position_unconfirmed_sets_exiting_state() {
    let (pool, _tmp) = create_test_db().await;
    let uuid = "uuid-unconfirmed-exit";

    insert_trade(
        &pool,
        uuid,
        "wallet_unconf",
        "token_unconf",
        Some("UNC"),
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
        "wallet_unconf",
        "token_unconf",
        Some("UNC"),
        "SHIELD",
        Decimal::from_str("1.0").unwrap(),
        Decimal::from_str("100.0").unwrap(),
        "sig_unconf_buy",
        None,
    )
    .await
    .unwrap();

    // Close with confirmed = false
    let result = close_position(
        &pool,
        "token_unconf",
        "wallet_unconf",
        Decimal::from_str("120.0").unwrap(),
        "sig_unconf_sell",
        uuid,
        None,
        Decimal::ONE,
        false, // confirmed = false
    )
    .await;

    assert!(result.is_ok());

    let (state, closed_at): (String, Option<String>) =
        sqlx::query_as("SELECT state, closed_at FROM positions WHERE trade_uuid = ?")
            .bind(uuid)
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_eq!(state, "EXITING");
    assert!(closed_at.is_none());
}

// ─── Test 49 ── revert_position_exit restores state and amount ──────────────────

#[tokio::test]
async fn test_revert_position_exit_restores_state_and_amount() {
    let (pool, _tmp) = create_test_db().await;
    let entry_uuid = "uuid-revert-entry";
    let exit_uuid = "uuid-revert-exit";

    // Setup Entry Trade and Position
    insert_trade(
        &pool,
        entry_uuid,
        "wallet_revert",
        "token_revert",
        Some("REV"),
        "SHIELD",
        "BUY",
        Decimal::from_str("1.5").unwrap(),
        "ACTIVE",
    )
    .await
    .unwrap();
    open_position(
        &pool,
        entry_uuid,
        "wallet_revert",
        "token_revert",
        Some("REV"),
        "SHIELD",
        Decimal::from_str("1.5").unwrap(),
        Decimal::from_str("100.0").unwrap(),
        "sig_revert_buy",
        None,
    )
    .await
    .unwrap();

    // Setup Exit Trade row representing the pending/failed exit
    insert_trade(
        &pool,
        exit_uuid,
        "wallet_revert",
        "token_revert",
        Some("REV"),
        "EXIT",
        "SELL",
        Decimal::from_str("0.5").unwrap(),
        "EXITING",
    )
    .await
    .unwrap();

    // Update status to associate signature
    sqlx::query("UPDATE trades SET tx_signature = ? WHERE trade_uuid = ?")
        .bind("sig_revert_sell")
        .bind(exit_uuid)
        .execute(&pool)
        .await
        .unwrap();

    // Call close_position with confirmed = false for partial exit (0.5 SOL / 1.5 SOL = 0.333333 fraction)
    // Note: With M3 fix, trade_uuid parameter must match the position's trade_uuid (entry_uuid)
    close_position(
        &pool,
        "token_revert",
        "wallet_revert",
        Decimal::from_str("120.0").unwrap(),
        "sig_revert_sell",
        entry_uuid,  // M3 FIX: Use entry_uuid (position's trade_uuid), not exit_uuid
        None,
        Decimal::from_str("0.33333333").unwrap(),
        false, // confirmed = false
    )
    .await
    .unwrap();

    // Verify DB states after unconfirmed partial close
    let (state_before, amount_before, exit_price_before, exit_sig_before, pnl_before): (String, f64, Option<f64>, Option<String>, Option<f64>) =
        sqlx::query_as("SELECT state, entry_amount_sol, exit_price, exit_tx_signature, realized_pnl_sol FROM positions WHERE trade_uuid = ?")
            .bind(entry_uuid)
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_eq!(state_before, "EXITING");
    assert!((amount_before - 1.0).abs() < 1e-6); // Decremented from 1.5 to 1.0
    assert!(exit_price_before.is_some());
    assert_eq!(exit_sig_before, Some("sig_revert_sell".to_string()));
    assert!(pnl_before.is_some());

    // Revert the failed exit
    let revert_res = revert_position_exit(&pool, entry_uuid).await;
    assert!(revert_res.is_ok());

    // Verify DB states after reversion
    let (state_after, amount_after, exit_price_after, exit_sig_after, pnl_after): (String, f64, Option<f64>, Option<String>, Option<f64>) =
        sqlx::query_as("SELECT state, entry_amount_sol, exit_price, exit_tx_signature, realized_pnl_sol FROM positions WHERE trade_uuid = ?")
            .bind(entry_uuid)
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_eq!(state_after, "ACTIVE");
    assert!((amount_after - 1.5).abs() < 1e-6); // Restored back to 1.5
    assert!(exit_price_after.is_none());
    assert!(exit_sig_after.is_none());
    assert!(pnl_after.is_none());

    // Verify the exit trade status is marked FAILED
    let exit_trade_status: (String,) =
        sqlx::query_as("SELECT status FROM trades WHERE trade_uuid = ?")
            .bind(exit_uuid)
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_eq!(exit_trade_status.0, "FAILED");
}
