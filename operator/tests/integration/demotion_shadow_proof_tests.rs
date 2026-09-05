//! M1 shadow-proof demotion exemption (2026-08-30).
//!
//! An ACTIVE wallet whose trailing deduped mirror_main book is proven-positive
//! (≥20 exits, positive expectancy) must NOT be demoted by the inactivity
//! rotation, even when its activity anchors (speculative signal, trades,
//! promotion) are all stale. Backtest basis: shadow-proven roster policy
//! simulated +120.46 SOL/60d vs −1.44 status quo; measured victim
//! 12kNFpfihj (+71.2 SOL book demoted by stale heuristics).

use chimera_infra::monitoring::wallet_performance::{DemotionReason, WalletPerformanceTracker};
use chimera_operator::config::AppConfig;
use chimera_operator::db_abstraction::{Database, DbPool};
use rust_decimal::Decimal;
use sqlx::Pool;
use sqlx::Postgres;
use std::str::FromStr;
use std::sync::Arc;

#[path = "../common/mod.rs"]
mod common;

fn pg_pool(db: &Arc<dyn Database>) -> Pool<Postgres> {
    let DbPool::PostgreSQL(pool) = db.pool();
    pool
}

fn monitor_with_rotation(db: Arc<dyn Database>) -> WalletPerformanceTracker {
    let mut cfg = AppConfig::default();
    let mut m = cfg.monitoring.take().unwrap_or_default();
    m.inactivity_rotation_enabled = true;
    // 1h thresholds — the 30d-stale seeded wallets breach all tiers.
    let mut rotation = chimera_core::config::InactivityRotationConfig::default();
    rotation.high_conviction_threshold_secs = 3600;
    rotation.regular_conviction_threshold_secs = 3600;
    rotation.low_conviction_threshold_secs = 3600;
    m.inactivity_rotation = Some(rotation);
    cfg.monitoring = Some(m);
    WalletPerformanceTracker::new_with_config(db, Arc::new(cfg))
}

async fn seed_stale_active_wallet(db: &Arc<dyn Database>, address: &str) {
    sqlx::query(
        "INSERT INTO wallets (address, status, wqs_score, wqs_confidence, last_trade_at, promoted_at) \
         VALUES ($1, 'ACTIVE', 80.0, 0.9, NOW() - INTERVAL '30 days', NOW() - INTERVAL '30 days')",
    )
    .bind(address)
    .execute(&pg_pool(&db))
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO wallet_monitoring (wallet_address, last_speculative_signal_at) \
         VALUES ($1, NOW() - INTERVAL '30 days')",
    )
    .bind(address)
    .execute(&pg_pool(&db))
    .await
    .unwrap();
}

/// Seed 25 dedup-safe positive mirror_main shadow exits for the wallet
/// (one per hour → proven-positive book: 0.8 win rate, +20%/−2% legs).
async fn seed_proven_shadow_book(db: &Arc<dyn Database>, wallet: &str) {
    for i in 1..=25 {
        let sid = format!("m1proof-{wallet}-{i}");
        let pct = if i % 5 == 0 { "-2.0" } else { "20.0" };
        sqlx::query(
            "INSERT INTO shadow_positions (shadow_id, decision_id, run_id, wallet_address, token_address, main_admitted, entry_amount_sol, ingress, opened_at) \
             VALUES ($1, 'd', 'run', $2, 'seedtoken', false, 0.1, 'webhook', NOW() - make_interval(hours => $3))",
        )
        .bind(&sid)
        .bind(wallet)
        .bind(i as i32)
        .execute(&pg_pool(&db))
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO shadow_exits (shadow_id, exit_strategy, pnl_pct, exit_reason) \
             VALUES ($1, 'mirror_main', $2, 'profit_target')",
        )
        .bind(&sid)
        .bind(Decimal::from_str(pct).unwrap())
        .execute(&pg_pool(&db))
        .await
        .unwrap();
    }
}

/// M1: shadow-proven + fully stale activity anchors → NO demotion.
#[tokio::test]
async fn test_shadow_proven_wallet_is_exempt_from_inactivity_demotion() {
    let (db, _guard) = common::create_test_db().await;
    let wallet = "m1-proven-wallet-111111111111111111111";
    seed_stale_active_wallet(&db, wallet).await;
    seed_proven_shadow_book(&db, wallet).await;

    let monitor = monitor_with_rotation(db.clone());
    let verdict = monitor.should_demote(wallet).await;
    assert!(
        verdict.is_none(),
        "shadow-proven book must suppress demotion, got {:?}",
        verdict
    );
}

/// Control: identical staleness WITHOUT a shadow book → demoted (Inactivity).
#[tokio::test]
async fn test_unproven_stale_wallet_still_demoted() {
    let (db, _guard) = common::create_test_db().await;
    let wallet = "m1-unproven-wallet-11111111111111111111";
    seed_stale_active_wallet(&db, wallet).await;

    let monitor = monitor_with_rotation(db.clone());
    let verdict = monitor.should_demote(wallet).await;
    assert!(
        matches!(verdict, Some(DemotionReason::Inactivity)),
        "unproven stale wallet must still be demoted, got {:?}",
        verdict
    );
}

/// A below-cost shadow book (negative expectancy) does NOT earn the
/// exemption — the wallet is demoted despite having exits.
#[tokio::test]
async fn test_negative_expectancy_book_does_not_exempt() {
    let (db, _guard) = common::create_test_db().await;
    let wallet = "m1-negative-wallet-11111111111111111111";
    seed_stale_active_wallet(&db, wallet).await;
    // 25 exits, 20% win rate, +1% wins / −5% losses → negative expectancy.
    for i in 1..=25 {
        let sid = format!("m1neg-{wallet}-{i}");
        let pct = if i % 5 == 0 { "1.0" } else { "-5.0" };
        sqlx::query(
            "INSERT INTO shadow_positions (shadow_id, decision_id, run_id, wallet_address, token_address, main_admitted, entry_amount_sol, ingress, opened_at) \
             VALUES ($1, 'd', 'run', $2, 'seedtoken', false, 0.1, 'webhook', NOW() - make_interval(hours => $3))",
        )
        .bind(&sid)
        .bind(wallet)
        .bind(i as i32)
        .execute(&pg_pool(&db))
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO shadow_exits (shadow_id, exit_strategy, pnl_pct, exit_reason) \
             VALUES ($1, 'mirror_main', $2, 'stop_loss')",
        )
        .bind(&sid)
        .bind(Decimal::from_str(pct).unwrap())
        .execute(&pg_pool(&db))
        .await
        .unwrap();
    }

    let monitor = monitor_with_rotation(db.clone());
    let verdict = monitor.should_demote(wallet).await;
    assert!(
        verdict.is_some(),
        "negative-expectancy book must not exempt"
    );
}

// ── Fix B: time-decayed shadow-proof exemption (2026-09-01) ─────────────────

/// Seed exits with EXPLICIT exited_at timestamps (for the 48h-net condition).
async fn seed_exits_with_age(
    db: &Arc<dyn Database>,
    wallet: &str,
    prefix: &str,
    hours_ago: i32,
    count: usize,
    pct: &str,
) {
    for i in 0..count {
        let sid = format!("{prefix}-{wallet}-{i}");
        sqlx::query(
            "INSERT INTO shadow_positions (shadow_id, decision_id, run_id, wallet_address, token_address, main_admitted, entry_amount_sol, ingress, opened_at) \
             VALUES ($1, 'd', 'run', $2, 'seedtoken', false, 0.1, 'webhook', NOW() - make_interval(hours => $3))",
        )
        .bind(&sid)
        .bind(wallet)
        .bind(i as i32 + hours_ago + 1)
        .execute(&pg_pool(db))
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO shadow_exits (shadow_id, exit_strategy, pnl_pct, exit_reason, pnl_sol, exited_at) \
             VALUES ($1, 'mirror_main', $2, 'profit_target', $4, NOW() - make_interval(hours => $3))",
        )
        .bind(&sid)
        .bind(Decimal::from_str(pct).unwrap())
        .bind(i as i32 + hours_ago)
        .bind(Decimal::from_str(pct).unwrap())
        .execute(&pg_pool(db))
        .await
        .unwrap();
    }
}

/// Fix B: a 30d-proven book whose trailing-48h net is NEGATIVE loses the
/// demotion exemption — the whale's CURRENT flow is bleeding (12kNFpfihj:
/// +73.8 outlier day then −4.39/24h at −36.6%).
#[tokio::test]
async fn test_exemption_lapses_when_recent_48h_net_negative() {
    let (db, _guard) = common::create_test_db().await;
    let wallet = "m1decay-neg-wallet-1111111111111111111";
    seed_stale_active_wallet(&db, wallet).await;
    // 20 old positive exits (+20% each, exited 3-23d ago) → 30d-proven ✓
    seed_exits_with_age(&db, wallet, "old", 72, 20, "20.0").await;
    // 6 recent NEGATIVE exits (−30% each, within 48h) → 48h net −1.8 SOL
    seed_exits_with_age(&db, wallet, "new", 6, 6, "-30.0").await;

    let monitor = monitor_with_rotation(db.clone());
    let verdict = monitor.should_demote(wallet).await;
    assert!(
        verdict.is_some(),
        "48h-negative book must lapse the shadow-proof exemption, got {:?}",
        verdict
    );
}

/// Control: a 30d-proven book with POSITIVE 48h net keeps the exemption.
#[tokio::test]
async fn test_exemption_holds_when_recent_48h_net_positive() {
    let (db, _guard) = common::create_test_db().await;
    let wallet = "m1decay-pos-wallet-1111111111111111111";
    seed_stale_active_wallet(&db, wallet).await;
    seed_exits_with_age(&db, wallet, "old", 72, 20, "20.0").await;
    // Recent exits POSITIVE (+5% each, within 48h) → 48h net positive.
    seed_exits_with_age(&db, wallet, "new", 6, 6, "5.0").await;

    let monitor = monitor_with_rotation(db.clone());
    let verdict = monitor.should_demote(wallet).await;
    assert!(
        verdict.is_none(),
        "positive 48h net must keep the exemption"
    );
}

// ── Zero-yield rotation for NEVER-traded wallets (2026-09-05) ───────────────
//
// The metrics cache is written only by record_trade_result (trade-close
// events), so a wallet with ZERO copy trades ever has no cache entry. The
// old Some(metrics)-only check never fired for it — 27/29 ACTIVE wallets on
// prod had last_trade_at NULL and stayed ACTIVE indefinitely.

/// Seed an ACTIVE wallet past promotion grace with FRESH on-chain activity
/// (speculative signal now) but zero copy trades — the zero-yield class.
async fn seed_active_never_traded_wallet(db: &Arc<dyn Database>, address: &str) {
    sqlx::query(
        "INSERT INTO wallets (address, status, wqs_score, wqs_confidence, last_trade_at, promoted_at) \
         VALUES ($1, 'ACTIVE', 80.0, 0.9, NULL, NOW() - INTERVAL '30 days')",
    )
    .bind(address)
    .execute(&pg_pool(db))
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO wallet_monitoring (wallet_address, last_speculative_signal_at) \
         VALUES ($1, NOW())",
    )
    .bind(address)
    .execute(&pg_pool(db))
    .await
    .unwrap();
}

/// A never-traded wallet (no metrics entry) with FRESH activity anchors must
/// be classified ZeroYield — not slip through because the cache is empty.
#[tokio::test]
async fn test_zero_yield_never_traded_wallet_demoted() {
    let (db, _guard) = common::create_test_db().await;
    let wallet = "m1zeroyield-wallet-1111111111111111111";
    seed_active_never_traded_wallet(&db, wallet).await;

    let monitor = monitor_with_rotation(db.clone());
    let verdict = monitor.should_demote(wallet).await;
    assert!(
        matches!(verdict, Some(DemotionReason::ZeroYield)),
        "never-traded ACTIVE wallet must be ZeroYield, got {:?}",
        verdict
    );
}

/// The periodic sweep must park never-traded ACTIVE wallets in PROVING while
/// leaving freshly promoted wallets untouched.
#[tokio::test]
async fn test_demotion_sweep_parks_never_traded_wallets() {
    let (db, _guard) = common::create_test_db().await;
    let stale_wallet = "m1sweep-nevertraded-1111111111111111111";
    let fresh_promoted = "m1sweep-freshpromo-111111111111111111";
    seed_active_never_traded_wallet(&db, stale_wallet).await;

    // Freshly promoted 1h ago, also never traded — inside the 7d grace.
    sqlx::query(
        "INSERT INTO wallets (address, status, wqs_score, wqs_confidence, last_trade_at, promoted_at) \
         VALUES ($1, 'ACTIVE', 80.0, 0.9, NULL, NOW() - INTERVAL '1 hour')",
    )
    .bind(fresh_promoted)
    .execute(&pg_pool(&db))
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO wallet_monitoring (wallet_address, last_speculative_signal_at) \
         VALUES ($1, NOW())",
    )
    .bind(fresh_promoted)
    .execute(&pg_pool(&db))
    .await
    .unwrap();

    let monitor = monitor_with_rotation(db.clone());
    let demoted = monitor.run_active_wallet_demotion_sweep().await.unwrap();
    assert!(
        demoted >= 1,
        "at least the never-traded wallet must be demoted"
    );

    async fn status(db: &Arc<dyn Database>, addr: &str) -> String {
        sqlx::query_scalar::<_, String>("SELECT status FROM wallets WHERE address = $1")
            .bind(addr)
            .fetch_one(&pg_pool(db))
            .await
            .unwrap()
    }

    assert_eq!(
        status(&db, stale_wallet).await,
        "PROVING",
        "never-traded wallet must be parked in PROVING"
    );
    assert_eq!(
        status(&db, fresh_promoted).await,
        "ACTIVE",
        "fresh promotion grace must protect the wallet"
    );
}
