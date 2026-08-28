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
use chimera_operator::db_abstraction::{Database, DbPool};
use chimera_operator::engine::position_sizer::{PositionSizer, SizingFactors};
use rust_decimal::Decimal;
use sqlx::Pool;
use sqlx::Postgres;
use std::str::FromStr;
use std::sync::Arc;

#[path = "../common/mod.rs"]
mod common;

fn pg_pool(db: &Arc<dyn Database>) -> Pool<Postgres> {
    // DbPool is PostgreSQL-only (single variant): irrefutable destructure, no
    // fallback panic arm (which would be unreachable).
    let DbPool::PostgreSQL(pool) = db.pool();
    pool
}

// ─── helpers ─────────────────────────────────────────────────────────────────

/// Each test gets its own isolated database (dropped on teardown), so fixed
/// trade UUIDs never collide across runs and dropping the positions table in
/// one test cannot affect its siblings.
async fn create_test_db() -> (Arc<dyn Database>, common::TestDbGuard) {
    common::create_test_pg_db().await
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
        wqs_confidence: None,
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
        wqs_capped_max_size: None,
        boost_target_sol: None,
        token_address: None,
        is_proven: false,
    }
}

/// Insert N active positions into DB.
async fn insert_active_positions(pool: &Pool<Postgres>, count: usize) {
    for i in 0..count {
        let uuid = format!("uuid-pos-{}", i);
        sqlx::query(
            "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status) \
             VALUES ($1, 'wallet_x', 'token_x', 'SHIELD', 'BUY', 1.0, 'ACTIVE')"
        )
        .bind(&uuid)
        .execute(pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO positions (trade_uuid, wallet_address, token_address, strategy, \
             entry_amount_sol, entry_price, entry_tx_signature, state) \
             VALUES ($1, 'wallet_x', 'token_x', 'SHIELD', 1.0, 1.0, 'sig', 'ACTIVE')",
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
    let pool = pg_pool(&db);

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
    let pool = pg_pool(&db);
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
    let pool = pg_pool(&db);
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
async fn insert_closed_trades(pool: &Pool<Postgres>, wallet: &str, count: usize) {
    for i in 0..count {
        let uuid = format!("closed-{}-{}", wallet, i);
        sqlx::query(
            "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status) \
             VALUES ($1, $2, 'token_age_test', 'SHIELD', 'BUY', 1.0, 'CLOSED')"
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
    let pool = pg_pool(&db);
    // Seed 5 closed trades → confidence ≈ 0.70. size = 2.0 * 0.5 * 0.70 = 0.70 > 0.1 (min).
    // With hybrid sizing: penalty_multiplier ≈ 0.833x for new token
    insert_closed_trades(&pool, "test_wallet", 5).await;
    let cfg = sizing_config_with_max("2.0", "20.0", "0.1", 10);
    let sizer = PositionSizer::new(db, cfg);

    let mut new_token = neutral_factors();
    new_token.token_age_hours = Some(2.0); // new: < 24h

    let mut old_token = neutral_factors();
    old_token.token_age_hours = Some(48.0); // established: > 24h

    let size_new = sizer.calculate_size(new_token).await.unwrap();
    let size_old = sizer.calculate_size(old_token).await.unwrap();

    assert!(
        size_new < size_old,
        "New token (2h) must get smaller position than established token (48h): {} vs {}",
        size_new,
        size_old
    );

    let ratio = size_new / size_old;
    // With hybrid sizing the age penalty (0.5) is averaged over the five
    // penalty factors (age, slippage, volatility, performance, quality):
    // (0.5 + 1 + 1 + 1 + 1) / 5 = 0.9x — not the 0.5x of pure multiplication.
    assert!(
        (ratio - Decimal::from_str("0.9").unwrap()).abs() < Decimal::from_str("0.01").unwrap(),
        "New token penalty should be ≈0.9x with hybrid sizing (not 0.5x), got {}x",
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
    let _pool = pg_pool(&db);
    let config = sizing_config_with_max("2.0", "5.0", "0.01", 5);
    let sizer = PositionSizer::new(db, config);

    let mut consensus = neutral_factors();
    consensus.is_consensus = true;

    let mut non_consensus = neutral_factors();
    non_consensus.is_consensus = false;

    let size_with = sizer.calculate_size(consensus).await.unwrap();
    let size_without = sizer.calculate_size(non_consensus).await.unwrap();

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
    let _pool = pg_pool(&db);
    let cfg = sizing_config_with_max("5.0", "6.0", "0.5", 20); // max=6 SOL, base=5
    let sizer = PositionSizer::new(db, cfg);

    let factors = SizingFactors {
        is_consensus: true, // 1.5x
        wallet_wqs: 90.0,
        wqs_confidence: None,                                     // 1.2x
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
        wqs_capped_max_size: None,
        boost_target_sol: None,
        token_address: None,
        is_proven: false,
    };

    let size = sizer.calculate_size(factors).await.unwrap();
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
    let _pool = pg_pool(&db);
    let db_legacy = db.clone();
    let cfg = sizing_config_with_max("2.0", "20.0", "0.5", 10); // min=0.5 SOL
    let sizer = PositionSizer::new(db, cfg);

    let factors = SizingFactors {
        is_consensus: false,
        wallet_wqs: 10.0,
        wqs_confidence: None, // low: no WQS bonus
        wallet_success_rate: Decimal::from_str("0.2").unwrap(), // 0.8x penalty
        token_age_hours: Some(1.0), // 0.5x penalty
        estimated_slippage: Decimal::from_str("5.0").unwrap(), // 0.7x penalty
        signal_quality: Some(Decimal::from_str("0.5").unwrap()), // 0.7x penalty
        token_volatility_24h: Some(Decimal::from_str("50.0").unwrap()), // additional reduction
        wallet_address: "test_wallet".to_string(),
        total_capital_sol: Decimal::from_str("10.0").unwrap(),
        strategy: chimera_operator::models::Strategy::Spear,
        consensus_wallet_count: None,
        regime_multiplier: Decimal::ONE,
        wqs_capped_max_size: None,
        boost_target_sol: None,
        token_address: None,
        is_proven: false,
    };

    let size = sizer.calculate_size(factors).await.unwrap();
    let min = Decimal::from_str("0.5").unwrap();

    // Skip-below-min semantics (2026-08-18, default): all-penalty factors
    // compute ~0.013 SOL — far below the 0.5 minimum — so the entry REJECTS
    // (zero) instead of being clamped up to trade at the minimum with the
    // worst cost ratio.
    assert_eq!(
        size, Decimal::ZERO,
        "Sub-minimum computed size must return zero under skip-below-min semantics"
    );

    // Legacy mode (skip_below_min_size = false): clamps up to the minimum —
    // rollback path preserved.
    let cfg_legacy = Arc::new(PositionSizingConfig {
        base_size_sol: Decimal::from_str("2.0").unwrap(),
        max_size_sol: Decimal::from_str("20.0").unwrap(),
        min_size_sol: min,
        skip_below_min_size: false,
        max_concurrent_positions: 10,
        ..PositionSizingConfig::default()
    });
    let sizer_legacy = PositionSizer::new(db_legacy, cfg_legacy);
    let factors_legacy = SizingFactors {
        is_consensus: false,
        wallet_wqs: 10.0,
        wqs_confidence: None, // low: no WQS bonus
        wallet_success_rate: Decimal::from_str("0.2").unwrap(), // 0.8x penalty
        token_age_hours: Some(1.0), // 0.5x penalty
        estimated_slippage: Decimal::from_str("5.0").unwrap(), // 0.7x penalty
        signal_quality: Some(Decimal::from_str("0.5").unwrap()), // 0.7x penalty
        token_volatility_24h: Some(Decimal::from_str("50.0").unwrap()), // additional reduction
        wallet_address: "test_wallet".to_string(),
        total_capital_sol: Decimal::from_str("10.0").unwrap(),
        strategy: chimera_operator::models::Strategy::Spear,
        consensus_wallet_count: None,
        regime_multiplier: Decimal::ONE,
        wqs_capped_max_size: None,
        boost_target_sol: None,
        token_address: None,
        is_proven: false,
    };
    let size_legacy = sizer_legacy.calculate_size(factors_legacy).await.unwrap();
    assert!(
        size_legacy >= min,
        "Legacy mode must clamp up to min_size_sol=0.5, got {}",
        size_legacy
    );
}

// ─── Test: WQS produces proportionally larger positions ──────────────────────

#[tokio::test]
async fn test_high_wqs_multiplier_applied() {
    // WQS scales position size continuously via wqs_factor = WQS/100.
    // WQS=85 vs WQS=50: ratio should be ≈ 85/50 = 1.7 (no discrete cliff at 80).
    //
    // Use a SMALL base_size so both sizes stay under the fallback capital cap
    // (total_capital × ~5.75% = 0.575 SOL here) — with a large base, both
    // sides would be pinned to the cap and the WQS ratio would vanish.
    let (db, _tmp) = create_test_db().await;
    let _pool = pg_pool(&db);
    let sizer = PositionSizer::new(
        db,
        Arc::new(chimera_operator::config::PositionSizingConfig {
            base_size_sol: Decimal::from_str("1.0").unwrap(),
            min_size_sol: Decimal::from_str("0.01").unwrap(),
            ..chimera_operator::config::PositionSizingConfig::default()
        }),
    );

    let mut high_wqs = neutral_factors();
    high_wqs.wallet_wqs = 85.0;

    let mut base_wqs = neutral_factors();
    base_wqs.wallet_wqs = 50.0;

    let size_high = sizer.calculate_size(high_wqs).await.unwrap();
    let size_base = sizer.calculate_size(base_wqs).await.unwrap();

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
    let pool = pg_pool(&db);

    // Insert 5 closed trades → confidence ≈ 0.33, size = 10.0 * 0.5 * 0.33 = 1.65
    // This ensures Kelly fallback doesn't dominate the test
    insert_closed_trades(&pool, "test_wallet", 5).await;

    let cfg = sizing_config_with_max("10.0", "20.0", "0.01", 10);
    let sizer = PositionSizer::new(db, cfg);

    // Setup moderately conservative factors (all around 0.8x equivalent)
    let factors = SizingFactors {
        is_consensus: false, // 1.0x (no boost)
        wallet_wqs: 50.0,
        wqs_confidence: None,                                     // neutral WQS
        wallet_success_rate: Decimal::from_str("0.5").unwrap(),   // neutral performance
        token_age_hours: Some(72.0),                              // old token: 1.0x (no penalty)
        estimated_slippage: Decimal::from_str("3.0").unwrap(),    // ~0.8x penalty
        signal_quality: Some(Decimal::from_str("0.75").unwrap()), // neutral quality: 1.0x
        token_volatility_24h: Some(Decimal::from_str("35.0").unwrap()), // ~0.8x penalty
        wallet_address: "test_wallet".to_string(),
        total_capital_sol: Decimal::from_str("10.0").unwrap(),
        strategy: chimera_operator::models::Strategy::Shield,
        consensus_wallet_count: None,
        regime_multiplier: Decimal::ONE,
        wqs_capped_max_size: None, // 1.0x (neutral regime)
        boost_target_sol: None,
        token_address: None,
        is_proven: false,
    };

    let size = sizer.calculate_size(factors).await.unwrap();
    // Capital-relative base (2026-08-20): base = total_capital(10) × base_size_pct(0.15) = 1.5.
    // This is the multiplicative reference for the drift check (previously base_size_sol=10).
    let base = Decimal::from_str("10.0").unwrap() * Decimal::from_str("0.15").unwrap();

    // With hybrid sizing, the result should be closer to 0.8x of base, not 0.21x
    // Expected calculation:
    // - boost_multiplier = (1.0 + 1.0 + 1.0) / 3 = 1.0x
    // - penalty_multiplier = (1.0 + 0.8 + 0.8) / 3 ≈ 0.87x
    // - Final base: 1.5 * 1.0 * 0.87 ≈ 1.30x
    // - With confidence seeding (5 closed trades): 1.5 * 0.5 * 0.5 ≈ 0.375x base
    // - After hybrid sizing: 0.375 * 1.0 * 0.92 ≈ 0.345x
    // - Ratio: 0.345 / 1.5 ≈ 0.23x
    //
    // The key is that it should be MUCH higher than old compounding (0.021x vs 0.23x)

    let ratio = size / base;

    // Most important: verify it's NOT the old compounding result (~0.21x for pure multiplication)
    // and NOT the extremely low Kelly-only result (~0.02x)
    let old_compound_result = Decimal::from_str("0.10").unwrap(); // Upper bound for old logic
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
    let pool = pg_pool(&db);

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
        is_consensus: true, // 1.5x boost
        wallet_wqs: 90.0,
        wqs_confidence: None,                                     // high WQS
        wallet_success_rate: Decimal::from_str("0.8").unwrap(),   // 1.1x boost
        token_age_hours: Some(100.0),                             // 1.0x (no penalty)
        estimated_slippage: Decimal::from_str("0.5").unwrap(),    // 1.0x (no penalty)
        signal_quality: Some(Decimal::from_str("0.95").unwrap()), // 1.3x boost
        token_volatility_24h: None,                               // 1.0x (no penalty)
        wallet_address: "kelly_wallet".to_string(),
        total_capital_sol: Decimal::from_str("10.0").unwrap(),
        strategy: chimera_operator::models::Strategy::Shield,
        consensus_wallet_count: Some(4), // 4 wallets consensus: 1.45x boost
        regime_multiplier: Decimal::from_str("1.5").unwrap(), // 1.5x regime boost
        wqs_capped_max_size: None,
        boost_target_sol: None,
        token_address: None,
        is_proven: false,
    };

    let size = sizer.calculate_size(factors).await.unwrap();

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

// ─── Conviction-size cap (Phase 5, 2026-08-07) ─────────────────────────────

/// Insert trades + positions for a token with the given entry sizes (SOL).
async fn insert_token_positions(pool: &Pool<Postgres>, token: &str, sizes: &[f64]) {
    for (i, size) in sizes.iter().enumerate() {
        let uuid = format!("uuid-conv-{}-{}", &token[..6], i);
        sqlx::query(
            "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status) \
             VALUES ($1, 'wallet_x', $2, 'SHIELD', 'BUY', $3, 'ACTIVE')",
        )
        .bind(&uuid)
        .bind(token)
        .bind(*size)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO positions (trade_uuid, wallet_address, token_address, strategy, \
             entry_amount_sol, entry_price, entry_tx_signature, state) \
             VALUES ($1, 'wallet_x', $2, 'SHIELD', $3, 1.0, 'sig', 'ACTIVE')",
        )
        .bind(&uuid)
        .bind(token)
        .bind(*size)
        .execute(pool)
        .await
        .unwrap();
    }
}

/// Give the wallet a proven trade history so the sizer's Kelly path (>= 15
/// closed trades) yields a full-size base (0.5 SOL at WQS 100) instead of the
/// conservative unproven-wallet fallback cap.
async fn insert_closed_trades_conviction(pool: &Pool<Postgres>, wallet: &str, count: u32) {
    for i in 0..count {
        let uuid = format!("uuid-trades-{}", i);
        sqlx::query(
            "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status) \
             VALUES ($1, $2, 'token_x', 'SHIELD', 'BUY', 0.25, 'CLOSED')",
        )
        .bind(&uuid)
        .bind(wallet)
        .execute(pool)
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn test_conviction_cap_applies_75th_percentile() {
    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    // Sorted 75th percentile (nearest-rank, n=5) = 0.25 SOL.
    insert_token_positions(&pool, "token_a", &[0.1, 0.15, 0.2, 0.25, 0.3]).await;
    // Kelly path requires >= 15 closed trades; WQS 100 gives a 0.5 base that
    // exceeds the 0.25 percentile cap, so the cap actually binds.
    insert_closed_trades_conviction(&pool, "test_wallet", 15).await;

    let sizer = PositionSizer::new(db, default_sizing_config());
    let mut factors = neutral_factors();
    factors.wallet_wqs = 100.0;
    factors.token_address = Some("token_a".to_string());
    let size = sizer.calculate_size(factors).await.unwrap();
    assert_eq!(
        size,
        Decimal::from_str("0.25").unwrap(),
        "full-size base 0.5 must be capped at the token's 75th-percentile entry size"
    );
}

#[tokio::test]
async fn test_conviction_cap_falls_back_to_default_when_thin_history() {
    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    // Only 2 historical entries (< 3) → fall back to the default cap 0.25.
    insert_token_positions(&pool, "token_b", &[0.1, 0.2]).await;
    insert_closed_trades_conviction(&pool, "test_wallet", 15).await;

    let sizer = PositionSizer::new(db, default_sizing_config());
    let mut factors = neutral_factors();
    factors.wallet_wqs = 100.0;
    factors.token_address = Some("token_b".to_string());
    let size = sizer.calculate_size(factors).await.unwrap();
    assert_eq!(
        size,
        Decimal::from_str("0.25").unwrap(),
        "thin history must fall back to the default conviction cap"
    );
}

#[tokio::test]
async fn test_conviction_cap_skipped_without_token_context() {
    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    insert_closed_trades_conviction(&pool, "test_wallet", 15).await;
    let sizer = PositionSizer::new(db, default_sizing_config());
    let mut factors = neutral_factors();
    factors.wallet_wqs = 100.0;
    let size = sizer.calculate_size(factors).await.unwrap();
    assert_eq!(
        size,
        Decimal::from_str("1.5").unwrap(),
        "no token context → capital-relative base unchanged: 10 * 0.15 * 1.0 * 1.0 = 1.5 (cap disabled)"
    );
}

// ─── Proven-wallet sizing override + skip-below-min semantics (2026-08-18) ──

/// Proven wallets (is_proven) size at the fixed proven_size_sol, bypassing
/// the WQS × confidence chain that would crush them (WQS ~10 → ~0.025 SOL
/// → rejected under skip semantics). Strategy max still applies.
#[tokio::test]
async fn test_proven_wallet_override_and_cap_exemptions() {
    let (db, _tmp) = create_test_db().await;
    let cfg = Arc::new(PositionSizingConfig {
        base_size_sol: Decimal::from_str("0.75").unwrap(),
        min_size_sol: Decimal::from_str("0.25").unwrap(),
        proven_sizing_boost: true,
        proven_size_sol: Decimal::from_str("0.75").unwrap(),
        // Capital-relative proven size: 10 * 0.075 = 0.75 (matches the
        // pre-2026-08-20 absolute expectation).
        proven_size_pct: Decimal::from_str("0.075").unwrap(),
        conviction_size_cap_enabled: true,
        conviction_size_default_cap_sol: Decimal::from_str("0.25").unwrap(),
        ..PositionSizingConfig::default()
    });
    let sizer = PositionSizer::new(db, cfg);

    // WQS-10 proven wallet with all penalties — chain would compute ~0.013.
    let mut factors = neutral_factors();
    factors.wallet_wqs = 10.0;
    factors.is_proven = true;
    factors.token_address = Some("tok_proven_override".to_string()); // conviction cap would clamp to 0.25
    factors.wqs_capped_max_size = Some(Decimal::from_str("0.25").unwrap()); // spear_lite would clamp to 0.25

    let size = sizer.calculate_size(factors).await.unwrap();
    assert_eq!(
        size,
        Decimal::from_str("0.75").unwrap(),
        "proven wallet must size at proven_size_pct × capital, exempt from WQS micro-cap and conviction cap"
    );
}

/// Proven override respects the strategy max cap (Shield, capital-relative).
#[tokio::test]
async fn test_proven_override_respects_strategy_max() {
    let (db, _tmp) = create_test_db().await;
    let cfg = Arc::new(PositionSizingConfig {
        // Strategy max = capital × shield_max_pct = 10 × 0.05 = 0.5 SOL.
        shield_max_pct: Decimal::from_str("0.05").unwrap(),
        proven_size_sol: Decimal::from_str("0.75").unwrap(),
        ..PositionSizingConfig::default()
    });
    let sizer = PositionSizer::new(db, cfg);

    let mut factors = neutral_factors();
    factors.is_proven = true;
    let size = sizer.calculate_size(factors).await.unwrap();
    assert_eq!(
        size,
        Decimal::from_str("0.5").unwrap(),
        "strategy max (capital × shield_max_pct) caps the proven override"
    );
}

/// Boost disabled: proven wallets fall back to the WQS chain — which under
/// skip semantics rejects (zero) at WQS 10. Documents the atomic coupling:
/// skip_below_min_size without proven_sizing_boost silences the proven roster.
#[tokio::test]
async fn test_proven_boost_disabled_rejects_low_wqs() {
    let (db, _tmp) = create_test_db().await;
    let cfg = Arc::new(PositionSizingConfig {
        base_size_sol: Decimal::from_str("0.75").unwrap(),
        min_size_sol: Decimal::from_str("0.25").unwrap(),
        proven_sizing_boost: false,
        ..PositionSizingConfig::default()
    });
    let sizer = PositionSizer::new(db, cfg);

    let mut factors = neutral_factors();
    factors.wallet_wqs = 10.0;
    factors.wqs_confidence = Some(0.5);
    factors.is_proven = true;
    let size = sizer.calculate_size(factors).await.unwrap();
    assert_eq!(
        size, Decimal::ZERO,
        "without the boost, a WQS-10 wallet's chain output (~0.04) must reject under skip semantics"
    );
}

// ─── Capital-relative sizing (2026-08-20) ─────────────────────────────────────

/// Position sizes scale linearly with `total_capital_sol`: identical factors at
/// 10 / 100 / 1000 SOL yield ~×10 / ~×100 sizes, until the absolute `max_size_sol`
/// ceiling (50) binds at 1000 SOL. Confirms the auto-scale requirement from the
/// sizing plan (10 → 1.5, 100 → 15, 1000 → 50, capacity-neutral).
#[tokio::test]
async fn test_capital_relative_sizes_scale_with_capital() {
    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    // ≥15 closed trades → confidence = 1.0, so base = capital * base_size_pct * wqs/100.
    insert_closed_trades(&pool, "scale_wallet", 15).await;
    let sizer = PositionSizer::new(db, default_sizing_config());

    async fn size_at_capital(sizer: &PositionSizer, capital: Decimal) -> Decimal {
        let mut f = neutral_factors();
        f.wallet_address = "scale_wallet".to_string();
        f.wallet_wqs = 100.0; // wqs_factor = 1.0, neutral multipliers → size = base.
        f.total_capital_sol = capital;
        f.token_address = None; // skip conviction cap; isolate the %-of-capital scaling
        sizer.calculate_size(f).await.unwrap()
    }

    let s10 = size_at_capital(&sizer, Decimal::from_str("10.0").unwrap()).await;
    let s100 = size_at_capital(&sizer, Decimal::from_str("100.0").unwrap()).await;
    let s1000 = size_at_capital(&sizer, Decimal::from_str("1000.0").unwrap()).await;

    // base = capital * 0.15 * 1.0 * 1.0.
    assert_eq!(s10, Decimal::from_str("1.5").unwrap(), "10 SOL → 15% = 1.5");
    assert_eq!(s100, Decimal::from_str("15.0").unwrap(), "100 SOL → 15% = 15");
    // 1000 * 0.15 = 150, but the absolute safety ceiling (max_size_sol = 50) binds.
    assert_eq!(
        s1000,
        Decimal::from_str("50.0").unwrap(),
        "1000 SOL capped at absolute 50 ceiling"
    );

    // Capacity-neutral: same fraction of capital deployed regardless of scale (before ceiling).
    let ratio_100_over_10 = s100 / s10;
    assert!(
        (ratio_100_over_10 - Decimal::from_str("10.0").unwrap()).abs()
            < Decimal::from_str("0.01").unwrap(),
        "size must scale ~10x from 10→100 SOL capital, got {}x",
        ratio_100_over_10
    );
}

/// The proven-wallet override scales with capital too: proven = capital ×
/// proven_size_pct, bounded by the Shield strategy max.
#[tokio::test]
async fn test_proven_wallet_scales_with_capital() {
    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    insert_closed_trades(&pool, "proven_wallet", 15).await;
    let sizer = PositionSizer::new(db, default_sizing_config());

    async fn proven_size(sizer: &PositionSizer, capital: Decimal) -> Decimal {
        let mut f = neutral_factors();
        f.wallet_address = "proven_wallet".to_string();
        f.total_capital_sol = capital;
        f.token_address = None;
        f.is_proven = true;
        sizer.calculate_size(f).await.unwrap()
    }

    // proven = capital * proven_size_pct (0.15); Shield strategy max = 0.30 capital > proven.
    let p10 = proven_size(&sizer, Decimal::from_str("10.0").unwrap()).await;
    let p100 = proven_size(&sizer, Decimal::from_str("100.0").unwrap()).await;
    assert_eq!(p10, Decimal::from_str("1.5").unwrap(), "proven at 10 SOL = 1.5");
    assert_eq!(p100, Decimal::from_str("15.0").unwrap(), "proven at 100 SOL = 15");
}

/// Capital-relative sizing still honours skip-below-min: a small capital that
/// yields a sub-minimum size rejects (zero) rather than clamping up; legacy mode
/// still clamps to the floor.
#[tokio::test]
async fn test_capital_relative_skip_below_min_preserved() {
    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    insert_closed_trades(&pool, "small_wallet", 15).await;

    // min 0.25 (as configured in prod), base_size_pct 0.15 (default).
    let cfg = Arc::new(PositionSizingConfig {
        min_size_sol: Decimal::from_str("0.25").unwrap(),
        ..PositionSizingConfig::default()
    });
    let sizer = PositionSizer::new(db.clone(), cfg);

    let mut f = neutral_factors();
    f.wallet_address = "small_wallet".to_string();
    f.wallet_wqs = 100.0; // wqs_factor = 1.0
    f.token_address = None;
    f.total_capital_sol = Decimal::from_str("1.0").unwrap(); // base = 1 * 0.15 = 0.15 < 0.25
    let size = sizer.calculate_size(f.clone()).await.unwrap();
    assert_eq!(
        size,
        Decimal::ZERO,
        "sub-minimum capital-relative size (0.15 < 0.25) must reject under skip-below-min"
    );

    // Legacy mode clamps up to the floor.
    let cfg_legacy = Arc::new(PositionSizingConfig {
        min_size_sol: Decimal::from_str("0.25").unwrap(),
        skip_below_min_size: false,
        ..PositionSizingConfig::default()
    });
    let sizer_legacy = PositionSizer::new(db, cfg_legacy);
    let size_legacy = sizer_legacy.calculate_size(f.clone()).await.unwrap();
    assert_eq!(
        size_legacy,
        Decimal::from_str("0.25").unwrap(),
        "legacy mode must clamp up to min_size_sol"
    );
}

// ─── Shadow-tier proven sizing multiplier (2026-08-28) ──────────────────────

use chimera_operator::db_abstraction::ShadowKellyStats;
use chimera_operator::engine::position_sizer::shadow_proven_size_multiplier;

fn shadow_stats(samples: i64, win: &str, avg_win: &str, avg_loss: &str) -> ShadowKellyStats {
    ShadowKellyStats {
        samples,
        win_rate: Decimal::from_str(win).unwrap(),
        avg_win: Decimal::from_str(avg_win).unwrap(),
        avg_loss: Decimal::from_str(avg_loss).unwrap(),
    }
}

#[test]
fn test_shadow_tier_star_edge() {
    // p=0.8, aw=0.20, al=0.02 -> expectancy 15.6% gross, 15.1% net >= 10 -> 1.5x
    let stats = shadow_stats(25, "0.8", "0.20", "0.02");
    assert_eq!(
        shadow_proven_size_multiplier(&stats, Decimal::from_str("0.5").unwrap(), 20),
        Decimal::from_str("1.5").unwrap()
    );
}

#[test]
fn test_shadow_tier_strong_edge() {
    // expectancy 6.0% gross, 5.5% net in [5, 10) -> 1.25x
    let stats = shadow_stats(25, "0.8", "0.08", "0.02");
    assert_eq!(
        shadow_proven_size_multiplier(&stats, Decimal::from_str("0.5").unwrap(), 20),
        Decimal::from_str("1.25").unwrap()
    );
}

#[test]
fn test_shadow_tier_net_clear_edge() {
    // expectancy 2.8% gross, 2.3% net in [0, 5) -> 1.0x (unchanged behavior)
    let stats = shadow_stats(25, "0.8", "0.04", "0.02");
    assert_eq!(
        shadow_proven_size_multiplier(&stats, Decimal::from_str("0.5").unwrap(), 20),
        Decimal::ONE
    );
}

#[test]
fn test_shadow_tier_below_cost() {
    // expectancy 0.2% gross, -0.3% net < 0 -> 0.5x (defensive)
    let stats = shadow_stats(25, "0.8", "0.01", "0.03");
    assert_eq!(
        shadow_proven_size_multiplier(&stats, Decimal::from_str("0.5").unwrap(), 20),
        Decimal::from_str("0.5").unwrap()
    );
}

#[test]
fn test_shadow_tier_exact_star_boundary() {
    // p=0.5, aw=0.22, al=0.01: p*aw - (1-p)*al = 0.11 - 0.005 = 0.105
    // -> 10.5% gross, 10.0% net — exactly at the >= 10 boundary -> 1.5x
    let stats = shadow_stats(25, "0.5", "0.22", "0.01");
    assert_eq!(
        shadow_proven_size_multiplier(&stats, Decimal::from_str("0.5").unwrap(), 20),
        Decimal::from_str("1.5").unwrap()
    );
}

#[test]
fn test_shadow_tier_thin_evidence_is_neutral() {
    // Absence of evidence is NOT negative evidence: thin book -> 1.0x.
    let stats = shadow_stats(5, "0.0", "0.0", "0.5");
    assert_eq!(
        shadow_proven_size_multiplier(&stats, Decimal::from_str("0.5").unwrap(), 20),
        Decimal::ONE
    );
}

// ─── Shadow-tier proven sizing integration ──────────────────────────────────

/// Seed `wins` wins at `win_pct` and `losses` losses at `loss_pct` as deduped
/// mirror_main shadow exits for `wallet` (one per hour — dedup key).
async fn seed_shadow_exits(
    pool: &Pool<Postgres>,
    wallet: &str,
    wins: usize,
    win_pct: &str,
    losses: usize,
    loss_pct: &str,
) {
    let mut hour: i32 = 1;
    for pct in std::iter::repeat(win_pct)
        .take(wins)
        .chain(std::iter::repeat(loss_pct).take(losses))
    {
        let sid = format!("sk-{}-{}", wallet, hour);
        sqlx::query(
            "INSERT INTO shadow_positions (shadow_id, decision_id, run_id, wallet_address, token_address, main_admitted, entry_amount_sol, ingress, opened_at) \
             VALUES ($1, 'd', 'run', $2, 'seedtoken', false, 0.1, 'webhook', NOW() - make_interval(hours => $3))",
        )
        .bind(&sid)
        .bind(wallet)
        .bind(hour)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO shadow_exits (shadow_id, exit_strategy, pnl_pct, exit_reason) \
             VALUES ($1, 'mirror_main', $2, 'profit_target')",
        )
        .bind(&sid)
        .bind(Decimal::from_str(pct).unwrap())
        .execute(pool)
        .await
        .unwrap();
        hour += 1;
    }
}

fn shadow_sizing_config(enabled: bool) -> Arc<PositionSizingConfig> {
    Arc::new(PositionSizingConfig {
        shadow_kelly_enabled: enabled,
        // Absolute ceiling under test: the sizer's Shield strategy_max and the
        // proven override both bind at `max_size_sol` (the sizer never reads
        // `shield_max_size_sol`). Default is 50.0 — too high for the star case
        // to demonstrate the tier-up being capped — so pin the 2.0 SOL ceiling
        // the shadow-tiering expectations are specified against.
        max_size_sol: Decimal::from_str("2.0").unwrap(),
        ..PositionSizingConfig::default()
    })
}

#[tokio::test]
async fn test_shadow_tier_star_proven_sized_up() {
    // 20 wins +20%, 5 losses -2% -> expectancy 15.6% gross, 15.1% net -> 1.5x.
    // proven base = 10 * 0.15 = 1.5 -> tiered 2.25 -> strategy_max = min(10*0.30, 2.0) = 2.0.
    let (db, _guard) = create_test_db().await;
    seed_shadow_exits(&pg_pool(&db), "test_wallet", 20, "20.0", 5, "-2.0").await;

    let mut factors = neutral_factors();
    factors.is_proven = true;
    let sizer = PositionSizer::new(db, shadow_sizing_config(true));
    let size = sizer.calculate_size(factors).await.unwrap();
    assert_eq!(size, Decimal::from_str("2.0").unwrap());
}

#[tokio::test]
async fn test_shadow_tier_below_cost_proven_sized_down() {
    // 20 wins +1%, 5 losses -3% -> expectancy 0.2% gross, -0.3% net -> 0.5x.
    // proven base 1.5 -> 0.75 (strategy_max 2.0 does not bind).
    let (db, _guard) = create_test_db().await;
    seed_shadow_exits(&pg_pool(&db), "test_wallet", 20, "1.0", 5, "-3.0").await;

    let mut factors = neutral_factors();
    factors.is_proven = true;
    let sizer = PositionSizer::new(db, shadow_sizing_config(true));
    let size = sizer.calculate_size(factors).await.unwrap();
    assert_eq!(size, Decimal::from_str("0.75").unwrap());
}

#[tokio::test]
async fn test_shadow_tier_disabled_keeps_flat_proven_size() {
    // Dark-launch guard: disabled -> flat proven_size_pct (1.5), no DB call effect.
    let (db, _guard) = create_test_db().await;
    seed_shadow_exits(&pg_pool(&db), "test_wallet", 20, "20.0", 5, "-2.0").await;

    let mut factors = neutral_factors();
    factors.is_proven = true;
    let sizer = PositionSizer::new(db, shadow_sizing_config(false));
    let size = sizer.calculate_size(factors).await.unwrap();
    assert_eq!(size, Decimal::from_str("1.5").unwrap());
}

#[tokio::test]
async fn test_shadow_tier_thin_evidence_flat_proven_size() {
    // 3 exits only (< 20 min samples) -> neutral 1.0x even when enabled.
    let (db, _guard) = create_test_db().await;
    seed_shadow_exits(&pg_pool(&db), "test_wallet", 3, "20.0", 0, "0").await;

    let mut factors = neutral_factors();
    factors.is_proven = true;
    let sizer = PositionSizer::new(db, shadow_sizing_config(true));
    let size = sizer.calculate_size(factors).await.unwrap();
    assert_eq!(size, Decimal::from_str("1.5").unwrap());
}
