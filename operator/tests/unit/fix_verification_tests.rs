//! Fix-Verification Tests
//!
//! Each test asserts the CORRECT (post-fix) behavior for a documented bug.
//! A FAILING test = the bug is not yet fixed.
//! A PASSING test = the fix is in place and has not regressed.
//!
//! Bugs covered:
//!   F3/F7 — Hard stop sign bug: default hard_stop_loss=15.0 (positive) fires on ALL losses
//!   F4    — Trailing stop ratchet: stop_price never updates after initial activation
//!   F6    — Silent status update: update_trade_status returns Ok on non-existent UUID

use chimera_operator::engine::profit_targets::{ProfitTargetManager, ProfitTargetAction};
use chimera_operator::engine::stop_loss::{StopLossManager, StopLossAction};
use chimera_operator::config::{ProfitManagementConfig, DatabaseConfig};
use chimera_operator::db::{init_pool, run_migrations, update_trade_status, insert_trade};
use chimera_operator::price_cache::{PriceCache, PriceSource};
use rust_decimal::Decimal;
use std::str::FromStr;
use std::sync::Arc;
use tempfile::TempDir;

// ─── helpers ─────────────────────────────────────────────────────────────────

async fn create_test_db() -> (chimera_operator::db::DbPool, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let db_config = DatabaseConfig {
        path: temp_dir.path().join("fix_verification_test.db"),
        max_connections: 5,
    };
    let pool = init_pool(&db_config).await.unwrap();
    run_migrations(&pool).await.unwrap();
    (pool, temp_dir)
}

fn config_with_hard_stop(hard_stop: &str) -> Arc<ProfitManagementConfig> {
    Arc::new(ProfitManagementConfig {
        hard_stop_loss: Decimal::from_str(hard_stop).unwrap(),
        ..ProfitManagementConfig::default()
    })
}

/// Insert a wallet row so stop_loss WQS lookup succeeds (returns WQS 70 → -20% threshold).
async fn insert_wallet(pool: &chimera_operator::db::DbPool, address: &str, wqs: f64) {
    sqlx::query(
        "INSERT INTO wallets (address, status, wqs_score, created_at, updated_at) \
         VALUES (?, 'ACTIVE', ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)"
    )
    .bind(address)
    .bind(wqs)
    .execute(pool)
    .await
    .unwrap();
}

/// Insert a trade row so update_trade_status has something real to update.
async fn seed_trade(pool: &chimera_operator::db::DbPool, trade_uuid: &str) {
    insert_trade(
        pool, trade_uuid, "wallet_fix", "token_fix", Some("FIX"), "SHIELD", "BUY",
        Decimal::from_str("1.0").unwrap(), "PENDING",
    )
    .await
    .unwrap();
}

// ─── F3/F7: Hard stop sign bug ───────────────────────────────────────────────

#[tokio::test]
async fn should_not_fire_hard_stop_at_2pct_loss_with_default_config() {
    // BUG (F7): default hard_stop_loss = Decimal::from_str("15.0") (positive).
    // Check: loss_percent <= hard_stop → -2.0 <= 15.0 → TRUE → exits EVERY losing position.
    //
    // Fix: hard_stop_loss default should be -15.0 (negative), OR the comparison
    // should negate the config value: loss_percent <= -hard_stop_loss.
    //
    // This test asserts the CORRECT behavior: a 2% loss must NOT trigger the hard stop.
    // It FAILS while the bug exists, and PASSES after the fix.

    let (pool, _tmp) = create_test_db().await;
    let price_cache = Arc::new(PriceCache::new());
    const TOKEN: &str = "token_hard_stop_fix";
    const WALLET: &str = "wallet_hard_stop_fix";

    insert_wallet(&pool, WALLET, 75.0).await;

    // Use DEFAULT config (hard_stop_loss = 15.0 with the bug, should be -15.0 after fix)
    let cfg = Arc::new(ProfitManagementConfig::default());
    let mgr = StopLossManager::new(pool, cfg, price_cache.clone());

    // Entry = $100, Current = $98 → loss = -2%
    // Dynamic threshold at WQS=75: -20% (not hit at -2%)
    // Hard stop at -15% (after fix): not hit at -2%
    // Hard stop at +15.0 (with bug): -2.0 <= 15.0 → EXIT fires → BUG
    price_cache.set_price(TOKEN, Decimal::from_str("100.00").unwrap(), PriceSource::Jupiter);
    price_cache.set_price(TOKEN, Decimal::from_str("98.00").unwrap(), PriceSource::Jupiter);

    let action = mgr.check_stop_loss(
        "uuid-hard-stop-fix",
        WALLET,
        Decimal::from_str("100.00").unwrap(),
        TOKEN,
    ).await;

    assert_eq!(
        action,
        StopLossAction::None,
        "A 2% loss must NOT trigger the hard stop (threshold is -15%, not +15%). \
         BUG: hard_stop_loss default is 15.0 (positive) causing it to fire on any negative loss_percent."
    );
}

#[tokio::test]
async fn should_fire_hard_stop_at_16pct_loss_with_default_config() {
    // Companion to the test above: after fixing the sign, the hard stop SHOULD fire
    // when loss exceeds -15% (e.g., at -16%).
    //
    // With the bug: fires at -2% (too early).
    // After fix: fires at -16% (correct).

    let (pool, _tmp) = create_test_db().await;
    let price_cache = Arc::new(PriceCache::new());
    const TOKEN: &str = "token_hard_stop_16";
    const WALLET: &str = "wallet_hard_stop_16";

    insert_wallet(&pool, WALLET, 30.0).await; // Low WQS → dynamic threshold = -10%

    // With low WQS (threshold = -10%), the dynamic stop fires first at -10%.
    // Use a high WQS so dynamic threshold = -20%, making hard stop (-15%) fire first.
    sqlx::query(
        "UPDATE wallets SET wqs_score = 75.0 WHERE address = ?"
    )
    .bind(WALLET)
    .execute(&pool)
    .await
    .unwrap();

    let cfg = Arc::new(ProfitManagementConfig::default());
    let mgr = StopLossManager::new(pool, cfg, price_cache.clone());

    // Entry = $100, Current = $84 → loss = -16% (exceeds hard stop of -15%)
    price_cache.set_price(TOKEN, Decimal::from_str("84.00").unwrap(), PriceSource::Jupiter);

    let action = mgr.check_stop_loss(
        "uuid-hard-stop-16",
        WALLET,
        Decimal::from_str("100.00").unwrap(),
        TOKEN,
    ).await;

    assert_eq!(
        action,
        StopLossAction::Exit,
        "A 16% loss must trigger the hard stop (-15% threshold). \
         After fix: loss_percent=-16.0 <= hard_stop=-15.0 → Exit."
    );
}

// ─── F4: Trailing stop ratchet ───────────────────────────────────────────────

#[tokio::test]
async fn should_ratchet_trailing_stop_price_as_peak_rises() {
    // BUG (F4): profit_targets.rs ~L216 checks `current_price > peak_price` AFTER
    // peak_price was already set to current_price → condition is always FALSE.
    // Result: trailing_stop_price never ratchets up after initial activation.
    //
    // Sequence:
    //   Entry $1.00
    //   Price $1.20 (+20%): trailing activates (activation=10%), stop locks at $0.96
    //   Price $2.00 (+100%): new peak → correct stop = $2.00 × 0.80 = $1.60
    //   Price $1.40: below $1.60 ratcheted stop → should Exit
    //
    // With bug: stop stays at $0.96 → $1.40 > $0.96 → no exit (loses $0.60/SOL of gain)
    // After fix: stop ratchets to $1.60 → $1.40 < $1.60 → Exit (correct capital protection)

    let (pool, _tmp) = create_test_db().await;
    let price_cache = Arc::new(PriceCache::new());
    const TOKEN: &str = "token_ratchet_fix";

    let cfg = Arc::new(ProfitManagementConfig {
        targets: vec![],
        trailing_stop_activation: Decimal::from_str("10.0").unwrap(),
        trailing_stop_distance: Decimal::from_str("20.0").unwrap(),
        ..ProfitManagementConfig::default()
    });
    let mgr = ProfitTargetManager::new(pool, cfg, price_cache.clone());

    price_cache.set_price(TOKEN, Decimal::from_str("1.00").unwrap(), PriceSource::Jupiter);
    mgr.register_position(
        "uuid-ratchet-fix",
        Decimal::from_str("1.00").unwrap(),
        Decimal::from_str("5.0").unwrap(),
        TOKEN,
    ).await;

    // Activate trailing stop at $1.20
    price_cache.set_price(TOKEN, Decimal::from_str("1.20").unwrap(), PriceSource::Jupiter);
    let _ = mgr.check_targets("uuid-ratchet-fix", TOKEN).await;

    // New peak at $2.00 → correct ratcheted stop = $1.60
    price_cache.set_price(TOKEN, Decimal::from_str("2.00").unwrap(), PriceSource::Jupiter);
    let _ = mgr.check_targets("uuid-ratchet-fix", TOKEN).await;

    // Price falls to $1.40 — below ratcheted stop $1.60 → must Exit
    price_cache.set_price(TOKEN, Decimal::from_str("1.40").unwrap(), PriceSource::Jupiter);
    let action = mgr.check_targets("uuid-ratchet-fix", TOKEN).await;

    assert!(
        matches!(action, ProfitTargetAction::FullExit),
        "After ratchet fix: price $1.40 < ratcheted trailing stop $1.60 must trigger FullExit. \
         BUG: stop_price locked at $0.96 (activation-time peak) instead of ratcheting to $1.60."
    );
}

// ─── F6: Silent status update ─────────────────────────────────────────────────

#[tokio::test]
async fn should_return_error_on_status_update_for_missing_uuid() {
    // BUG (F6): update_trade_status executes UPDATE ... WHERE trade_uuid=?
    // and calls .execute() which returns QueryResult with rows_affected().
    // The current code does NOT check rows_affected() — returns Ok(()) even
    // when UUID does not exist, causing phantom state transitions.
    //
    // Fix: after execute(), check rows_affected() == 0 → return Err(AppError::NotFound)
    //
    // This test calls update_trade_status with a nonexistent UUID and asserts Err.
    // It FAILS while the bug exists (returns Ok), PASSES after the fix.

    let (pool, _tmp) = create_test_db().await;

    let result = update_trade_status(
        &pool,
        "00000000-0000-0000-0000-nonexistent00",
        "QUEUED",
        None,
        None,
    ).await;

    assert!(
        result.is_err(),
        "update_trade_status must return Err when the trade_uuid does not exist. \
         BUG: currently returns Ok(()) silently, allowing phantom state transitions."
    );
}

#[tokio::test]
async fn should_succeed_on_status_update_for_existing_trade() {
    // Complement to F6: verify the fix does not break the happy path.
    // A real UUID must still return Ok(()).

    let (pool, _tmp) = create_test_db().await;
    let uuid = "aaaabbbb-cccc-dddd-eeee-ffffffffffff";
    seed_trade(&pool, uuid).await;

    let result = update_trade_status(&pool, uuid, "QUEUED", None, None).await;

    assert!(
        result.is_ok(),
        "update_trade_status must return Ok for an existing trade UUID"
    );

    // Verify status was actually changed
    let status: String = sqlx::query_scalar("SELECT status FROM trades WHERE trade_uuid = ?")
        .bind(uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "QUEUED", "Trade status must be updated to QUEUED");
}
