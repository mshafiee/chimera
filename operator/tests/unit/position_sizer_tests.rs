//! Position Sizer Unit Tests
//!
//! Tests capital deployment errors:
//! - Concurrent position limit bypassed on DB error (fail-open)
//! - Max concurrent positions enforced correctly
//! - New token age penalty applied (<24h)
//! - Consensus multiplier increases position size
//! - Position size capped at configured maximum
//! - Low-WQS wallet gets performance penalty

use chimera_operator::config::{DatabaseConfig, PositionSizingConfig};
use chimera_operator::db::{init_pool, run_migrations};
use chimera_operator::engine::position_sizer::{PositionSizer, SizingFactors};
use rust_decimal::Decimal;
use std::str::FromStr;
use std::sync::Arc;
use tempfile::TempDir;

// ─── helpers ─────────────────────────────────────────────────────────────────

async fn create_test_db() -> (chimera_operator::db::DbPool, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let db_config = DatabaseConfig {
        path: temp_dir.path().join("position_sizer_test.db"),
        max_connections: 5,
    };
    let pool = init_pool(&db_config).await.unwrap();
    run_migrations(&pool).await.unwrap();
    (pool, temp_dir)
}

fn default_sizing_config() -> Arc<PositionSizingConfig> {
    Arc::new(PositionSizingConfig::default())
}

fn sizing_config_with_max(
    base: &str,
    max: &str,
    min: &str,
    max_concurrent: usize,
) -> Arc<PositionSizingConfig> {
    Arc::new(PositionSizingConfig {
        base_size_sol: Decimal::from_str(base).unwrap(),
        max_size_sol: Decimal::from_str(max).unwrap(),
        min_size_sol: Decimal::from_str(min).unwrap(),
        max_concurrent_positions: max_concurrent,
        ..PositionSizingConfig::default()
    })
}

fn neutral_factors() -> SizingFactors {
    SizingFactors {
        is_consensus: false,
        wallet_wqs: 50.0,
        wallet_success_rate: Decimal::from_str("0.5").unwrap(),
        token_age_hours: Some(72.0), // >24h: no penalty
        estimated_slippage: Decimal::from_str("1.0").unwrap(), // <2%: no penalty
        signal_quality: None,
        token_volatility_24h: None,
        wallet_address: "test_wallet".to_string(),
        total_capital_sol: Decimal::from_str("10.0").unwrap(),
        strategy: chimera_operator::models::Strategy::Shield,
        consensus_wallet_count: None,
        regime_multiplier: Decimal::ONE,
    }
}

/// Insert N active positions into DB.
async fn insert_active_positions(pool: &chimera_operator::db::DbPool, count: usize) {
    for i in 0..count {
        let uuid = format!("uuid-pos-{}", i);
        sqlx::query(
            "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status) \
             VALUES (?, 'wallet_x', 'token_x', 'SHIELD', 'BUY', 1.0, 'ACTIVE')"
        )
        .bind(&uuid)
        .execute(pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO positions (trade_uuid, wallet_address, token_address, strategy, \
             entry_amount_sol, entry_price, entry_tx_signature, state) \
             VALUES (?, 'wallet_x', 'token_x', 'SHIELD', 1.0, 1.0, 'sig', 'ACTIVE')",
        )
        .bind(&uuid)
        .execute(pool)
        .await
        .unwrap();
    }
}

// ─── Test 25 (plan) ── DB error in can_open_position allows trade ─────────────

#[tokio::test]
async fn test_concurrent_position_limit_bypassed_on_db_error() {
    // BUG DOCUMENTED: When the active position COUNT query fails, can_open_position()
    // returns true (allow) with only a WARN log.
    // Risk: during DB connectivity issues, unlimited concurrent positions can be opened.

    let (pool, _tmp) = create_test_db().await;

    // Drop the positions table to force a query error
    sqlx::query("DROP TABLE IF EXISTS positions")
        .execute(&pool)
        .await
        .unwrap();

    let sizer = PositionSizer::new(pool, default_sizing_config());
    let can_open = sizer.can_open_position().await;

    assert!(
        can_open,
        "BUG DOCUMENTED: DB error causes fail-open (returns true), bypassing position limit"
    );
}

// ─── Test 26 (plan) ── max concurrent positions enforced ─────────────────────

#[tokio::test]
async fn test_max_concurrent_positions_enforced() {
    // At exactly max_concurrent_positions ACTIVE positions, can_open_position() = false.

    let (pool, _tmp) = create_test_db().await;
    let max = 5_usize;
    let cfg = sizing_config_with_max("1.0", "10.0", "0.1", max);

    // Insert max active positions
    insert_active_positions(&pool, max).await;

    let sizer = PositionSizer::new(pool, cfg);
    let can_open = sizer.can_open_position().await;

    assert!(
        !can_open,
        "At {} active positions (= max), can_open_position must return false",
        max
    );
}

#[tokio::test]
async fn test_one_below_max_allows_new_position() {
    // At max-1 active positions, one more should be allowed.

    let (pool, _tmp) = create_test_db().await;
    let max = 5_usize;
    let cfg = sizing_config_with_max("1.0", "10.0", "0.1", max);

    insert_active_positions(&pool, max - 1).await;

    let sizer = PositionSizer::new(pool, cfg);
    let can_open = sizer.can_open_position().await;

    assert!(
        can_open,
        "At {}/{} active positions, one more should be allowed",
        max - 1,
        max
    );
}

/// Insert N closed trades for a specific wallet (used for confidence seeding).
async fn insert_closed_trades(pool: &chimera_operator::db::DbPool, wallet: &str, count: usize) {
    for i in 0..count {
        let uuid = format!("closed-{}-{}", wallet, i);
        sqlx::query(
            "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status) \
             VALUES (?, ?, 'token_age_test', 'SHIELD', 'BUY', 1.0, 'CLOSED')"
        )
        .bind(&uuid)
        .bind(wallet)
        .execute(pool)
        .await
        .unwrap();
    }
}

// ─── Test 29 (plan) ── new token age penalty ─────────────────────────────────

#[tokio::test]
async fn test_new_token_age_penalty_halves_size() {
    // Token < 24h old gets a 0.5x multiplier.
    // Compare size for 2h-old vs 48h-old token, all other factors equal.
    // Wallet must have enough closed trades so confidence-adjusted size exceeds
    // min_size_sol before the age penalty is applied (otherwise both cases hit
    // the min floor and no difference is visible).

    let (pool, _tmp) = create_test_db().await;
    // Seed 5 closed trades → confidence ≈ 0.70. size = 2.0 * 0.5 * 0.70 = 0.70 > 0.1 (min).
    // Age penalty: 0.70 * 0.5 = 0.35 vs 0.70 — ratio ≈ 0.5. ✓
    insert_closed_trades(&pool, "test_wallet", 5).await;
    let cfg = sizing_config_with_max("2.0", "20.0", "0.1", 10);
    let sizer = PositionSizer::new(pool, cfg);

    let mut new_token = neutral_factors();
    new_token.token_age_hours = Some(2.0); // new: < 24h

    let mut old_token = neutral_factors();
    old_token.token_age_hours = Some(48.0); // established: > 24h

    let size_new = sizer.calculate_size(new_token).await;
    let size_old = sizer.calculate_size(old_token).await;

    assert!(
        size_new < size_old,
        "New token (2h) must get smaller position than established token (48h): {} vs {}",
        size_new,
        size_old
    );

    let ratio = size_new / size_old;
    assert!(
        (ratio - Decimal::from_str("0.5").unwrap()).abs() < Decimal::from_str("0.01").unwrap(),
        "New token penalty should halve the size (ratio ≈ 0.5), got {}",
        ratio
    );
}

// ─── Test 28 (plan) ── consensus multiplier increases size ───────────────────

#[tokio::test]
async fn test_consensus_multiplier_increases_size() {
    // is_consensus=true applies the consensus_multiplier (default 1.5x).
    // Non-consensus position should be smaller.

    let (pool, _tmp) = create_test_db().await;
    let sizer = PositionSizer::new(pool, default_sizing_config());

    let mut consensus = neutral_factors();
    consensus.is_consensus = true;

    let mut non_consensus = neutral_factors();
    non_consensus.is_consensus = false;

    let size_with = sizer.calculate_size(consensus).await;
    let size_without = sizer.calculate_size(non_consensus).await;

    assert!(
        size_with > size_without,
        "Consensus signal must produce larger position: {} vs {}",
        size_with,
        size_without
    );
}

// ─── Test 30 (plan) ── size capped at maximum ────────────────────────────────

#[tokio::test]
async fn test_position_size_capped_at_max() {
    // Even with maximum multipliers (consensus + high WQS + high quality), size ≤ max.

    let (pool, _tmp) = create_test_db().await;
    let cfg = sizing_config_with_max("5.0", "6.0", "0.5", 20); // max=6 SOL, base=5
    let sizer = PositionSizer::new(pool, cfg);

    let factors = SizingFactors {
        is_consensus: true,                                       // 1.5x
        wallet_wqs: 90.0,                                         // 1.2x
        wallet_success_rate: Decimal::from_str("0.8").unwrap(),   // 1.1x
        token_age_hours: Some(100.0),                             // no penalty
        estimated_slippage: Decimal::from_str("0.5").unwrap(),    // no penalty
        signal_quality: Some(Decimal::from_str("0.95").unwrap()), // 1.3x
        token_volatility_24h: None,
        wallet_address: "test_wallet".to_string(),
        total_capital_sol: Decimal::from_str("10.0").unwrap(),
        strategy: chimera_operator::models::Strategy::Shield,
        consensus_wallet_count: None,
        regime_multiplier: Decimal::ONE,
    };

    let size = sizer.calculate_size(factors).await;
    let max = Decimal::from_str("6.0").unwrap();

    assert!(
        size <= max,
        "Position size must not exceed max_size_sol=6.0, got {}",
        size
    );
}

// ─── Test: min size floor ────────────────────────────────────────────────────

#[tokio::test]
async fn test_position_size_floor_at_minimum() {
    // All penalties applied: new token, high slippage, low WQS, low quality.
    // Size must not go below min_size_sol.

    let (pool, _tmp) = create_test_db().await;
    let cfg = sizing_config_with_max("2.0", "20.0", "0.5", 10); // min=0.5 SOL
    let sizer = PositionSizer::new(pool, cfg);

    let factors = SizingFactors {
        is_consensus: false,
        wallet_wqs: 10.0,                                        // low: no WQS bonus
        wallet_success_rate: Decimal::from_str("0.2").unwrap(),  // 0.8x penalty
        token_age_hours: Some(1.0),                              // 0.5x penalty
        estimated_slippage: Decimal::from_str("5.0").unwrap(),   // 0.7x penalty
        signal_quality: Some(Decimal::from_str("0.5").unwrap()), // 0.7x penalty
        token_volatility_24h: Some(Decimal::from_str("50.0").unwrap()), // additional reduction
        wallet_address: "test_wallet".to_string(),
        total_capital_sol: Decimal::from_str("10.0").unwrap(),
        strategy: chimera_operator::models::Strategy::Spear,
        consensus_wallet_count: None,
        regime_multiplier: Decimal::ONE,
    };

    let size = sizer.calculate_size(factors).await;
    let min = Decimal::from_str("0.5").unwrap();

    assert!(
        size >= min,
        "Position size must not go below min_size_sol=0.5, got {}",
        size
    );
}

// ─── Test: WQS produces proportionally larger positions ──────────────────────

#[tokio::test]
async fn test_high_wqs_multiplier_applied() {
    // WQS scales position size continuously via wqs_factor = WQS/100.
    // WQS=85 vs WQS=50: ratio should be ≈ 85/50 = 1.7 (no discrete cliff at 80).
    //
    // Use a large base_size so the WQS factor pushes both values above min_size_sol
    // (with 0 closed trades, confidence=0.05: 10.0 * 0.85 * 0.05 = 0.425 vs 0.25).
    let (pool, _tmp) = create_test_db().await;
    let sizer = PositionSizer::new(
        pool,
        Arc::new(chimera_operator::config::PositionSizingConfig {
            base_size_sol: Decimal::from_str("10.0").unwrap(),
            min_size_sol: Decimal::from_str("0.01").unwrap(),
            ..chimera_operator::config::PositionSizingConfig::default()
        }),
    );

    let mut high_wqs = neutral_factors();
    high_wqs.wallet_wqs = 85.0;

    let mut base_wqs = neutral_factors();
    base_wqs.wallet_wqs = 50.0;

    let size_high = sizer.calculate_size(high_wqs).await;
    let size_base = sizer.calculate_size(base_wqs).await;

    assert!(
        size_high > size_base,
        "High WQS must produce larger position"
    );
    let ratio = size_high / size_base;
    assert!(
        (ratio - Decimal::from_str("1.7").unwrap()).abs() < Decimal::from_str("0.01").unwrap(),
        "High WQS ratio should be ≈1.7 (85/50), got {}",
        ratio
    );
}
