//! Financial-precision regression tests (Phase 2 validation).
//!
//! These tests round-trip a high-precision `Decimal` (18 fractional digits) through the
//! database and assert EXACT equality on read-back. This is the precision guarantee that
//! the legacy `f64_to_decimal` / `decimal_to_f64` bridge helpers could NOT provide: an
//! f64 has only ~15-17 significant digits, so `Decimal -> f64 -> Decimal` would corrupt
//! `0.123456789012345678` into `0.12345678901234568`. After Phase 2 the data path is
//! `Decimal -> NUMERIC -> Decimal` (lossless) on Postgres and `Decimal -> TEXT -> Decimal`
//! (lossless) on SQLite, so these values must survive unchanged on BOTH backends.
//!
//! Run against SQLite (default):
//!   cargo test --test precision_regression_tests
//!
//! Run against Postgres (requires a live instance):
//!   TEST_DATABASE_URL="postgresql://user:pass@localhost:5432/postgres" \
//!     cargo test --test precision_regression_tests --features postgres

mod common;

use chimera_operator::db_abstraction::InsertTrade;
use rust_decimal::prelude::*;

/// Round-trip a single 18-digit Decimal through insert + read on the active backend.
#[tokio::test]
async fn test_high_precision_amount_round_trip() {
    let (db, _temp_dir, backend) = common::create_test_db_from_env().await;

    // 18 fractional digits — beyond f64 precision (~15-17 sig digits).
    let precise_amount = Decimal::from_str("0.123456789012345678").unwrap();

    let trade_uuid = "precision-amount-roundtrip";
    db.insert_trade(&InsertTrade {
        trade_uuid: trade_uuid.to_string(),
        wallet_address: "precision-wallet".to_string(),
        token_address: "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263".to_string(),
        token_symbol: Some("BONK".to_string()),
        strategy: "SHIELD".to_string(),
        side: "BUY".to_string(),
        amount_sol: precise_amount,
        status: "PENDING".to_string(),
    })
    .await
    .expect("insert_trade should succeed");

    let trades = db
        .get_trades_filtered(None, None, None, None, None, 10, 0)
        .await
        .expect("get_trades_filtered should succeed");

    let trade = trades
        .iter()
        .find(|t| t.trade_uuid == trade_uuid)
        .unwrap_or_else(|| panic!("inserted trade not found on {} backend", backend));

    assert_eq!(
        trade.amount_sol, precise_amount,
        "amount_sol must round-trip with EXACT precision on {} backend (would fail under f64 bridge)",
        backend
    );
}

/// Round-trip a high-precision realized PnL through update + read.
#[tokio::test]
async fn test_high_precision_net_pnl_round_trip() {
    let (db, _temp_dir, backend) = common::create_test_db_from_env().await;

    // Negative 18-digit value — also beyond f64 precision.
    let precise_pnl = Decimal::from_str("-0.987654321098765432").unwrap();
    let trade_uuid = "precision-pnl-roundtrip";

    db.insert_trade(&InsertTrade {
        trade_uuid: trade_uuid.to_string(),
        wallet_address: "precision-wallet".to_string(),
        token_address: "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263".to_string(),
        token_symbol: Some("BONK".to_string()),
        strategy: "SHIELD".to_string(),
        side: "BUY".to_string(),
        amount_sol: Decimal::from_str("1.0").unwrap(),
        status: "CLOSED".to_string(),
    })
    .await
    .expect("insert_trade should succeed");

    db.update_trade_net_pnl(trade_uuid, precise_pnl)
        .await
        .expect("update_trade_net_pnl should succeed");

    let trades = db
        .get_trades_filtered(None, None, None, None, None, 10, 0)
        .await
        .expect("get_trades_filtered should succeed");

    let trade = trades
        .iter()
        .find(|t| t.trade_uuid == trade_uuid)
        .unwrap_or_else(|| panic!("inserted trade not found on {} backend", backend));

    assert_eq!(
        trade.net_pnl_sol,
        Some(precise_pnl),
        "net_pnl_sol must round-trip with EXACT precision on {} backend (would fail under f64 bridge)",
        backend
    );
}

/// Sanity check: confirm the f64 path would actually corrupt this value, proving the
/// test is meaningful (not vacuous). This is a pure-logic assertion, no DB involved.
#[test]
fn test_f64_would_lose_precision() {
    let precise = Decimal::from_str("0.123456789012345678").unwrap();
    // Simulate the old bridge: Decimal -> f64 -> Decimal.
    let via_f64 = Decimal::from_f64_retain(precise.to_f64().unwrap()).unwrap();
    assert_ne!(
        precise, via_f64,
        "precondition: the f64 round-trip must differ — otherwise this guard is vacuous"
    );
}
