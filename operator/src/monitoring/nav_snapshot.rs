//! Periodic mark-to-market NAV snapshot writer for the dashboard equity curve.
//!
//! Every [`SNAPSHOT_INTERVAL_SECS`] seconds this task records one row into
//! `portfolio_snapshots` with:
//!
//! ```text
//! nav_sol = total_capital_sol + realized_pnl_sol + unrealized_pnl_sol
//! ```
//!
//! `realized_pnl_sol` and `unrealized_pnl_sol` come from the positions table
//! (CLOSED / ACTIVE), so the resulting equity curve matches the operator's own
//! portfolio-risk and circuit-breaker accounting. Rows older than
//! [`RETENTION_DAYS`] are purged periodically. The task exits cleanly when the
//! supplied [`CancellationToken`] is cancelled.

use std::sync::Arc;

use rust_decimal::Decimal;
use tokio_util::sync::CancellationToken;

use crate::config::AppConfig;
use crate::db_abstraction::Database;
use crate::error::AppResult;
use crate::price_cache::PriceCache;

/// How often a NAV snapshot is written.
const SNAPSHOT_INTERVAL_SECS: u64 = 60;
/// Maximum age of retained snapshots (older rows are purged).
const RETENTION_DAYS: i32 = 90;
/// Purge once per hour (every 60 ticks @ 60s).
const PURGE_EVERY_N_TICKS: u64 = 60;

/// Spawn the periodic NAV snapshot writer. Returns immediately; the task runs
/// until `cancel_token` is cancelled.
#[allow(clippy::too_many_arguments)]
pub fn spawn_nav_snapshot_task(
    db: Arc<dyn Database>,
    config: Arc<tokio::sync::RwLock<AppConfig>>,
    price_cache: Arc<PriceCache>,
    trade_mode: String,
    cancel_token: CancellationToken,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(
            SNAPSHOT_INTERVAL_SECS,
        ));
        let mut tick: u64 = 0;

        tracing::info!(
            interval_secs = SNAPSHOT_INTERVAL_SECS,
            retention_days = RETENTION_DAYS,
            "NAV snapshot task started"
        );

        loop {
            tokio::select! {
                biased;
                _ = cancel_token.cancelled() => break,
                _ = interval.tick() => {
                    if let Err(e) = record_snapshot(&db, &config, &price_cache, &trade_mode).await {
                        tracing::warn!(error = %e, "Failed to record NAV snapshot");
                    }

                    tick = tick.saturating_add(1);
                    if tick.is_multiple_of(PURGE_EVERY_N_TICKS) {
                        match db.delete_portfolio_snapshots_before(RETENTION_DAYS).await {
                            Ok(n) if n > 0 => {
                                tracing::debug!(purged = n, "Purged old NAV snapshots");
                            }
                            Ok(_) => {}
                            Err(e) => tracing::debug!(error = %e, "NAV snapshot purge failed"),
                        }
                    }
                }
            }
        }

        tracing::info!("NAV snapshot task stopped");
    });
}

/// Compute and persist a single NAV snapshot.
async fn record_snapshot(
    db: &Arc<dyn Database>,
    config: &Arc<tokio::sync::RwLock<AppConfig>>,
    price_cache: &Arc<PriceCache>,
    trade_mode: &str,
) -> AppResult<()> {
    let capital = config.read().await.position_sizing.total_capital_sol;
    let realized = db.get_total_realized_pnl().await.unwrap_or(Decimal::ZERO);

    let positions = db.get_active_positions().await.unwrap_or_default();
    let open_positions = positions.len() as i32;
    let unrealized: Decimal = positions.iter().filter_map(|p| p.unrealized_pnl_sol).sum();

    let nav = capital + realized + unrealized;
    let sol_price = price_cache.get_sol_price_usd();

    db.record_portfolio_snapshot(
        nav,
        capital,
        realized,
        unrealized,
        open_positions,
        sol_price,
        Some(trade_mode.to_string()),
    )
    .await
}
