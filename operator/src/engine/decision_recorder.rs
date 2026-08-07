//! Fire-and-forget persistence of run-scoped decision records (Phase C1).
//!
//! Every `decide_buy` / `decide_sell` call hands its [`BuyDecision`] to a
//! [`DecisionRecorder`] as the last step before returning. The recorder spawns
//! a task that inserts the row into `decision_records` — the trading path is
//! never blocked on a database write.
//!
//! ## Completeness
//! Because persistence is asynchronous and best-effort, the recorder tracks
//! two counters: `attempted` (every `record` call) and `persisted` (successful
//! inserts). Their ratio is the **completeness** metric consumed by the C4
//! go/no-go verdict gate, which requires ≥ 99%.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use rust_decimal::prelude::ToPrimitive;

use crate::db_abstraction::{Database, DbPool};
use crate::engine::run_context::RunContext;
use crate::engine::selection::{BuyDecision, SelectionRequest};
use crate::models::Action;

/// Persists decision records off the hot trading path.
pub struct DecisionRecorder {
    db: Arc<dyn Database>,
    run_context: Arc<RunContext>,
    /// Every `record` call increments this.
    attempted: Arc<AtomicU64>,
    /// Every successful insert increments this.
    persisted: Arc<AtomicU64>,
    /// Caps the number of in-flight persistence tasks so a decision flood (or
    /// slow DB) cannot pile up unbounded tasks and saturate the connection pool.
    write_semaphore: Arc<tokio::sync::Semaphore>,
}

impl DecisionRecorder {
    pub fn new(db: Arc<dyn Database>, run_context: Arc<RunContext>) -> Self {
        Self {
            db,
            run_context,
            attempted: Arc::new(AtomicU64::new(0)),
            persisted: Arc::new(AtomicU64::new(0)),
            write_semaphore: Arc::new(tokio::sync::Semaphore::new(64)),
        }
    }

    pub fn run_context(&self) -> &Arc<RunContext> {
        &self.run_context
    }

    /// Fire-and-forget: record a decision. Increments `attempted` always and
    /// `persisted` on a successful insert. Never blocks the caller.
    pub fn record(
        &self,
        decision: &BuyDecision,
        req: &SelectionRequest,
        trade_uuid: Option<&str>,
        received_at: DateTime<Utc>,
    ) {
        self.attempted.fetch_add(1, Ordering::Relaxed);

        let db = self.db.clone();
        let run_context = self.run_context.clone();
        let persisted = self.persisted.clone();
        let write_semaphore = self.write_semaphore.clone();

        // Snapshot everything the insert needs before spawning so the spawned
        // task is 'static and does not borrow the decision.
        let row = DecisionRow::from_decision(decision, req, trade_uuid, received_at, &run_context);

        tokio::spawn(async move {
            // Bound concurrent writes: when the semaphore is saturated, skip
            // the insert rather than queue unboundedly. The completeness
            // metric surfaces the loss immediately.
            let Ok(permit) = write_semaphore.try_acquire_owned() else {
                tracing::warn!(
                    decision_id = %row.decision_id,
                    "Decision persistence saturated; dropping decision record"
                );
                return;
            };
            if let Err(e) = insert_decision_record(&db, &row).await {
                tracing::warn!(
                    error = %e,
                    decision_id = %row.decision_id,
                    "Failed to persist decision record"
                );
            } else {
                persisted.fetch_add(1, Ordering::Relaxed);
            }
            drop(permit);
        });
    }

    /// Update the Jupiter quote for an admitted decision (C3).
    ///
    /// Fire-and-forget: spawns a task that sets `quote_json` on the row.
    /// Failures leave `quote_json` NULL and never block trading.
    pub fn update_quote(&self, decision_id: String, quote_json: serde_json::Value) {
        let db = self.db.clone();
        tokio::spawn(async move {
            let DbPool::PostgreSQL(pool) = db.pool();
            let res = retry_update(&pool, || {
                sqlx::query("UPDATE decision_records SET quote_json = $1 WHERE decision_id = $2")
                    .bind(quote_json.clone())
                    .bind(&decision_id)
            })
            .await;
            if let Err(e) = res {
                tracing::warn!(
                    error = %e,
                    decision_id = %decision_id,
                    "Failed to attach Jupiter quote to decision record"
                );
            }
        });
    }

    /// Store the C3 shadow-fill model output for a decision (fire-and-forget).
    ///
    /// Writes `quote_json` (the structured model payload: decision quote,
    /// delayed quote, modeled slippage) and `simulated_fill_model_version`,
    /// and records the modeled slippage as `price_impact_pct` when provided.
    pub fn update_fill_model(
        &self,
        decision_id: String,
        quote_json: serde_json::Value,
        model_version: &str,
        modeled_slippage_pct: Option<f64>,
    ) {
        let db = self.db.clone();
        let model_version = model_version.to_string();
        tokio::spawn(async move {
            let DbPool::PostgreSQL(pool) = db.pool();
            let res = retry_update(&pool, || {
                sqlx::query(
                    r#"UPDATE decision_records
                       SET quote_json = $1,
                           simulated_fill_model_version = $2,
                           price_impact_pct = COALESCE($3, price_impact_pct)
                       WHERE decision_id = $4"#,
                )
                .bind(quote_json.clone())
                .bind(&model_version)
                .bind(modeled_slippage_pct)
                .bind(&decision_id)
            })
            .await;
            if let Err(e) = res {
                tracing::warn!(
                    error = %e,
                    decision_id = %decision_id,
                    "Failed to attach shadow-fill model to decision record"
                );
            }
        });
    }

    /// Link a persisted decision to the trade it produced (C1).
    ///
    /// Called by handlers after the trade row is inserted, once the
    /// `trade_uuid` is known. (The Helius path derives the uuid from the
    /// decision size, so it is not available at decide time.) Rejected
    /// decisions are never linked — their `trade_uuid` stays NULL.
    pub fn link_trade(&self, decision_id: String, trade_uuid: String) {
        let db = self.db.clone();
        tokio::spawn(async move {
            let DbPool::PostgreSQL(pool) = db.pool();
            let res = retry_update(&pool, || {
                sqlx::query("UPDATE decision_records SET trade_uuid = $1 WHERE decision_id = $2")
                    .bind(&trade_uuid)
                    .bind(&decision_id)
            })
            .await;
            if let Err(e) = res {
                tracing::warn!(
                    error = %e,
                    decision_id = %decision_id,
                    "Failed to link decision record to trade"
                );
            }
        });
    }

    /// Persistence completeness ratio in `[0, 1]`. Returns 1.0 when nothing
    /// has been attempted yet (no evidence of loss).
    pub fn completeness(&self) -> f64 {
        let a = self.attempted.load(Ordering::Acquire);
        if a == 0 {
            return 1.0;
        }
        let p = self.persisted.load(Ordering::Acquire);
        // Clamp so transient read skew can never yield a ratio outside [0,1].
        (p as f64 / a as f64).clamp(0.0, 1.0)
    }
}

/// Flat, owned representation of a decision row ready for insertion.
struct DecisionRow {
    decision_id: String,
    run_id: String,
    trade_uuid: Option<String>,
    ingress: String,
    wallet_address: String,
    token_address: String,
    action: String,
    strategy: Option<String>,
    admitted: bool,
    rejection_code: Option<String>,
    rejection_reason: Option<String>,
    size_sol: Option<f64>,
    source_amount_sol: f64,
    wqs: Option<f64>,
    wqs_confidence: Option<f64>,
    quality_score: Option<f64>,
    consensus_wallet_count: Option<i32>,
    regime_multiplier: Option<f64>,
    token_age_hours: Option<f64>,
    liquidity_usd: Option<f64>,
    volume_24h_usd: Option<f64>,
    price_impact_pct: Option<f64>,
    source_slot: Option<i64>,
    received_at: DateTime<Utc>,
    decided_at: DateTime<Utc>,
    code_revision: String,
    config_hash: String,
    roster_hash: String,
}

/// Column precision bounds for decision_records numeric columns.
/// NUMERIC(30,18) → 12 integer digits; NUMERIC(20,10) → 10 integer digits.
/// A garbage in-flight value (corrupted DexScreener/Helius response) must
/// never fail the insert with `numeric field overflow` and lose the record
/// (observed live 2026-08-07: 12+ decision persists failed with overflow).
const NUMERIC_30_18_BOUND: f64 = 999_999_999_999.0; // 12 digits before the point
const NUMERIC_20_10_BOUND: f64 = 9_999_999_999.0; // 10 digits before the point

fn clamp_num(v: f64, bound: f64) -> f64 {
    v.clamp(-bound, bound)
}

impl DecisionRow {
    fn from_decision(
        decision: &BuyDecision,
        req: &SelectionRequest,
        trade_uuid: Option<&str>,
        received_at: DateTime<Utc>,
        run_context: &RunContext,
    ) -> Self {
        Self {
            decision_id: decision.decision_id.clone(),
            run_id: run_context.run_id.clone(),
            trade_uuid: trade_uuid.map(|s| s.to_string()),
            ingress: decision.ingress.as_str().to_string(),
            wallet_address: req.wallet_address.clone(),
            token_address: req.token_address.clone(),
            action: match req.action {
                Action::Buy => "BUY".to_string(),
                Action::Sell => "SELL".to_string(),
            },
            strategy: decision.strategy.map(|s| s.to_string()),
            admitted: decision.admitted,
            rejection_code: decision.rejection_code.map(|s| s.to_string()),
            rejection_reason: decision.rejection_reason.clone(),
            size_sol: decision
                .size_sol
                .and_then(|d| d.to_f64())
                .map(|v| clamp_num(v, NUMERIC_30_18_BOUND)),
            source_amount_sol: clamp_num(
                decision.source_amount_sol.to_f64().unwrap_or(0.0),
                NUMERIC_30_18_BOUND,
            ),
            wqs: decision.wqs,
            wqs_confidence: decision.wqs_confidence,
            quality_score: decision.quality_score,
            consensus_wallet_count: decision.consensus_wallet_count.and_then(|c| i32::try_from(c).ok()),
            regime_multiplier: decision
                .regime_multiplier
                .and_then(|d| d.to_f64())
                .map(|v| clamp_num(v, NUMERIC_20_10_BOUND)),
            token_age_hours: decision.token_age_hours,
            liquidity_usd: decision
                .liquidity_usd
                .and_then(|d| d.to_f64())
                .map(|v| clamp_num(v, NUMERIC_30_18_BOUND)),
            volume_24h_usd: decision
                .volume_24h_usd
                .and_then(|d| d.to_f64())
                .map(|v| clamp_num(v, NUMERIC_30_18_BOUND)),
            price_impact_pct: decision
                .price_impact_pct
                .and_then(|d| d.to_f64())
                .map(|v| clamp_num(v, NUMERIC_20_10_BOUND)),
            source_slot: req.source_slot.and_then(|s| i64::try_from(s).ok()),
            received_at,
            decided_at: Utc::now(),
            code_revision: run_context.code_revision.clone(),
            config_hash: run_context.config_hash.clone(),
            roster_hash: run_context.roster_hash.clone(),
        }
    }
}

/// Run a bounded-retry UPDATE against the decision records table.
///
/// Transient DB failures are retried with a short backoff so a hiccup does not
/// permanently leave the row missing `quote_json`/`trade_uuid`.
async fn retry_update<'q, F>(
    pool: &sqlx::PgPool,
    run: F,
) -> Result<(), sqlx::Error>
where
    F: Fn() -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
{
    for attempt in 1..=3u32 {
        match run().execute(pool).await {
            Ok(_) => return Ok(()),
            Err(e) if attempt < 3 => {
                tracing::warn!(
                    attempt,
                    error = %e,
                    "Decision record update failed transiently; retrying"
                );
                tokio::time::sleep(std::time::Duration::from_millis(100 * attempt as u64)).await;
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

async fn insert_decision_record(
    db: &Arc<dyn Database>,
    row: &DecisionRow,
) -> Result<(), crate::error::AppError> {
    let DbPool::PostgreSQL(pool) = db.pool();
    sqlx::query(
        r#"
        INSERT INTO decision_records (
            decision_id, run_id, trade_uuid, ingress, wallet_address, token_address,
            action, strategy, admitted, rejection_code, rejection_reason,
            size_sol, source_amount_sol, wqs, wqs_confidence, quality_score,
            consensus_wallet_count, regime_multiplier, token_age_hours, liquidity_usd,
            volume_24h_usd, price_impact_pct, source_slot, received_at, decided_at,
            code_revision, config_hash, roster_hash
        ) VALUES (
            $1, $2, $3, $4, $5, $6,
            $7, $8, $9, $10, $11,
            $12, $13, $14, $15, $16,
            $17, $18, $19, $20,
            $21, $22, $23, $24, $25,
            $26, $27, $28
        )
        "#,
    )
    .bind(&row.decision_id)
    .bind(&row.run_id)
    .bind(&row.trade_uuid)
    .bind(&row.ingress)
    .bind(&row.wallet_address)
    .bind(&row.token_address)
    .bind(&row.action)
    .bind(&row.strategy)
    .bind(row.admitted)
    .bind(&row.rejection_code)
    .bind(&row.rejection_reason)
    .bind(row.size_sol)
    .bind(row.source_amount_sol)
    .bind(row.wqs)
    .bind(row.wqs_confidence)
    .bind(row.quality_score)
    .bind(row.consensus_wallet_count)
    .bind(row.regime_multiplier)
    .bind(row.token_age_hours)
    .bind(row.liquidity_usd)
    .bind(row.volume_24h_usd)
    .bind(row.price_impact_pct)
    .bind(row.source_slot)
    .bind(row.received_at)
    .bind(row.decided_at)
    .bind(&row.code_revision)
    .bind(&row.config_hash)
    .bind(&row.roster_hash)
    .execute(&pool)
    .await
    .map_err(crate::error::AppError::Database)?;
    Ok(())
}
