//! Profitability go/no-go verdict endpoint (Phase C4).
//!
//! `GET /api/v1/profitability/verdict` evaluates the pre-registered gates
//! (see `docs/profitability-gates.md`) against the immutable `decision_records`
//! joined with closed `trades`. The verdict is computed live from current
//! DB state; there is no cached assertion.

use axum::{extract::Query, extract::State, Json};
use rust_decimal::prelude::ToPrimitive;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::db_abstraction::{Database, DbPool};
use crate::error::AppError;

/// Optional query: evaluate a specific run (defaults to the current run).
#[derive(Debug, Deserialize)]
pub struct VerdictQuery {
    pub run_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct VerdictResponse {
    pub verdict: String,
    pub run_id: String,
    pub gates: VerdictGates,
    pub computed_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize, Clone)]
pub struct VerdictGates {
    pub sample_size: GateValue,
    pub net_return: NetReturnGate,
    pub cohort_positivity: CohortGate,
    pub paper_live_bias: BiasGate,
    pub max_single_loss: LossGate,
    pub max_drawdown: DrawdownGate,
    pub integrity: IntegrityGate,
    pub completeness: CompletenessGate,
}

#[derive(Debug, Serialize, Clone)]
pub struct GateValue {
    pub status: String,
    pub value: i64,
    pub threshold: i64,
}

#[derive(Debug, Serialize, Clone)]
pub struct NetReturnGate {
    pub status: String,
    pub lower_95_ci: f64,
    pub upper_95_ci: f64,
    pub mean: f64,
    pub samples: i64,
}

#[derive(Debug, Serialize, Clone)]
pub struct CohortGate {
    pub status: String,
    pub cohorts_evaluated: i64,
    pub cohorts_positive: i64,
    pub cohort_min_count: i64,
}

#[derive(Debug, Serialize, Clone)]
pub struct BiasGate {
    pub status: String,
    pub declared_bias: f64,
    pub threshold: f64,
}

#[derive(Debug, Serialize, Clone)]
pub struct LossGate {
    pub status: String,
    pub worst_loss_pct: f64,
    pub threshold: f64,
}

#[derive(Debug, Serialize, Clone)]
pub struct DrawdownGate {
    pub status: String,
    pub max_drawdown_pct: f64,
    pub threshold: f64,
}

#[derive(Debug, Serialize, Clone)]
pub struct IntegrityGate {
    pub status: String,
    pub invalid_pnl_count: i64,
    pub missing_outcomes: i64,
}

#[derive(Debug, Serialize, Clone)]
pub struct CompletenessGate {
    pub status: String,
    pub rate: f64,
    pub threshold: f64,
}

/// Cached profitability verdict for live trading enforcement.
///
/// Stored in `Arc<RwLock<Option<CachedVerdict>>>` and refreshed
/// periodically in the background. The verdict is checked before
/// executing signals in `SignalProcessor::process_signal`.
#[derive(Debug, Clone)]
pub struct CachedVerdict {
    pub verdict: String,
    pub gates: VerdictGates,
    pub computed_at: std::time::Instant,
}

/// Row from the outcome join (decision_records ⨝ trades).
pub struct Outcome {
    pub net_pnl_sol: f64,
    pub size_sol: f64,
    pub strategy: Option<String>,
    pub liquidity_usd: Option<f64>,
    pub price_impact_pct: Option<f64>,
    pub decided_at: chrono::DateTime<chrono::Utc>,
}

const SAMPLE_THRESHOLD: i64 = 60;
const NET_RETURN_THRESHOLD: f64 = 0.0;
const COHORT_MIN_COUNT: i64 = 10;
const BIAS_THRESHOLD: f64 = 0.05;
const MAX_SINGLE_LOSS_THRESHOLD: f64 = 0.10;
const MAX_DRAWDOWN_THRESHOLD: f64 = 0.20;
const COMPLETENESS_THRESHOLD: f64 = 0.99;

/// GET /api/v1/profitability/verdict
pub async fn profitability_verdict(
    State(state): State<Arc<crate::handlers::ApiState>>,
    Query(params): Query<VerdictQuery>,
) -> Result<Json<VerdictResponse>, AppError> {
    let pool = match state.db.pool() {
        DbPool::PostgreSQL(p) => p,
    };

    let run_id = params
        .run_id
        .or_else(|| state.run_context.as_ref().map(|rc| rc.run_id.clone()))
        .unwrap_or_default();

    let total_capital_sol: f64 = state
        .config
        .read()
        .await
        .position_sizing
        .total_capital_sol
        .to_f64()
        .unwrap_or(0.0)
        .max(1.0);

    // ── Outcomes: admitted BUY decisions linked to closed SELL trades ──
    let outcomes = fetch_outcomes(&pool, &run_id).await?;

    // ── Integrity: admitted decisions with no closed outcome + invalid PnL ──
    let missing_outcomes = count_missing_outcomes(&pool, &run_id).await?;
    let invalid_pnl = count_invalid_pnl(&pool, &run_id).await?;

    // ── Completeness: DecisionRecorder counters (current run only) ──
    let (completeness_rate, completeness_ok) = match &state.decision_recorder {
        Some(recorder) => {
            let rate = recorder.completeness();
            (rate, rate >= COMPLETENESS_THRESHOLD)
        }
        None => (1.0, true), // no recorder → no evidence of loss
    };

    let (gates, verdict) = evaluate_gates(
        outcomes,
        missing_outcomes,
        invalid_pnl,
        completeness_rate,
        completeness_ok,
        total_capital_sol,
    );

    Ok(Json(VerdictResponse {
        verdict: verdict.to_string(),
        run_id,
        gates,
        computed_at: chrono::Utc::now(),
    }))
}

/// Evaluate all 8 gates against pre-fetched outcomes and integrity/completeness
/// counters. Returns the per-gate results and the overall verdict string.
///
/// Pure function: no DB, no state. Testable in isolation.
pub fn evaluate_gates(
    outcomes: Vec<Outcome>,
    missing_outcomes: i64,
    invalid_pnl: i64,
    completeness_rate: f64,
    completeness_ok: bool,
    total_capital_sol: f64,
) -> (VerdictGates, &'static str) {
    let sample_size = outcomes.len() as i64;
    let sample_status = if sample_size >= SAMPLE_THRESHOLD {
        "PASS"
    } else {
        "FAIL"
    };

    // ── Net return per deployed SOL (95% CI, normal approx) ──
    let returns: Vec<f64> = outcomes
        .iter()
        .map(|o| {
            let denom = if o.size_sol > 0.0 {
                o.size_sol
            } else {
                total_capital_sol
            };
            o.net_pnl_sol / denom
        })
        .collect();
    let (mean, lower_ci, upper_ci, net_status) = confidence_interval(&returns);

    // ── Cohort positivity (by strategy; liquidity/latency bands optional) ──
    let (cohorts_evaluated, cohorts_positive, cohort_status) =
        cohort_positivity(&outcomes, COHORT_MIN_COUNT);

    // ── Paper/live bias: mean modeled slippage from price_impact_pct ──
    let bias_vals: Vec<f64> = outcomes
        .iter()
        .filter_map(|o| o.price_impact_pct)
        .collect();
    let declared_bias = if bias_vals.is_empty() {
        0.0
    } else {
        bias_vals.iter().sum::<f64>() / bias_vals.len() as f64
    };
    let bias_status = if declared_bias.abs() <= BIAS_THRESHOLD {
        "PASS"
    } else {
        "FAIL"
    };

    // ── Max single loss (% of deployed capital) ──
    let worst_loss_pct = outcomes
        .iter()
        .map(|o| {
            let loss = if o.net_pnl_sol < 0.0 { -o.net_pnl_sol } else { 0.0 };
            loss / total_capital_sol
        })
        .fold(0.0_f64, f64::max);
    let loss_status = if worst_loss_pct <= MAX_SINGLE_LOSS_THRESHOLD {
        "PASS"
    } else {
        "FAIL"
    };

    // ── Max drawdown of cumulative PnL (% of deployed capital) ──
    let max_drawdown_pct = max_drawdown(&outcomes) / total_capital_sol;
    let drawdown_status = if max_drawdown_pct <= MAX_DRAWDOWN_THRESHOLD {
        "PASS"
    } else {
        "FAIL"
    };

    let integrity_fail = missing_outcomes > 0 || invalid_pnl > 0;
    let integrity_status = if integrity_fail { "FAIL" } else { "PASS" };
    let completeness_status = if completeness_ok { "PASS" } else { "FAIL" };

    // ── Overall verdict ──
    let verdict = if integrity_fail || completeness_status == "FAIL" {
        "STOP"
    } else if sample_size < SAMPLE_THRESHOLD {
        "INCONCLUSIVE"
    } else if net_status == "INCONCLUSIVE"
        || cohort_status == "FAIL"
        || bias_status == "FAIL"
        || loss_status == "FAIL"
        || drawdown_status == "FAIL"
    {
        "INCONCLUSIVE"
    } else {
        "GO"
    };

    (
        VerdictGates {
            sample_size: GateValue {
                status: sample_status.to_string(),
                value: sample_size,
                threshold: SAMPLE_THRESHOLD,
            },
            net_return: NetReturnGate {
                status: net_status.to_string(),
                lower_95_ci: lower_ci,
                upper_95_ci: upper_ci,
                mean,
                samples: sample_size,
            },
            cohort_positivity: CohortGate {
                status: cohort_status.to_string(),
                cohorts_evaluated,
                cohorts_positive,
                cohort_min_count: COHORT_MIN_COUNT,
            },
            paper_live_bias: BiasGate {
                status: bias_status.to_string(),
                declared_bias,
                threshold: BIAS_THRESHOLD,
            },
            max_single_loss: LossGate {
                status: loss_status.to_string(),
                worst_loss_pct,
                threshold: MAX_SINGLE_LOSS_THRESHOLD,
            },
            max_drawdown: DrawdownGate {
                status: drawdown_status.to_string(),
                max_drawdown_pct,
                threshold: MAX_DRAWDOWN_THRESHOLD,
            },
            integrity: IntegrityGate {
                status: integrity_status.to_string(),
                invalid_pnl_count: invalid_pnl,
                missing_outcomes,
            },
            completeness: CompletenessGate {
                status: completeness_status.to_string(),
                rate: completeness_rate,
                threshold: COMPLETENESS_THRESHOLD,
            },
        },
        verdict,
    )
}

pub async fn fetch_outcomes(
    pool: &sqlx::Pool<sqlx::Postgres>,
    run_id: &str,
) -> Result<Vec<Outcome>, AppError> {
    let rows = sqlx::query_as::<_, OutcomeRow>(
        r#"
        SELECT
            t.net_pnl_sol::DOUBLE PRECISION       AS "net_pnl_sol",
            dr.size_sol::DOUBLE PRECISION         AS "size_sol",
            dr.strategy                           AS "strategy",
            dr.liquidity_usd::DOUBLE PRECISION    AS "liquidity_usd",
            dr.price_impact_pct::DOUBLE PRECISION AS "price_impact_pct",
            dr.decided_at                         AS "decided_at"
        FROM decision_records dr
        JOIN trades t ON t.trade_uuid = dr.trade_uuid
        WHERE dr.admitted = TRUE
          AND dr.action = 'BUY'
          AND t.status = 'CLOSED'
          AND t.pnl_data_valid = TRUE
          AND t.side = 'SELL'
          AND ($1 = '' OR dr.run_id = $1)
        "#,
    )
    .bind(run_id)
    .fetch_all(pool)
    .await
    .map_err(AppError::Database)?;

    Ok(rows
        .into_iter()
        .map(|r| Outcome {
            net_pnl_sol: r.net_pnl_sol,
            size_sol: r.size_sol,
            strategy: r.strategy,
            liquidity_usd: r.liquidity_usd,
            price_impact_pct: r.price_impact_pct,
            decided_at: r.decided_at,
        })
        .collect())
}

#[derive(sqlx::FromRow)]
struct OutcomeRow {
    net_pnl_sol: f64,
    size_sol: f64,
    strategy: Option<String>,
    liquidity_usd: Option<f64>,
    price_impact_pct: Option<f64>,
    decided_at: chrono::DateTime<chrono::Utc>,
}

pub async fn count_missing_outcomes(
    pool: &sqlx::Pool<sqlx::Postgres>,
    run_id: &str,
) -> Result<i64, AppError> {
    let n: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM decision_records dr
        WHERE dr.admitted = TRUE
          AND dr.action = 'BUY'
          AND dr.trade_uuid IS NULL
          AND ($1 = '' OR dr.run_id = $1)
        "#,
    )
    .bind(run_id)
    .fetch_one(pool)
    .await
    .map_err(AppError::Database)?;
    Ok(n)
}

pub async fn count_invalid_pnl(
    pool: &sqlx::Pool<sqlx::Postgres>,
    run_id: &str,
) -> Result<i64, AppError> {
    let n: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM decision_records dr
        JOIN trades t ON t.trade_uuid = dr.trade_uuid
        WHERE dr.admitted = TRUE
          AND dr.action = 'BUY'
          AND t.pnl_data_valid = FALSE
          AND ($1 = '' OR dr.run_id = $1)
        "#,
    )
    .bind(run_id)
    .fetch_one(pool)
    .await
    .map_err(AppError::Database)?;
    Ok(n)
}

/// 95% confidence interval (normal approximation). Returns
/// (mean, lower, upper, status). Status is INCONCLUSIVE when the lower bound
/// crosses zero (or sample too small for a meaningful interval).
fn confidence_interval(returns: &[f64]) -> (f64, f64, f64, &'static str) {
    let n = returns.len() as f64;
    if returns.is_empty() {
        return (0.0, 0.0, 0.0, "INCONCLUSIVE");
    }
    let mean = returns.iter().sum::<f64>() / n;
    if n < 2.0 {
        return (mean, mean, mean, "INCONCLUSIVE");
    }
    let variance = returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1.0);
    let se = variance.sqrt() / n.sqrt();
    // z_{0.975} ≈ 1.96
    let margin = 1.96 * se;
    let lower = mean - margin;
    let upper = mean + margin;
    let status = if lower > NET_RETURN_THRESHOLD {
        "PASS"
    } else {
        "INCONCLUSIVE"
    };
    (mean, lower, upper, status)
}

/// Cohort positivity by strategy. Every cohort with ≥ `min_count` outcomes
/// must show positive mean net return.
fn cohort_positivity(outcomes: &[Outcome], min_count: i64) -> (i64, i64, &'static str) {
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for o in outcomes {
        let key = o.strategy.clone().unwrap_or_else(|| "UNKNOWN".to_string());
        groups.entry(key).or_default().push(o.net_pnl_sol);
    }
    let mut evaluated = 0_i64;
    let mut positive = 0_i64;
    let mut all_positive = true;
    for (_key, vals) in groups {
        if (vals.len() as i64) < min_count {
            continue;
        }
        evaluated += 1;
        let mean = vals.iter().sum::<f64>() / vals.len() as f64;
        if mean > 0.0 {
            positive += 1;
        } else {
            all_positive = false;
        }
    }
    let status = if evaluated == 0 {
        "INCONCLUSIVE"
    } else if all_positive {
        "PASS"
    } else {
        "FAIL"
    };
    (evaluated, positive, status)
}

/// Peak-to-trough drawdown of cumulative PnL (absolute SOL).
fn max_drawdown(outcomes: &[Outcome]) -> f64 {
    let mut sorted: Vec<&Outcome> = outcomes.iter().collect();
    sorted.sort_by_key(|o| o.decided_at);
    let mut peak = 0.0_f64;
    let mut cumulative = 0.0_f64;
    let mut max_dd = 0.0_f64;
    for o in sorted {
        cumulative += o.net_pnl_sol;
        if cumulative > peak {
            peak = cumulative;
        }
        let dd = peak - cumulative;
        if dd > max_dd {
            max_dd = dd;
        }
    }
    max_dd
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(pnl: f64, decided_at: i64, strategy: Option<&str>) -> Outcome {
        Outcome {
            net_pnl_sol: pnl,
            size_sol: 1.0,
            strategy: strategy.map(|s| s.to_string()),
            liquidity_usd: None,
            price_impact_pct: None,
            decided_at: chrono::DateTime::from_timestamp(decided_at, 0).unwrap(),
        }
    }

    #[test]
    fn confidence_interval_empty_is_inconclusive() {
        let (mean, lo, hi, status) = confidence_interval(&[]);
        assert_eq!(status, "INCONCLUSIVE");
        assert_eq!(mean, 0.0);
        assert!(lo <= 0.0 && hi >= 0.0);
    }

    #[test]
    fn confidence_interval_positive_mean_passes_when_n_large() {
        // 100 outcomes of +0.01 → tight positive CI
        let returns: Vec<f64> = (0..100).map(|_| 0.01).collect();
        let (_mean, lo, _hi, status) = confidence_interval(&returns);
        assert_eq!(status, "PASS");
        assert!(lo > 0.0);
    }

    #[test]
    fn confidence_interval_negative_mean_is_inconclusive() {
        let returns: Vec<f64> = (0..100).map(|_| -0.005).collect();
        let (_mean, _lo, hi, status) = confidence_interval(&returns);
        assert_eq!(status, "INCONCLUSIVE");
        assert!(hi < 0.0);
    }

    #[test]
    fn cohort_positivity_all_positive_passes() {
        let outcomes: Vec<Outcome> = (0..12)
            .map(|i| outcome(if i % 2 == 0 { 0.1 } else { 0.05 }, i, Some("SHIELD")))
            .collect();
        let (eval, pos, status) = cohort_positivity(&outcomes, 10);
        assert_eq!(eval, 1);
        assert_eq!(pos, 1);
        assert_eq!(status, "PASS");
    }

    #[test]
    fn cohort_positivity_negative_mean_fails() {
        let outcomes: Vec<Outcome> =
            (0..12).map(|i| outcome(-0.01, i, Some("SPEAR"))).collect();
        let (eval, _pos, status) = cohort_positivity(&outcomes, 10);
        assert_eq!(eval, 1);
        assert_eq!(status, "FAIL");
    }

    #[test]
    fn cohort_positivity_below_min_count_is_inconclusive() {
        let outcomes: Vec<Outcome> =
            (0..5).map(|i| outcome(0.1, i, Some("SHIELD"))).collect();
        let (eval, _pos, status) = cohort_positivity(&outcomes, 10);
        assert_eq!(eval, 0);
        assert_eq!(status, "INCONCLUSIVE");
    }

    #[test]
    fn max_drawdown_measures_peak_to_trough() {
        // +1, +1, -3, +1 → cumulative 1, 2, -1, 0 → peak 2, trough -1 → dd 3
        let outcomes = vec![
            outcome(1.0, 1, None),
            outcome(1.0, 2, None),
            outcome(-3.0, 3, None),
            outcome(1.0, 4, None),
        ];
        assert!((max_drawdown(&outcomes) - 3.0).abs() < 1e-9);
    }

    #[test]
    fn max_drawdown_monotonic_increase_is_zero() {
        let outcomes = vec![outcome(0.5, 1, None), outcome(0.5, 2, None)];
        assert_eq!(max_drawdown(&outcomes), 0.0);
    }

    // ── Pure evaluate_gates verdict precedence ──

    #[test]
    fn evaluate_gates_stop_precedence() {
        // integrity failure (missing outcome) overrides everything, even a
        // tiny sample that would otherwise be INCONCLUSIVE.
        let outcomes: Vec<Outcome> = Vec::new();
        let (gates, verdict) = evaluate_gates(outcomes, 1, 0, 1.0, true, 1.0);
        assert_eq!(verdict, "STOP");
        assert_eq!(gates.integrity.status, "FAIL");
    }

    #[test]
    fn evaluate_gates_inconclusive_when_sample_small() {
        // 59 positive outcomes: numeric gates would pass, but sample < 60 →
        // INCONCLUSIVE (never GO until sample threshold is met).
        let outcomes: Vec<Outcome> = (0..59).map(|i| outcome(0.01, i, Some("SHIELD"))).collect();
        let (gates, verdict) = evaluate_gates(outcomes, 0, 0, 1.0, true, 1.0);
        assert_eq!(verdict, "INCONCLUSIVE");
        assert_eq!(gates.sample_size.status, "FAIL");
        assert_eq!(gates.net_return.status, "PASS");
    }

    #[test]
    fn evaluate_gates_go() {
        // 60 positive outcomes, single positive cohort, no bias/loss/drawdown,
        // integrity & completeness clean → GO.
        let outcomes: Vec<Outcome> = (0..60).map(|i| outcome(0.01, i, Some("SHIELD"))).collect();
        let (gates, verdict) = evaluate_gates(outcomes, 0, 0, 1.0, true, 1.0);
        assert_eq!(verdict, "GO");
        assert_eq!(gates.sample_size.status, "PASS");
        assert_eq!(gates.net_return.status, "PASS");
        assert_eq!(gates.cohort_positivity.status, "PASS");
        assert_eq!(gates.integrity.status, "PASS");
        assert_eq!(gates.completeness.status, "PASS");
    }
}

