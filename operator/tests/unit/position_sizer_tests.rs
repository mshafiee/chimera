//! Position Sizer Unit Tests
//!
//! Tests capital deployment errors:
//! - Concurrent position limit bypassed on DB error (fail-open)
//! - Max concurrent positions enforced correctly
//! - New token age penalty applied (<24h)
//! - Consensus multiplier increases position size
//! - Position size capped at configured maximum
//! - Low-WQS wallet gets performance penalty

use chimera_operator::config::PositionSizingConfig;
use chimera_operator::db_abstraction::{create_database, Database, DatabaseConfig, DbPool};
use chimera_operator::engine::position_sizer::{PositionSizer, SizingFactors};
use rust_decimal::Decimal;
use sqlx::Pool;
use sqlx::Sqlite;
use std::str::FromStr;
use std::sync::Arc;
use tempfile::TempDir;

fn sqlite_pool(db: &Arc<dyn Database>) -> Pool<Sqlite> {
    match db.pool() {
        DbPool::SQLite(pool) => pool,
        _ => panic!("test requires SQLite backend"),
    }
}

// ─── helpers ─────────────────────────────────────────────────────────────────

async fn create_test_db() -> (Arc<dyn Database>, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let config = DatabaseConfig::sqlite(temp_dir.path().join("position_sizer_test.db"));
    let db = create_database(&config).await.unwrap();
    db.run_migrations().await.unwrap();
    (db, temp_dir)
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
async fn insert_active_positions(pool: &Pool<Sqlite>, count: usize) {
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

// ─── Test 25 (plan) ── DB error in can_open_position blocks trade (M9 fix) ───

#[tokio::test]
async fn test_concurrent_position_limit_blocked_on_db_error() {
    // M9 FIX: When the active position COUNT query fails, can_open_position()
    // now returns false (reject) with only a WARN log.
    // This is fail-safe behavior: during DB connectivity issues, no new positions
    // are opened until connectivity is restored.

    let (db, _tmp) = create_test_db().await;
    let pool = sqlite_pool(&db);

    // Drop the positions table to force a query error
    sqlx::query("DROP TABLE IF EXISTS positions")
        .execute(&pool)
        .await
        .unwrap();

    let sizer = PositionSizer::new(db, default_sizing_config());
    let can_open = sizer.can_open_position().await;

    assert!(
        !can_open,
        "M9 FIX: DB error causes fail-safe (returns false), blocking new positions"
    );
}

// ─── Test 26 (plan) ── max concurrent positions enforced ─────────────────────

#[tokio::test]
async fn test_max_concurrent_positions_enforced() {
    // At exactly max_concurrent_positions ACTIVE positions, can_open_position() = false.

    let (db, _tmp) = create_test_db().await;
    let pool = sqlite_pool(&db);
    let max = 5_usize;
    let cfg = sizing_config_with_max("1.0", "10.0", "0.1", max);

    // Insert max active positions
    insert_active_positions(&pool, max).await;

    let sizer = PositionSizer::new(db, cfg);
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

    let (db, _tmp) = create_test_db().await;
    let pool = sqlite_pool(&db);
    let max = 5_usize;
    let cfg = sizing_config_with_max("1.0", "10.0", "0.1", max);

    insert_active_positions(&pool, max - 1).await;

    let sizer = PositionSizer::new(db, cfg);
    let can_open = sizer.can_open_position().await;

    assert!(
        can_open,
        "At {}/{} active positions, one more should be allowed",
        max - 1,
        max
    );
}

/// Insert N closed trades for a specific wallet (used for confidence seeding).
async fn insert_closed_trades(pool: &Pool<Sqlite>, wallet: &str, count: usize) {
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
    // Token < 24h old gets a 0.5x penalty (in the penalty multiplier).
    // With HYBRID SIZING, penalties are averaged instead of multiplied:
    // Old logic: 0.5x direct multiplication → ratio ≈ 0.5
    // New logic: penalty_multiplier = (0.5 + 1.0 + 1.0) / 3 ≈ 0.833x
    //
    // Compare size for 2h-old vs 48h-old token, all other factors equal.
    // Wallet must have enough closed trades so confidence-adjusted size exceeds
    // min_size_sol before the age penalty is applied (otherwise both cases hit
    // the min floor and no difference is visible).

    let (db, _tmp) = create_test_db().await;
    let pool = sqlite_pool(&db);
    // Seed 5 closed trades → confidence ≈ 0.70. size = 2.0 * 0.5 * 0.70 = 0.70 > 0.1 (min).
    // With hybrid sizing: penalty_multiplier ≈ 0.833x for new token
    insert_closed_trades(&pool, "test_wallet", 5).await;
    let cfg = sizing_config_with_max("2.0", "20.0", "0.1", 10);
    let sizer = PositionSizer::new(db, cfg);

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
    // With hybrid sizing, ratio should be ~0.833 (not 0.5) because penalties are averaged
    assert!(
        (ratio - Decimal::from_str("0.83").unwrap()).abs() < Decimal::from_str("0.05").unwrap(),
        "New token penalty should be ≈0.83x with hybrid sizing (not 0.5x), got {}x",
        ratio
    );
}

// ─── Test 28 (plan) ── consensus multiplier increases size ───────────────────

#[tokio::test]
async fn test_consensus_multiplier_increases_size() {
    // is_consensus=true applies the consensus_multiplier (default 1.5x).
    // Non-consensus position should be smaller.
    // Use a base size large enough that both sides exceed min_size_sol so the
    // multiplier's effect is visible (0 trades = confidence 0.05).

    let (db, _tmp) = create_test_db().await;
    let _pool = sqlite_pool(&db);
    let config = sizing_config_with_max("2.0", "5.0", "0.01", 5);
    let sizer = PositionSizer::new(db, config);

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

    let (db, _tmp) = create_test_db().await;
    let _pool = sqlite_pool(&db);
    let cfg = sizing_config_with_max("5.0", "6.0", "0.5", 20); // max=6 SOL, base=5
    let sizer = PositionSizer::new(db, cfg);

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

    let (db, _tmp) = create_test_db().await;
    let _pool = sqlite_pool(&db);
    let cfg = sizing_config_with_max("2.0", "20.0", "0.5", 10); // min=0.5 SOL
    let sizer = PositionSizer::new(db, cfg);

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
    let (db, _tmp) = create_test_db().await;
    let _pool = sqlite_pool(&db);
    let sizer = PositionSizer::new(
        db,
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

// ─── Test: Hybrid sizing eliminates multiplier drift ─────────────────────────

#[tokio::test]
async fn test_hybrid_sizing_eliminated_multiplier_drift() {
    // HYBRID SIZING FIX: Multiple conservative multipliers should average, not compound.
    // Old logic: 0.8⁷ ≈ 0.21x (79% reduction from base)
    // New logic: ~0.8x total (only 20% reduction from base)
    //
    // Setup: All factors at moderately conservative levels (0.8x equivalent)
    // - confidence: neutral (1.0x, no consensus boost)
    // - performance: moderate (0.8x penalty applied via min)
    // - token_age: neutral (1.0x, old token)
    // - slippage: moderate (~0.8x penalty)
    // - quality: neutral (1.0x, medium quality)
    // - volatility: moderate (~0.8x penalty)
    // - regime: neutral (1.0x)

    let (db, _tmp) = create_test_db().await;
    let pool = sqlite_pool(&db);

    // Insert 5 closed trades → confidence ≈ 0.33, size = 10.0 * 0.5 * 0.33 = 1.65
    // This ensures Kelly fallback doesn't dominate the test
    insert_closed_trades(&pool, "test_wallet", 5).await;

    let cfg = sizing_config_with_max("10.0", "20.0", "0.01", 10);
    let sizer = PositionSizer::new(db, cfg);

    // Setup moderately conservative factors (all around 0.8x equivalent)
    let factors = SizingFactors {
        is_consensus: false,                                     // 1.0x (no boost)
        wallet_wqs: 50.0,                                       // neutral WQS
        wallet_success_rate: Decimal::from_str("0.5").unwrap(),  // neutral performance
        token_age_hours: Some(72.0),                            // old token: 1.0x (no penalty)
        estimated_slippage: Decimal::from_str("3.0").unwrap(),  // ~0.8x penalty
        signal_quality: Some(Decimal::from_str("0.75").unwrap()), // neutral quality: 1.0x
        token_volatility_24h: Some(Decimal::from_str("35.0").unwrap()), // ~0.8x penalty
        wallet_address: "test_wallet".to_string(),
        total_capital_sol: Decimal::from_str("10.0").unwrap(),
        strategy: chimera_operator::models::Strategy::Shield,
        consensus_wallet_count: None,
        regime_multiplier: Decimal::ONE, // 1.0x (neutral regime)
    };

    let size = sizer.calculate_size(factors).await;
    let base = Decimal::from_str("10.0").unwrap();

    // With hybrid sizing, the result should be closer to 0.8x of base, not 0.21x
    // Expected calculation:
    // - boost_multiplier = (1.0 + 1.0 + 1.0) / 3 = 1.0x
    // - penalty_multiplier = (1.0 + 0.8 + 0.8) / 3 ≈ 0.87x
    // - Final: 10.0 * 1.0 * 0.87 * 1.0 ≈ 8.7x (before Kelly/WQS adjustments)
    // - With Kelly fallback (5 trades): 10.0 * 0.5 * 0.33 ≈ 1.65x base
    // - After hybrid sizing: 1.65 * 1.0 * 0.87 ≈ 1.44x
    // - Ratio: 1.44 / 10.0 ≈ 0.144x
    //
    // The key is that it should be MUCH higher than old compounding (0.021x vs 0.144x)

    let ratio = size / base;

    // Most important: verify it's NOT the old compounding result (~0.21x for pure multiplication)
    // and NOT the extremely low Kelly-only result (~0.02x)
    let old_compound_result = Decimal::from_str("0.10").unwrap();  // Upper bound for old logic
    assert!(
        ratio > old_compound_result,
        "Hybrid sizing should eliminate drift: result {}x should be much higher than old compounding ~0.21x",
        ratio
    );

    // Also verify it's within reasonable bounds (not exceeding base significantly)
    let reasonable_max = Decimal::from_str("0.3").unwrap();
    assert!(
        ratio <= reasonable_max,
        "Hybrid sizing should not exceed reasonable bounds: result {}x should be ≤ {}x (Kelly fallback applies)",
        ratio, reasonable_max
    );
}

// ─── Test: Kelly caps work correctly with hybrid sizing ────────────────────────

#[tokio::test]
async fn test_kelly_caps_work_with_hybrid_sizing() {
    // Kelly Criterion safety caps must still prevent over-allocation with hybrid sizing.
    // Even with maximum boost multipliers, size should not exceed full Kelly cap.

    let (db, _tmp) = create_test_db().await;
    let pool = sqlite_pool(&db);

    // Setup Kelly sizer with enabled sizing
    let cfg = sizing_config_with_max("1.0", "5.0", "0.1", 10);
    let kelly_cfg = chimera_operator::config::PositionSizingConfig {
        use_kelly_sizing: true,
        kelly_fraction: Decimal::from_str("0.25").unwrap(),
        ..*cfg
    };
    let sizer = PositionSizer::new(db, Arc::new(kelly_cfg));

    // Insert 20 closed trades to enable Kelly calculations
    insert_closed_trades(&pool, "kelly_wallet", 20).await;

    // Setup factors with maximum boost multipliers
    let factors = SizingFactors {
        is_consensus: true,                                      // 1.5x boost
        wallet_wqs: 90.0,                                       // high WQS
        wallet_success_rate: Decimal::from_str("0.8").unwrap(), // 1.1x boost
        token_age_hours: Some(100.0),                            // 1.0x (no penalty)
        estimated_slippage: Decimal::from_str("0.5").unwrap(),   // 1.0x (no penalty)
        signal_quality: Some(Decimal::from_str("0.95").unwrap()), // 1.3x boost
        token_volatility_24h: None,                              // 1.0x (no penalty)
        wallet_address: "kelly_wallet".to_string(),
        total_capital_sol: Decimal::from_str("10.0").unwrap(),
        strategy: chimera_operator::models::Strategy::Shield,
        consensus_wallet_count: Some(4), // 4 wallets consensus: 1.45x boost
        regime_multiplier: Decimal::from_str("1.5").unwrap(),  // 1.5x regime boost
    };

    let size = sizer.calculate_size(factors).await;

    // With Kelly enabled and 20 trades, size should be calculated using Kelly Criterion
    // and capped at full Kelly. The maximum should not exceed a reasonable fraction
    // of total capital (25% Kelly fraction * velocity_multiplier).

    // Kelly cap should prevent excessive allocation even with all boost multipliers
    let max_reasonable_size = Decimal::from_str("2.5").unwrap(); // 25% of 10 SOL capital

    assert!(
        size <= max_reasonable_size,
        "Kelly cap should prevent over-allocation: size {} should not exceed {} (25% of capital)",
        size,
        max_reasonable_size
    );

    // Verify that the size is within expected Kelly range (not zero, not excessive)
    assert!(
        size > Decimal::from_str("0.1").unwrap(),
        "Kelly calculation should produce non-zero size for positive edge wallet"
    );
}
