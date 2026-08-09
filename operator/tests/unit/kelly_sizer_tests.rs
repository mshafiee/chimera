//! Unit tests for Kelly Criterion position sizing

use chimera_operator::db_abstraction::{Database, DbPool, InsertTrade};
use chimera_operator::engine::kelly_sizer::KellySizer;
use chimera_operator::models::Strategy;
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
use sqlx::Pool;
use sqlx::Postgres;
use std::sync::Arc;

#[path = "../common/mod.rs"]
mod common;

fn pg_pool(db: &Arc<dyn Database>) -> Pool<Postgres> {
    // DbPool is PostgreSQL-only (single variant): irrefutable destructure, no
    // fallback panic arm (which would be unreachable).
    let DbPool::PostgreSQL(pool) = db.pool();
    pool
}

/// Each test gets its own isolated database (dropped on teardown), so the
/// fixed trade UUIDs below never collide across runs.
async fn setup_test_db() -> (Arc<dyn Database>, common::TestDbGuard) {
    common::create_test_pg_db().await
}

/// Insert a CLOSED trade with the given size and net PnL.
async fn insert_closed_trade(
    db: &Arc<dyn Database>,
    uuid: &str,
    wallet: &str,
    token: &str,
    amount_sol: &str,
    net_pnl_sol: &str,
) {
    db.insert_trade(&InsertTrade {
        trade_uuid: uuid.to_string(),
        wallet_address: wallet.to_string(),
        token_address: token.to_string(),
        token_symbol: Some("BONK".to_string()),
        strategy: "SHIELD".to_string(),
        side: "BUY".to_string(),
        amount_sol: Decimal::from_str(amount_sol).unwrap(),
        status: "CLOSED".to_string(),
    })
    .await
    .unwrap();
    sqlx::query("UPDATE trades SET net_pnl_sol = $1 WHERE trade_uuid = $2")
        .bind(Decimal::from_str(net_pnl_sol).unwrap())
        .bind(uuid)
        .execute(&pg_pool(db))
        .await
        .unwrap();
}

#[tokio::test]
async fn test_kelly_zero_trade_history() {
    // With no closed trades, calculate_kelly should return an error.
    let (db, _dir) = setup_test_db().await;
    let sizer = KellySizer::new(db);

    let result = sizer
        .calculate_kelly("wallet_with_no_trades", Strategy::Shield, 30)
        .await;
    assert!(result.is_err(), "Expected error for wallet with no trades");
}

#[tokio::test]
async fn test_kelly_positive_edge() {
    // 60% win rate: 12 wins of +0.1 SOL and 8 losses of -0.05 SOL on
    // amount_sol = 1.0 → pnl percentages are 10% and 5% (not 100%).
    //
    // full_kelly = (p*avg_win − q*avg_loss) / (avg_win*avg_loss)
    //            = (0.6*0.1 − 0.4*0.05) / (0.1*0.05) = 0.04/0.005 = 8.0
    //            → hard-capped at 0.5 (50%).
    // Velocity: 20 trades in a clamped 1-day span → 1.25× → conservative =
    //            min(0.5*1.25*0.25, 0.5, 1.0) = 0.15625.
    let (db, _dir) = setup_test_db().await;

    let wallet = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
    let token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263";

    for i in 0..12u32 {
        insert_closed_trade(&db, &format!("win-{}", i), wallet, token, "1.0", "0.1").await;
    }
    for i in 0..8u32 {
        insert_closed_trade(&db, &format!("loss-{}", i), wallet, token, "1.0", "-0.05").await;
    }

    let sizer = KellySizer::new(db);
    let result = sizer
        .calculate_kelly(wallet, Strategy::Shield, 30)
        .await
        .unwrap();

    assert_eq!(result.win_rate, dec!(0.6), "win rate must be 12/20 = 0.6");
    assert_eq!(
        result.full_kelly,
        dec!(0.5),
        "full_kelly must hit the 0.5 cap"
    );
    assert_eq!(
        result.conservative_kelly,
        dec!(0.15625),
        "conservative = min(0.5 * 1.25 velocity * 0.25, full) = 0.15625"
    );
    assert!(
        result.conservative_kelly <= result.full_kelly,
        "conservative should be <= full kelly"
    );
}

#[tokio::test]
async fn test_kelly_negative_edge() {
    // More losses than wins → kelly fraction should be 0 (never go negative)
    let (db, _dir) = setup_test_db().await;

    let wallet = "9mNpQrAbCdEfGhIjKlMnOpQrStUvWxYz1234567890AA";
    let token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263";

    // 6 wins of 0.05 and 14 losses of 0.1 → negative edge (30% win rate, 20 trades total)
    for i in 0..6u32 {
        insert_closed_trade(&db, &format!("neg-win-{}", i), wallet, token, "1.0", "0.05").await;
    }
    for i in 0..14u32 {
        insert_closed_trade(
            &db,
            &format!("neg-loss-{}", i),
            wallet,
            token,
            "1.0",
            "-0.1",
        )
        .await;
    }

    let sizer = KellySizer::new(db);
    let result = sizer
        .calculate_kelly(wallet, Strategy::Shield, 30)
        .await
        .unwrap();

    // Negative edge: kelly is clamped to zero (implementation uses .max(Decimal::ZERO))
    assert_eq!(
        result.full_kelly,
        Decimal::ZERO,
        "full_kelly should be 0 when edge is negative"
    );
    assert_eq!(
        result.recommended_size_percent,
        Decimal::ZERO,
        "Position size must be 0 with negative edge"
    );
}

#[tokio::test]
async fn test_expected_profit_calculation() {
    // Test expected profit calculation with a positive edge wallet
    // 60% win rate, avg_win = 0.1 (10%), avg_loss = 0.05 (5%)
    // Expected return = (0.6 * 0.1) - (0.4 * 0.05) = 0.06 - 0.02 = 0.04 (4%)
    let (db, _dir) = setup_test_db().await;

    let wallet = "expected_profit_test_wallet";
    let token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263";

    for i in 0..12u32 {
        insert_closed_trade(&db, &format!("ep-win-{}", i), wallet, token, "1.0", "0.1").await;
    }
    for i in 0..8u32 {
        insert_closed_trade(
            &db,
            &format!("ep-loss-{}", i),
            wallet,
            token,
            "1.0",
            "-0.05",
        )
        .await;
    }

    let sizer = KellySizer::new(db);
    let kelly = sizer
        .calculate_kelly(wallet, Strategy::Shield, 30)
        .await
        .unwrap();

    // Test expected_return_pct calculation
    let expected_return = kelly.expected_return_pct();
    assert!(
        expected_return > Decimal::ZERO,
        "Expected return should be positive for profitable wallet"
    );

    // Expected return should be close to 4% (with some tolerance for rounding)
    let expected_approx = Decimal::from_str("0.04").unwrap();
    let tolerance = Decimal::from_str("0.005").unwrap(); // 0.5% tolerance
    assert!(
        (expected_return - expected_approx).abs() < tolerance,
        "Expected return should be approximately 4%, got {}",
        expected_return
    );

    // Test expected_profit_sol calculation
    let position_size = Decimal::from_str("1.0").unwrap();
    let expected_profit = kelly.expected_profit_sol(position_size);
    let profit_approx = Decimal::from_str("0.04").unwrap();
    assert!(
        (expected_profit - profit_approx).abs() < tolerance,
        "Expected profit should be approximately 0.04 SOL, got {}",
        expected_profit
    );

    // Test with different position sizes
    let small_position = Decimal::from_str("0.5").unwrap();
    let small_profit = kelly.expected_profit_sol(small_position);
    let small_profit_approx = Decimal::from_str("0.02").unwrap();
    assert!(
        (small_profit - small_profit_approx).abs() < tolerance,
        "Expected profit for 0.5 SOL should be approximately 0.02 SOL, got {}",
        small_profit
    );

    let large_position = Decimal::from_str("2.0").unwrap();
    let large_profit = kelly.expected_profit_sol(large_position);
    let large_profit_approx = Decimal::from_str("0.08").unwrap();
    assert!(
        (large_profit - large_profit_approx).abs() < tolerance,
        "Expected profit for 2.0 SOL should be approximately 0.08 SOL, got {}",
        large_profit
    );
}

#[tokio::test]
async fn test_expected_profit_negative_edge() {
    // Test expected profit calculation with a negative edge wallet
    let (db, _dir) = setup_test_db().await;

    let wallet = "negative_edge_wallet";
    let token = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263";

    for i in 0..6u32 {
        insert_closed_trade(
            &db,
            &format!("neg-ep-win-{}", i),
            wallet,
            token,
            "1.0",
            "0.05",
        )
        .await;
    }
    for i in 0..14u32 {
        insert_closed_trade(
            &db,
            &format!("neg-ep-loss-{}", i),
            wallet,
            token,
            "1.0",
            "-0.1",
        )
        .await;
    }

    let sizer = KellySizer::new(db);
    let kelly = sizer
        .calculate_kelly(wallet, Strategy::Shield, 30)
        .await
        .unwrap();

    let expected_return = kelly.expected_return_pct();
    assert!(
        expected_return < Decimal::ZERO,
        "Expected return should be negative for losing wallet"
    );

    let position_size = Decimal::from_str("1.0").unwrap();
    let expected_profit = kelly.expected_profit_sol(position_size);
    assert!(
        expected_profit < Decimal::ZERO,
        "Expected profit should be negative for losing wallet"
    );
}
