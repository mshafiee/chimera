//! Dormancy-demotion promotion grace (2026-08-29).
//!
//! `demote_dormant_active_wallets` must anchor dormancy on
//! GREATEST(promoted_at, last_trade_at): a freshly promoted wallet gets the
//! full inactivity window before rotation reclaims it. Keying on
//! last_trade_at alone made the scout promoter and the operator's roster
//! refill fight — promote (trailing shadow bar) → demote (stale
//! last_trade_at) every 2h cycle, measured on 8jfDh7hABX/9bzPrKYb
//! 2026-08-28/29.

use chimera_operator::db_abstraction::{Database, DbPool};
use sqlx::Pool;
use sqlx::Postgres;
use std::sync::Arc;

#[path = "../common/mod.rs"]
mod common;

fn pg_pool(db: &Arc<dyn Database>) -> Pool<Postgres> {
    let DbPool::PostgreSQL(pool) = db.pool();
    pool
}

/// Seed an ACTIVE wallet with explicit promotion/trade timestamps.
async fn seed_active_wallet(db: &Arc<dyn Database>, address: &str, promoted_sql: &str, traded_sql: &str) {
    let query = format!(
        "INSERT INTO wallets (address, status, wqs_score, wqs_confidence, last_trade_at, promoted_at) \
         VALUES ('{address}', 'ACTIVE', 80.0, 0.9, {traded_sql}, {promoted_sql})"
    );
    sqlx::query(&query).execute(&pg_pool(db)).await.unwrap();
}

/// The scout promoter re-promotes wallets whose trailing shadow book clears
/// the bar (promoted_at = promotion moment), even when their whale has been
/// quiet for weeks. Those wallets must survive the dormancy rotation for the
/// full grace window instead of being demoted immediately (the 2h promote/
/// demote flap).
#[tokio::test]
async fn test_recently_promoted_wallet_with_stale_trades_survives_dormancy_sweep() {
    let (db, _guard) = common::create_test_db().await;

    // Promoted 1 day ago; last on-chain trade 30 days ago. Within the 7-day
    // window the promotion anchor dominates — NOT dormant.
    seed_active_wallet(
        &db,
        "grace-wallet-1111111111111111111111111111",
        "NOW() - INTERVAL '1 day'",
        "NOW() - INTERVAL '30 days'",
    )
    .await;

    let demoted = db.demote_dormant_active_wallets(7).await.unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM wallets WHERE address = 'grace-wallet-1111111111111111111111111111'")
        .fetch_one(&pg_pool(&db))
        .await
        .unwrap();
    assert_eq!(status, "ACTIVE", "promotion grace must shield a 1-day-old promotion");
    assert_eq!(demoted, 0);
}

/// Control: a wallet that is stale on BOTH anchors (promoted long ago AND no
/// trades since) is dormant and must be reclaimed.
#[tokio::test]
async fn test_stale_promotion_and_stale_trades_are_demoted() {
    let (db, _guard) = common::create_test_db().await;

    seed_active_wallet(
        &db,
        "stale-wallet-111111111111111111111111111111",
        "NOW() - INTERVAL '30 days'",
        "NOW() - INTERVAL '30 days'",
    )
    .await;

    db.demote_dormant_active_wallets(7).await.unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM wallets WHERE address = 'stale-wallet-111111111111111111111111111111'")
        .fetch_one(&pg_pool(&db))
        .await
        .unwrap();
    assert_eq!(status, "CANDIDATE", "dormant on both anchors must be reclaimed");
}

/// A wallet that traded RECENTLY but was promoted long ago is kept: the
/// GREATEST anchor takes the fresher of the two.
#[tokio::test]
async fn test_recent_trade_keeps_old_promotion_alive() {
    let (db, _guard) = common::create_test_db().await;

    seed_active_wallet(
        &db,
        "recent-trade-1111111111111111111111111111",
        "NOW() - INTERVAL '30 days'",
        "NOW() - INTERVAL '1 day'",
    )
    .await;

    db.demote_dormant_active_wallets(7).await.unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM wallets WHERE address = 'recent-trade-1111111111111111111111111111'")
        .fetch_one(&pg_pool(&db))
        .await
        .unwrap();
    assert_eq!(status, "ACTIVE", "recent on-chain trade must anchor dormancy");
}

/// Legacy skip preserved: a wallet with BOTH timestamps NULL is left to the
/// inactivity rotation's own logic, never demoted here.
#[tokio::test]
async fn test_null_timestamps_are_skipped() {
    let (db, _guard) = common::create_test_db().await;

    sqlx::query(
        "INSERT INTO wallets (address, status, wqs_score, wqs_confidence) \
         VALUES ('null-ts-wallet-11111111111111111111111', 'ACTIVE', 80.0, 0.9)",
    )
    .execute(&pg_pool(&db))
    .await
    .unwrap();

    db.demote_dormant_active_wallets(7).await.unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM wallets WHERE address = 'null-ts-wallet-11111111111111111111111'")
        .fetch_one(&pg_pool(&db))
        .await
        .unwrap();
    assert_eq!(status, "ACTIVE", "NULL timestamps must be skipped (legacy behavior)");
}

/// Regression (2026-08-29, first deployment of the GREATEST anchor): an
/// over-broad NULL guard demoted 24 last_trade_at-NULL wallets — including
/// the 132Tkgf5YE star — on the first post-deploy sweep. The contract:
/// last_trade_at-NULL wallets are NEVER demoted here, regardless of
/// promoted_at; they belong to the inactivity rotation's own logic.
#[tokio::test]
async fn test_promoted_at_only_wallet_with_null_last_trade_is_never_demoted() {
    let (db, _guard) = common::create_test_db().await;

    // Exactly the demoted-star shape: stale promoted_at, NULL last_trade_at.
    sqlx::query(
        "INSERT INTO wallets (address, status, wqs_score, wqs_confidence, promoted_at) \
         VALUES ('null-lt-promoted-1111111111111111111', 'ACTIVE', 10.0, 0.9, NOW() - INTERVAL '11 days')",
    )
    .execute(&pg_pool(&db))
    .await
    .unwrap();

    db.demote_dormant_active_wallets(7).await.unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM wallets WHERE address = 'null-lt-promoted-1111111111111111111'")
        .fetch_one(&pg_pool(&db))
        .await
        .unwrap();
    assert_eq!(status, "ACTIVE", "last_trade_at-NULL wallets must never be demoted here (legacy skip)");
}
