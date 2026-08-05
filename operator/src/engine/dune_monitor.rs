//! Dune Analytics wallet PnL monitor.
//!
//! Periodically queries Dune for 24h wallet profitability on Solana DEXes.
//! ACTIVE wallets with significant negative PnL are auto-demoted to CANDIDATE
//! — a fast feedback loop that catches failing wallets before the
//! WalletPerformanceTracker (which requires 4+ admitted losing trades).
//!
//! Also promotes Dune-verified profitable CANDIDATE wallets to ACTIVE with
//! webhook registration — closing the gap where scout's WQS evaluation
//! rejects ground-truth profitable traders (WQS 0.0 despite positive net PnL).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::config::DuneConfig;
use crate::db_abstraction::{Database, DbPool};
use crate::error::{AppError, AppResult};
use crate::experiment::ToxicFlowDetector;
use crate::monitoring::helius::HeliusClient;
use crate::monitoring::rate_limiter::RateLimiter;
use crate::monitoring::webhook_lifecycle::{WebhookLifecycleConfig, WebhookLifecycleManager};

const DUNE_API_BASE: &str = "https://api.dune.com/api/v1";
const POLL_INTERVAL_SECS: u64 = 10;
const MAX_POLLS: usize = 30;

/// A wallet with negative 24h PnL, parsed from the Dune CSV result.
#[derive(Debug)]
struct LosingWallet {
    address: String,
    net_pnl_usd: f64,
    margin_pct: f64,
}

/// A Dune-verified profitable wallet, parsed from the top-traders CSV result.
#[derive(Debug)]
struct ProfitableWallet {
    address: String,
    trade_count: i64,
    net_pnl_usd: f64,
    roi: f64,
}

/// Components needed to promote Dune-verified wallets (webhook + toxic baseline).
#[derive(Clone)]
pub struct DunePromotionContext {
    pub helius_client: Option<Arc<HeliusClient>>,
    pub webhook_rate_limiter: Option<Arc<RateLimiter>>,
    pub webhook_lifecycle_config: Option<WebhookLifecycleConfig>,
    pub toxic_detector: Option<Arc<ToxicFlowDetector>>,
}

/// Periodic Dune PnL monitor that auto-demotes losing ACTIVE wallets and
/// promotes Dune-verified profitable CANDIDATE wallets.
pub struct DunePnlMonitor {
    api_key: String,
    query_id: u64,
    check_interval_secs: u64,
    promote_check_interval_secs: u64,
    demote_losers_enabled: bool,
    db: Arc<dyn Database>,
    http: reqwest::Client,
    promote_enabled: bool,
    promote_query_id: u64,
    promote_min_roi: f64,
    promote_max_per_cycle: u32,
    promote_max_active_total: u32,
    shadow_quality_enabled: bool,
    shadow_quality_min_samples: i64,
    shadow_quality_demote_threshold_pct: f64,
    shadow_quality_window_hours: i64,
    onchain_config: crate::config::OnchainAssessmentConfig,
    promotion_ctx: Option<DunePromotionContext>,
}

#[derive(Deserialize)]
struct ExecutionResponse {
    execution_id: String,
}

#[derive(Deserialize)]
struct StatusResponse {
    state: String,
    #[serde(default)]
    error: Option<DuneError>,
}

#[derive(Deserialize)]
struct DuneError {
    message: String,
}

impl DunePnlMonitor {
    pub fn new(config: &DuneConfig, db: Arc<dyn Database>) -> Self {
        let api_key = std::env::var("DUNE_API_KEY").unwrap_or_default();
        if api_key.is_empty() {
            warn!("DUNE_API_KEY not set — Dune PnL monitor will run but skip checks");
        }
        Self {
            api_key,
            query_id: config.pnl_query_id,
            check_interval_secs: config.check_interval_secs,
            promote_check_interval_secs: config.promote_check_interval_secs,
            demote_losers_enabled: config.demote_losers_enabled,
            db,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            promote_enabled: config.promote_enabled,
            promote_query_id: config.promote_query_id,
            promote_min_roi: config.promote_min_roi,
            promote_max_per_cycle: config.promote_max_per_cycle,
            promote_max_active_total: config.promote_max_active_total,
            shadow_quality_enabled: config.shadow_quality_enabled,
            shadow_quality_min_samples: config.shadow_quality_min_samples,
            shadow_quality_demote_threshold_pct: config.shadow_quality_demote_threshold_pct,
            shadow_quality_window_hours: config.shadow_quality_window_hours,
            onchain_config: config.onchain_assessment.clone(),
            promotion_ctx: None,
        }
    }

    /// Attach webhook registration + toxic baseline components for promotion.
    pub fn with_promotion_context(mut self, ctx: Option<DunePromotionContext>) -> Self {
        self.promotion_ctx = ctx;
        self
    }

    /// Run the periodic monitor loops until the cancel token fires.
    ///
    /// Two independent timers, decoupled from each other:
    /// - Shadow quality demote (local DB, no external API) — every
    ///   `check_interval_secs` (default 2h). Runs even without a Dune key.
    /// - Dune promote + on-chain audit (Dune + Helius API) — every
    ///   `promote_check_interval_secs` (default 6h). The Dune promote query
    ///   needs a Dune key; the on-chain audit only needs Helius, so it keeps
    ///   running (and protecting the roster) even if the Dune key is unset.
    pub async fn run(self: Arc<Self>, cancel_token: CancellationToken) {
        info!(
            query_id = self.query_id,
            promote_query_id = self.promote_query_id,
            shadow_interval_secs = self.check_interval_secs,
            promote_interval_secs = self.promote_check_interval_secs,
            demote_losers_enabled = self.demote_losers_enabled,
            "Dune PnL monitor started"
        );

        // --- Timer 1: shadow quality demote (local DB only) ---
        {
            let task_token = cancel_token.clone();
            let task_self = self.clone();
            tokio::spawn(async move {
                // Catch-up cycle ~30s after startup so demotions resume
                // immediately after a restart instead of waiting a full
                // interval. The 30s delay lets other startup tasks (webhook
                // management check, etc.) settle first.
                tokio::select! {
                    _ = task_token.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_secs(30)) => {
                        if let Err(e) = task_self.demote_shadow_losers().await {
                            warn!(error = %e, "Shadow quality startup check failed");
                        }
                    }
                }

                let mut interval =
                    tokio::time::interval(Duration::from_secs(task_self.check_interval_secs));
                interval.tick().await; // consume first immediate tick

                loop {
                    tokio::select! {
                        _ = task_token.cancelled() => break,
                        _ = interval.tick() => {
                            if let Err(e) = task_self.demote_shadow_losers().await {
                                warn!(error = %e, "Shadow quality check failed");
                            }
                        }
                    }
                }
            });
        }

        // --- Timer 2: Dune promote + on-chain audit (external APIs) ---
        {
            let task_token = cancel_token.clone();
            let task_self = self.clone();
            tokio::spawn(async move {
                let has_key = !task_self.api_key.is_empty();
                if !has_key {
                    warn!("Dune promote disabled — DUNE_API_KEY not set (on-chain audit still runs)");
                }

                // Catch-up cycle ~30s after startup.
                tokio::select! {
                    _ = task_token.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_secs(30)) => {
                        if has_key {
                            if task_self.demote_losers_enabled {
                                if let Err(e) = task_self.run_check().await {
                                    warn!(error = %e, "Dune PnL monitor startup check failed");
                                }
                            }
                            if let Err(e) = task_self.promote_dune_verified().await {
                                warn!(error = %e, "Dune PnL monitor startup promotion failed");
                            }
                        }
                        if let Err(e) = task_self.audit_actives_onchain().await {
                            warn!(error = %e, "On-chain audit startup check failed");
                        }
                    }
                }

                let mut interval = tokio::time::interval(Duration::from_secs(
                    task_self.promote_check_interval_secs,
                ));
                interval.tick().await; // consume first immediate tick

                loop {
                    tokio::select! {
                        _ = task_token.cancelled() => break,
                        _ = interval.tick() => {
                            if has_key {
                                if task_self.demote_losers_enabled {
                                    if let Err(e) = task_self.run_check().await {
                                        warn!(error = %e, "Dune PnL monitor check failed");
                                    }
                                }
                                if let Err(e) = task_self.promote_dune_verified().await {
                                    warn!(error = %e, "Dune PnL monitor promotion failed");
                                }
                            }
                            if let Err(e) = task_self.audit_actives_onchain().await {
                                warn!(error = %e, "On-chain audit check failed");
                            }
                        }
                    }
                }
            });
        }

        // Keep run() alive until cancellation (both timer tasks are spawned
        // independently and die with the shared token).
        let _ = cancel_token.cancelled().await;
        info!("Dune PnL monitor shutting down");
    }

    /// Execute one full check cycle: query Dune → parse → demote.
    async fn run_check(&self) -> AppResult<()> {
        let started = std::time::Instant::now();

        // 1. Execute the Dune query.
        let execution_id = self.execute_query(self.query_id).await?;

        // 2. Poll until complete, then fetch CSV.
        let csv = self.poll_and_fetch_csv(&execution_id).await?;

        // 3. Parse losing wallets. Fall back to JSON results if the CSV
        //    endpoint returned an empty body (intermittent Dune flakiness).
        let mut losing = Self::parse_csv(&csv);
        if losing.is_empty() {
            if let Ok(rows) = self.fetch_completed_json_rows(&execution_id).await {
                losing = Self::parse_csv(&Self::rows_to_csv(&rows));
            }
        }
        info!(
            losing_wallets = losing.len(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "Dune PnL query completed"
        );

        // 4. Cross-reference with ACTIVE wallets and demote.
        let demoted = self.demote_losing_active_wallets(&losing).await?;

        if demoted > 0 {
            warn!(
                demoted,
                total_losing = losing.len(),
                "Dune PnL monitor: demoted ACTIVE wallets with negative 24h PnL"
            );
        }

        Ok(())
    }

    /// Trigger execution of a Dune query.
    async fn execute_query(&self, query_id: u64) -> AppResult<String> {
        let url = format!("{DUNE_API_BASE}/query/{query_id}/execute");
        let resp = self
            .http
            .post(&url)
            .header("X-Dune-Api-Key", &self.api_key)
            .send()
            .await
            .map_err(|e| AppError::Internal(format!("Dune execute request failed: {e}")))?;

        let exec: ExecutionResponse = resp
            .json()
            .await
            .map_err(|e| AppError::Internal(format!("Dune execute parse failed: {e}")))?;

        Ok(exec.execution_id)
    }

    /// Poll execution status, then download CSV results.
    async fn poll_and_fetch_csv(&self, execution_id: &str) -> AppResult<String> {
        let status_url = format!("{DUNE_API_BASE}/execution/{execution_id}/status");
        let csv_url = format!("{DUNE_API_BASE}/execution/{execution_id}/results/csv");

        for _ in 0..MAX_POLLS {
            tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_SECS)).await;

            let resp = self
                .http
                .get(&status_url)
                .header("X-Dune-Api-Key", &self.api_key)
                .send()
                .await
                .map_err(|e| AppError::Internal(format!("Dune status request failed: {e}")))?;

            let status: StatusResponse = resp
                .json()
                .await
                .map_err(|e| AppError::Internal(format!("Dune status parse failed: {e}")))?;

            match status.state.as_str() {
                "QUERY_STATE_COMPLETED" => {
                    let csv_resp = self
                        .http
                        .get(&csv_url)
                        .header("X-Dune-Api-Key", &self.api_key)
                        .send()
                        .await
                        .map_err(|e| {
                            AppError::Internal(format!("Dune CSV fetch failed: {e}"))
                        })?;

                    let csv = csv_resp
                        .text()
                        .await
                        .map_err(|e| AppError::Internal(format!("Dune CSV read failed: {e}")))?;

                    return Ok(csv);
                }
                "QUERY_STATE_FAILED" => {
                    let msg = status
                        .error
                        .map(|e| e.message)
                        .unwrap_or_else(|| "unknown error".to_string());
                    return Err(AppError::Internal(format!(
                        "Dune query failed: {msg}"
                    )));
                }
                _ => { /* still pending/executing */ }
            }
        }

        Err(AppError::Internal(format!(
            "Dune query timed out after {} polls"
        , MAX_POLLS)))
    }

    /// Fetch JSON result rows for an already-COMPLETED execution.
    /// Used as a fallback when the CSV endpoint returns an empty body
    /// (observed: `/results/csv` intermittently returns nothing while
    /// `/results` returns 200 rows — silently zeroing promotion cycles).
    async fn fetch_completed_json_rows(
        &self,
        execution_id: &str,
    ) -> AppResult<Vec<serde_json::Value>> {
        let results_url = format!("{DUNE_API_BASE}/execution/{execution_id}/results");
        let resp = self
            .http
            .get(&results_url)
            .header("X-Dune-Api-Key", &self.api_key)
            .send()
            .await
            .map_err(|e| AppError::Internal(format!("Dune JSON results request failed: {e}")))?;

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| AppError::Internal(format!("Dune JSON results parse failed: {e}")))?;

        Ok(body
            .get("result")
            .and_then(|r| r.get("rows"))
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default())
    }

    /// Convert JSON result rows into CSV text so the existing CSV parsers
    /// (parse_csv / parse_profitable_csv) can be reused for both formats.
    fn rows_to_csv(rows: &[serde_json::Value]) -> String {
        let mut out = String::new();
        if let Some(first) = rows.first() {
            if let Some(obj) = first.as_object() {
                out.push_str(&obj.keys().cloned().collect::<Vec<_>>().join(","));
                out.push('\n');
            }
        }
        for row in rows {
            if let Some(obj) = row.as_object() {
                let vals: Vec<String> = obj.values().map(|v| v.to_string()).collect();
                out.push_str(&vals.join(","));
                out.push('\n');
            }
        }
        out
    }

    /// Parse the Dune CSV result into a list of losing wallets.
    /// Expected columns: wallet,trades_24h,net_pnl_usd,volume_usd,margin_pct
    fn parse_csv(csv: &str) -> Vec<LosingWallet> {
        let mut result = Vec::new();
        for (i, line) in csv.lines().enumerate() {
            if i == 0 {
                continue; // skip header
            }
            let cols: Vec<&str> = line.split(',').collect();
            if cols.len() < 5 {
                continue;
            }
            let address = cols[0].trim().to_string();
            if address.len() < 32 {
                continue;
            }
            let net_pnl_usd = cols[2].trim().parse::<f64>().unwrap_or(0.0);
            let margin_pct = cols[4].trim().parse::<f64>().unwrap_or(0.0);
            result.push(LosingWallet {
                address,
                net_pnl_usd,
                margin_pct,
            });
        }
        result
    }

    /// Demote ACTIVE wallets that appear in the Dune losing-wallet list.
    async fn demote_losing_active_wallets(&self, losing: &[LosingWallet]) -> AppResult<usize> {
        if losing.is_empty() {
            return Ok(0);
        }

        let addresses: Vec<String> = losing.iter().map(|w| w.address.clone()).collect();
        let pnl_map: std::collections::HashMap<&str, &LosingWallet> =
            losing.iter().map(|w| (w.address.as_str(), w)).collect();

        let DbPool::PostgreSQL(pool) = self.db.pool();

        // Find which losing wallets are currently ACTIVE in our system.
        let active: Vec<String> = sqlx::query_scalar(
            r#"
            SELECT address FROM wallets
            WHERE address = ANY($1) AND status = 'ACTIVE'
            "#,
        )
        .bind(&addresses)
        .fetch_all(&pool)
        .await
        .map_err(|e| AppError::Internal(format!("Dune monitor DB query failed: {e}")))?;

        if active.is_empty() {
            return Ok(0);
        }

        // Demote each one with a reason referencing its Dune PnL.
        let mut demoted = 0;
        for address in &active {
            if let Some(w) = pnl_map.get(address.as_str()) {
                let reason = format!(
                    "Dune 24h PnL: ${:.0} (margin {:.1}%) — auto-demoted",
                    w.net_pnl_usd, w.margin_pct
                );
                match self.db.demote_wallet(address, &reason).await {
                    Ok(_) => {
                        demoted += 1;
                        warn!(
                            wallet = %address,
                            net_pnl_usd = w.net_pnl_usd,
                            margin_pct = w.margin_pct,
                            "Dune PnL monitor: demoted losing ACTIVE wallet"
                        );
                    }
                    Err(e) => {
                        warn!(wallet = %address, error = %e, "Failed to demote wallet via Dune PnL monitor");
                    }
                }
            }
        }

        Ok(demoted)
    }

    /// Parse the Dune top-traders CSV into profitable wallets.
    /// Expected columns: wallet,trade_count,total_volume_usd,sell_volume_usd,
    /// buy_volume_usd,net_pnl_usd,roi,unique_tokens
    fn parse_profitable_csv(csv: &str, min_roi: f64) -> Vec<ProfitableWallet> {
        let mut result = Vec::new();
        for (i, line) in csv.lines().enumerate() {
            if i == 0 {
                continue; // skip header
            }
            let cols: Vec<&str> = line.split(',').collect();
            if cols.len() < 7 {
                continue;
            }
            let address = cols[0].trim().to_string();
            if address.len() < 32 {
                continue;
            }
            let trade_count = cols[1].trim().parse::<i64>().unwrap_or(0);
            let net_pnl_usd = cols[5].trim().parse::<f64>().unwrap_or(0.0);
            let roi = cols[6].trim().parse::<f64>().unwrap_or(0.0);
            if roi.is_nan() || roi < min_roi || net_pnl_usd <= 0.0 || trade_count < 5 {
                continue;
            }
            result.push(ProfitableWallet {
                address,
                trade_count,
                net_pnl_usd,
                roi,
            });
        }
        result
    }

    /// Promote Dune-verified profitable CANDIDATE wallets to ACTIVE with
    /// webhook registration. Overrides scout's WQS rejection (which scores
    /// ground-truth profitable traders 0.0) — the missing "promote" half of
    /// the Dune integration.
    async fn promote_dune_verified(&self) -> AppResult<usize> {
        if !self.promote_enabled || self.api_key.is_empty() {
            return Ok(0);
        }

        let started = std::time::Instant::now();

        // 1. Execute the Dune top-traders query.
        let execution_id = self.execute_query(self.promote_query_id).await?;
        let csv = self.poll_and_fetch_csv(&execution_id).await?;
        let mut profitable = Self::parse_profitable_csv(&csv, self.promote_min_roi);
        if profitable.is_empty() {
            // CSV endpoint flaky — fall back to JSON results.
            if let Ok(rows) = self.fetch_completed_json_rows(&execution_id).await {
                profitable = Self::parse_profitable_csv(
                    &Self::rows_to_csv(&rows),
                    self.promote_min_roi,
                );
            }
        }
        if profitable.is_empty() {
            return Ok(0);
        }

        // 2. Respect the ACTIVE total cap (same as scout's max_active_wallets).
        let DbPool::PostgreSQL(pool) = self.db.pool();
        let active_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM wallets WHERE status = 'ACTIVE'")
            .fetch_one(&pool)
            .await?;
        if active_total as u32 >= self.promote_max_active_total {
            info!(
                active_total,
                cap = self.promote_max_active_total,
                "Dune promotion: ACTIVE cap reached, skipping promotion cycle"
            );
            return Ok(0);
        }

        // 3. Find which profitable wallets are CANDIDATE in our system, plus
        //    ACTIVE wallets missing a webhook (retry failed registrations).
        let addresses: Vec<String> = profitable.iter().map(|w| w.address.clone()).collect();
        let candidates: Vec<String> = sqlx::query_scalar(
            r#"SELECT address FROM wallets
               WHERE address = ANY($1)
                 AND (status = 'CANDIDATE'
                      OR (status = 'ACTIVE' AND NOT EXISTS (
                          SELECT 1 FROM wallet_monitoring wm
                          WHERE wm.wallet_address = wallets.address
                            AND wm.helius_webhook_id IS NOT NULL)))"#,
        )
        .bind(&addresses)
        .fetch_all(&pool)
        .await?;
        if candidates.is_empty() {
            return Ok(0);
        }

        let pnl_map: HashMap<&str, &ProfitableWallet> =
            profitable.iter().map(|w| (w.address.as_str(), w)).collect();

        // 3b. On-chain assessment gate: verify each candidate's ACTUAL
        //     round-trip trading on Solana before admitting it. Dune's
        //     aggregate PnL can hide negative per-trade expectancy (rare big
        //     winners masking many small losers) — the on-chain assessment
        //     measures the true copy-trading edge (win rate + expectancy over
        //     completed round trips). Wallets that fail the assessment are
        //     left as CANDIDATE (shadow-monitored only).
        let onchain_config = &self.onchain_config;
        let mut verified: Vec<String> = Vec::new();
        if onchain_config.enabled {
            if let Some(ctx) = &self.promotion_ctx {
                if let Some(helius) = &ctx.helius_client {
                    let assessor =
                        crate::engine::onchain_assessment::OnchainAssessor::new(helius.clone());
                    for addr in &candidates {
                        match assessor
                            .assess_wallet(addr, onchain_config.tx_limit)
                            .await
                        {
                            Ok(a) => {
                                let pass = a.round_trips
                                    >= onchain_config.min_round_trips
                                    && a.expectancy_pct > onchain_config.min_expectancy_pct;
                                info!(
                                    wallet = %addr,
                                    round_trips = a.round_trips,
                                    win_rate = a.win_rate_pct,
                                    expectancy = a.expectancy_pct,
                                    pass,
                                    "On-chain assessment for Dune promotion"
                                );
                                if pass {
                                    verified.push(addr.clone());
                                }
                            }
                            Err(e) => {
                                warn!(
                                    wallet = %addr,
                                    error = %e,
                                    "On-chain assessment failed — skipping promotion"
                                );
                            }
                        }
                    }
                }
            }
        } else {
            verified = candidates.clone();
        }
        if verified.is_empty() {
            return Ok(0);
        }

        // 4. Promote (capped per cycle).
        let mut promoted = 0;
        for address in verified
            .iter()
            .take(self.promote_max_per_cycle as usize)
        {
            let w = pnl_map.get(address.as_str());
            let (roi, net_pnl, trades) = w
                .map(|w| (w.roi, w.net_pnl_usd, w.trade_count))
                .unwrap_or((0.0, 0.0, 0));
            let reason = format!(
                "Dune-verified profitable trader: ROI={:.2}, net PnL=${:.0}, {} trades (7d)",
                roi, net_pnl, trades
            );

            // Toxic baseline so the detector can track post-promotion ROI.
            if let Some(ctx) = &self.promotion_ctx {
                if let Some(td) = &ctx.toxic_detector {
                    if let Err(e) = td
                        .register_wallet_promotion(address.clone(), roi)
                        .await
                    {
                        warn!(
                            wallet = %address,
                            error = %e,
                            "Dune promotion: toxic baseline registration failed"
                        );
                    }
                }
            }

            // Status update: CANDIDATE -> ACTIVE (no-op for already-ACTIVE).
            match self
                .db
                .update_wallet_status_ext(address, "ACTIVE", None, Some(&reason))
                .await
            {
                Ok(true) => {
                    promoted += 1;
                    info!(
                        wallet = %address,
                        roi,
                        net_pnl_usd = net_pnl,
                        trades,
                        "Dune promotion: CANDIDATE -> ACTIVE"
                    );
                }
                Ok(false) => {
                    warn!(wallet = %address, "Dune promotion: status update returned false");
                    continue;
                }
                Err(e) => {
                    warn!(wallet = %address, error = %e, "Dune promotion: status update failed");
                    continue;
                }
            }

            // Dune-verified: set WQS to 80 so the selection WQS gate
            // (min_wqs_score 15) lets BUY signals through. These wallets were
            // never scout-evaluated, so their stored WQS is 0.0 which silently
            // blocked every signal (WQS_TOO_LOW rejections).
            {
                let DbPool::PostgreSQL(pool) = self.db.pool();
                if let Err(e) = sqlx::query(
                    r#"UPDATE wallets
                       SET wqs_score = GREATEST(COALESCE(wqs_score, 0), 80.0),
                           notes = COALESCE(notes, '') || ' | Dune-verified: WQS floor 80'
                       WHERE address = $1"#,
                )
                .bind(address)
                .execute(&pool)
                .await
                {
                    warn!(wallet = %address, error = %e, "Dune promotion: WQS update failed");
                }
            }

            // Webhook registration so signals start flowing.
            if let Some(ctx) = &self.promotion_ctx {
                if let (Some(helius), Some(limiter), Some(wl_config)) = (
                    &ctx.helius_client,
                    &ctx.webhook_rate_limiter,
                    &ctx.webhook_lifecycle_config,
                ) {
                    let manager = WebhookLifecycleManager::new(
                        self.db.clone(),
                        helius.clone(),
                        limiter.clone(),
                        wl_config.clone(),
                    );
                    match manager.register_wallet_webhook(address).await {
                        Ok(r) if r.success => {
                            info!(
                                wallet = %address,
                                webhook_id = %r.webhook_id,
                                "Dune promotion: webhook registered"
                            );
                        }
                        Ok(r) => {
                            warn!(
                                wallet = %address,
                                error = ?r.error_message,
                                "Dune promotion: webhook registration failed"
                            );
                        }
                        Err(e) => {
                            warn!(
                                wallet = %address,
                                error = %e,
                                "Dune promotion: webhook registration error"
                            );
                        }
                    }
                }
            }
        }

        if promoted > 0 {
            warn!(
                promoted,
                eligible = candidates.len(),
                elapsed_ms = started.elapsed().as_millis() as u64,
                "Dune PnL monitor: promoted verified profitable wallets"
            );
        }
        Ok(promoted)
    }

    /// Demote ACTIVE wallets whose admitted DEX signals lose money under our
    /// own exit logic (shadow `mirror_main` exits over a rolling window).
    ///
    /// Rationale: a wallet's own behavior (wallet_sell) can diverge wildly
    /// from what we would realize (Grxr6m: mirror +2.2% vs wallet_sell
    /// -25.9%). What matters for OUR PnL is how its signals perform with OUR
    /// exits — mirror_main. Wallets with consistently negative mirror_main
    /// PnL on admitted DEX signals are demoted, with WQS lowered below the
    /// auto-promote threshold so the refill does not instantly re-promote.
    async fn demote_shadow_losers(&self) -> AppResult<usize> {
        // Local DB only — no Dune API, so no Dune-key guard (a missing
        // DUNE_API_KEY must not silently disable shadow quality demotion).
        if !self.shadow_quality_enabled {
            return Ok(0);
        }

        let DbPool::PostgreSQL(pool) = self.db.pool();

        #[derive(Debug, sqlx::FromRow)]
        struct ShadowLoser {
            wallet_address: String,
            n: i64,
            avg_pnl: f64,
        }

        let losers: Vec<ShadowLoser> = sqlx::query_as(
            r#"
            SELECT sp.wallet_address, COUNT(*) AS n, AVG(se.pnl_pct)::float8 AS avg_pnl
            FROM shadow_exits se
            JOIN shadow_positions sp ON sp.shadow_id = se.shadow_id
            WHERE sp.opened_at > NOW() - ($1 || ' hours')::interval
              AND se.exit_strategy = 'mirror_main'
              AND sp.token_address NOT LIKE '%pump'
              AND sp.main_admitted = true
            GROUP BY sp.wallet_address
            HAVING COUNT(*) >= $2 AND AVG(se.pnl_pct) < $3
            "#,
        )
        .bind(self.shadow_quality_window_hours)
        .bind(self.shadow_quality_min_samples)
        .bind(self.shadow_quality_demote_threshold_pct)
        .fetch_all(&pool)
        .await?;

        if losers.is_empty() {
            return Ok(0);
        }

        let mut demoted = 0;
        for l in &losers {
            // Only demote wallets that are still ACTIVE.
            let is_active: Option<String> = sqlx::query_scalar(
                "SELECT address FROM wallets WHERE address = $1 AND status = 'ACTIVE'",
            )
            .bind(&l.wallet_address)
            .fetch_optional(&pool)
            .await?;
            if is_active.is_none() {
                continue;
            }

            let reason = format!(
                "Shadow quality: {} admitted DEX signals avg {:.2}% over {}h (threshold {:.1}%)",
                l.n,
                l.avg_pnl,
                self.shadow_quality_window_hours,
                self.shadow_quality_demote_threshold_pct
            );
            match self.db.demote_wallet(&l.wallet_address, &reason).await {
                Ok(_) => {
                    // Lower WQS below the auto-promote threshold (30) so the
                    // refill does not instantly re-promote this wallet.
                    let _ = sqlx::query(
                        "UPDATE wallets SET wqs_score = LEAST(COALESCE(wqs_score, 0), 10.0) WHERE address = $1",
                    )
                    .bind(&l.wallet_address)
                    .execute(&pool)
                    .await;
                    demoted += 1;
                    warn!(
                        wallet = %l.wallet_address,
                        avg_pnl = l.avg_pnl,
                        samples = l.n,
                        "Shadow quality: demoted negative-EV wallet"
                    );
                }
                Err(e) => {
                    warn!(
                        wallet = %l.wallet_address,
                        error = %e,
                        "Shadow quality demote failed"
                    );
                }
            }
        }

        if demoted > 0 {
            warn!(
                demoted,
                eligible = losers.len(),
                "Shadow quality monitor: demoted wallets with consistently negative shadow PnL"
            );
        }
        Ok(demoted)
    }

    /// Retroactive on-chain audit of the ACTIVE roster.
    ///
    /// Assesses ACTIVE wallets with recent trade activity using the same
    /// round-trip expectancy analysis used for admission, and demotes those
    /// that fail it (fewer than `min_round_trips` completed round trips or
    /// non-positive expectancy). Catches wallets admitted under the old
    /// criteria — e.g. 2snHHreXbp with 13 round trips at -89% expectancy
    /// that remained ACTIVE and trading. WQS is floored below the
    /// auto-promote threshold so the refill cannot instantly re-promote.
    ///
    /// API-cost control: only the `audit_max_per_cycle` most-active wallets
    /// are assessed per cycle, so the sweep spreads over a few cycles.
    async fn audit_actives_onchain(&self) -> AppResult<usize> {
        // Helius API only — no Dune API, so no Dune-key guard (a missing
        // DUNE_API_KEY must not silently disable the retroactive audit).
        if !self.onchain_config.enabled || !self.onchain_config.audit_actives_enabled {
            return Ok(0);
        }
        let Some(ctx) = &self.promotion_ctx else {
            return Ok(0);
        };
        let Some(helius) = &ctx.helius_client else {
            return Ok(0);
        };

        let DbPool::PostgreSQL(pool) = self.db.pool();

        let actives: Vec<String> = sqlx::query_scalar(
            r#"
            SELECT w.address FROM wallets w
            WHERE w.status = 'ACTIVE'
              AND EXISTS (SELECT 1 FROM decision_records dr
                          WHERE dr.wallet_address = w.address
                            AND dr.decided_at > NOW() - INTERVAL '24 hours')
            ORDER BY (SELECT count(*) FROM decision_records dr2
                      WHERE dr2.wallet_address = w.address
                        AND dr2.decided_at > NOW() - INTERVAL '24 hours') DESC
            LIMIT $1
            "#,
        )
        .bind(self.onchain_config.audit_max_per_cycle as i64)
        .fetch_all(&pool)
        .await?;

        if actives.is_empty() {
            return Ok(0);
        }

        let assessor = crate::engine::onchain_assessment::OnchainAssessor::new(helius.clone());
        let mut demoted = 0;
        for wallet in &actives {
            match assessor
                .assess_wallet(wallet, self.onchain_config.tx_limit)
                .await
            {
                Ok(a) => {
                    let pass = a.round_trips >= self.onchain_config.min_round_trips
                        && a.expectancy_pct > self.onchain_config.min_expectancy_pct;
                    info!(
                        wallet = %wallet,
                        txs_fetched = a.txs_fetched,
                        round_trips = a.round_trips,
                        win_rate = a.win_rate_pct,
                        expectancy = a.expectancy_pct,
                        pass,
                        "On-chain audit of ACTIVE wallet"
                    );
                    if !pass {
                        let reason = format!(
                            "On-chain audit: {} round trips, expectancy {:.2}% — no proven copy-trading edge",
                            a.round_trips, a.expectancy_pct
                        );
                        match self.db.demote_wallet(wallet, &reason).await {
                            Ok(_) => {
                                let _ = sqlx::query(
                                    "UPDATE wallets SET wqs_score = LEAST(COALESCE(wqs_score, 0), 10.0) WHERE address = $1",
                                )
                                .bind(wallet)
                                .execute(&pool)
                                .await;
                                demoted += 1;
                                warn!(
                                    wallet = %wallet,
                                    round_trips = a.round_trips,
                                    expectancy = a.expectancy_pct,
                                    "On-chain audit: demoted ACTIVE wallet (no proven edge)"
                                );
                            }
                            Err(e) => {
                                warn!(wallet = %wallet, error = %e, "On-chain audit demote failed");
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(wallet = %wallet, error = %e, "On-chain audit assessment failed");
                }
            }
        }

        if demoted > 0 {
            warn!(
                demoted,
                audited = actives.len(),
                "On-chain audit: demoted wallets without proven round-trip edge"
            );
        }
        Ok(demoted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_csv_basic() {
        let csv = "wallet,trades_24h,net_pnl_usd,volume_usd,margin_pct\n\
            7oLDfykjJVDmR8ZKcgoehW6z4zhnBnGC8mGUFLhDHxxg,50,-500.0,5000.0,-10.0\n\
            short,5,-100.0,1000.0,-10.0\n";

        let wallets = DunePnlMonitor::parse_csv(csv);
        assert_eq!(wallets.len(), 1); // "short" filtered out (< 32 chars)
        assert_eq!(wallets[0].address, "7oLDfykjJVDmR8ZKcgoehW6z4zhnBnGC8mGUFLhDHxxg");
        assert!((wallets[0].net_pnl_usd - (-500.0)).abs() < 0.01);
        assert!((wallets[0].margin_pct - (-10.0)).abs() < 0.01);
    }

    #[test]
    fn test_parse_csv_empty() {
        assert!(DunePnlMonitor::parse_csv("wallet,trades_24h,net_pnl_usd,volume_usd,margin_pct\n").is_empty());
        assert!(DunePnlMonitor::parse_csv("").is_empty());
    }

    #[test]
    fn test_parse_profitable_csv_filters() {
        let csv = "wallet,trade_count,total_volume_usd,sell_volume_usd,buy_volume_usd,net_pnl_usd,roi,unique_tokens\n\
            7oLDfykjJVDmR8ZKcgoehW6z4zhnBnGC8mGUFLhDHxxg,50,50000,40000,10000,30000,3.0,9\n\
            9HsFJKqobLFZ6QLT7xXhS3ggDfSGTJPUh2Rfug4VFGWh,3,1000,800,200,600,3.0,2\n\
            badroi,50,50000,40000,10000,30000,0.5,9\n\
            short,50,50000,40000,10000,30000,3.0,9\n\
            A6Wch1mJJ1PyooNSAUtctcNmQTxqtkcWManMBQPmKceM,25,5000,4000,1000,2000,2.0,4\n";

        let wallets = DunePnlMonitor::parse_profitable_csv(csv, 1.2);
        // 7oLD: ROI 3.0 >= 1.2, 50 trades >= 5, net PnL > 0 -> kept
        // 9HsFJKqo: 3 trades < 5 -> filtered
        // badroi: ROI 0.5 < 1.2 -> filtered
        // short: < 32 chars -> filtered
        // A6Wch1mJ: ROI 2.0, 25 trades -> kept
        assert_eq!(wallets.len(), 2);
        assert_eq!(wallets[0].address, "7oLDfykjJVDmR8ZKcgoehW6z4zhnBnGC8mGUFLhDHxxg");
        assert_eq!(wallets[1].address, "A6Wch1mJJ1PyooNSAUtctcNmQTxqtkcWManMBQPmKceM");
    }

    #[test]
    fn test_parse_profitable_csv_nan_roi() {
        let csv = "wallet,trade_count,total_volume_usd,sell_volume_usd,buy_volume_usd,net_pnl_usd,roi,unique_tokens\n\
            7oLDfykjJVDmR8ZKcgoehW6z4zhnBnGC8mGUFLhDHxxg,50,50000,40000,0,30000,NaN,9\n";
        assert!(DunePnlMonitor::parse_profitable_csv(csv, 1.2).is_empty());
    }
}
