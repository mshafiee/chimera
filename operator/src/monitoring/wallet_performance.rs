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
    /// Whether auto-demotion is enabled
    auto_demote_enabled: bool,
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
            auto_demote_enabled: false,
        }
    }

    /// Create with auto-demotion enabled
    pub fn with_auto_demotion(db: DbPool, enabled: bool) -> Self {
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
        pnl_sol: f64,
    ) -> Result<(), String> {
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
        
        // Update 7-day PnL from database (actual 7-day window)
        let seven_days_ago = chrono::Utc::now() - chrono::Duration::days(7);
        let from_date_str = seven_days_ago.format("%Y-%m-%dT%H:%M:%SZ").to_string();
        
        let trades = crate::db::get_trades(
            &self.db,
            Some(&from_date_str),
            None,
            Some("CLOSED"),
            None,
            Some(wallet_address),
            None,
            None,
        )
        .await
        .map_err(|e| format!("Failed to query trades: {}", e))?;

        let copy_pnl_7d: f64 = trades
            .iter()
            .filter_map(|t| t.net_pnl_sol)
            .sum();

        metrics.copy_pnl_7d = copy_pnl_7d;
        metrics.avg_return_per_trade = if metrics.total_trades > 0 {
            copy_pnl_7d / metrics.total_trades as f64
        } else {
            0.0
        };
        metrics.last_updated = std::time::SystemTime::now();

        // Update WQS in database based on copy performance
        self.update_wqs_from_copy_performance(wallet_address, &metrics).await?;

        Ok(())
    }

    /// Update WQS based on copy performance
    async fn update_wqs_from_copy_performance(
        &self,
        wallet_address: &str,
        metrics: &WalletCopyMetrics,
    ) -> Result<(), String> {
        // Get current wallet from database
        let wallet = crate::db::get_wallet_by_address(&self.db, wallet_address)
            .await
            .map_err(|e| format!("Failed to get wallet: {}", e))?;

        if let Some(mut wallet) = wallet {
            // Get original wallet WQS (from Scout analysis)
            let original_wqs = wallet.wqs_score.unwrap_or(50.0);

            // Calculate copy performance factor
            // If copy PnL < original PnL * 0.7 for 7 days, reduce WQS
            // For now, we'll use a simple adjustment based on success rate
            let copy_performance_factor = if metrics.signal_success_rate >= 60.0 {
                1.0  // Good copy performance
            } else if metrics.signal_success_rate >= 50.0 {
                0.9  // Moderate copy performance
            } else {
                0.8  // Poor copy performance
            };

            // Adjust WQS (but don't go below 40% of original)
            let adjusted_wqs = (original_wqs * copy_performance_factor).max(original_wqs * 0.4);

            // Update wallet WQS in database
            wallet.wqs_score = Some(adjusted_wqs);
            
            // Update database (would need an update_wallet_wqs function)
            // For now, we'll log it - full update would require roster merge
            tracing::info!(
                wallet_address = %wallet_address,
                original_wqs = original_wqs,
                adjusted_wqs = adjusted_wqs,
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
                
                match crate::db::update_wallet_status(
                    &self.db,
                    wallet_address,
                    "CANDIDATE",
                    None, // No TTL
                    Some(&reason),
                ).await {
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

    /// Check if wallet should be auto-demoted
    /// Auto-demote if copy PnL < original PnL * 0.7 for 7 days
    pub async fn should_demote(&self, wallet_address: &str) -> bool {
        // Get wallet from database to compare copy vs original performance
        if let Ok(Some(wallet)) = crate::db::get_wallet_by_address(&self.db, wallet_address).await {
            if let Some(metrics) = self.get_metrics(wallet_address).await {
                // Get original wallet ROI (from Scout analysis)
                let original_roi_7d = wallet.roi_7d.unwrap_or(0.0);
                
                // Calculate expected copy PnL (simplified: assume same ROI)
                // In reality, we'd need to track original wallet's actual PnL
                let expected_copy_pnl = original_roi_7d * 0.01; // Rough estimate
                
                // If copy PnL is significantly worse than expected (less than 70% of expected)
                if metrics.copy_pnl_7d < expected_copy_pnl * 0.7 {
                    // Check if this has been the case for 7+ days
                    if let Ok(elapsed) = metrics.last_updated.elapsed() {
                        if elapsed.as_secs() >= 7 * 24 * 3600 {
                            return true;
                        }
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
