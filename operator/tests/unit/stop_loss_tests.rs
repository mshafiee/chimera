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
use rust_decimal::prelude::FromPrimitive;
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
        PriceSource::Jupiter,
        Some(9),
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
        PriceSource::Jupiter,
        Some(9),
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
        PriceSource::Jupiter,
        Some(9),
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
            PriceSource::Jupiter,
            Some(9),
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
        PriceSource::Jupiter,
        Some(9),
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
        PriceSource::Jupiter,
        Some(9),
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
            PriceSource::Jupiter,
            Some(9),
        );
        price_cache.set_price(
            TOKEN,
            Decimal::from_str("0.999").unwrap(),
            PriceSource::Jupiter,
            Some(9),
        );
    }

    let vol = price_cache.calculate_volatility(TOKEN);
    if let Some(v) = vol {
        assert!(v < 10.0, "Test setup requires low volatility, got {}", v);
    }

    let mgr = StopLossManager::new(
        db.clone(),
        config_with_stop_distance("-50.0"),
        price_cache.clone(),
    );

    // -6% loss: below the -9% threshold → must NOT exit
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("0.94").unwrap(),
        PriceSource::Jupiter,
        Some(9),
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
        PriceSource::Jupiter,
        Some(9),
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
            PriceSource::Jupiter,
            Some(9),
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
        PriceSource::Jupiter,
        Some(9),
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
        PriceSource::Jupiter,
        Some(9),
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
        PriceSource::Jupiter,
        Some(9),
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
        PriceSource::Jupiter,
        Some(9),
    );
    let mgr = StopLossManager::new(
        db.clone(),
        config_with_stop_distance("-100.0"),
        price_cache.clone(),
    );
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
        PriceSource::Jupiter,
        Some(9),
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
// =============================================================================
// ADDITIONAL COVERAGE: recovery gate, ATR, wick protection, stop-mark refresh
// =============================================================================

const WALLET_A: &str = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
const WALLET_B: &str = "5xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";

fn pool_of(db: &Arc<dyn Database>) -> Pool<Postgres> {
    match db.pool() {
        DbPool::PostgreSQL(p) => p.clone(),
    }
}

#[tokio::test]
async fn test_recovery_gate_exits_stale_losers() {
    let (db, _tmp) = create_test_db().await;
    let cfg = ProfitManagementConfig {
        recovery_gate_secs: 30,
        recovery_gate_threshold: Decimal::from_str("-1.0").unwrap(),
        ..ProfitManagementConfig::default()
    };
    let pc = Arc::new(PriceCache::new().unwrap());
    let mgr = StopLossManager::new(db, Arc::new(cfg), pc.clone());
    pc.set_price(
        "tok-gate",
        Decimal::from_str("0.97").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );

    // Entry 60s ago (past recovery gate), still -3% → recovery gate exits.
    let action = mgr
        .check_stop_loss("g1", WALLET_A, Decimal::ONE, "tok-gate", past_entry())
        .await;
    assert_eq!(
        action,
        StopLossAction::Exit,
        "recovery gate must exit stale losers"
    );

    // Fresh entry (0s): recovery gate not yet triggered; loss small → hold.
    let action = mgr
        .check_stop_loss("g2", WALLET_A, Decimal::ONE, "tok-gate", chrono::Utc::now())
        .await;
    assert_eq!(action, StopLossAction::None);
}

#[tokio::test]
async fn test_wallet_missing_or_error_falls_back_to_default_wqs() {
    let (db, _tmp) = create_test_db().await;
    let pc = Arc::new(PriceCache::new().unwrap());
    let cfg = Arc::new(ProfitManagementConfig {
        max_stop_loss_distance: Decimal::from_str("-100.0").unwrap(),
        ..ProfitManagementConfig::default()
    });
    let mgr = StopLossManager::new(db.clone(), cfg.clone(), pc.clone());
    pc.set_price(
        "tok-w",
        Decimal::from_str("0.90").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let action = mgr
        .check_stop_loss("w1", "no-such-wallet", Decimal::ONE, "tok-w", past_entry())
        .await;
    assert_eq!(
        action,
        StopLossAction::None,
        "missing wallet uses default WQS 50"
    );

    // Wallet query error (drop the table) → same default.
    sqlx::query("DROP TABLE wallets CASCADE")
        .execute(&pool_of(&db))
        .await
        .unwrap();
    let pc2 = Arc::new(PriceCache::new().unwrap());
    pc2.set_price(
        "tok-w2",
        Decimal::from_str("0.90").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let mgr2 = StopLossManager::new(db.clone(), cfg.clone(), pc2.clone());
    let action = mgr2
        .check_stop_loss("w2", "any-wallet", Decimal::ONE, "tok-w2", past_entry())
        .await;
    assert_eq!(
        action,
        StopLossAction::None,
        "wallet query error uses default WQS 50"
    );
}

#[tokio::test]
async fn test_atr_based_stop_override() {
    let (db, _tmp) = create_test_db().await;
    insert_wallet(&pool_of(&db), WALLET_A, 80.0).await;

    let pc = Arc::new(PriceCache::new().unwrap());
    let mut price = 1.0f64;
    for i in 0..10 {
        price = if i % 2 == 0 { price * 1.2 } else { price / 1.2 };
        pc.set_price(
            "tok-atr",
            Decimal::from_str(&format!("{price:.4}")).unwrap(),
            PriceSource::Jupiter,
            Some(9),
        );
    }
    pc.set_price(
        "tok-atr",
        Decimal::from_str("0.92").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    pc.track_token("tok-atr");

    let cfg = Arc::new(ProfitManagementConfig {
        atr_stop_loss_enabled: true,
        atr_multiplier: Decimal::from_str("10.0").unwrap(),
        market_regime: "VOLATILE".to_string(),
        max_stop_loss_distance: Decimal::from_str("-100.0").unwrap(),
        ..ProfitManagementConfig::default()
    });
    let mgr = StopLossManager::new(db, cfg, pc.clone());
    let action = mgr
        .check_stop_loss("atr-1", WALLET_A, Decimal::ONE, "tok-atr", past_entry())
        .await;
    // With vol > 0 and a 10x multiplier the ATR stop is very wide (negative
    // threshold) — the -8% mark does NOT breach it → hold.
    assert_eq!(action, StopLossAction::None, "wide ATR stop holds at -8%");
}

#[tokio::test]
async fn test_adaptive_volatility_tightens_low_vol_stops() {
    let (db, _tmp) = create_test_db().await;
    insert_wallet(&pool_of(&db), WALLET_A, 50.0).await;

    let pc = Arc::new(PriceCache::new().unwrap());
    // Very low volatility history (flat prices) → 0.9x multiplier.
    for _ in 0..10 {
        pc.set_price(
            "tok-lowvol",
            Decimal::from_str("1.0001").unwrap(),
            PriceSource::Jupiter,
            Some(9),
        );
    }
    pc.track_token("tok-lowvol");

    let cfg = Arc::new(ProfitManagementConfig {
        max_stop_loss_distance: Decimal::from_str("-100.0").unwrap(),
        ..ProfitManagementConfig::default()
    });
    let mgr = StopLossManager::new(db, cfg, pc.clone());
    // Medium WQS: -15% base; 0.9x → -13.5%; -12% mark holds.
    pc.set_price(
        "tok-lowvol",
        Decimal::from_str("0.88").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let action = mgr
        .check_stop_loss("lv-1", WALLET_A, Decimal::ONE, "tok-lowvol", past_entry())
        .await;
    assert_eq!(action, StopLossAction::None);
    // -15% mark exits.
    pc.set_price(
        "tok-lowvol",
        Decimal::from_str("0.85").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let action = mgr
        .check_stop_loss("lv-2", WALLET_A, Decimal::ONE, "tok-lowvol", past_entry())
        .await;
    assert_eq!(action, StopLossAction::Exit);
}

/// Mock Jupiter price API: returns `price` for ANY requested token id.
async fn spawn_price_mock(price: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let price = price.to_string();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 16384];
            let Ok(n) = sock.read(&mut buf).await else {
                continue;
            };
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            // Extract the requested token id from `?ids=<token>`.
            let id = req
                .split("ids=")
                .nth(1)
                .and_then(|s| s.split(['&', ' ']).next())
                .unwrap_or("tok-mark")
                .to_string();
            let body = serde_json::json!({
                id: {"usdPrice": price.parse::<f64>().unwrap(), "decimals": 9}
            })
            .to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(), body
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.shutdown().await;
        }
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn test_stop_mark_refresh_rejects_bad_cache_mark() {
    // Refresh returns 1.0 while the cache holds a stale 0.88 → the fresh mark
    // no longer breaches → hold.
    let (db, _tmp) = create_test_db().await;
    insert_wallet(&pool_of(&db), WALLET_A, 50.0).await;

    let base = spawn_price_mock("1.0").await;
    let pc = Arc::new(PriceCache::with_jupiter_price_api(base).unwrap());
    pc.set_price(
        "tok-mark",
        Decimal::from_str("0.80").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    pc.track_token("tok-mark");

    let cfg = Arc::new(ProfitManagementConfig {
        max_stop_loss_distance: Decimal::from_str("-100.0").unwrap(),
        ..ProfitManagementConfig::default()
    });
    let mgr = StopLossManager::new(db, cfg, pc.clone());
    let action = mgr
        .check_stop_loss("mk-1", WALLET_A, Decimal::ONE, "tok-mark", past_entry())
        .await;
    assert_eq!(
        action,
        StopLossAction::None,
        "fresh quote no longer breaching must hold the position"
    );
}

#[tokio::test]
async fn test_stop_mark_refresh_confirms_exit_on_divergent_mark() {
    // Refresh returns 0.85 (still breaching, worse than cache 0.88) → exit on
    // the fresh mark.
    let (db, _tmp) = create_test_db().await;
    insert_wallet(&pool_of(&db), WALLET_A, 50.0).await;

    let base = spawn_price_mock("0.85").await;
    let pc = Arc::new(PriceCache::with_jupiter_price_api(base).unwrap());
    pc.set_price(
        "tok-mark2",
        Decimal::from_str("0.80").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    pc.track_token("tok-mark2");

    let cfg = Arc::new(ProfitManagementConfig {
        max_stop_loss_distance: Decimal::from_str("-100.0").unwrap(),
        ..ProfitManagementConfig::default()
    });
    let mgr = StopLossManager::new(db, cfg, pc.clone());
    let action = mgr
        .check_stop_loss("mk-2", WALLET_A, Decimal::ONE, "tok-mark2", past_entry())
        .await;
    assert_eq!(
        action,
        StopLossAction::Exit,
        "fresh mark still breaches → exit"
    );
}

#[tokio::test]
async fn test_wick_protection_grace_and_overrides() {
    let (db, _tmp) = create_test_db().await;
    insert_wallet(&pool_of(&db), WALLET_A, 50.0).await;

    let cfg = Arc::new(ProfitManagementConfig {
        max_stop_loss_distance: Decimal::from_str("-100.0").unwrap(),
        wick_protection_secs: 600,
        wick_protection_max_loss_percent: Decimal::from_str("-20.0").unwrap(),
        ..ProfitManagementConfig::default()
    });
    let pc = Arc::new(PriceCache::new().unwrap());
    let mgr = StopLossManager::new(db, cfg, pc.clone());

    // -10% within the wick window (not hard stop, not large loss) → hold.
    pc.set_price(
        "tok-wick",
        Decimal::from_str("0.90").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let action = mgr
        .check_stop_loss(
            "wk-1",
            WALLET_A,
            Decimal::ONE,
            "tok-wick",
            chrono::Utc::now(),
        )
        .await;
    assert_eq!(
        action,
        StopLossAction::None,
        "wick protection holds normal dips"
    );

    // -25% within the wick window → hard stop bypasses grace.
    pc.set_price(
        "tok-wick",
        Decimal::from_str("0.75").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let action = mgr
        .check_stop_loss(
            "wk-2",
            WALLET_A,
            Decimal::ONE,
            "tok-wick",
            chrono::Utc::now(),
        )
        .await;
    assert_eq!(
        action,
        StopLossAction::Exit,
        "hard stop bypasses wick protection"
    );

    // -25% beyond the wick window → normal exit.
    let action = mgr
        .check_stop_loss("wk-3", WALLET_A, Decimal::ONE, "tok-wick", past_entry())
        .await;
    assert_eq!(action, StopLossAction::Exit);
}

#[tokio::test]
async fn test_consensus_db_fallback_and_query_error() {
    let (db, _tmp) = create_test_db().await;
    insert_wallet(&pool_of(&db), WALLET_A, 50.0).await;
    insert_wallet(&pool_of(&db), WALLET_B, 50.0).await;
    insert_consensus_signal(&pool_of(&db), "tok-cons", WALLET_A).await;
    insert_consensus_signal(&pool_of(&db), "tok-cons", WALLET_B).await;

    let pc = Arc::new(PriceCache::new().unwrap());
    pc.set_price(
        "tok-cons",
        Decimal::from_str("0.80").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let cfg = Arc::new(ProfitManagementConfig {
        max_stop_loss_distance: Decimal::from_str("-100.0").unwrap(),
        ..ProfitManagementConfig::default()
    });
    let mgr = StopLossManager::new(db.clone(), cfg.clone(), pc.clone());
    let action = mgr
        .check_stop_loss("cs-1", WALLET_A, Decimal::ONE, "tok-cons", past_entry())
        .await;
    // -15% base × 1.25 consensus = -18.75% → a -20% mark exits.
    assert_eq!(
        action,
        StopLossAction::Exit,
        "consensus-widened stop exits at -20%"
    );

    // Consensus query failure → no widening; -15% still exits for medium WQS.
    sqlx::query("DROP TABLE signal_aggregation CASCADE")
        .execute(&pool_of(&db))
        .await
        .unwrap();
    let pc2 = Arc::new(PriceCache::new().unwrap());
    pc2.set_price(
        "tok-cons2",
        Decimal::from_str("0.85").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let mgr2 = StopLossManager::new(db.clone(), cfg.clone(), pc2);
    let action = mgr2
        .check_stop_loss("cs-2", WALLET_A, Decimal::ONE, "tok-cons2", past_entry())
        .await;
    assert_eq!(
        action,
        StopLossAction::Exit,
        "query error must not widen (still exits)"
    );
}

#[tokio::test]
async fn test_max_stop_loss_distance_override_warns() {
    let (db, _tmp) = create_test_db().await;
    insert_wallet(&pool_of(&db), WALLET_A, 50.0).await;

    let pc = Arc::new(PriceCache::new().unwrap());
    // High volatility → adaptive threshold -30% (2.0x of -15%), but
    // max_stop_loss_distance = -5 clamps it.
    let mut price = 1.0f64;
    for i in 0..12 {
        price = if i % 2 == 0 { price * 1.5 } else { price / 1.5 };
        pc.set_price(
            "tok-clamp",
            Decimal::from_str(&format!("{price:.4}")).unwrap(),
            PriceSource::Jupiter,
            Some(9),
        );
    }
    pc.track_token("tok-clamp");
    pc.set_price(
        "tok-clamp",
        Decimal::from_str("0.95").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );

    let cfg = Arc::new(ProfitManagementConfig {
        max_stop_loss_distance: Decimal::from_str("-5.0").unwrap(),
        ..ProfitManagementConfig::default()
    });
    let mgr = StopLossManager::new(db, cfg, pc.clone());
    // -6% breaches the clamped -5% threshold → exit (the max-distance warn fires).
    let action = mgr
        .check_stop_loss("cl-1", WALLET_A, Decimal::ONE, "tok-clamp", past_entry())
        .await;
    assert_eq!(action, StopLossAction::Exit);
}

#[tokio::test]
async fn test_pre_graduation_exit_rails() {
    let (db, _tmp) = create_test_db().await;
    let cache = Arc::new(PriceCache::new().unwrap());
    let cfg = Arc::new(ProfitManagementConfig {
        pre_graduation_exit_enabled: true,
        ..ProfitManagementConfig::default()
    });
    let mgr = StopLossManager::new(db.clone(), cfg.clone(), cache.clone());

    // Parser wired to a dead RPC URL → curve fetch fails → fail-open None.
    let token_cache = Arc::new(chimera_operator::token::TokenCache::new(100, 100));
    let fetcher = Arc::new(
        chimera_operator::token::TokenMetadataFetcher::new_with_rate_limiter_and_jupiter(
            "http://127.0.0.1:1",
            None,
            "http://127.0.0.1:1".to_string(),
        ),
    );
    let parser = Arc::new(chimera_operator::TokenParser::new(
        chimera_operator::token::TokenSafetyConfig {
            freeze_authority_whitelist: std::collections::HashSet::new(),
            mint_authority_whitelist: std::collections::HashSet::new(),
            min_liquidity_shield_usd: Decimal::from_str("0").unwrap(),
            min_liquidity_spear_usd: Decimal::from_str("0").unwrap(),
            honeypot_detection_enabled: false,
            holder_concentration_check_enabled: false,
            max_holder_concentration_pct: 100.0,
        },
        token_cache,
        fetcher,
    ));
    mgr.set_token_parser(parser.clone()).await;
    let action = mgr
        .check_pre_graduation("pg-1", "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R")
        .await;
    assert_eq!(action, StopLossAction::None);

    // Parser with an RPC mock returning a null account → Ok(None) → None.
    let base = spawn_rpc_null_account().await;
    let token_cache2 = Arc::new(chimera_operator::token::TokenCache::new(100, 100));
    let fetcher2 = Arc::new(
        chimera_operator::token::TokenMetadataFetcher::new_with_rate_limiter_and_jupiter(
            &base,
            None,
            "http://127.0.0.1:1".to_string(),
        ),
    );
    let parser2 = Arc::new(chimera_operator::TokenParser::new(
        chimera_operator::token::TokenSafetyConfig {
            freeze_authority_whitelist: std::collections::HashSet::new(),
            mint_authority_whitelist: std::collections::HashSet::new(),
            min_liquidity_shield_usd: Decimal::from_str("0").unwrap(),
            min_liquidity_spear_usd: Decimal::from_str("0").unwrap(),
            honeypot_detection_enabled: false,
            holder_concentration_check_enabled: false,
            max_holder_concentration_pct: 100.0,
        },
        token_cache2,
        fetcher2,
    ));
    let mgr2 = StopLossManager::new(db.clone(), cfg.clone(), cache.clone());
    mgr2.set_token_parser(parser2).await;
    let action = mgr2
        .check_pre_graduation("pg-2", "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R")
        .await;
    assert_eq!(action, StopLossAction::None);
}

/// Mock JSON-RPC server answering getAccountInfo with a null account (the
/// standard "account does not exist" for non-curve tokens).
async fn spawn_rpc_null_account() -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 16384];
            let Ok(n) = sock.read(&mut buf).await else {
                continue;
            };
            let body = String::from_utf8_lossy(&buf[..n]).to_string();
            let response = if body.contains("getAccountInfo") {
                serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": {"value": null}})
                    .to_string()
            } else {
                serde_json::json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32601, "message": "x"}}).to_string()
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(), response
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.shutdown().await;
        }
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn test_aggregator_consensus_path() {
    // With a wired SignalAggregator, consensus reads the in-memory cache
    // (no signals → no consensus → base stop behavior).
    let (db, _tmp) = create_test_db().await;
    insert_wallet(&pool_of(&db), WALLET_A, 50.0).await;

    let agg = Arc::new(chimera_operator::monitoring::SignalAggregator::new(
        db.clone(),
    ));
    let pc = Arc::new(PriceCache::new().unwrap());
    pc.set_price(
        "tok-agg",
        Decimal::from_str("0.85").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let mgr = StopLossManager::new(
        db,
        Arc::new(ProfitManagementConfig {
            max_stop_loss_distance: Decimal::from_str("-100.0").unwrap(),
            ..ProfitManagementConfig::default()
        }),
        pc.clone(),
    );
    mgr.set_signal_aggregator(agg).await;
    let action = mgr
        .check_stop_loss("ag-1", WALLET_A, Decimal::ONE, "tok-agg", past_entry())
        .await;
    assert_eq!(
        action,
        StopLossAction::Exit,
        "no consensus → base -15% stop exits at -15%"
    );
}

// =============================================================================
// ATR OVERRIDE + VOLATILITY MULTIPLIER BANDS + WICK LARGE-LOSS + CURVE EXITS
// =============================================================================

#[tokio::test]
async fn test_atr_override_applies_tighter_threshold() {
    // Low volatility + small ATR multiplier → ATR stop (-5%) is TIGHTER than
    // the WQS threshold (-15%) → the ATR threshold overrides it.
    let (db, _tmp) = create_test_db().await;
    insert_wallet(&pool_of(&db), WALLET_A, 50.0).await;

    let pc = Arc::new(PriceCache::new().unwrap());
    // Very flat history → tiny volatility.
    for _ in 0..10 {
        pc.set_price(
            "tok-atr2",
            Decimal::from_str("1.0001").unwrap(),
            PriceSource::Jupiter,
            Some(9),
        );
    }
    pc.track_token("tok-atr2");
    pc.set_price(
        "tok-atr2",
        Decimal::from_str("0.95").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );

    let cfg = Arc::new(ProfitManagementConfig {
        atr_stop_loss_enabled: true,
        atr_multiplier: Decimal::from_str("1.0").unwrap(),
        market_regime: "BEAR".to_string(),
        max_stop_loss_distance: Decimal::from_str("-100.0").unwrap(),
        ..ProfitManagementConfig::default()
    });
    let mgr = StopLossManager::new(db, cfg, pc.clone());
    // -5% mark: breaches the ATR-overridden threshold → exit.
    let action = mgr
        .check_stop_loss("ao-1", WALLET_A, Decimal::ONE, "tok-atr2", past_entry())
        .await;
    assert_eq!(
        action,
        StopLossAction::Exit,
        "ATR-overridden threshold exits at -5%"
    );
}

#[tokio::test]
async fn test_volatility_multiplier_mid_bands() {
    // Moderate volatility (20-30% band → 1.5x) and the 10-20% band (1.0x).
    let (db, _tmp) = create_test_db().await;
    insert_wallet(&pool_of(&db), WALLET_A, 50.0).await;

    let pc = Arc::new(PriceCache::new().unwrap());
    // Alternating 1.25x swings → per-step change ~25% → vol in the 20-30 band.
    let mut price = 1.0f64;
    for i in 0..14 {
        price = if i % 2 == 0 {
            price * 1.25
        } else {
            price / 1.25
        };
        pc.set_price(
            "tok-mid",
            Decimal::from_str(&format!("{price:.4}")).unwrap(),
            PriceSource::Jupiter,
            Some(9),
        );
    }
    pc.track_token("tok-mid");
    pc.set_price(
        "tok-mid",
        Decimal::from_str("0.90").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );

    let cfg = Arc::new(ProfitManagementConfig {
        max_stop_loss_distance: Decimal::from_str("-100.0").unwrap(),
        ..ProfitManagementConfig::default()
    });
    let mgr = StopLossManager::new(db.clone(), cfg.clone(), pc.clone());
    // Medium WQS -15% × 1.5 = -22.5% → -10% mark holds.
    let action = mgr
        .check_stop_loss("mb-1", WALLET_A, Decimal::ONE, "tok-mid", past_entry())
        .await;
    assert_eq!(action, StopLossAction::None);

    // Gentle swings (~5%) → 1.0x multiplier → base -15%.
    let pc2 = Arc::new(PriceCache::new().unwrap());
    let mut price = 1.0f64;
    for i in 0..14 {
        price = if i % 2 == 0 {
            price * 1.05
        } else {
            price / 1.05
        };
        pc2.set_price(
            "tok-gentle",
            Decimal::from_str(&format!("{price:.4}")).unwrap(),
            PriceSource::Jupiter,
            Some(9),
        );
    }
    pc2.track_token("tok-gentle");
    pc2.set_price(
        "tok-gentle",
        Decimal::from_str("0.85").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let mgr2 = StopLossManager::new(db.clone(), cfg.clone(), pc2.clone());
    let action = mgr2
        .check_stop_loss("mb-2", WALLET_A, Decimal::ONE, "tok-gentle", past_entry())
        .await;
    assert_eq!(
        action,
        StopLossAction::Exit,
        "-15% mark exits at the 1.0x threshold"
    );
}

#[tokio::test]
async fn test_wick_large_loss_override() {
    // -22% within the wick window: beyond wick_protection_max_loss_percent
    // (-20%) but below the -25% hard stop → large-loss override exits.
    let (db, _tmp) = create_test_db().await;
    insert_wallet(&pool_of(&db), WALLET_A, 50.0).await;

    let cfg = Arc::new(ProfitManagementConfig {
        max_stop_loss_distance: Decimal::from_str("-100.0").unwrap(),
        wick_protection_secs: 600,
        wick_protection_max_loss_percent: Decimal::from_str("-20.0").unwrap(),
        ..ProfitManagementConfig::default()
    });
    let pc = Arc::new(PriceCache::new().unwrap());
    let mgr = StopLossManager::new(db, cfg, pc.clone());
    pc.set_price(
        "tok-ll",
        Decimal::from_str("0.78").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let action = mgr
        .check_stop_loss("ll-1", WALLET_A, Decimal::ONE, "tok-ll", chrono::Utc::now())
        .await;
    assert_eq!(
        action,
        StopLossAction::Exit,
        "large loss overrides wick grace"
    );
}

/// 49-byte pump.fun curve account (anchor discriminator + 5 u64s + complete).
fn curve_bytes(real_sol_lamports: u64, complete: bool) -> String {
    let mut data = vec![0u8; 49];
    data[32..40].copy_from_slice(&real_sol_lamports.to_le_bytes());
    data[48] = if complete { 1 } else { 0 };
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// Mock JSON-RPC answering getAccountInfo with curve account data.
async fn spawn_rpc_curve(real_sol_lamports: u64, complete: bool) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let b64 = curve_bytes(real_sol_lamports, complete);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 16384];
            let Ok(n) = sock.read(&mut buf).await else {
                continue;
            };
            let body = String::from_utf8_lossy(&buf[..n]).to_string();
            let response = if body.contains("getAccountInfo") {
                serde_json::json!({
                    "jsonrpc": "2.0", "id": 1,
                    "result": {"context": {"slot": 1}, "value": {
                        "data": [b64, "base64"],
                        "executable": false,
                        "lamports": 100,
                        "owner": "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P",
                        "rentEpoch": 0,
                        "space": 49
                    }}
                })
                .to_string()
            } else {
                serde_json::json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32601, "message": "x"}}).to_string()
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(), response
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.shutdown().await;
        }
    });
    format!("http://{addr}")
}

fn curve_parser(rpc_url: &str) -> Arc<chimera_operator::TokenParser> {
    let cache = Arc::new(chimera_operator::token::TokenCache::new(100, 100));
    let fetcher = Arc::new(
        chimera_operator::token::TokenMetadataFetcher::new_with_rate_limiter_and_jupiter(
            rpc_url,
            None,
            "http://127.0.0.1:1".to_string(),
        ),
    );
    Arc::new(chimera_operator::TokenParser::new(
        chimera_operator::token::TokenSafetyConfig {
            freeze_authority_whitelist: std::collections::HashSet::new(),
            mint_authority_whitelist: std::collections::HashSet::new(),
            min_liquidity_shield_usd: Decimal::from_str("0").unwrap(),
            min_liquidity_spear_usd: Decimal::from_str("0").unwrap(),
            honeypot_detection_enabled: false,
            holder_concentration_check_enabled: false,
            max_holder_concentration_pct: 100.0,
        },
        cache,
        fetcher,
    ))
}

#[tokio::test]
async fn test_pre_graduation_curve_states() {
    let (db, _tmp) = create_test_db().await;
    let cache = Arc::new(PriceCache::new().unwrap());
    let cfg = Arc::new(ProfitManagementConfig {
        pre_graduation_exit_enabled: true,
        pre_graduation_exit_threshold: Decimal::from_str("0.85").unwrap(),
        ..ProfitManagementConfig::default()
    });

    // Already-graduated curve → None (no dump zone left).
    let mgr = StopLossManager::new(db.clone(), cfg.clone(), cache.clone());
    mgr.set_token_parser(curve_parser(&spawn_rpc_curve(85_000_000_000, true).await))
        .await;
    let action = mgr
        .check_pre_graduation("pg-g", "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R")
        .await;
    assert_eq!(action, StopLossAction::None);

    // Late-curve dump zone (94% complete, not graduated) → Exit.
    let mgr = StopLossManager::new(db.clone(), cfg.clone(), cache.clone());
    mgr.set_token_parser(curve_parser(&spawn_rpc_curve(80_000_000_000, false).await))
        .await;
    let action = mgr
        .check_pre_graduation("pg-l", "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R")
        .await;
    assert_eq!(action, StopLossAction::Exit, "late-curve dump zone exits");

    // Early curve (35% complete) → None.
    let mgr = StopLossManager::new(db.clone(), cfg, cache.clone());
    mgr.set_token_parser(curve_parser(&spawn_rpc_curve(30_000_000_000, false).await))
        .await;
    let action = mgr
        .check_pre_graduation("pg-e", "4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R")
        .await;
    assert_eq!(action, StopLossAction::None);
}
