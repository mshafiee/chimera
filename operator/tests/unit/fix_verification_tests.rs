//! Fix-Verification Tests
//!
//! Each test asserts the CORRECT (post-fix) behavior for a documented bug.
//! The bugs below are FIXED in the current codebase; these tests exist to
//! prevent regressions:
//!
//!   F3/F7 — Hard stop sign bug: default was +15.0 (positive) which fired on
//!           EVERY losing position. Fix: default max_stop_loss_distance = -5.0.
//!   F4    — Trailing stop ratchet: stop_price never updated after activation.
//!   F6    — Silent status update: update_trade_status now returns
//!           Err(NotFound) when rows_affected() == 0.
//!
//! If any of these tests fail, the corresponding bug has regressed.

use chimera_operator::config::ProfitManagementConfig;
use chimera_operator::db_abstraction::{Database, DbPool, InsertTrade, UpdateTradeStatus};
use chimera_operator::engine::profit_targets::{ProfitTargetAction, ProfitTargetManager};
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

fn past_entry() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now() - chrono::TimeDelta::seconds(60)
}

// ─── helpers ─────────────────────────────────────────────────────────────────

async fn create_test_db() -> (Arc<dyn Database>, common::TestDbGuard) {
    common::create_test_pg_db().await
}

/// A config with an explicit `max_stop_loss_distance` (negative; the config
/// validator rejects positive values). A wide value (e.g. -50.0) lets the
/// WQS-based dynamic thresholds operate instead of being clamped to the
/// default -5.0.
fn config_with_stop_distance(stop_distance: &str) -> Arc<ProfitManagementConfig> {
    Arc::new(ProfitManagementConfig {
        max_stop_loss_distance: Decimal::from_str(stop_distance).unwrap(),
        ..ProfitManagementConfig::default()
    })
}

/// Insert a wallet row so stop_loss WQS lookup succeeds (returns WQS 70 → -20% threshold).
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

// ─── F3/F7: Hard stop sign bug (FIXED) ────────────────────────────────────────

#[tokio::test]
async fn should_not_fire_hard_stop_at_2pct_loss_with_default_config() {
    // F7 regression guard: the default max_stop_loss_distance is now -5.0
    // (negative). A 2% loss must NOT trigger any stop.

    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    let price_cache = Arc::new(PriceCache::new().unwrap());
    const TOKEN: &str = "token_hard_stop_fix";
    const WALLET: &str = "wallet_hard_stop_fix";

    insert_wallet(&pool, WALLET, 75.0).await;

    // Use the DEFAULT config (max_stop_loss_distance = -5.0 after the fix)
    let cfg = Arc::new(ProfitManagementConfig::default());
    let mgr = StopLossManager::new(db, cfg, price_cache.clone());

    // Entry = $100, Current = $98 → loss = -2%
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("100.00").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("98.00").unwrap(),
        PriceSource::Jupiter,
        Some(9),
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
        "A 2% loss must NOT trigger any stop (default threshold is -5%)"
    );
}

#[tokio::test]
async fn should_fire_dynamic_stop_at_21pct_loss_for_high_wqs_wallet() {
    // With a wide max_stop_loss_distance the WQS-based dynamic stop operates:
    //   - WQS=75 → dynamic base = -20%
    //   - effective threshold ≈ -18% (volatility × 0.9 with sub-10% vol)
    //
    // Scenario A: -16% loss → -16% > -18% → no exit
    // Scenario B: -21% loss → -21% <= -18% → Exit (dynamic stop fires)
    //
    // NOTE: with the DEFAULT max_stop_loss_distance (-5.0), every dynamic
    // threshold is clamped to -5% — that is the config's intent (tight stops)
    // and the "Adaptive stop-loss widening overridden" warning in the logs.

    let (db, _tmp) = create_test_db().await;
    let pool = pg_pool(&db);
    let price_cache = Arc::new(PriceCache::new().unwrap());
    const TOKEN: &str = "token_dynamic_stop_21";
    const WALLET: &str = "wallet_dynamic_stop_21";

    insert_wallet(&pool, WALLET, 75.0).await; // High WQS → dynamic threshold = -20%

    let cfg = config_with_stop_distance("-50.0");
    let mgr = StopLossManager::new(db, cfg, price_cache.clone());

    // Scenario A: Entry = $100, Current = $84 → loss = -16% (not past ~-18% dynamic stop)
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("84.00").unwrap(),
        PriceSource::Jupiter,
        Some(9),
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
        "A -16% loss must NOT fire for a high-WQS wallet (dynamic stop ≈ -18%)"
    );

    // Scenario B: Entry = $100, Current = $79 → loss = -21% (past ~-18% dynamic stop)
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("79.00").unwrap(),
        PriceSource::Jupiter,
        Some(9),
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
        "A -21% loss must trigger the dynamic stop (≈ -18% threshold) for a high-WQS wallet"
    );
}

// ─── F4: Trailing stop ratchet (FIXED) ────────────────────────────────────────

#[tokio::test]
async fn should_ratchet_trailing_stop_price_as_peak_rises() {
    // F4 regression guard: the trailing stop must ratchet up as the peak
    // rises (is_new_peak is captured before peak_price is overwritten).
    //
    // Sequence:
    //   Entry $1.00
    //   Price $1.20 (+20%): trailing activates (activation=10%), stop ≈ $0.96
    //   Price $2.00 (+100%): new peak → stop ratchets to ≈ $1.60
    //   Price $1.40: below ratcheted stop → must FullExit

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
        Some(9),
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
        Some(9),
    );
    let _ = mgr.check_targets("uuid-ratchet-fix", TOKEN, "SHIELD").await;

    // New peak at $2.00 → ratcheted stop ≈ $1.60
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("2.00").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let _ = mgr.check_targets("uuid-ratchet-fix", TOKEN, "SHIELD").await;

    // Price falls to $1.40 — below ratcheted stop ≈ $1.60 → must Exit
    price_cache.set_price(
        TOKEN,
        Decimal::from_str("1.40").unwrap(),
        PriceSource::Jupiter,
        Some(9),
    );
    let action = mgr.check_targets("uuid-ratchet-fix", TOKEN, "SHIELD").await;

    assert!(
        matches!(action, ProfitTargetAction::FullExit),
        "Price $1.40 < ratcheted trailing stop ≈ $1.60 must trigger FullExit"
    );
}

// ─── F6: Silent status update (FIXED) ─────────────────────────────────────────

#[tokio::test]
async fn should_return_error_on_status_update_for_missing_uuid() {
    // F6 regression guard: update_trade_status returns Err(NotFound) when
    // rows_affected() == 0, so callers can detect phantom state transitions.

    let (db, _tmp) = create_test_db().await;

    let result = db
        .update_trade_status(&UpdateTradeStatus {
            // Well-formed UUID that does not exist in the database.
            trade_uuid: "00000000-0000-0000-0000-000000000000".to_string(),
            status: "QUEUED".to_string(),
            tx_signature: None,
            error_message: None,
            network_fee_sol: None,
        })
        .await;

    assert!(
        result.is_err(),
        "update_trade_status must return Err when the trade_uuid does not exist"
    );
}

#[tokio::test]
async fn should_succeed_on_status_update_for_existing_trade() {
    // Complement to F6: the fix must not break the happy path.

    let (db, _tmp) = create_test_db().await;
    let uuid = "aaaabbbb-cccc-dddd-eeee-ffffffffffff";
    seed_trade(&db, uuid).await;

    let result = db
        .update_trade_status(&UpdateTradeStatus {
            trade_uuid: uuid.to_string(),
            status: "QUEUED".to_string(),
            tx_signature: None,
            error_message: None,
            network_fee_sol: None,
        })
        .await;

    assert!(
        result.is_ok(),
        "update_trade_status must return Ok for an existing trade UUID"
    );

    // Verify status was actually changed
    let pool = pg_pool(&db);
    let status: String = sqlx::query_scalar("SELECT status FROM trades WHERE trade_uuid = $1")
        .bind(uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "QUEUED", "Trade status must be updated to QUEUED");
}

/// Log pruning must run through the real pruning entry point
/// (`engine::prune_logs_if_needed`) with an injectable directory — no
/// process-global env mutation. The CHIMERA_LOG_DIR env resolution itself
/// lives in main.rs and is outside the testable library surface; on a healthy
/// disk the pruning call is a no-op (Ok), and the deletion path only activates
/// under disk pressure.
#[tokio::test]
async fn test_log_pruning_uses_correct_directory_path() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("operator.log"), "test log").unwrap();

    let result = chimera_operator::engine::prune_logs_if_needed(dir.path(), 7).await;
    assert!(
        result.is_ok(),
        "prune_logs_if_needed must complete without error for a valid log dir"
    );

    // The active log file must never be pruned.
    assert!(dir.path().join("operator.log").exists());
}
