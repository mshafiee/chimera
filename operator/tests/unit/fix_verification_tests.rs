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

use chimera_operator::config::ProfitManagementConfig;
use chimera_operator::db_abstraction::{
    create_database, Database, DatabaseConfig, DbPool, InsertTrade, UpdateTradeStatus,
};
use chimera_operator::engine::profit_targets::{ProfitTargetAction, ProfitTargetManager};
use chimera_operator::engine::stop_loss::{StopLossAction, StopLossManager};
use chimera_operator::price_cache::{PriceCache, PriceSource};
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

fn past_entry() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now() - chrono::TimeDelta::seconds(60)
}

// ─── helpers ─────────────────────────────────────────────────────────────────

async fn create_test_db() -> (Arc<dyn Database>, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let config = DatabaseConfig::sqlite(temp_dir.path().join("fix_verification_test.db"));
    let db = create_database(&config).await.unwrap();
    db.run_migrations().await.unwrap();
    (db, temp_dir)
}

#[allow(dead_code)]
fn config_with_hard_stop(hard_stop: &str) -> Arc<ProfitManagementConfig> {
    Arc::new(ProfitManagementConfig {
        max_stop_loss_distance: Decimal::from_str(hard_stop).unwrap(),
        ..ProfitManagementConfig::default()
    })
}

/// Insert a wallet row so stop_loss WQS lookup succeeds (returns WQS 70 → -20% threshold).
async fn insert_wallet(pool: &Pool<Sqlite>, address: &str, wqs: f64) {
    sqlx::query(
        "INSERT INTO wallets (address, status, wqs_score, created_at, updated_at) \
         VALUES (?, 'ACTIVE', ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
    )
    .bind(address)
    .bind(wqs)
    .execute(pool)
    .await
    .unwrap();
}

/// Insert a trade row so update_trade_status has something real to update.
async fn seed_trade(db: &Arc<dyn Database>, trade_uuid: &str) {
    db.insert_trade(&InsertTrade {
        trade_uuid: trade_uuid.to_string(),
        wallet_address: "wallet_fix".to_string(),
        token_address: "token_fix".to_string(),
        token_symbol: Some("FIX".to_string()),
        strategy: "SHIELD".to_string(),
        side: "BUY".to_string(),
        amount_sol: Decimal::from_str("1.0").unwrap(),
        status: "PENDING".to_string(),
    })
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

    let (db, _tmp) = create_test_db().await;
    let pool = sqlite_pool(&db);
    let price_cache = Arc::new(PriceCache::new().unwrap());
    const TOKEN: &str = "token_hard_stop_fix";
    const WALLET: &str = "wallet_hard_stop_fix";

    insert_wallet(&pool, WALLET, 75.0).await;

    // Use DEFAULT config (hard_stop_loss = 15.0 with the bug, should be -15.0 after fix)
    let cfg = Arc::new(ProfitManagementConfig::default());
    let mgr = StopLossManager::new(db, cfg, price_cache.clone());

    // Entry = $100, Current = $98 → loss = -2%
    // Dynamic threshold at WQS=75: -20% (not hit at -2%)
    // Hard stop at -15% (after fix): not hit at -2%
    // Hard stop at +15.0 (with bug): -2.0 <= 15.0 → EXIT fires → BUG
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("100.00").unwrap(),
        PriceSource::Jupiter,
    );
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("98.00").unwrap(),
        PriceSource::Jupiter,
    );

    let action = mgr
        .check_stop_loss(
            "uuid-hard-stop-fix",
            WALLET,
            Decimal::from_str("100.00").unwrap(),
            TOKEN,
            past_entry(),
        )
        .await;

    assert_eq!(
        action,
        StopLossAction::None,
        "A 2% loss must NOT trigger the hard stop (threshold is -15%, not +15%). \
         BUG: hard_stop_loss default is 15.0 (positive) causing it to fire on any negative loss_percent."
    );
}

#[tokio::test]
async fn should_fire_dynamic_stop_at_21pct_loss_for_high_wqs_wallet() {
    // With hard_stop_loss default changed to -25%, the WQS-based dynamic stop now has room
    // to operate:
    //   - WQS=75 → dynamic base = -20%
    //   - effective_threshold = max(-20, -25) = -20%  (hard stop no longer overrides)
    //
    // Scenario A: -16% loss → -16% > -20% → no exit (dynamic stop not yet reached)
    // Scenario B: -21% loss → -21% <= -20% → Exit   (dynamic stop fires correctly)

    let (db, _tmp) = create_test_db().await;
    let pool = sqlite_pool(&db);
    let price_cache = Arc::new(PriceCache::new().unwrap());
    const TOKEN: &str = "token_dynamic_stop_21";
    const WALLET: &str = "wallet_dynamic_stop_21";

    insert_wallet(&pool, WALLET, 75.0).await; // High WQS → dynamic threshold = -20%

    let cfg = Arc::new(ProfitManagementConfig::default()); // hard_stop = -25%
    let mgr = StopLossManager::new(db, cfg, price_cache.clone());

    // Scenario A: Entry = $100, Current = $84 → loss = -16% (not past -20% dynamic stop)
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("84.00").unwrap(),
        PriceSource::Jupiter,
    );
    let action_a = mgr
        .check_stop_loss(
            "uuid-dynamic-a",
            WALLET,
            Decimal::from_str("100.00").unwrap(),
            TOKEN,
            past_entry(),
        )
        .await;
    assert_eq!(
        action_a,
        StopLossAction::None,
        "A -16% loss must NOT fire for a high-WQS wallet (dynamic stop = -20%)"
    );

    // Scenario B: Entry = $100, Current = $79 → loss = -21% (past -20% dynamic stop)
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("79.00").unwrap(),
        PriceSource::Jupiter,
    );
    let action_b = mgr
        .check_stop_loss(
            "uuid-dynamic-b",
            WALLET,
            Decimal::from_str("100.00").unwrap(),
            TOKEN,
            past_entry(),
        )
        .await;
    assert_eq!(
        action_b,
        StopLossAction::Exit,
        "A -21% loss must trigger the dynamic stop (-20% threshold) for a high-WQS wallet"
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

    let (db, _tmp) = create_test_db().await;
    let price_cache = Arc::new(PriceCache::new().unwrap());
    const TOKEN: &str = "token_ratchet_fix";

    let cfg = Arc::new(ProfitManagementConfig {
        targets: vec![],
        trailing_stop_activation: Decimal::from_str("10.0").unwrap(),
        trailing_stop_distance: Decimal::from_str("20.0").unwrap(),
        ..ProfitManagementConfig::default()
    });
    let mgr = ProfitTargetManager::new(db, cfg, price_cache.clone());

    price_cache.set_price(
        TOKEN,
        Decimal::from_str("1.00").unwrap(),
        PriceSource::Jupiter,
    );
    mgr.register_position(
        "uuid-ratchet-fix",
        Decimal::from_str("1.00").unwrap(),
        Decimal::from_str("5.0").unwrap(),
        TOKEN,
        std::time::SystemTime::now(),
    )
    .await;

    // Activate trailing stop at $1.20
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("1.20").unwrap(),
        PriceSource::Jupiter,
    );
    let _ = mgr.check_targets("uuid-ratchet-fix", TOKEN, "SHIELD").await;

    // New peak at $2.00 → correct ratcheted stop = $1.60
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("2.00").unwrap(),
        PriceSource::Jupiter,
    );
    let _ = mgr.check_targets("uuid-ratchet-fix", TOKEN, "SHIELD").await;

    // Price falls to $1.40 — below ratcheted stop $1.60 → must Exit
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("1.40").unwrap(),
        PriceSource::Jupiter,
    );
    let action = mgr.check_targets("uuid-ratchet-fix", TOKEN, "SHIELD").await;

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

    let (db, _tmp) = create_test_db().await;

    let result = db
        .update_trade_status(&UpdateTradeStatus {
            trade_uuid: "00000000-0000-0000-0000-nonexistent00".to_string(),
            status: "QUEUED".to_string(),
            tx_signature: None,
            error_message: None,
        })
        .await;

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

    let (db, _tmp) = create_test_db().await;
    let uuid = "aaaabbbb-cccc-dddd-eeee-ffffffffffff";
    seed_trade(&db, uuid).await;

    let result = db
        .update_trade_status(&UpdateTradeStatus {
            trade_uuid: uuid.to_string(),
            status: "QUEUED".to_string(),
            tx_signature: None,
            error_message: None,
        })
        .await;

    assert!(
        result.is_ok(),
        "update_trade_status must return Ok for an existing trade UUID"
    );

    // Verify status was actually changed
    let pool = sqlite_pool(&db);
    let status: String = sqlx::query_scalar("SELECT status FROM trades WHERE trade_uuid = ?")
        .bind(uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "QUEUED", "Trade status must be updated to QUEUED");
}
