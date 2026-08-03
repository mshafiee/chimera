//! Profitability go/no-go verdict integration tests (Phase C4).
//!
//! Exercises the full gate pipeline against a real PostgreSQL database:
//!   fetch_outcomes + count_missing_outcomes + count_invalid_pnl + evaluate_gates
//!
//! Each test gets an isolated database via `common::create_test_pg_db()` (the
//! shared-DB bug in the harness was fixed so concurrent runs are isolated).
//! These tests never construct the full `ApiState`; they drive the extracted
//! pure evaluator directly, which is the whole point of the refactor.

use chimera_operator::handlers::{
    count_invalid_pnl, count_missing_outcomes, evaluate_gates, fetch_outcomes, VerdictGates,
};
use sqlx::{Pool, Postgres};

use crate::common;

const RUN: &str = "run-verdict";
const CAPITAL_SOL: f64 = 1.0;

/// Absolute epoch base (s) for deterministic `decided_at` ordering. Offsets are
/// added on top so drawdown sequences sort in insertion order.
const BASE_TS: i64 = 1_700_000_000;

fn uid(prefix: &str, run_id: &str) -> String {
    format!("{prefix}-{run_id}-{}", uuid::Uuid::new_v4())
}

/// Insert a `decision_records` row. Mirrors the NOT-NULL contract of
/// `0008_decision_records.sql` (decision_id, run_id, ingress, wallet_address,
/// token_address, action, admitted, source_amount_sol, received_at, decided_at,
/// code_revision, config_hash).
#[allow(clippy::too_many_arguments)]
async fn insert_decision(
    pool: &Pool<Postgres>,
    decision_id: &str,
    run_id: &str,
    trade_uuid: Option<&str>,
    wallet: &str,
    token: &str,
    action: &str,
    admitted: bool,
    strategy: Option<&str>,
    size_sol: Option<&str>,
    price_impact_pct: Option<f64>,
    decided_at_offset: i64,
) {
    sqlx::query(
        r#"INSERT INTO decision_records
             (decision_id, run_id, trade_uuid, ingress, wallet_address, token_address,
              action, admitted, strategy, size_sol, source_amount_sol,
              price_impact_pct, received_at, decided_at, code_revision, config_hash)
           VALUES ($1,$2,$3,'webhook',$4,$5,$6,$7,$8,$9::NUMERIC,0::NUMERIC,
                   $10, to_timestamp($11), to_timestamp($11), 'test-rev', 'test-hash')"#,
    )
    .bind(decision_id)
    .bind(run_id)
    .bind(trade_uuid)
    .bind(wallet)
    .bind(token)
    .bind(action)
    .bind(admitted)
    .bind(strategy)
    .bind(size_sol)
    .bind(price_impact_pct)
    .bind(BASE_TS + decided_at_offset)
    .execute(pool)
    .await
    .unwrap();
}

/// Insert a `trades` row. `strategy` must satisfy the SHIELD/SPEAR/EXIT CHECK.
#[allow(clippy::too_many_arguments)]
async fn insert_trade(
    pool: &Pool<Postgres>,
    trade_uuid: &str,
    wallet: &str,
    token: &str,
    strategy: &str,
    side: &str,
    amount_sol: &str,
    status: &str,
    net_pnl_sol: Option<&str>,
    pnl_data_valid: bool,
) {
    sqlx::query(
        r#"INSERT INTO trades
             (trade_uuid, wallet_address, token_address, strategy, side, amount_sol,
              status, net_pnl_sol, pnl_data_valid)
           VALUES ($1,$2,$3,$4,$5,$6::NUMERIC,$7,$8::NUMERIC,$9)"#,
    )
    .bind(trade_uuid)
    .bind(wallet)
    .bind(token)
    .bind(strategy)
    .bind(side)
    .bind(amount_sol)
    .bind(status)
    .bind(net_pnl_sol)
    .bind(pnl_data_valid)
    .execute(pool)
    .await
    .unwrap();
}

/// Insert an admitted BUY decision linked to a closed SELL trade (a valid
/// outcome). Returns (decision_id, trade_uuid).
#[allow(clippy::too_many_arguments)]
async fn seed_outcome(
    pool: &Pool<Postgres>,
    run_id: &str,
    wallet: &str,
    token: &str,
    strategy: &str,
    size_sol: &str,
    net_pnl_sol: &str,
    price_impact_pct: Option<f64>,
    decided_at_offset: i64,
    pnl_data_valid: bool,
) -> (String, String) {
    let decision_id = uid("dec", run_id);
    let trade_uuid = uid("trd", run_id);
    // Fail loudly on bad fixtures instead of silently normalizing them — a
    // silent rewrite would mask a decision↔trade strategy mismatch.
    assert!(
        matches!(strategy, "SHIELD" | "SPEAR" | "EXIT"),
        "unexpected strategy in seed_outcome: {strategy}"
    );
    let trade_strategy = strategy;
    insert_decision(
        pool,
        &decision_id,
        run_id,
        Some(&trade_uuid),
        wallet,
        token,
        "BUY",
        true,
        Some(strategy),
        Some(size_sol),
        price_impact_pct,
        decided_at_offset,
    )
    .await;
    insert_trade(
        pool,
        &trade_uuid,
        wallet,
        token,
        trade_strategy,
        "SELL",
        size_sol,
        "CLOSED",
        Some(net_pnl_sol),
        pnl_data_valid,
    )
    .await;
    (decision_id, trade_uuid)
}

/// Insert an admitted BUY decision with trade_uuid = NULL (missing outcome).
async fn seed_missing_outcome(
    pool: &Pool<Postgres>,
    run_id: &str,
    wallet: &str,
    token: &str,
) -> String {
    let decision_id = uid("dec", run_id);
    insert_decision(
        pool,
        &decision_id,
        run_id,
        None,
        wallet,
        token,
        "BUY",
        true,
        None,
        None,
        None,
        0,
    )
    .await;
    decision_id
}

/// Seed `n` valid outcomes with identical PnL and the same strategy.
async fn seed_n_outcomes(
    pool: &Pool<Postgres>,
    run_id: &str,
    n: i64,
    strategy: &str,
    pnl_each: &str,
    start_offset: i64,
    batch: u64,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for i in 0..n {
        out.push(
            seed_outcome(
                pool,
                run_id,
                // Per-batch prefix keeps wallet/token names unique across
                // multiple seed_n_outcomes calls in one test run.
                &format!("b{batch}-w{i}"),
                &format!("b{batch}-tok{i}"),
                strategy,
                "1.0",
                pnl_each,
                None,
                start_offset + i,
                true,
            )
            .await,
        );
    }
    out
}

/// Drive the full verdict pipeline against the DB for a run.
async fn run_verdict(
    pool: &Pool<Postgres>,
    run_id: &str,
    completeness_rate: f64,
    completeness_ok: bool,
    total_capital_sol: f64,
) -> (VerdictGates, &'static str) {
    let outcomes = fetch_outcomes(pool, run_id).await.unwrap();
    let missing = count_missing_outcomes(pool, run_id).await.unwrap();
    let invalid = count_invalid_pnl(pool, run_id).await.unwrap();
    evaluate_gates(
        outcomes,
        missing,
        invalid,
        completeness_rate,
        completeness_ok,
        total_capital_sol,
    )
}

// ─── Sample size gate ───────────────────────────────────────────────────────

#[tokio::test]
async fn sample_size_below_60_is_inconclusive() {
    let (db, _tmp) = common::create_test_pg_db().await;
    let pool = common::pg_pool(&db);
    seed_n_outcomes(&pool, RUN, 59, "SHIELD", "0.01", 0, 0).await;

    let (gates, verdict) = run_verdict(&pool, RUN, 1.0, true, CAPITAL_SOL).await;
    assert_eq!(verdict, "INCONCLUSIVE");
    assert_eq!(gates.sample_size.status, "FAIL");
    assert_eq!(gates.sample_size.value, 59);
}

#[tokio::test]
async fn sample_size_at_60_passes_gate() {
    let (db, _tmp) = common::create_test_pg_db().await;
    let pool = common::pg_pool(&db);
    seed_n_outcomes(&pool, RUN, 60, "SHIELD", "0.01", 0, 1).await;

    let (gates, _verdict) = run_verdict(&pool, RUN, 1.0, true, CAPITAL_SOL).await;
    assert_eq!(gates.sample_size.status, "PASS");
    assert_eq!(gates.sample_size.value, 60);
}

// ─── Net return gate ────────────────────────────────────────────────────────

#[tokio::test]
async fn net_return_positive_ci_passes() {
    let (db, _tmp) = common::create_test_pg_db().await;
    let pool = common::pg_pool(&db);
    seed_n_outcomes(&pool, RUN, 100, "SHIELD", "0.01", 0, 2).await;

    let (gates, _verdict) = run_verdict(&pool, RUN, 1.0, true, CAPITAL_SOL).await;
    assert_eq!(gates.net_return.status, "PASS");
    assert!(gates.net_return.lower_95_ci > 0.0);
}

#[tokio::test]
async fn net_return_clearly_negative_mean_fails() {
    // 100 × -0.5%: the mean is clearly negative and the upper CI stays below
    // 0 → net FAIL → STOP. (INCONCLUSIVE is reserved for CIs that straddle 0.)
    let (db, _tmp) = common::create_test_pg_db().await;
    let pool = common::pg_pool(&db);
    seed_n_outcomes(&pool, RUN, 100, "SHIELD", "-0.005", 0, 3).await;

    let (gates, verdict) = run_verdict(&pool, RUN, 1.0, true, CAPITAL_SOL).await;
    assert_eq!(gates.net_return.status, "FAIL");
    assert!(gates.net_return.upper_95_ci < 0.0);
    assert_eq!(verdict, "STOP");
}

#[tokio::test]
async fn net_return_ci_crossing_zero_is_inconclusive() {
    let (db, _tmp) = common::create_test_pg_db().await;
    let pool = common::pg_pool(&db);
    // Alternating +0.1 / -0.1 → mean ~0, wide CI crossing zero.
    for i in 0..100 {
        let pnl = if i % 2 == 0 { "0.1" } else { "-0.1" };
        seed_outcome(
            &pool,
            RUN,
            &format!("w{i}"),
            &format!("tok{i}"),
            "SHIELD",
            "1.0",
            pnl,
            None,
            i,
            true,
        )
        .await;
    }

    let (gates, verdict) = run_verdict(&pool, RUN, 1.0, true, CAPITAL_SOL).await;
    assert_eq!(gates.net_return.status, "INCONCLUSIVE");
    assert!(gates.net_return.lower_95_ci <= 0.0);
    assert_eq!(verdict, "INCONCLUSIVE");
}

// ─── Cohort positivity ──────────────────────────────────────────────────────

#[tokio::test]
async fn cohort_all_positive_passes() {
    let (db, _tmp) = common::create_test_pg_db().await;
    let pool = common::pg_pool(&db);
    seed_n_outcomes(&pool, RUN, 12, "SHIELD", "0.1", 0, 4).await;
    seed_n_outcomes(&pool, RUN, 12, "SPEAR", "0.05", 100, 5).await;

    let (gates, _verdict) = run_verdict(&pool, RUN, 1.0, true, CAPITAL_SOL).await;
    assert_eq!(gates.cohort_positivity.status, "PASS");
    assert_eq!(gates.cohort_positivity.cohorts_evaluated, 2);
    assert_eq!(gates.cohort_positivity.cohorts_positive, 2);
}

#[tokio::test]
async fn cohort_one_negative_fails() {
    let (db, _tmp) = common::create_test_pg_db().await;
    let pool = common::pg_pool(&db);
    seed_n_outcomes(&pool, RUN, 12, "SHIELD", "0.1", 0, 6).await;
    seed_n_outcomes(&pool, RUN, 12, "SPEAR", "-0.01", 100, 7).await;

    let (gates, verdict) = run_verdict(&pool, RUN, 1.0, true, CAPITAL_SOL).await;
    assert_eq!(gates.cohort_positivity.status, "FAIL");
    assert_eq!(verdict, "INCONCLUSIVE");
}

#[tokio::test]
async fn cohort_below_min_count_skipped() {
    let (db, _tmp) = common::create_test_pg_db().await;
    let pool = common::pg_pool(&db);
    seed_n_outcomes(&pool, RUN, 5, "SHIELD", "0.1", 0, 8).await;

    let (gates, _verdict) = run_verdict(&pool, RUN, 1.0, true, CAPITAL_SOL).await;
    assert_eq!(gates.cohort_positivity.status, "INCONCLUSIVE");
    assert_eq!(gates.cohort_positivity.cohorts_evaluated, 0);
}

// ─── Paper/live bias ────────────────────────────────────────────────────────

#[tokio::test]
async fn bias_within_threshold_passes() {
    let (db, _tmp) = common::create_test_pg_db().await;
    let pool = common::pg_pool(&db);
    for i in 0..60 {
        seed_outcome(
            &pool,
            RUN,
            &format!("w{i}"),
            &format!("tok{i}"),
            "SHIELD",
            "1.0",
            "0.01",
            Some(0.03),
            i,
            true,
        )
        .await;
    }

    let (gates, _verdict) = run_verdict(&pool, RUN, 1.0, true, CAPITAL_SOL).await;
    assert_eq!(gates.paper_live_bias.status, "PASS");
    assert!((gates.paper_live_bias.declared_bias - 0.03).abs() < 1e-9);
}

#[tokio::test]
async fn bias_exceeds_threshold_fails() {
    let (db, _tmp) = common::create_test_pg_db().await;
    let pool = common::pg_pool(&db);
    for i in 0..60 {
        seed_outcome(
            &pool,
            RUN,
            &format!("w{i}"),
            &format!("tok{i}"),
            "SHIELD",
            "1.0",
            "0.01",
            Some(0.07),
            i,
            true,
        )
        .await;
    }

    let (gates, verdict) = run_verdict(&pool, RUN, 1.0, true, CAPITAL_SOL).await;
    assert_eq!(gates.paper_live_bias.status, "FAIL");
    assert_eq!(verdict, "INCONCLUSIVE");
}

#[tokio::test]
async fn bias_null_values_reported_inconclusive() {
    // Missing bias data (NULL price_impact_pct) must NOT read as zero bias:
    // the gate reports INCONCLUSIVE so "no data" is distinguishable from
    // "no bias".
    let (db, _tmp) = common::create_test_pg_db().await;
    let pool = common::pg_pool(&db);
    seed_n_outcomes(&pool, RUN, 60, "SHIELD", "0.01", 0, 9).await;

    let (gates, _verdict) = run_verdict(&pool, RUN, 1.0, true, CAPITAL_SOL).await;
    assert_eq!(gates.paper_live_bias.status, "INCONCLUSIVE");
    assert_eq!(gates.paper_live_bias.declared_bias, 0.0);
}

// ─── Max single loss ────────────────────────────────────────────────────────

#[tokio::test]
async fn single_loss_within_10pct_passes() {
    let (db, _tmp) = common::create_test_pg_db().await;
    let pool = common::pg_pool(&db);
    seed_n_outcomes(&pool, RUN, 59, "SHIELD", "0.01", 0, 10).await;
    seed_outcome(
        &pool, RUN, "w-loss", "tok-loss", "SHIELD", "1.0", "-0.08", None, 999, true,
    )
    .await;

    let (gates, _verdict) = run_verdict(&pool, RUN, 1.0, true, CAPITAL_SOL).await;
    assert_eq!(gates.max_single_loss.status, "PASS");
    assert!((gates.max_single_loss.worst_loss_pct - 0.08).abs() < 1e-9);
}

#[tokio::test]
async fn single_loss_exceeds_10pct_fails() {
    let (db, _tmp) = common::create_test_pg_db().await;
    let pool = common::pg_pool(&db);
    seed_n_outcomes(&pool, RUN, 59, "SHIELD", "0.01", 0, 11).await;
    seed_outcome(
        &pool, RUN, "w-loss", "tok-loss", "SHIELD", "1.0", "-0.15", None, 999, true,
    )
    .await;

    let (gates, verdict) = run_verdict(&pool, RUN, 1.0, true, CAPITAL_SOL).await;
    assert_eq!(gates.max_single_loss.status, "FAIL");
    assert!((gates.max_single_loss.worst_loss_pct - 0.15).abs() < 1e-9);
    assert_eq!(verdict, "INCONCLUSIVE");
}

// ─── Max drawdown ───────────────────────────────────────────────────────────

#[tokio::test]
async fn drawdown_within_20pct_passes() {
    let (db, _tmp) = common::create_test_pg_db().await;
    let pool = common::pg_pool(&db);
    // +0.01 ×30 (peak 0.30), −0.05 ×3 (drop to 0.15 → dd 0.15), +0.01 ×27.
    seed_n_outcomes(&pool, RUN, 30, "SHIELD", "0.01", 0, 12).await;
    seed_n_outcomes(&pool, RUN, 3, "SHIELD", "-0.05", 30, 13).await;
    seed_n_outcomes(&pool, RUN, 27, "SHIELD", "0.01", 33, 14).await;

    let (gates, _verdict) = run_verdict(&pool, RUN, 1.0, true, CAPITAL_SOL).await;
    assert_eq!(gates.max_drawdown.status, "PASS");
    assert!((gates.max_drawdown.max_drawdown_pct - 0.15).abs() < 1e-9);
}

#[tokio::test]
async fn drawdown_exceeds_20pct_fails() {
    let (db, _tmp) = common::create_test_pg_db().await;
    let pool = common::pg_pool(&db);
    // +0.02 ×40 (peak 0.80), −0.09 ×3 (drop to 0.53 → dd 0.27), +0.02 ×17.
    seed_n_outcomes(&pool, RUN, 40, "SHIELD", "0.02", 0, 15).await;
    seed_n_outcomes(&pool, RUN, 3, "SHIELD", "-0.09", 40, 16).await;
    seed_n_outcomes(&pool, RUN, 17, "SHIELD", "0.02", 43, 17).await;

    let (gates, verdict) = run_verdict(&pool, RUN, 1.0, true, CAPITAL_SOL).await;
    assert_eq!(gates.max_drawdown.status, "FAIL");
    assert!((gates.max_drawdown.max_drawdown_pct - 0.27).abs() < 1e-9);
    assert_eq!(verdict, "INCONCLUSIVE");
}

// ─── Integrity gate (STOP) ──────────────────────────────────────────────────

#[tokio::test]
async fn missing_outcome_triggers_stop() {
    let (db, _tmp) = common::create_test_pg_db().await;
    let pool = common::pg_pool(&db);
    seed_n_outcomes(&pool, RUN, 60, "SHIELD", "0.01", 0, 18).await;
    seed_missing_outcome(&pool, RUN, "w-miss", "tok-miss").await;

    let (gates, verdict) = run_verdict(&pool, RUN, 1.0, true, CAPITAL_SOL).await;
    assert_eq!(verdict, "STOP");
    assert_eq!(gates.integrity.status, "FAIL");
    assert_eq!(gates.integrity.missing_outcomes, 1);
}

#[tokio::test]
async fn invalid_pnl_triggers_stop() {
    let (db, _tmp) = common::create_test_pg_db().await;
    let pool = common::pg_pool(&db);
    seed_n_outcomes(&pool, RUN, 60, "SHIELD", "0.01", 0, 19).await;
    seed_outcome(
        &pool, RUN, "w-bad", "tok-bad", "SHIELD", "1.0", "0.01", None, 999, false,
    )
    .await;

    let (gates, verdict) = run_verdict(&pool, RUN, 1.0, true, CAPITAL_SOL).await;
    assert_eq!(verdict, "STOP");
    assert_eq!(gates.integrity.status, "FAIL");
    assert_eq!(gates.integrity.invalid_pnl_count, 1);
}

#[tokio::test]
async fn integrity_failure_overrides_good_gates() {
    let (db, _tmp) = common::create_test_pg_db().await;
    let pool = common::pg_pool(&db);
    // All numeric gates would pass, but an admitted BUY with no outcome → STOP.
    seed_n_outcomes(&pool, RUN, 60, "SHIELD", "0.01", 0, 20).await;
    seed_missing_outcome(&pool, RUN, "w-miss", "tok-miss").await;

    let (gates, verdict) = run_verdict(&pool, RUN, 1.0, true, CAPITAL_SOL).await;
    assert_eq!(verdict, "STOP");
    assert_eq!(gates.sample_size.status, "PASS");
    assert_eq!(gates.net_return.status, "PASS");
}

// ─── Completeness gate (STOP) ───────────────────────────────────────────────

#[tokio::test]
async fn completeness_below_99pct_triggers_stop() {
    let (db, _tmp) = common::create_test_pg_db().await;
    let pool = common::pg_pool(&db);
    seed_n_outcomes(&pool, RUN, 60, "SHIELD", "0.01", 0, 21).await;

    let (gates, verdict) = run_verdict(&pool, RUN, 0.98, false, CAPITAL_SOL).await;
    assert_eq!(verdict, "STOP");
    assert_eq!(gates.completeness.status, "FAIL");
    assert!((gates.completeness.rate - 0.98).abs() < 1e-9);
}

#[tokio::test]
async fn completeness_above_99pct_passes() {
    let (db, _tmp) = common::create_test_pg_db().await;
    let pool = common::pg_pool(&db);
    seed_n_outcomes(&pool, RUN, 60, "SHIELD", "0.01", 0, 22).await;

    let (gates, _verdict) = run_verdict(&pool, RUN, 1.0, true, CAPITAL_SOL).await;
    assert_eq!(gates.completeness.status, "PASS");
}

// ─── Verdict precedence ─────────────────────────────────────────────────────

#[tokio::test]
async fn stop_beats_inconclusive() {
    let (db, _tmp) = common::create_test_pg_db().await;
    let pool = common::pg_pool(&db);
    // No outcomes (sample=0 → would be INCONCLUSIVE) but integrity fails → STOP.
    seed_missing_outcome(&pool, RUN, "w-miss", "tok-miss").await;

    let (gates, verdict) = run_verdict(&pool, RUN, 1.0, true, CAPITAL_SOL).await;
    assert_eq!(verdict, "STOP");
    assert_eq!(gates.sample_size.status, "FAIL");
}

#[tokio::test]
async fn inconclusive_beats_go() {
    let (db, _tmp) = common::create_test_pg_db().await;
    let pool = common::pg_pool(&db);
    // Sample sufficient, but net CI crosses zero → INCONCLUSIVE (not GO).
    for i in 0..60 {
        let pnl = if i % 2 == 0 { "0.1" } else { "-0.1" };
        seed_outcome(
            &pool,
            RUN,
            &format!("w{i}"),
            &format!("tok{i}"),
            "SHIELD",
            "1.0",
            pnl,
            None,
            i,
            true,
        )
        .await;
    }

    let (_gates, verdict) = run_verdict(&pool, RUN, 1.0, true, CAPITAL_SOL).await;
    assert_eq!(verdict, "INCONCLUSIVE");
}

#[tokio::test]
async fn all_gates_pass_yields_go() {
    let (db, _tmp) = common::create_test_pg_db().await;
    let pool = common::pg_pool(&db);
    seed_n_outcomes(&pool, RUN, 60, "SHIELD", "0.01", 0, 23).await;

    // The bias gate needs evidence: NULL price_impact now reports
    // INCONCLUSIVE (and blocks GO), so seed a small declared bias.
    sqlx::query("UPDATE decision_records SET price_impact_pct = 0.05 WHERE run_id = $1")
        .bind(RUN)
        .execute(&pool)
        .await
        .unwrap();

    let (gates, verdict) = run_verdict(&pool, RUN, 1.0, true, CAPITAL_SOL).await;
    assert_eq!(verdict, "GO");
    assert_eq!(gates.sample_size.status, "PASS");
    assert_eq!(gates.net_return.status, "PASS");
    assert_eq!(gates.cohort_positivity.status, "PASS");
    assert_eq!(gates.integrity.status, "PASS");
    assert_eq!(gates.completeness.status, "PASS");
}

// ─── SQL join correctness ───────────────────────────────────────────────────

#[tokio::test]
async fn outcome_join_requires_admitted_buy_and_closed_sell() {
    let (db, _tmp) = common::create_test_pg_db().await;
    let pool = common::pg_pool(&db);

    // (a) admitted BUY + closed SELL → counts.
    seed_outcome(
        &pool, RUN, "w-a", "tok-a", "SHIELD", "1.0", "0.01", None, 0, true,
    )
    .await;
    // (b) rejected BUY + closed SELL → excluded.
    let d_b = uid("dec", RUN);
    let t_b = uid("trd", RUN);
    insert_decision(
        &pool,
        &d_b,
        RUN,
        Some(&t_b),
        "w-b",
        "tok-b",
        "BUY",
        false,
        Some("SHIELD"),
        Some("1.0"),
        None,
        1,
    )
    .await;
    insert_trade(
        &pool,
        &t_b,
        "w-b",
        "tok-b",
        "SHIELD",
        "SELL",
        "1.0",
        "CLOSED",
        Some("0.01"),
        true,
    )
    .await;
    // (c) admitted BUY + PENDING SELL → excluded.
    let d_c = uid("dec", RUN);
    let t_c = uid("trd", RUN);
    insert_decision(
        &pool,
        &d_c,
        RUN,
        Some(&t_c),
        "w-c",
        "tok-c",
        "BUY",
        true,
        Some("SHIELD"),
        Some("1.0"),
        None,
        2,
    )
    .await;
    insert_trade(
        &pool,
        &t_c,
        "w-c",
        "tok-c",
        "SHIELD",
        "SELL",
        "1.0",
        "PENDING",
        Some("0.01"),
        true,
    )
    .await;
    // (d) admitted SELL + closed SELL → excluded.
    let d_d = uid("dec", RUN);
    let t_d = uid("trd", RUN);
    insert_decision(
        &pool,
        &d_d,
        RUN,
        Some(&t_d),
        "w-d",
        "tok-d",
        "SELL",
        true,
        Some("SHIELD"),
        Some("1.0"),
        None,
        3,
    )
    .await;
    insert_trade(
        &pool,
        &t_d,
        "w-d",
        "tok-d",
        "SHIELD",
        "SELL",
        "1.0",
        "CLOSED",
        Some("0.01"),
        true,
    )
    .await;

    let outcomes = fetch_outcomes(&pool, RUN).await.unwrap();
    assert_eq!(
        outcomes.len(),
        1,
        "only the admitted-BUY + closed-SELL row counts"
    );
}

#[tokio::test]
async fn outcome_join_filters_invalid_pnl() {
    let (db, _tmp) = common::create_test_pg_db().await;
    let pool = common::pg_pool(&db);
    // Closed SELL with pnl_data_valid = FALSE: excluded from outcomes but counted.
    seed_outcome(
        &pool, RUN, "w-bad", "tok-bad", "SHIELD", "1.0", "0.01", None, 0, false,
    )
    .await;

    let outcomes = fetch_outcomes(&pool, RUN).await.unwrap();
    assert!(
        outcomes.is_empty(),
        "invalid-PnL outcome must not appear in outcomes"
    );
    let invalid = count_invalid_pnl(&pool, RUN).await.unwrap();
    assert_eq!(invalid, 1);
}

#[tokio::test]
async fn run_id_filtering() {
    let (db, _tmp) = common::create_test_pg_db().await;
    let pool = common::pg_pool(&db);
    seed_n_outcomes(&pool, "runA", 3, "SHIELD", "0.01", 0, 100).await;
    seed_n_outcomes(&pool, "runB", 2, "SPEAR", "0.01", 0, 101).await;

    let a = fetch_outcomes(&pool, "runA").await.unwrap();
    let b = fetch_outcomes(&pool, "runB").await.unwrap();
    assert_eq!(a.len(), 3);
    assert_eq!(b.len(), 2, "run A outcomes must not leak into run B");
    assert_eq!(count_missing_outcomes(&pool, "runB").await.unwrap(), 0);
    assert_eq!(count_invalid_pnl(&pool, "runA").await.unwrap(), 0);
}
