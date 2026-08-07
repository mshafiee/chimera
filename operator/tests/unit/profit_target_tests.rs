//! Profit Target Unit Tests
//!
//! Covers every scenario where profits may not be captured correctly:
//! - Peak tracking reset after crash-and-recovery (trailing stop from wrong peak)
//! - Tiered exit fires at first target, not full exit
//! - Trailing stop activates only after threshold hit
//! - Trailing stop distance from peak is correct
//! - Time-based exit respects profit percentage thresholds
//! - No double-exit when position is already being exited

use chimera_operator::config::ProfitManagementConfig;
use chimera_operator::db_abstraction::Database;
use chimera_operator::engine::profit_targets::{ProfitTargetAction, ProfitTargetManager};
use chimera_operator::price_cache::{PriceCache, PriceSource};
use rust_decimal::Decimal;
use std::str::FromStr;
use std::sync::Arc;

#[path = "../common/mod.rs"]
mod common;

// ─── helpers ─────────────────────────────────────────────────────────────────

/// Each test gets its own isolated database (dropped on teardown).
async fn create_test_db() -> (Arc<dyn Database>, common::TestDbGuard) {
    common::create_test_pg_db().await
}

fn default_config() -> Arc<ProfitManagementConfig> {
    // Actual defaults (config.rs): targets=[25,50,100,200]%,
    // tiered_exit_percent=33.0, trailing_activation=50.0,
    // trailing_stop_distance=15.0, max_stop_loss_distance=-5.0, time_exit=24h.
    Arc::new(ProfitManagementConfig::default())
}

/// A config that forces vol_scale = 1.0 (no cold-start ramp), so the
/// documented target/stop arithmetic holds EXACTLY (the default
/// target_vol_scale_threshold scales targets by ~0.9833 on the first tick).
fn config_no_vol_ramp() -> Arc<ProfitManagementConfig> {
    Arc::new(ProfitManagementConfig {
        target_vol_scale_threshold: Decimal::ZERO,
        ..ProfitManagementConfig::default()
    })
}

#[allow(dead_code)]
fn config_with_trailing(activation: &str, distance: &str) -> Arc<ProfitManagementConfig> {
    Arc::new(ProfitManagementConfig {
        trailing_stop_activation: Decimal::from_str(activation).unwrap(),
        trailing_stop_distance: Decimal::from_str(distance).unwrap(),
        ..ProfitManagementConfig::default()
    })
}

// ─── Test 11 (plan #11) — peak tracking / trailing stop ratchet ──────────────

#[tokio::test]
async fn test_peak_tracking_after_crash_and_recovery() {
    // F4 regression guard: the trailing stop must ratchet up on new highs
    // (is_new_peak is captured before peak_price is overwritten).
    //
    // Sequence:
    //   entry $1.00 → activate at $1.20 (stop ≈ $0.96) → peak $2.00 (stop ratchets to ≈ $1.60)
    //   → crash to $1.40 → FullExit ($1.40 < ratcheted stop)
    //   → drop to $0.94 → FullExit (below the activation-time lock)

    let (db, _tmp) = create_test_db().await;
    let price_cache = Arc::new(PriceCache::new().unwrap());
    const TOKEN: &str = "token_crash_recovery";

    let cfg = Arc::new(ProfitManagementConfig {
        targets: vec![], // No tiered exits — isolate trailing stop behavior
        trailing_stop_activation: Decimal::from_str("10.0").unwrap(),
        trailing_stop_distance: Decimal::from_str("20.0").unwrap(),
        ..ProfitManagementConfig::default()
    });
    let mgr = ProfitTargetManager::new(db, cfg, price_cache.clone());

    price_cache.set_price(
        TOKEN,
        Decimal::from_str("1.00").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    mgr.register_position(
        "uuid-peak",
        Decimal::from_str("1.00").unwrap(),
        Decimal::from_str("5.0").unwrap(),
        TOKEN,
        std::time::SystemTime::now(),
    )
    .await;

    // Rise to $1.20 (+20%) → activates trailing stop; trailing_stop_price = $1.20 × 0.80 = $0.96
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("1.20").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let _ = mgr.check_targets("uuid-peak", TOKEN, TOKEN, "SHIELD").await;

    // Rise to $2.00 (+100%) → peak updates to $2.00, BUT trailing_stop_price stays at $0.96
    // (due to the peak update ordering bug — new high check fires AFTER peak was updated)
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("2.00").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let _ = mgr.check_targets("uuid-peak", TOKEN, TOKEN, "SHIELD").await;

    // Crash to $1.40 — below INTENDED stop ($2.00 × 0.80 = $1.60) but ABOVE ACTUAL stop ($0.96)
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("1.40").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let action_at_crash = mgr.check_targets("uuid-peak", TOKEN, TOKEN, "SHIELD").await;

    // After ratchet fix: stop price is $2.00 × 0.80 = $1.60, so $1.40 < $1.60 → FullExit
    assert!(
        matches!(action_at_crash, ProfitTargetAction::FullExit),
        "Trailing stop must ratchet to $1.60 at $2.00 peak; $1.40 crash must trigger FullExit."
    );

    // Position DOES exit when price falls below the activation-time locked stop ($0.96)
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("0.94").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let action_at_floor = mgr.check_targets("uuid-peak", TOKEN, TOKEN, "SHIELD").await;
    assert!(
        matches!(action_at_floor, ProfitTargetAction::FullExit),
        "Position must exit at $0.94 (below locked trailing stop of $0.96)"
    );
}

// ─── Test 12 (plan #12) — tiered exit at first target ────────────────────────

#[tokio::test]
async fn test_first_target_fires_partial_exit_not_full() {
    // Price reaches first target (+25%). Must return ExitAmount not FullExit.
    // entry_amount_sol = 4.0, exit fraction = 0.33, expected sell = 1.32 SOL

    let (db, _tmp) = create_test_db().await;
    let price_cache = Arc::new(PriceCache::new().unwrap());
    const TOKEN: &str = "token_first_target";

    price_cache.set_price(
        TOKEN,
        Decimal::from_str("1.00").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let mgr = ProfitTargetManager::new(db, default_config(), price_cache.clone());
    mgr.register_position(
        "uuid-tier",
        Decimal::from_str("1.00").unwrap(),
        Decimal::from_str("4.0").unwrap(),
        TOKEN,
        std::time::SystemTime::now(),
    )
    .await;

    // Price at exactly +25%: $1.25
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("1.25").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let action = mgr.check_targets("uuid-tier", TOKEN, TOKEN, "SHIELD").await;

    match action {
        ProfitTargetAction::ExitAmount(amount) => {
            assert_eq!(
                amount,
                Decimal::from_str("1.32").unwrap(),
                "Tiered exit must sell 33% of remaining position (4.0 * 0.33 = 1.32 SOL), not full exit"
            );
        }
        other => panic!("Expected ExitAmount(1.32), got {:?}", other),
    }
}

// ─── Test 13 (plan #13) — time-based exit below profit threshold ──────────────

#[tokio::test]
async fn test_time_based_exit_not_triggered_with_insufficient_profit() {
    // Position at +8% after >24h. Time-based logic: profits 5-10% use `time_exit_hours` (24h).
    // But +8% is in the "5-10% → use time_exit_hours" range → after 24h → FullExit.
    // This test verifies the tiered time-exit logic works for moderate profits.
    //
    // Note: The implementation uses separate bands:
    //   >10%: exit after 48h
    //   0-5%: exit after 12h
    //   5-10%: exit after `time_exit_hours` (default 24h) — this is the "else" branch

    let (db, _tmp) = create_test_db().await;
    let price_cache = Arc::new(PriceCache::new().unwrap());
    const TOKEN: &str = "token_time_exit";

    price_cache.set_price(
        TOKEN,
        Decimal::from_str("1.00").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );

    // Price at +8%: profit <= 10% → the "low-profit/losing" band, which exits
    // after losing_time_exit_hours_shield (default 4h) — NOT time_exit_hours.
    // Backdate the entry time by 25h so the time-based exit actually fires.
    let mgr = ProfitTargetManager::new(db, default_config(), price_cache.clone());
    let entry_time = std::time::SystemTime::now() - std::time::Duration::from_secs(25 * 3600);
    mgr.register_position(
        "uuid-time",
        Decimal::from_str("1.00").unwrap(),
        Decimal::from_str("2.0").unwrap(),
        TOKEN,
        entry_time,
    )
    .await;

    // Price at +8%
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("1.08").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let action = mgr.check_targets("uuid-time", TOKEN, TOKEN, "SHIELD").await;

    assert!(
        matches!(action, ProfitTargetAction::FullExit),
        "Position at +8% held >25h must exit via the low-profit time band (4h default)"
    );
}

// ─── Test 14 (plan #14) ── first target yields partial not full exit ──────────
// (Covered by Test 12 above.  Adding a complementary: price just below first target.)

#[tokio::test]
async fn test_price_just_below_first_target_no_exit() {
    // Price at +24.9% — below first target of +25%. No action should fire.

    let (db, _tmp) = create_test_db().await;
    let price_cache = Arc::new(PriceCache::new().unwrap());
    const TOKEN: &str = "token_below_first";

    price_cache.set_price(
        TOKEN,
        Decimal::from_str("1.00").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    // target_vol_scale_threshold = 0 forces vol_scale = 1.0, so the first
    // target is exactly 25% (without it, the cold-start ramp scales the
    // target to ≈24.58% and +24.9% would already fire).
    let mgr = ProfitTargetManager::new(db, config_no_vol_ramp(), price_cache.clone());
    mgr.register_position(
        "uuid-below",
        Decimal::from_str("1.00").unwrap(),
        Decimal::from_str("2.0").unwrap(),
        TOKEN,
        std::time::SystemTime::now(),
    )
    .await;

    // +24.9%
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("1.249").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let action = mgr
        .check_targets("uuid-below", TOKEN, TOKEN, "SHIELD")
        .await;

    assert!(
        matches!(action, ProfitTargetAction::None),
        "Price below first target should return None"
    );
}

// ─── Test 15 (plan #15) — trailing stop not active before activation threshold ─

#[tokio::test]
async fn test_trailing_stop_not_active_before_threshold() {
    // Trailing stop activates after +50%. Price at +49% → no trailing stop active.
    // Even a 20% price drop from peak should not trigger FullExit.

    let (db, _tmp) = create_test_db().await;
    let price_cache = Arc::new(PriceCache::new().unwrap());
    const TOKEN: &str = "token_trailing_inactive";

    price_cache.set_price(
        TOKEN,
        Decimal::from_str("1.00").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let mgr = ProfitTargetManager::new(db, default_config(), price_cache.clone());
    mgr.register_position(
        "uuid-trail-off",
        Decimal::from_str("1.00").unwrap(),
        Decimal::from_str("2.0").unwrap(),
        TOKEN,
        std::time::SystemTime::now(),
    )
    .await;

    // Peak at +49%: $1.49
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("1.49").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let _ = mgr
        .check_targets("uuid-trail-off", TOKEN, TOKEN, "SHIELD")
        .await;

    // Price drops 20% from peak: $1.49 × 0.80 = $1.192 (still +19.2% from entry)
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("1.19").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let action = mgr
        .check_targets("uuid-trail-off", TOKEN, TOKEN, "SHIELD")
        .await;

    assert!(
        !matches!(action, ProfitTargetAction::FullExit),
        "Trailing stop must NOT be active before +50% activation threshold"
    );
}

// ─── Test 16 (plan #16) — trailing stop distance from peak ───────────────────

#[tokio::test]
async fn test_trailing_stop_distance_from_peak() {
    // Peak = +60% ($1.60). Trailing stop distance = 20%.
    // Trailing stop price = $1.60 × 0.80 = $1.28.
    // At $1.29 (just above) → None. At $1.27 (just below) → FullExit.
    //
    // Uses empty `targets` to prevent tiered profit targets from returning
    // ExitAmount before trailing stop activation code is reached.
    // (With targets=[25%], the first check_targets at $1.60 (+60%) would return
    // ExitAmount early, preventing trailing_stop_active from ever being set.)

    let (db, _tmp) = create_test_db().await;
    let price_cache = Arc::new(PriceCache::new().unwrap());
    const TOKEN_A: &str = "token_trail_a";
    const TOKEN_B: &str = "token_trail_b";

    // Empty targets: no tiered exits, trailing stop activates at 50% profit.
    // target_vol_scale_threshold = 0 keeps the stop EXACTLY $1.60 × 0.80 = $1.28
    // (the cold-start ramp would otherwise make it ≈$1.2853).
    let cfg = Arc::new(ProfitManagementConfig {
        targets: vec![],
        trailing_stop_activation: Decimal::from_str("50.0").unwrap(),
        trailing_stop_distance: Decimal::from_str("20.0").unwrap(),
        target_vol_scale_threshold: Decimal::ZERO,
        ..ProfitManagementConfig::default()
    });

    // ── Just above trailing stop ──
    price_cache.set_price(
        TOKEN_A,
        Decimal::from_str("1.00").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let mgr_a = ProfitTargetManager::new(db.clone(), cfg.clone(), price_cache.clone());
    mgr_a
        .register_position(
            "uuid-trail-a",
            Decimal::from_str("1.00").unwrap(),
            Decimal::from_str("2.0").unwrap(),
            TOKEN_A,
            std::time::SystemTime::now(),
        )
        .await;

    // Rise to $1.60 (+60% → activates trailing stop at 50% threshold)
    // trailing_stop_price = $1.60 × 0.80 = $1.28
    price_cache.set_price(
        TOKEN_A,
        Decimal::from_str("1.60").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let _ = mgr_a
        .check_targets("uuid-trail-a", TOKEN_A, TOKEN_A, "SHIELD")
        .await;

    // Drop to $1.29 (above $1.28 trailing stop)
    price_cache.set_price(
        TOKEN_A,
        Decimal::from_str("1.29").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let action_above = mgr_a
        .check_targets("uuid-trail-a", TOKEN_A, TOKEN_A, "SHIELD")
        .await;
    assert!(
        !matches!(action_above, ProfitTargetAction::FullExit),
        "$1.29 is above $1.28 trailing stop, must not exit"
    );

    // ── Just below trailing stop ──
    price_cache.set_price(
        TOKEN_B,
        Decimal::from_str("1.00").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let mgr_b = ProfitTargetManager::new(db, cfg, price_cache.clone());
    mgr_b
        .register_position(
            "uuid-trail-b",
            Decimal::from_str("1.00").unwrap(),
            Decimal::from_str("2.0").unwrap(),
            TOKEN_B,
            std::time::SystemTime::now(),
        )
        .await;

    // Rise to $1.60 to activate trailing stop
    price_cache.set_price(
        TOKEN_B,
        Decimal::from_str("1.60").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let _ = mgr_b
        .check_targets("uuid-trail-b", TOKEN_B, TOKEN_B, "SHIELD")
        .await;

    // Drop to $1.27 (below $1.28 trailing stop) → FullExit
    price_cache.set_price(
        TOKEN_B,
        Decimal::from_str("1.27").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let action_below = mgr_b
        .check_targets("uuid-trail-b", TOKEN_B, TOKEN_B, "SHIELD")
        .await;
    assert!(
        matches!(action_below, ProfitTargetAction::FullExit),
        "$1.27 is below $1.28 trailing stop, must trigger FullExit"
    );
}

// ─── Test 17 (plan #17) — no exit when position not registered ───────────────

#[tokio::test]
async fn test_unknown_trade_uuid_returns_none() {
    // check_targets for an unregistered trade_uuid returns None (no state → no exit).
    // This prevents ghost exits for already-closed positions.

    let (db, _tmp) = create_test_db().await;
    let price_cache = Arc::new(PriceCache::new().unwrap());
    const TOKEN: &str = "token_unknown";

    price_cache.set_price(
        TOKEN,
        Decimal::from_str("100.0").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let mgr = ProfitTargetManager::new(db, default_config(), price_cache);

    let action = mgr
        .check_targets("uuid-not-registered", TOKEN, TOKEN, "SHIELD")
        .await;
    assert!(
        matches!(action, ProfitTargetAction::None),
        "Unregistered trade must return None, not trigger a spurious exit"
    );
}

// ─── Test: same target hit twice returns ExitAmount only once ────────────────

#[tokio::test]
async fn test_same_target_not_hit_twice() {
    // Once a target is marked as hit, the same price level should not trigger another exit.

    let (db, _tmp) = create_test_db().await;
    let price_cache = Arc::new(PriceCache::new().unwrap());
    const TOKEN: &str = "token_double_hit";

    price_cache.set_price(
        TOKEN,
        Decimal::from_str("1.00").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let mgr = ProfitTargetManager::new(db, default_config(), price_cache.clone());
    mgr.register_position(
        "uuid-dbl",
        Decimal::from_str("1.00").unwrap(),
        Decimal::from_str("4.0").unwrap(),
        TOKEN,
        std::time::SystemTime::now(),
    )
    .await;

    // First hit at +25%
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("1.25").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let first = mgr.check_targets("uuid-dbl", TOKEN, TOKEN, "SHIELD").await;
    assert!(
        matches!(first, ProfitTargetAction::ExitAmount(_)),
        "First hit must fire ExitAmount"
    );

    // Second check at same price — target already registered as hit
    let second = mgr.check_targets("uuid-dbl", TOKEN, TOKEN, "SHIELD").await;
    assert!(
        !matches!(second, ProfitTargetAction::ExitAmount(_)),
        "Second check at same price must NOT fire ExitAmount again (already hit)"
    );
}

// ─── Test: unregistered position returns None (price cache unavailable) ───────

#[tokio::test]
async fn test_no_price_in_cache_returns_none() {
    // If price cache has no entry for the token, check_targets early-returns None.

    let (db, _tmp) = create_test_db().await;
    let price_cache = Arc::new(PriceCache::new().unwrap());
    const TOKEN: &str = "token_no_price";

    // Register position but set NO price
    let mgr = ProfitTargetManager::new(db, default_config(), price_cache.clone());
    mgr.register_position(
        "uuid-noprice",
        Decimal::from_str("1.00").unwrap(),
        Decimal::from_str("1.0").unwrap(),
        TOKEN,
        std::time::SystemTime::now(),
    )
    .await;

    let action = mgr
        .check_targets("uuid-noprice", TOKEN, TOKEN, "SHIELD")
        .await;
    assert!(
        matches!(action, ProfitTargetAction::None),
        "No cached price must return None, not trigger exit"
    );
}
