//! Stop Loss Unit Tests
//!
//! Covers every scenario where the stop-loss system can fail to protect capital:
//! - Zero entry price bypasses dynamic stop (loss_percent=0 never ≤ negative threshold)
//! - Consensus query failure silently falls back to no stop widening
//! - Volatility multiplier boundary correctness
//! - Hard stop overrides wider dynamic threshold
//! - Fail-open when price cache is unavailable
//!
//! NOTE: the effective stop threshold is the dynamic (WQS × volatility)
//! threshold clamped to [max_stop_loss_distance, -5%] and the absolute -25%
//! floor. With the DEFAULT max_stop_loss_distance (-5.0) every dynamic
//! threshold collapses to -5%; tests that exercise dynamic thresholds use an
//! explicit wide distance (e.g. -50.0).

use chimera_operator::config::ProfitManagementConfig;
use chimera_operator::db_abstraction::{Database, DbPool};
use chimera_operator::engine::stop_loss::{StopLossAction, StopLossManager};
use chimera_operator::price_cache::{PriceCache, PriceSource};
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

/// Returns an entry_time sufficiently in the past to clear the 10-second wick-protection
/// grace period, so stop-loss checks evaluate the threshold rather than bailing early.
fn past_entry() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now() - chrono::TimeDelta::seconds(60)
}

// ─── helpers ─────────────────────────────────────────────────────────────────

/// Each test gets its own isolated database (dropped on teardown), so the
/// fixed wallet addresses below never collide across parallel tests.
async fn create_test_db() -> (Arc<dyn Database>, common::TestDbGuard) {
    common::create_test_pg_db().await
}

fn default_config() -> Arc<ProfitManagementConfig> {
    Arc::new(ProfitManagementConfig::default())
}

/// A config with an explicit `max_stop_loss_distance` (negative; the config
/// validator rejects positive values).
fn config_with_stop_distance(stop_distance: &str) -> Arc<ProfitManagementConfig> {
    Arc::new(ProfitManagementConfig {
        max_stop_loss_distance: Decimal::from_str(stop_distance).unwrap(),
        ..ProfitManagementConfig::default()
    })
}

/// Insert a wallet with a specific WQS score.
async fn insert_wallet(pool: &Pool<Postgres>, address: &str, wqs: f64) {
    sqlx::query(
        "INSERT INTO wallets (address, status, wqs_score, created_at, updated_at) \
         VALUES ($1, 'ACTIVE', $2, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
    )
    .bind(address)
    .bind(wqs)
    .execute(pool)
    .await
    .unwrap();
}

/// Insert a consensus BUY signal into signal_aggregation within the last 5 minutes.
async fn insert_consensus_signal(pool: &Pool<Postgres>, token: &str, wallet: &str) {
    sqlx::query(
        "INSERT INTO signal_aggregation \
         (token_address, wallet_address, direction, amount_sol, created_at) \
         VALUES ($1, $2, 'BUY', 1.0, CURRENT_TIMESTAMP)",
    )
    .bind(token)
    .bind(wallet)
    .execute(pool)
    .await
    .unwrap();
}

// ─── Test 1 ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_zero_entry_price_forces_immediate_exit() {
    // Zero entry_price means the position's cost basis is corrupt.
    // We cannot calculate a valid loss percentage, so the safest action is to force
    // an immediate exit to recover capital rather than hold indefinitely.

    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    let price_cache = Arc::new(PriceCache::new().unwrap());
    insert_wallet(&pool, "wallet_a", 50.0).await;

    const TOKEN: &str = "token_zero_entry";
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("1.0").unwrap(),
        PriceSource::Jupiter, Some(9),
    );

    let mgr = StopLossManager::new(db, default_config(), price_cache);

    let action = mgr
        .check_stop_loss("uuid-1", "wallet_a", Decimal::ZERO, TOKEN, past_entry())
        .await;

    assert_eq!(
        action,
        StopLossAction::Exit,
        "Corrupt zero entry_price must trigger immediate exit to recover capital"
    );
}

// ─── Test 2 ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_consensus_query_failure_no_stop_widening() {
    // When the signal_aggregation table query fails (DB error), is_consensus defaults to false.
    // This means stop-loss does NOT widen by 5% for what should be a consensus signal.
    // Effect: a -17% loss exits early when the widened (-20%) threshold shouldn't have triggered.

    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    let price_cache = Arc::new(PriceCache::new().unwrap());

    // Insert wallet with WQS 80 → dynamic stop = -20%
    insert_wallet(&pool, "wallet_b", 80.0).await;

    const TOKEN: &str = "token_consensus_fail";
    // Entry price: $1.00, current price: $0.83 → -17% loss
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("0.83").unwrap(),
        PriceSource::Jupiter, Some(9),
    );

    // No signal_aggregation rows → query returns 0 → is_consensus = false.
    // Two-point history → volatility 0 → ×0.9 → effective threshold ≈ -18%.
    // -17% > -18% → should NOT exit.
    let mgr = StopLossManager::new(db, config_with_stop_distance("-100.0"), price_cache);

    let action = mgr
        .check_stop_loss(
            "uuid-2",
            "wallet_b",
            Decimal::from_str("1.00").unwrap(),
            TOKEN,
            past_entry(),
        )
        .await;

    assert_eq!(
        action,
        StopLossAction::None,
        "At -17% with high-WQS ≈-18% threshold and no consensus: should not exit yet"
    );
}

#[tokio::test]
async fn test_consensus_widens_stop_for_high_wqs_wallet() {
    // With 2+ wallets buying the same token, is_consensus=true widens the stop.
    // High WQS (-20%) × 0.9 volatility × 1.25 consensus ≈ -22.5% threshold.
    // A -22% loss should NOT exit.

    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    let price_cache = Arc::new(PriceCache::new().unwrap());

    insert_wallet(&pool, "wallet_c", 80.0).await;

    const TOKEN: &str = "token_consensus_wide";
    // Insert 2 consensus signals (both wallets must exist for FK constraint)
    insert_wallet(&pool, "wallet_d", 80.0).await;
    insert_consensus_signal(&pool, TOKEN, "wallet_c").await;
    insert_consensus_signal(&pool, TOKEN, "wallet_d").await;

    // Entry $1.00, current $0.78 → -22% loss
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("0.78").unwrap(),
        PriceSource::Jupiter, Some(9),
    );

    let mgr = StopLossManager::new(db, config_with_stop_distance("-100.0"), price_cache);

    let action = mgr
        .check_stop_loss(
            "uuid-3",
            "wallet_c",
            Decimal::from_str("1.00").unwrap(),
            TOKEN,
            past_entry(),
        )
        .await;

    assert_eq!(
        action,
        StopLossAction::None,
        "At -22% with consensus-widened ≈-22.5% threshold: should not exit"
    );
}

// ─── Test 3 ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_high_wqs_high_volatility_widens_to_40pct() {
    // WQS ≥ 70 → base stop = -20%.  Volatility > 30% → multiplier = 2.0.
    // -20% × 2.0 = -40%, clamped to the -25% absolute floor (widest allowed).
    // -24% loss → None. -26% loss → Exit.

    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    insert_wallet(&pool, "wallet_vol", 75.0).await;

    const TOKEN: &str = "token_high_vol";
    let price_cache = Arc::new(PriceCache::new().unwrap());

    // Push enough price history to compute volatility > 30%.
    let prices = [
        1.00, 1.35, 0.90, 1.30, 0.88, 1.40, 0.87, 1.35, 0.86, 1.30_f64,
    ];
    for p in prices {
        price_cache.set_price(
            TOKEN,
            Decimal::from_str(&p.to_string()).unwrap(),
            PriceSource::Jupiter, Some(9),
        );
    }

    let vol = price_cache.calculate_volatility(TOKEN);
    assert!(vol.is_some(), "Volatility must be calculable");
    assert!(
        vol.unwrap() > 30.0,
        "Test setup requires volatility > 30%, got {}",
        vol.unwrap()
    );

    // At -24%: entry $1.00, current $0.76 → -24% → None (threshold clamped to -25%)
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("0.76").unwrap(),
        PriceSource::Jupiter, Some(9),
    );
    let mgr = StopLossManager::new(
        db.clone(),
        config_with_stop_distance("-25.0"),
        price_cache.clone(),
    );
    let action_near = mgr
        .check_stop_loss(
            "uuid-vol-near",
            "wallet_vol",
            Decimal::from_str("1.00").unwrap(),
            TOKEN,
            past_entry(),
        )
        .await;
    assert_eq!(
        action_near,
        StopLossAction::None,
        "-24% loss with -25% (clamped) threshold should not exit"
    );

    // At -26%: current $0.74 → Exit (past the -25% clamp)
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("0.74").unwrap(),
        PriceSource::Jupiter, Some(9),
    );
    let mgr2 = StopLossManager::new(db, config_with_stop_distance("-25.0"), price_cache);
    let action_over = mgr2
        .check_stop_loss(
            "uuid-vol-over",
            "wallet_vol",
            Decimal::from_str("1.00").unwrap(),
            TOKEN,
            past_entry(),
        )
        .await;
    assert_eq!(
        action_over,
        StopLossAction::Exit,
        "-26% loss with -25% (clamped) threshold must exit"
    );
}

// ─── Test 4 ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_low_wqs_low_volatility_tightens_to_9pct() {
    // WQS < 40 → base stop = -10%.  Volatility < 10% → multiplier = 0.9.
    // -10% × 0.9 = -9%.
    // A -6% loss must NOT exit (< 9% threshold).
    // A -10% loss MUST exit (exceeds -9% threshold).
    //
    // Uses a wide max_stop_loss_distance (-50) so the -9% threshold is
    // effective (with the DEFAULT -5.0 it would collapse to -5%).

    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    insert_wallet(&pool, "wallet_tight", 30.0).await;

    const TOKEN: &str = "token_low_vol";
    let price_cache = Arc::new(PriceCache::new().unwrap());

    // Push prices with very small variance to get volatility < 10%
    for _ in 0..5 {
        price_cache.set_price(
            TOKEN,
            Decimal::from_str("1.001").unwrap(),
            PriceSource::Jupiter, Some(9),
        );
        price_cache.set_price(
            TOKEN,
            Decimal::from_str("0.999").unwrap(),
            PriceSource::Jupiter, Some(9),
        );
    }

    let vol = price_cache.calculate_volatility(TOKEN);
    if let Some(v) = vol {
        assert!(v < 10.0, "Test setup requires low volatility, got {}", v);
    }

    let mgr = StopLossManager::new(db.clone(), config_with_stop_distance("-50.0"), price_cache.clone());

    // -6% loss: below the -9% threshold → must NOT exit
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("0.94").unwrap(),
        PriceSource::Jupiter, Some(9),
    );
    let action_small = mgr
        .check_stop_loss(
            "uuid-tight-small",
            "wallet_tight",
            Decimal::from_str("1.00").unwrap(),
            TOKEN,
            past_entry(),
        )
        .await;
    assert_eq!(
        action_small,
        StopLossAction::None,
        "-6% loss must not exit (threshold = -9%)"
    );

    // -10% loss: exceeds the -9% threshold → must exit
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("0.90").unwrap(),
        PriceSource::Jupiter, Some(9),
    );
    let action_large = mgr
        .check_stop_loss(
            "uuid-tight-large",
            "wallet_tight",
            Decimal::from_str("1.00").unwrap(),
            TOKEN,
            past_entry(),
        )
        .await;
    assert_eq!(
        action_large,
        StopLossAction::Exit,
        "-10% loss must exit (exceeds -9% threshold)"
    );
}

// ─── Test 5 ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_consensus_plus_high_volatility_widens_further() {
    // WQS ≥ 70 (-20%) × 2.0 (>30% volatility) = -40%, then ×1.25 consensus = -50%,
    // clamped to the -25% absolute floor. Effective threshold: -25%.
    // -24% loss → None. -26% loss → Exit.

    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    insert_wallet(&pool, "wallet_cv", 75.0).await;

    const TOKEN: &str = "token_cv";
    let price_cache = Arc::new(PriceCache::new().unwrap());

    // Build high volatility
    let prices = [1.0, 1.4, 0.85, 1.35, 0.88, 1.42_f64];
    for p in prices {
        price_cache.set_price(
            TOKEN,
            Decimal::from_str(&p.to_string()).unwrap(),
            PriceSource::Jupiter, Some(9),
        );
    }
    assert!(price_cache.calculate_volatility(TOKEN).unwrap_or(0.0) > 30.0);

    // Insert 2 consensus signals (both wallets must exist for FK constraint)
    insert_wallet(&pool, "wallet_other", 75.0).await;
    insert_consensus_signal(&pool, TOKEN, "wallet_cv").await;
    insert_consensus_signal(&pool, TOKEN, "wallet_other").await;

    // -24% loss: $0.76 from $1.00 — within -25% threshold → None
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("0.76").unwrap(),
        PriceSource::Jupiter, Some(9),
    );
    let mgr = StopLossManager::new(
        db.clone(),
        config_with_stop_distance("-25.0"),
        price_cache.clone(),
    );
    let none = mgr
        .check_stop_loss(
            "uuid-cv-1",
            "wallet_cv",
            Decimal::from_str("1.00").unwrap(),
            TOKEN,
            past_entry(),
        )
        .await;
    assert_eq!(
        none,
        StopLossAction::None,
        "-24% should not exit when threshold is -25% (vol×2.0 × consensus×1.25 clamped)"
    );

    // -26% loss: $0.74 from $1.00 — exceeds -25% threshold → Exit
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("0.74").unwrap(),
        PriceSource::Jupiter, Some(9),
    );
    let mgr2 = StopLossManager::new(db, config_with_stop_distance("-25.0"), price_cache);
    let exit = mgr2
        .check_stop_loss(
            "uuid-cv-2",
            "wallet_cv",
            Decimal::from_str("1.00").unwrap(),
            TOKEN,
            past_entry(),
        )
        .await;
    assert_eq!(
        exit,
        StopLossAction::Exit,
        "-26% must exit when threshold is -25% (vol×2.0 × consensus×1.25 clamped)"
    );
}

// ─── Test 6 ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_hard_stop_overrides_wider_dynamic_threshold() {
    // High WQS (≥70) sets a dynamic threshold near -20% (×0.9 vol ≈ -18%).
    // Config max_stop_loss_distance = -12.0 clamps the effective threshold to
    // -12% (the operator-configured floor is tighter than the dynamic stop).
    // At -13% loss: -13 <= -12 → Exit — the configured floor fires before the
    // dynamic -18% threshold is reached.

    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    insert_wallet(&pool, "wallet_hardstop", 75.0).await;

    const TOKEN: &str = "token_hardstop";
    let price_cache = Arc::new(PriceCache::new().unwrap());
    // -13% loss: entry $1.00, current $0.87
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("0.87").unwrap(),
        PriceSource::Jupiter, Some(9),
    );

    let cfg = config_with_stop_distance("-12.0");
    let mgr = StopLossManager::new(db, cfg, price_cache);

    let action = mgr
        .check_stop_loss(
            "uuid-hardstop",
            "wallet_hardstop",
            Decimal::from_str("1.00").unwrap(),
            TOKEN,
            past_entry(),
        )
        .await;

    assert_eq!(
        action,
        StopLossAction::Exit,
        "Configured floor (-12.0) must fire at -13% even though the dynamic threshold is ≈-18%"
    );
}

// ─── Test 10 ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_stop_loss_price_cache_unavailable_returns_none() {
    // When the price cache has no entry for the token, get_price_usd() returns None.
    // check_stop_loss() early-returns StopLossAction::None (fail-open).
    // Documents that capital is unprotected when price data is unavailable.

    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    let price_cache = Arc::new(PriceCache::new().unwrap());

    // No price is set for the token
    insert_wallet(&pool, "wallet_nocache", 50.0).await;

    let mgr = StopLossManager::new(db, default_config(), price_cache);
    let action = mgr
        .check_stop_loss(
            "uuid-nocache",
            "wallet_nocache",
            Decimal::from_str("1.00").unwrap(),
            "token_nocache",
            past_entry(),
        )
        .await;

    assert_eq!(
        action,
        StopLossAction::None,
        "Missing price cache entry returns None — position unprotected until price data arrives"
    );
}

// ─── Test 11 — medium-WQS standard stop ──────────────────────────────────────

#[tokio::test]
async fn test_medium_wqs_standard_stop_at_15pct() {
    // WQS 40–70 → dynamic threshold = -15%, ×0.9 (two-point history, vol ≈ 0)
    // → effective ≈ -13.5%.
    // -10% → None. -15% → Exit.

    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    insert_wallet(&pool, "wallet_med", 55.0).await;

    const TOKEN: &str = "token_med_wqs";
    let price_cache = Arc::new(PriceCache::new().unwrap());

    // -10%: entry $1.00, current $0.90 → None (well above ≈-13.5% threshold)
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("0.90").unwrap(),
        PriceSource::Jupiter, Some(9),
    );
    let mgr = StopLossManager::new(db.clone(), config_with_stop_distance("-100.0"), price_cache.clone());
    let none = mgr
        .check_stop_loss(
            "uuid-med-1",
            "wallet_med",
            Decimal::from_str("1.00").unwrap(),
            TOKEN,
            past_entry(),
        )
        .await;
    assert_eq!(
        none,
        StopLossAction::None,
        "-10% must NOT exit for a medium-WQS wallet (threshold ≈ -13.5%)"
    );

    // -15%: current $0.85 → Exit
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("0.85").unwrap(),
        PriceSource::Jupiter, Some(9),
    );
    let mgr2 = StopLossManager::new(db, config_with_stop_distance("-100.0"), price_cache);
    let exit = mgr2
        .check_stop_loss(
            "uuid-med-2",
            "wallet_med",
            Decimal::from_str("1.00").unwrap(),
            TOKEN,
            past_entry(),
        )
        .await;
    assert_eq!(
        exit,
        StopLossAction::Exit,
        "-15% must trigger exit for medium-WQS wallet"
    );
}
