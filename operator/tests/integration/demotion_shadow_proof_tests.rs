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
use std::str::FromStr;
use sqlx::Pool;
use sqlx::Postgres;
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
    assert!(verdict.is_some(), "negative-expectancy book must not exempt");
}
