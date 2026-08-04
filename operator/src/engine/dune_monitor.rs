//! Dune Analytics wallet PnL monitor.
//!
//! Periodically queries Dune for 24h wallet profitability on Solana DEXes.
//! ACTIVE wallets with significant negative PnL are auto-demoted to CANDIDATE
//! — a fast feedback loop that catches failing wallets before the
//! WalletPerformanceTracker (which requires 4+ admitted losing trades).

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::config::DuneConfig;
use crate::db_abstraction::{Database, DbPool};
use crate::error::{AppError, AppResult};

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

/// Periodic Dune PnL monitor that auto-demotes losing ACTIVE wallets.
pub struct DunePnlMonitor {
    api_key: String,
    query_id: u64,
    check_interval_secs: u64,
    db: Arc<dyn Database>,
    http: reqwest::Client,
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
            db,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Run the periodic monitor loop until the cancel token fires.
    pub async fn run(&self, cancel_token: CancellationToken) {
        if self.api_key.is_empty() {
            warn!("Dune PnL monitor disabled — DUNE_API_KEY not set");
            return;
        }

        info!(
            query_id = self.query_id,
            interval_secs = self.check_interval_secs,
            "Dune PnL monitor started"
        );

        let mut interval =
            tokio::time::interval(Duration::from_secs(self.check_interval_secs));
        interval.tick().await; // consume first immediate tick

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    info!("Dune PnL monitor shutting down");
                    break;
                }
                _ = interval.tick() => {
                    if let Err(e) = self.run_check().await {
                        warn!(error = %e, "Dune PnL monitor check failed");
                    }
                }
            }
        }
    }

    /// Execute one full check cycle: query Dune → parse → demote.
    async fn run_check(&self) -> AppResult<()> {
        let started = std::time::Instant::now();

        // 1. Execute the Dune query.
        let execution_id = self.execute_query().await?;

        // 2. Poll until complete, then fetch CSV.
        let csv = self.poll_and_fetch_csv(&execution_id).await?;

        // 3. Parse losing wallets.
        let losing = Self::parse_csv(&csv);
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

    /// Trigger execution of the configured Dune query.
    async fn execute_query(&self) -> AppResult<String> {
        let url = format!("{DUNE_API_BASE}/query/{}/execute", self.query_id);
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
}
