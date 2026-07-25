//! Reconciliation Tests
//!
//! Tests the daily reconciliation process against the real PostgreSQL schema and the
//! real [`run_reconciliation`] runner:
//! - On-chain vs DB state comparison (signature-based)
//! - Auto-resolution of *confirmed exits* (the only auto-resolution the runner does)
//! - Reconciliation log entries (real `discrepancy` kinds)
//! - Stuck-position recovery via the RPC-free `get_stuck_positions` query
//!
//! The `runner_*` tests exercise the real [`run_reconciliation`] runner against an
//! isolated PostgreSQL database using a stub on-chain checker.

mod common;

use async_trait::async_trait;
use chimera_operator::db_abstraction::{
    types::{InsertPosition, InsertTrade, UpdatePosition},
    Database,
};
use chimera_operator::engine::reconciliation::{
    run_reconciliation, OnChainTxChecker, OnChainTxStatus,
};
use chimera_operator::metrics::MetricsState;
use rust_decimal::Decimal;
use sqlx::Pool;
use sqlx::Postgres;
use std::sync::Arc;

/// Stub on-chain checker: signatures in `found` → Found, in `not_found` → NotFound,
/// everything else → Error.
struct StubChecker {
    found: Vec<String>,
    not_found: Vec<String>,
}

#[async_trait]
impl OnChainTxChecker for StubChecker {
    async fn check_signature(&self, signature: &str) -> OnChainTxStatus {
        if self.found.iter().any(|s| s == signature) {
            OnChainTxStatus::Found
        } else if self.not_found.iter().any(|s| s == signature) {
            OnChainTxStatus::NotFound
        } else {
            OnChainTxStatus::Error
        }
    }
}

fn metrics() -> Arc<MetricsState> {
    Arc::new(MetricsState::new().expect("metrics"))
}

async fn seed_position(db: &Arc<dyn Database>, uuid: &str, entry_sig: &str) {
    db.insert_trade(&InsertTrade {
        trade_uuid: uuid.to_string(),
        wallet_address: "Wallet1".to_string(),
        token_address: "Token1".to_string(),
        token_symbol: Some("TST".to_string()),
        strategy: "SHIELD".to_string(),
        side: "BUY".to_string(),
        amount_sol: Decimal::ONE,
        status: "ACTIVE".to_string(),
    })
    .await
    .unwrap();

    db.insert_position(&InsertPosition {
        trade_uuid: uuid.to_string(),
        wallet_address: "Wallet1".to_string(),
        token_address: "Token1".to_string(),
        token_symbol: Some("TST".to_string()),
        strategy: "SHIELD".to_string(),
        entry_amount_sol: Decimal::ONE,
        entry_price: Decimal::from(10),
        entry_tx_signature: entry_sig.to_string(),
    })
    .await
    .unwrap();
}

fn pg_pool(db: &Arc<dyn Database>) -> Pool<Postgres> {
    common::pg_pool(db)
}

/// Age `opened_at` past the entry-finalization grace window so a missing entry becomes
/// actionable. Binds a `DateTime<Utc>` (never an RFC3339 string — `opened_at` is
/// `TIMESTAMPTZ`).
async fn age_opened_at(db: &Arc<dyn Database>, uuid: &str, seconds_ago: i64) {
    let pool = pg_pool(db);
    let old = chrono::Utc::now() - chrono::Duration::seconds(seconds_ago);
    sqlx::query("UPDATE positions SET opened_at = $1 WHERE trade_uuid = $2")
        .bind(old)
        .bind(uuid)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn runner_logs_confirmed_entry_for_active_position() {
    let (db, _temp) = common::create_test_db().await;
    seed_position(&db, "uuid-active", "entry-sig-active").await;

    let checker = StubChecker {
        found: vec!["entry-sig-active".to_string()],
        not_found: vec![],
    };
    let metrics = metrics();

    let result = run_reconciliation(db.as_ref(), &checker, &metrics).await;

    // One ACTIVE position checked, no discrepancies, none auto-resolved.
    assert_eq!(result.checked_count, 1);
    assert_eq!(result.discrepancies, 0);
    assert_eq!(result.auto_resolved, 0);

    // A log row was inserted recording the confirmed entry.
    let status = db.get_reconciliation_status(100).await.unwrap();
    assert!(status.checked_count >= 1, "checked_count should reflect the run");

    // The checked counter advanced.
    assert!(metrics.reconciliation_checked.get() >= 1);
}

#[tokio::test]
async fn runner_flags_missing_entry_transaction() {
    let (db, _temp) = common::create_test_db().await;
    seed_position(&db, "uuid-missing", "entry-sig-missing").await;

    // Age past the entry-finalization grace window so a missing entry is actionable.
    age_opened_at(&db, "uuid-missing", 120).await;

    let checker = StubChecker {
        found: vec![],
        not_found: vec!["entry-sig-missing".to_string()],
    };
    let metrics = metrics();

    let result = run_reconciliation(db.as_ref(), &checker, &metrics).await;

    assert_eq!(result.checked_count, 1);
    assert_eq!(result.discrepancies, 1, "missing entry should be a discrepancy");
    assert!(metrics.reconciliation_discrepancies.get() >= 1);
}

#[tokio::test]
async fn runner_suppresses_fresh_entry_missing_within_grace() {
    let (db, _temp) = common::create_test_db().await;
    // Fresh position (opened_at = now) — within the entry-finalization grace window.
    seed_position(&db, "uuid-fresh-missing", "entry-sig-fresh-missing").await;

    let checker = StubChecker {
        found: vec![],
        not_found: vec!["entry-sig-fresh-missing".to_string()],
    };
    let metrics = metrics();

    let result = run_reconciliation(db.as_ref(), &checker, &metrics).await;

    assert_eq!(result.checked_count, 1);
    assert_eq!(
        result.discrepancies, 0,
        "fresh position's missing entry is pending, not a discrepancy"
    );
}

#[tokio::test]
async fn runner_auto_resolves_confirmed_exit() {
    let (db, _temp) = common::create_test_db().await;
    let uuid = "uuid-exiting";
    seed_position(&db, uuid, "entry-sig-exit").await;

    // Move the position to EXITING with an exit price + signature.
    db.update_position(&UpdatePosition {
        trade_uuid: uuid.to_string(),
        current_price: Some(Decimal::from(20)),
        unrealized_pnl_sol: None,
        unrealized_pnl_percent: None,
        state: Some("EXITING".to_string()),
        exit_price: Some(Decimal::from(20)),
        exit_tx_signature: Some("exit-sig-confirmed".to_string()),
        realized_pnl_sol: None,
        realized_pnl_usd: None,
    })
    .await
    .unwrap();

    // Age the position past the confirmation grace window so the exit is checked.
    age_opened_at(&db, uuid, 120).await;

    let checker = StubChecker {
        found: vec![
            "entry-sig-exit".to_string(),
            "exit-sig-confirmed".to_string(),
        ],
        not_found: vec![],
    };
    let metrics = metrics();

    let result = run_reconciliation(db.as_ref(), &checker, &metrics).await;

    assert_eq!(result.auto_resolved, 1, "confirmed exit should auto-resolve");

    // The position should now be CLOSED.
    let positions = db.get_positions(Some("CLOSED")).await.unwrap();
    assert!(
        positions.iter().any(|p| p.trade_uuid == uuid),
        "position should be CLOSED after auto-resolve"
    );
}

#[tokio::test]
async fn runner_treats_missing_exit_as_pending() {
    // An EXITING position whose exit tx is NotFound is treated as PENDING (the exit
    // may be in-flight), not as a discrepancy, and is not auto-resolved.
    let (db, _temp) = common::create_test_db().await;
    let uuid = "uuid-pending-exit";
    seed_position(&db, uuid, "entry-sig-pending").await;

    db.update_position(&UpdatePosition {
        trade_uuid: uuid.to_string(),
        current_price: Some(Decimal::from(20)),
        unrealized_pnl_sol: None,
        unrealized_pnl_percent: None,
        state: Some("EXITING".to_string()),
        exit_price: Some(Decimal::from(20)),
        exit_tx_signature: Some("exit-sig-pending".to_string()),
        realized_pnl_sol: None,
        realized_pnl_usd: None,
    })
    .await
    .unwrap();

    let checker = StubChecker {
        found: vec!["entry-sig-pending".to_string()],
        not_found: vec!["exit-sig-pending".to_string()],
    };
    let metrics = metrics();

    let result = run_reconciliation(db.as_ref(), &checker, &metrics).await;

    assert_eq!(result.auto_resolved, 0, "pending exit is not auto-resolved");
    assert_eq!(result.discrepancies, 0, "pending exit is not a discrepancy");
    // Position remains EXITING.
    let exiting = db.get_positions(Some("EXITING")).await.unwrap();
    assert!(exiting.iter().any(|p| p.trade_uuid == uuid));
}

// =============================================================================
// Scenario tests — real schema, real API, isolated DBs.
//
// These pin the *actual* reconciliation behavior. The runner is signature-based and
// amount-agnostic: it flags a `MISSING_TX`/`TX_CHECK_ERROR` entry discrepancy and
// auto-resolves only *confirmed exits*. It does NOT implement amount/epsilon
// auto-resolution or auto-`FAILED` on a missing entry (those phantom behaviors are
// out of scope here — see the plan's "Consequence" note).
// =============================================================================

#[cfg(test)]
mod tests {
    use super::common::{create_test_db, pg_pool};
    use super::{metrics, seed_position, StubChecker};
    use chimera_operator::db_abstraction::types::UpdatePosition;
    use chimera_operator::db_abstraction::Database;
    use chimera_operator::engine::reconciliation::run_reconciliation;
    use rust_decimal::Decimal;
    use std::sync::Arc;

    /// Seed an `EXITING` position (entry + exit confirmed on the DB side) for the
    /// stuck-position recovery tests.
    async fn seed_exiting(db: &Arc<dyn Database>, uuid: &str, entry_sig: &str, exit_sig: &str) {
        seed_position(db, uuid, entry_sig).await;
        db.update_position(&UpdatePosition {
            trade_uuid: uuid.to_string(),
            current_price: Some(Decimal::from(20)),
            unrealized_pnl_sol: None,
            unrealized_pnl_percent: None,
            state: Some("EXITING".to_string()),
            exit_price: Some(Decimal::from(20)),
            exit_tx_signature: Some(exit_sig.to_string()),
            realized_pnl_sol: None,
            realized_pnl_usd: None,
        })
        .await
        .unwrap();
    }

    /// Plant a controlled `last_updated` on a position. The `positions_updated_at`
    /// BEFORE-UPDATE trigger force-resets `last_updated = CURRENT_TIMESTAMP` on every
    /// UPDATE, so it must be disabled around the timestamp write (then re-enabled).
    async fn set_last_updated(db: &Arc<dyn Database>, uuid: &str, seconds_ago: i64) {
        let pool = pg_pool(db);
        sqlx::query("ALTER TABLE positions DISABLE TRIGGER positions_updated_at")
            .execute(&pool)
            .await
            .unwrap();
        let ts = chrono::Utc::now() - chrono::Duration::seconds(seconds_ago);
        sqlx::query("UPDATE positions SET last_updated = $1 WHERE trade_uuid = $2")
            .bind(ts)
            .bind(uuid)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("ALTER TABLE positions ENABLE TRIGGER positions_updated_at")
            .execute(&pool)
            .await
            .unwrap();
    }

    // 1. On-chain entry discrepancy detection (signature-based).
    #[tokio::test]
    async fn test_on_chain_discrepancy_detection() {
        let (db, _temp) = create_test_db().await;
        let uuid = "uuid-disc-det";
        seed_position(&db, uuid, "entry-sig-disc").await;

        // Age opened_at past the entry-finalization grace window.
        let pool = pg_pool(&db);
        let old = chrono::Utc::now() - chrono::Duration::seconds(120);
        sqlx::query("UPDATE positions SET opened_at = $1 WHERE trade_uuid = $2")
            .bind(old)
            .bind(uuid)
            .execute(&pool)
            .await
            .unwrap();

        let checker = StubChecker {
            found: vec![],
            not_found: vec!["entry-sig-disc".to_string()],
        };

        let result = run_reconciliation(db.as_ref(), &checker, &metrics()).await;

        assert_eq!(result.checked_count, 1);
        assert_eq!(result.discrepancies, 1, "missing entry should be a discrepancy");

        // A MISSING_TX reconciliation_log row was recorded for this position.
        let (cnt,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM reconciliation_log \
             WHERE trade_uuid = $1 AND discrepancy = 'MISSING_TX'",
        )
        .bind(uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(cnt, 1, "a MISSING_TX discrepancy row should be logged");
    }

    // 2. Epsilon tolerance for dust — pure math, no DB. (Kept unchanged.)
    //
    // NOTE: epsilon/amount tolerance is NOT part of real reconciliation, which is
    // signature-based and amount-agnostic. This test only documents the math.
    #[tokio::test]
    async fn test_epsilon_tolerance_for_dust() {
        let epsilon = 0.0001; // 0.01% tolerance

        let test_cases: Vec<(f64, f64, bool)> = vec![
            (0.5, 0.50001, true),
            (0.5, 0.5001, false),
            (1.0, 1.00001, true),
            (1.0, 1.001, false),
            (0.01, 0.010001, true),
        ];

        for (db_amount, on_chain_amount, should_match) in test_cases {
            let diff = (db_amount - on_chain_amount).abs();
            let relative_diff = diff / db_amount.max(on_chain_amount);
            let within_epsilon = relative_diff <= epsilon;

            assert_eq!(
                within_epsilon, should_match,
                "Amount comparison: db={}, on_chain={}, diff={}, relative={}",
                db_amount, on_chain_amount, diff, relative_diff
            );
        }
    }

    // 3. A missing entry transaction is a MISSING_TX discrepancy and the position is
    //    NOT auto-failed (the runner has no auto-FAILED path) — it stays ACTIVE.
    #[tokio::test]
    async fn test_auto_resolution_missing_transaction() {
        let (db, _temp) = create_test_db().await;
        let uuid = "uuid-missing-entry";
        seed_position(&db, uuid, "entry-sig-missing").await;

        let pool = pg_pool(&db);
        let old = chrono::Utc::now() - chrono::Duration::seconds(120);
        sqlx::query("UPDATE positions SET opened_at = $1 WHERE trade_uuid = $2")
            .bind(old)
            .bind(uuid)
            .execute(&pool)
            .await
            .unwrap();

        let checker = StubChecker {
            found: vec![],
            not_found: vec!["entry-sig-missing".to_string()],
        };

        let result = run_reconciliation(db.as_ref(), &checker, &metrics()).await;

        assert_eq!(result.discrepancies, 1, "missing entry is a MISSING_TX discrepancy");

        // The position is NOT auto-failed — it remains ACTIVE.
        let active = db.get_positions(Some("ACTIVE")).await.unwrap();
        assert!(
            active.iter().any(|p| p.trade_uuid == uuid),
            "missing entry must not auto-fail the position (stays ACTIVE)"
        );
    }

    // 4. Amount-agnosticism: amounts do not affect reconciliation. A found entry
    //    signature yields no discrepancy regardless of stored amounts.
    //
    // NOTE: there is no epsilon/amount-mismatch auto-resolution implemented in
    // production reconciliation; this pins that behavior.
    #[tokio::test]
    async fn test_auto_resolution_amount_mismatch() {
        let (db, _temp) = create_test_db().await;
        let uuid = "uuid-amount-agnostic";
        seed_position(&db, uuid, "entry-sig-found").await;

        let checker = StubChecker {
            found: vec!["entry-sig-found".to_string()],
            not_found: vec![],
        };

        let result = run_reconciliation(db.as_ref(), &checker, &metrics()).await;

        assert_eq!(result.checked_count, 1);
        assert_eq!(
            result.discrepancies, 0,
            "found entry sig → no discrepancy, regardless of amounts"
        );

        let pool = pg_pool(&db);
        let (non_none,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM reconciliation_log \
             WHERE trade_uuid = $1 AND discrepancy != 'NONE'",
        )
        .bind(uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(non_none, 0, "no non-NONE discrepancy rows for a found entry");
    }

    // 5. Reconciliation log captures the real discrepancy kinds via the real API.
    #[tokio::test]
    async fn test_reconciliation_log_entries() {
        let (db, _temp) = create_test_db().await;
        let kinds = ["MISSING_TX", "TX_CHECK_ERROR", "AUTO_RESOLVE_FAILED", "NONE"];

        for (idx, kind) in kinds.iter().enumerate() {
            db.insert_reconciliation_log(
                &format!("uuid-log-{idx}"),
                "ACTIVE",
                Some("FOUND"),
                kind,
                Some("sig"),
                Some("note"),
            )
            .await
            .unwrap();
        }

        let pool = pg_pool(&db);

        let (total,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM reconciliation_log")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(total, kinds.len() as i64, "one row per kind");

        for kind in &kinds {
            let (c,): (i64,) =
                sqlx::query_as("SELECT COUNT(*) FROM reconciliation_log WHERE discrepancy = $1")
                    .bind(kind)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(c, 1, "kind {kind} logged exactly once");
        }
    }

    // 6. Unresolved discrepancies are surfaced (resolved_at IS NULL) and resolve via
    //    the real `resolve_discrepancy` API.
    #[tokio::test]
    async fn test_unresolved_discrepancies_alert() {
        let (db, _temp) = create_test_db().await;

        // Two unresolved rows (resolved_at defaults to NULL on insert).
        let id1 = db
            .insert_reconciliation_log(
                "uuid-unres-1",
                "ACTIVE",
                Some("MISSING"),
                "MISSING_TX",
                Some("sig1"),
                Some("note"),
            )
            .await
            .unwrap();
        db.insert_reconciliation_log(
            "uuid-unres-2",
            "ACTIVE",
            Some("MISSING"),
            "MISSING_TX",
            Some("sig2"),
            Some("note"),
        )
        .await
        .unwrap();

        // Resolve one via the real API.
        db.resolve_discrepancy(id1, "AUTO", "resolved in test")
            .await
            .unwrap();

        let pool = pg_pool(&db);
        let (unresolved,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM reconciliation_log WHERE resolved_at IS NULL")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(unresolved, 1, "exactly one discrepancy remains unresolved");
    }

    // 7. A NULL exit signature on an ACTIVE position with a found entry is not a
    //    discrepancy — the run logs a NONE row.
    #[tokio::test]
    async fn test_reconciliation_handles_null_values() {
        let (db, _temp) = create_test_db().await;
        let uuid = "uuid-null-exit";
        // seed_position leaves exit_tx_signature NULL.
        seed_position(&db, uuid, "entry-sig-null-exit").await;

        let checker = StubChecker {
            found: vec!["entry-sig-null-exit".to_string()],
            not_found: vec![],
        };

        let result = run_reconciliation(db.as_ref(), &checker, &metrics()).await;

        assert_eq!(
            result.discrepancies, 0,
            "active position with found entry and NULL exit → no discrepancy"
        );

        let pool = pg_pool(&db);
        let (disc,): (String,) =
            sqlx::query_as("SELECT discrepancy FROM reconciliation_log WHERE trade_uuid = $1")
                .bind(uuid)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(disc, "NONE");
    }

    // 8. Stuck-position recovery: an EXITING position stale past the threshold is
    //    reported by the RPC-free `get_stuck_positions` query. (Full revert-to-ACTIVE
    //    needs an RPC check and is out of scope; this tests the real DB query.)
    #[tokio::test]
    async fn test_stuck_state_recovery_exiting_timeout() {
        let (db, _temp) = create_test_db().await;
        let uuid = "uuid-stuck-old";
        seed_exiting(&db, uuid, "entry-sig-stuck", "exit-sig-stuck").await;

        // Plant a stale last_updated (120s ago) past the 60s threshold.
        set_last_updated(&db, uuid, 120).await;

        let stuck = db.get_stuck_positions(60).await.unwrap();
        assert!(
            stuck.iter().any(|p| p.trade_uuid == uuid),
            "EXITING position stale by 120s should be reported stuck"
        );
    }

    // 9. A recent EXITING position (within the threshold) is NOT reported stuck.
    #[tokio::test]
    async fn test_stuck_state_recovery_recent_exiting() {
        let (db, _temp) = create_test_db().await;
        let uuid = "uuid-stuck-recent";
        seed_exiting(&db, uuid, "entry-sig-recent", "exit-sig-recent").await;

        // last_updated only 30s ago — within the 60s threshold.
        set_last_updated(&db, uuid, 30).await;

        let stuck = db.get_stuck_positions(60).await.unwrap();
        assert!(
            !stuck.iter().any(|p| p.trade_uuid == uuid),
            "EXITING position only 30s old should NOT be reported stuck"
        );
    }
}
