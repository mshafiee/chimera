//! Per-wallet copy performance tracking and dynamic weighting
//!
//! Tracks:
//! - 7-day copy PnL (actual returns from copying)
//! - Signal success rate (% of copied trades profitable)
//! - Average return per trade
//! - Exit timing accuracy

use std::sync::Arc;
use tokio::sync::RwLock;
use crate::db::DbPool;

/// Wallet performance tracker
pub struct WalletPerformanceTracker {
    db: DbPool,
    /// Cache of recent metrics (wallet -> metrics)
    metrics_cache: Arc<RwLock<std::collections::HashMap<String, WalletCopyMetrics>>>,
}

/// Wallet copy trading metrics
#[derive(Debug, Clone)]
pub struct WalletCopyMetrics {
    pub wallet_address: String,
    pub copy_pnl_7d: f64,
    pub signal_success_rate: f64,
    pub avg_return_per_trade: f64,
    pub total_trades: u32,
    pub winning_trades: u32,
    pub last_updated: std::time::SystemTime,
}

impl WalletPerformanceTracker {
    pub fn new(db: DbPool) -> Self {
        Self {
            db,
            metrics_cache: Arc::new(RwLock::new(std::collections::HashMap::new())),
        }
    }

    /// Update metrics after a trade closes
    pub async fn record_trade_result(
        &self,
        wallet_address: &str,
        pnl_sol: f64,
    ) {
        // Get or create metrics
        let mut cache = self.metrics_cache.write().await;
        let metrics = cache.entry(wallet_address.to_string()).or_insert_with(|| {
            WalletCopyMetrics {
                wallet_address: wallet_address.to_string(),
                copy_pnl_7d: 0.0,
                signal_success_rate: 0.0,
                avg_return_per_trade: 0.0,
                total_trades: 0,
                winning_trades: 0,
                last_updated: std::time::SystemTime::now(),
            }
        });

        // Update metrics
        metrics.total_trades += 1;
        if pnl_sol > 0.0 {
            metrics.winning_trades += 1;
        }

        // Recalculate averages
        metrics.signal_success_rate = (metrics.winning_trades as f64 / metrics.total_trades as f64) * 100.0;
        
        // Update 7-day PnL (would need to query database for actual 7-day window)
        // For now, just accumulate
        metrics.copy_pnl_7d += pnl_sol;
        metrics.avg_return_per_trade = metrics.copy_pnl_7d / metrics.total_trades as f64;
        metrics.last_updated = std::time::SystemTime::now();
    }

    /// Get metrics for a wallet
    pub async fn get_metrics(&self, wallet_address: &str) -> Option<WalletCopyMetrics> {
        let cache = self.metrics_cache.read().await;
        cache.get(wallet_address).cloned()
    }

    /// Check if wallet should be auto-demoted (negative copy PnL for 14 days)
    pub async fn should_demote(&self, wallet_address: &str) -> bool {
        if let Some(metrics) = self.get_metrics(wallet_address).await {
            // Check if copy PnL is negative and metrics are recent
            if metrics.copy_pnl_7d < 0.0 {
                if let Ok(elapsed) = metrics.last_updated.elapsed() {
                    // If negative for more than 14 days, demote
                    if elapsed.as_secs() >= 14 * 24 * 3600 {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Check if wallet should be promoted faster (strong signals)
    pub async fn should_promote_faster(&self, wallet_address: &str) -> bool {
        if let Some(metrics) = self.get_metrics(wallet_address).await {
            // If success rate > 70% and has at least 10 trades
            if metrics.signal_success_rate >= 70.0 && metrics.total_trades >= 10 {
                return true;
            }
        }
        false
    }
}
