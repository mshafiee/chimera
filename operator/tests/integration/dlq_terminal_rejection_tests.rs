//! Risk-gate rejections must be TERMINAL in the DLQ (2026-08-23).
//!
//! The DLQ retry worker re-injected gate-rejected BUY signals every 5
//! minutes until their short cooldown expired and executed them anyway —
//! measured -0.126 SOL on EjD5Y9 2026-08-22 (a "Duplicate token"-rejected
//! signal replayed 35 minutes later, past the 30-min loss cooldown, into
//! the same dump). A stacking / loss-cooldown / shadow-blacklist rejection
//! must never be retryable: fresh signals re-decide through decide_buy
//! with current gates; replaying a stale admitted payload bypasses them.

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

async fn seed_trade(db: &Arc<dyn Database>, trade_uuid: &str) {
    sqlx::query(
        "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status) \
         VALUES ($1, '7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU', \
                 '4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R', 'SHIELD', 'BUY', 1.0, 'PENDING')",
    )
    .bind(trade_uuid)
    .execute(&pg_pool(db))
    .await
    .unwrap();
}

async fn retryable_uuids(db: &Arc<dyn Database>) -> Vec<String> {
    db.get_retryable_dlq_items(50)
        .await
        .unwrap()
        .into_iter()
        .map(|item| item.trade_uuid)
        .collect()
}

/// The three risk-gate rejection reasons observed in signal_pipeline.rs
/// must land in the DLQ with can_retry=false.
#[tokio::test]
async fn test_risk_gate_rejections_are_terminal() {
    let (db, _guard) = common::create_test_db().await;

    let terminal_reasons = [
        "Duplicate token: EjD5Y9NVhXmtEqU7wYvAyZvDWZFQeEuHXFatJmTbpump already has 1 active position(s)",
        "Token recently lost >3% — 30min cooldown",
        "Token shadow blacklist: 3 shadow exits avg < 1.5% over 48h",
    ];
    for (i, reason) in terminal_reasons.iter().enumerate() {
        let uuid = format!("terminal-gate-{i}");
        seed_trade(&db, &uuid).await;
        db.mark_trade_dead_letter(&uuid, "{}", reason)
            .await
            .expect("dead-letter write succeeds");
    }

    let retryable = retryable_uuids(&db).await;
    for (i, reason) in terminal_reasons.iter().enumerate() {
        let uuid = format!("terminal-gate-{i}");
        assert!(
            !retryable.contains(&uuid),
            "{reason} must not be retryable, but {uuid} is"
        );
    }
}

/// Control: genuine transient failures keep their DLQ retry path.
#[tokio::test]
async fn test_transient_errors_stay_retryable() {
    let (db, _guard) = common::create_test_db().await;

    seed_trade(&db, "transient-rpc-timeout").await;
    db.mark_trade_dead_letter(
        "transient-rpc-timeout",
        "{}",
        "RPC connection timeout while submitting bundle",
    )
    .await
    .expect("dead-letter write succeeds");

    let retryable = retryable_uuids(&db).await;
    assert!(
        retryable.contains(&"transient-rpc-timeout".to_string()),
        "transient errors must remain retryable, got {retryable:?}"
    );
}

/// Seed a SELL trade (the whale-sell skip path creates SELL rows).
async fn seed_sell_trade(db: &Arc<dyn Database>, trade_uuid: &str) {
    sqlx::query(
        "INSERT INTO trades (trade_uuid, wallet_address, token_address, strategy, side, amount_sol, status) \
         VALUES ($1, '7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU', \
                 '4k3Dyjzvzp8eMZWUXbBCjEvwSkkk59S5iCNLY3QrkX6R', 'EXIT', 'SELL', 3.0, 'QUEUED')",
    )
    .bind(trade_uuid)
    .execute(&pg_pool(db))
    .await
    .unwrap();
}

/// The whale-SELL skip (copy_wallet_sells=false) is a DETERMINISTIC skip:
/// the exit system owns the position, and replaying the payload can never
/// change the decision. It must be terminal in the DLQ — measured 2026-08-28:
/// three whale-SELL skips created QUEUED rows, were swept stale at ~33min and
/// DLQ-retried once each (pure churn, zero execution intent).
#[tokio::test]
async fn test_whale_sell_skip_is_terminal() {
    let (db, _guard) = common::create_test_db().await;

    let uuid = "whale-sell-skip-1";
    seed_sell_trade(&db, uuid).await;
    let reason = "WHALE_SELL_SKIP: position managed by exit system (copy_wallet_sells=false) — deterministic skip";
    db.mark_trade_dead_letter(uuid, "{}", reason)
        .await
        .expect("dead-letter write succeeds");

    let retryable = retryable_uuids(&db).await;
    assert!(
        !retryable.contains(&uuid.to_string()),
        "WHALE_SELL_SKIP must not be retryable, but {uuid} is"
    );
}
