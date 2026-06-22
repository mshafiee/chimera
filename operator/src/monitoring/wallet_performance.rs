//! Per-wallet copy performance tracking and dynamic weighting
//!
//! Tracks:
//! - 7-day copy PnL (actual returns from copying)
//! - Signal success rate (% of copied trades profitable)
//! - Average return per trade
//! - Exit timing accuracy

use crate::db_abstraction::Database;
use rust_decimal::prelude::*;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Wallet performance tracker
pub struct WalletPerformanceTracker {
    db: Arc<dyn Database>,
    /// Cache of recent metrics (wallet -> metrics)
    metrics_cache: Arc<RwLock<std::collections::HashMap<String, WalletCopyMetrics>>>,
    /// Whether auto-demotion is enabled
    auto_demote_enabled: bool,
}

/// Wallet copy trading metrics
#[derive(Debug, Clone)]
pub struct WalletCopyMetrics {
    pub wallet_address: String,
    pub copy_pnl_7d: Decimal,
    pub signal_success_rate: f64,
    pub avg_return_per_trade: Decimal,
    pub total_trades: u32,
    pub winning_trades: u32,
    pub last_updated: std::time::SystemTime,
    /// When performance first continuously breached the demotion threshold.
    /// None means performance is currently within acceptable bounds.
    /// Set to Some(Instant::now()) on the first breach; cleared on recovery.
    pub breach_started_at: Option<std::time::Instant>,
}

impl WalletPerformanceTracker {
    pub fn new(db: Arc<dyn Database>) -> Self {
        Self {
            db,
            metrics_cache: Arc::new(RwLock::new(std::collections::HashMap::new())),
            auto_demote_enabled: false,
        }
    }

    /// Create with auto-demotion enabled
    pub fn with_auto_demotion(db: Arc<dyn Database>, enabled: bool) -> Self {
        Self {
            db,
            metrics_cache: Arc::new(RwLock::new(std::collections::HashMap::new())),
            auto_demote_enabled: enabled,
        }
    }

    /// Update metrics after a trade closes and recalculate WQS
    pub async fn record_trade_result(
        &self,
        wallet_address: &str,
        pnl_sol: Decimal,
    ) -> Result<(), String> {
        // Get or create metrics
        let mut cache = self.metrics_cache.write().await;
        let metrics =
            cache
                .entry(wallet_address.to_string())
                .or_insert_with(|| WalletCopyMetrics {
                    wallet_address: wallet_address.to_string(),
                    copy_pnl_7d: Decimal::ZERO,
                    signal_success_rate: 0.0,
                    avg_return_per_trade: Decimal::ZERO,
                    total_trades: 0,
                    winning_trades: 0,
                    last_updated: std::time::SystemTime::now(),
                    breach_started_at: None,
                });

        // Update metrics
        metrics.total_trades += 1;
        if pnl_sol > Decimal::ZERO {
            metrics.winning_trades += 1;
        }

        // Recalculate averages
        metrics.signal_success_rate =
            (metrics.winning_trades as f64 / metrics.total_trades as f64) * 100.0;

        // Update 7-day PnL from database (actual 7-day window)
        let seven_days_ago = chrono::Utc::now() - chrono::Duration::days(7);
        let from_date_str = seven_days_ago.format("%Y-%m-%dT%H:%M:%SZ").to_string();

        let trades = self.db.get_trades_filtered(
            Some(&from_date_str),
            None,
            Some("CLOSED"),
            None,
            Some(wallet_address),
            1000,
            0,
        )
        .await
        .map_err(|e| format!("Failed to query trades: {}", e))?;

        let copy_pnl_7d: Decimal = trades
            .iter()
            .filter_map(|t| t.net_pnl_sol)
            .fold(Decimal::ZERO, |acc, p| acc + p);

        // avg_return must use the 7d trade count, not all-time total_trades.
        let trades_7d_count = trades.len();

        metrics.copy_pnl_7d = copy_pnl_7d;
        metrics.avg_return_per_trade = if trades_7d_count > 0 {
            copy_pnl_7d / Decimal::from(trades_7d_count as u64)
        } else {
            Decimal::ZERO
        };
        metrics.last_updated = std::time::SystemTime::now();

        // Track how long this wallet has continuously breached the demotion threshold.
        // We need the wallet's ROI to compute the threshold; retrieve it from the cache
        // without holding the lock across await points. We clone what we need here.
        // The actual demotion decision is made in should_demote() which reads breach_started_at.
        //
        // Inline threshold check (mirrors should_demote logic) so we can update the timer:
        {
            // Compute expected PnL — same formula used in should_demote().
            // We don't have the wallet here without an await, so we use a conservative
            // heuristic: if copy_pnl_7d < 0 treat it as a breach (worst-case).
            // The full threshold check (with original_roi_7d) happens in should_demote().
            // Here we only maintain the timer: start it on first negative period, clear on recovery.
            let currently_breaching = metrics.copy_pnl_7d < Decimal::ZERO;
            if currently_breaching {
                if metrics.breach_started_at.is_none() {
                    metrics.breach_started_at = Some(std::time::Instant::now());
                }
                // else: timer already running — do not reset it
            } else {
                // Performance recovered — reset the breach timer
                metrics.breach_started_at = None;
            }
        }

        // Update WQS in database based on copy performance
        self.update_wqs_from_copy_performance(wallet_address, metrics)
            .await?;

        Ok(())
    }

    /// Update WQS based on copy performance
    async fn update_wqs_from_copy_performance(
        &self,
        wallet_address: &str,
        metrics: &WalletCopyMetrics,
    ) -> Result<(), String> {
        // Get current wallet from database
        let wallet = self.db.get_wallet(wallet_address)
            .await
            .map_err(|e| format!("Failed to get wallet: {}", e))?;

        if let Some(mut wallet) = wallet {
            // Get original wallet WQS (from Scout analysis)
            let original_wqs = wallet.wqs_score.unwrap_or(rust_decimal::Decimal::from_f64_retain(50.0).unwrap_or(rust_decimal::Decimal::ZERO));

            // Calculate copy performance factor
            // If copy PnL < original PnL * 0.7 for 7 days, reduce WQS
            // For now, we'll use a simple adjustment based on success rate
            let copy_performance_factor = if metrics.signal_success_rate >= 60.0 {
                1.0 // Good copy performance
            } else if metrics.signal_success_rate >= 50.0 {
                0.9 // Moderate copy performance
            } else {
                0.8 // Poor copy performance
            };

            // Adjust WQS (but don't go below 40% of original)
            let factor = rust_decimal::Decimal::from_f64_retain(copy_performance_factor).unwrap_or(rust_decimal::Decimal::ONE);
            let min_wqs = original_wqs * rust_decimal::Decimal::from_f64_retain(0.4).unwrap_or(rust_decimal::Decimal::ZERO);
            let adjusted_wqs = (original_wqs * factor).max(min_wqs);

            // Persist adjusted WQS back to the wallets table
            wallet.wqs_score = Some(adjusted_wqs);

            if let Err(e) = self.db.upsert_wallet(
                wallet_address,
                Some(adjusted_wqs),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
            {
                tracing::warn!(
                    wallet_address = %wallet_address,
                    error = %e,
                    "Failed to persist adjusted WQS to database"
                );
            }

            tracing::info!(
                wallet_address = %wallet_address,
                original_wqs = ?original_wqs,
                adjusted_wqs = ?adjusted_wqs,
                copy_success_rate = metrics.signal_success_rate,
                "WQS adjusted based on copy performance"
            );

            // Check if should auto-demote
            if self.auto_demote_enabled && self.should_demote(wallet_address).await {
                tracing::warn!(
                    wallet_address = %wallet_address,
                    "Auto-demoting wallet due to poor copy performance"
                );

                // Update wallet status from ACTIVE to CANDIDATE
                let reason = format!(
                    "Auto-demoted: Copy PnL ({:.2} SOL) < 70% of expected for 7+ days",
                    metrics.copy_pnl_7d
                );

                match self.db.update_wallet_status_ext(
                    wallet_address,
                    "CANDIDATE",
                    None, // No TTL
                    Some(&reason),
                )
                .await
                {
                    Ok(true) => {
                        tracing::info!(
                            wallet_address = %wallet_address,
                            "Wallet auto-demoted from ACTIVE to CANDIDATE"
                        );
                    }
                    Ok(false) => {
                        tracing::warn!(
                            wallet_address = %wallet_address,
                            "Wallet auto-demotion attempted but wallet not found or already demoted"
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            wallet_address = %wallet_address,
                            error = %e,
                            "Failed to auto-demote wallet"
                        );
                    }
                }
            } else if self.should_demote(wallet_address).await {
                tracing::debug!(
                    wallet_address = %wallet_address,
                    "Wallet should be demoted but auto-demotion is disabled"
                );
            }
        }

        Ok(())
    }

    /// Get metrics for a wallet
    pub async fn get_metrics(&self, wallet_address: &str) -> Option<WalletCopyMetrics> {
        let cache = self.metrics_cache.read().await;
        cache.get(wallet_address).cloned()
    }

    /// Check if wallet should be auto-demoted.
    /// Demotes if copy PnL < 70% of expected (based on original ROI) continuously for 7+ days.
    /// The 7-day timer starts when performance first breaches the threshold and is stored in
    /// `breach_started_at`; it is NOT reset on every trade close (that was the old bug).
    pub async fn should_demote(&self, wallet_address: &str) -> bool {
        // Get wallet from database to compare copy vs original performance
        if let Ok(Some(wallet)) = self.db.get_wallet(wallet_address).await {
            if let Some(metrics) = self.get_metrics(wallet_address).await {
                // Get original wallet ROI (from Scout analysis)
                let original_roi_7d = wallet.roi_7d.unwrap_or(rust_decimal::Decimal::ZERO);

                // Calculate expected copy PnL (simplified: assume same ROI)
                // In reality, we'd need to track original wallet's actual PnL
                let expected_copy_pnl =
                    (original_roi_7d * rust_decimal::Decimal::from_f64_retain(0.01).unwrap_or(rust_decimal::Decimal::ZERO)); // Rough estimate

                // If copy PnL is significantly worse than expected (less than 70% of expected)
                let threshold =
                    expected_copy_pnl * Decimal::from_str("0.7").unwrap_or(Decimal::ZERO);
                if metrics.copy_pnl_7d < threshold {
                    // Use breach_started_at to measure how long performance has been poor.
                    // This timer is set on the first breach and only reset on recovery, so
                    // it accurately reflects continuous underperformance rather than resetting
                    // on every trade close (which was the previous bug).
                    return metrics
                        .breach_started_at
                        .map(|t| t.elapsed().as_secs() >= 7 * 24 * 3600)
                        .unwrap_or(false);
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
