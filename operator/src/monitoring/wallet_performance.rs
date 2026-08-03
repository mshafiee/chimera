//! Per-wallet copy performance tracking and dynamic weighting
//!
//! Tracks:
//! - 7-day copy PnL (actual returns from copying)
//! - Signal success rate (% of copied trades profitable)
//! - Average return per trade
//! - Exit timing accuracy

use crate::config::AppConfig;
use crate::db_abstraction::Database;
use rust_decimal::prelude::*;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Per-wallet copy-performance tier driving position sizing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyTier {
    /// Default — size at the position floor.
    Base,
    /// Proven recent copy profitability — size at the boost target.
    Boosted,
}

/// Pure classification of a wallet's recent copy performance into a sizing tier.
/// All gates (enabled flag, sample size, net PnL, win rate, recency) must pass
/// for `Boosted`; otherwise `Base`. Kept pure (no DB) so it is unit-testable.
pub fn classify_copy_tier(
    sample_count: u32,
    net_sol: rust_decimal::Decimal,
    winrate: f64,
    days_since_last_trade: i64,
    cfg: &crate::config::MonitoringConfig,
) -> CopyTier {
    if !cfg.wallet_boost_enabled {
        return CopyTier::Base;
    }
    if sample_count < cfg.wallet_boost_min_sample {
        return CopyTier::Base;
    }
    if net_sol <= cfg.wallet_boost_min_net_sol {
        return CopyTier::Base;
    }
    if winrate < cfg.wallet_boost_min_winrate {
        return CopyTier::Base;
    }
    if days_since_last_trade > cfg.wallet_boost_recency_days {
        return CopyTier::Base;
    }
    CopyTier::Boosted
}

/// Why a wallet should be demoted. `should_demote` returns this so callers can
/// apply the correct demotion policy (inactivity rotation vs copy-PnL
/// underperformance) instead of conflating the two reasons — which previously
/// let an inactivity breach hit the copy-PnL branch first and skip the
/// inactivity oscillation / REJECTED escalation path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemotionReason {
    /// Wallet inactive beyond the tiered inactivity threshold.
    Inactivity,
    /// Copy PnL < 70% of expected for 7+ continuous days.
    CopyPnl,
}

/// Wallet performance tracker
pub struct WalletPerformanceTracker {
    db: Arc<dyn Database>,
    /// Cache of recent metrics (wallet -> metrics)
    metrics_cache: Arc<RwLock<std::collections::HashMap<String, WalletCopyMetrics>>>,
    /// Whether auto-demotion is enabled
    auto_demote_enabled: bool,
    /// Configuration
    config: Arc<AppConfig>,
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
    /// Count of CLOSED copy trades in the last 7 days (used for fast
    /// net-negative demotion — distinguishes "one unlucky trade" from a
    /// consistently-losing wallet churning a dying token).
    pub recent_trade_count: u32,
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
            config: Arc::new(AppConfig::default()),
        }
    }

    pub fn new_with_config(db: Arc<dyn Database>, config: Arc<AppConfig>) -> Self {
        let auto_demote_enabled = config
            .monitoring
            .as_ref()
            .map(|m| m.auto_demote_wallets)
            .unwrap_or(false);
        Self {
            db,
            metrics_cache: Arc::new(RwLock::new(std::collections::HashMap::new())),
            auto_demote_enabled,
            config,
        }
    }

    /// Create with auto-demotion enabled
    pub fn with_auto_demotion(db: Arc<dyn Database>, enabled: bool) -> Self {
        Self {
            db,
            metrics_cache: Arc::new(RwLock::new(std::collections::HashMap::new())),
            auto_demote_enabled: enabled,
            config: Arc::new(AppConfig::default()),
        }
    }

    /// Update metrics after a trade closes and recalculate WQS
    pub async fn record_trade_result(
        &self,
        wallet_address: &str,
        pnl_sol: Decimal,
    ) -> Result<(), String> {
        // Phase 1: update trade counters under the write lock, then release the guard
        // before any await that could re-enter the cache. Holding the write guard across
        // update_wqs_from_copy_performance -> should_demote -> get_metrics (which takes a
        // read lock) self-deadlocks.
        {
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
                        recent_trade_count: 0,
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
        }
        // write guard dropped here

        // Phase 2: query 7-day PnL from the database (no cache lock held)
        let seven_days_ago = chrono::Utc::now() - chrono::Duration::days(7);
        let from_date_str = seven_days_ago.format("%Y-%m-%dT%H:%M:%SZ").to_string();

        let trades = self
            .db
            .get_trades_filtered(
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

        // Phase 3: persist computed PnL + breach timer under the write lock, clone the
        // snapshot out, then release the guard before the WQS update below.
        let metrics_snapshot: WalletCopyMetrics = {
            let mut cache = self.metrics_cache.write().await;
            let metrics = cache
                .entry(wallet_address.to_string())
                .or_insert_with(|| WalletCopyMetrics {
                    wallet_address: wallet_address.to_string(),
                    copy_pnl_7d: Decimal::ZERO,
                    signal_success_rate: 0.0,
                    avg_return_per_trade: Decimal::ZERO,
                    total_trades: 0,
                    winning_trades: 0,
                    recent_trade_count: 0,
                    last_updated: std::time::SystemTime::now(),
                    breach_started_at: None,
                });

            metrics.copy_pnl_7d = copy_pnl_7d;
            metrics.recent_trade_count = trades_7d_count as u32;
            metrics.avg_return_per_trade = if trades_7d_count > 0 {
                copy_pnl_7d / Decimal::from(trades_7d_count as u64)
            } else {
                Decimal::ZERO
            };
            metrics.last_updated = std::time::SystemTime::now();

            // Inline threshold check (mirrors should_demote logic) to maintain the breach timer.
            let currently_breaching = metrics.copy_pnl_7d < Decimal::ZERO;
            if currently_breaching {
                if metrics.breach_started_at.is_none() {
                    metrics.breach_started_at = Some(std::time::Instant::now());
                }
            } else {
                metrics.breach_started_at = None;
            }

            metrics.clone()
        };
        // write guard dropped here

        // Phase 4: WQS update + auto-demote using the snapshot. This may re-enter the
        // cache (should_demote -> get_metrics), which is now safe because no guard is held.
        self.update_wqs_from_copy_performance(wallet_address, &metrics_snapshot)
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
        let wallet = self
            .db
            .get_wallet(wallet_address)
            .await
            .map_err(|e| format!("Failed to get wallet: {}", e))?;

        if let Some(mut wallet) = wallet {
            // Get original wallet WQS (from Scout analysis)
            let original_wqs = wallet.wqs_score.unwrap_or(
                rust_decimal::Decimal::from_f64_retain(50.0).unwrap_or(rust_decimal::Decimal::ZERO),
            );

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
            let factor = rust_decimal::Decimal::from_f64_retain(copy_performance_factor)
                .unwrap_or(rust_decimal::Decimal::ONE);
            let min_wqs = original_wqs
                * rust_decimal::Decimal::from_f64_retain(0.4)
                    .unwrap_or(rust_decimal::Decimal::ZERO);
            let adjusted_wqs = (original_wqs * factor).max(min_wqs);

            // Persist adjusted WQS back to the wallets table
            wallet.wqs_score = Some(adjusted_wqs);

            if let Err(e) = self
                .db
                .upsert_wallet(
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

            // Determine the demotion reason ONCE so inactivity and copy-PnL
            // policies can't be conflated. Previously should_demote returned a
            // single bool for both reasons, so an inactivity breach hit the
            // copy-PnL branch first (when auto_demote_enabled) and skipped the
            // inactivity oscillation / REJECTED escalation path.
            let demotion_reason = self.should_demote(wallet_address).await;
            if matches!(demotion_reason, Some(DemotionReason::CopyPnl)) && self.auto_demote_enabled {
                tracing::warn!(
                    wallet_address = %wallet_address,
                    "Auto-demoting wallet due to poor copy performance"
                );

                // Update wallet status from ACTIVE to CANDIDATE
                let reason = format!(
                    "Auto-demoted: Copy PnL ({:.2} SOL) < 70% of expected for 7+ days",
                    metrics.copy_pnl_7d
                );

                match self
                    .db
                    .update_wallet_status_ext(
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
            } else {
                // Phase 1: Inactivity-based rotation (separate from auto_demote_enabled)
                if let Some(monitoring_config) = &self.config.monitoring {
                    if monitoring_config.inactivity_rotation_enabled {
                        if matches!(demotion_reason, Some(DemotionReason::Inactivity)) {
                            // Determine target status and reason
                            let demotion_count = self.db.get_inactivity_demotion_count(wallet_address).await.unwrap_or(0);
                            let max_cycles = monitoring_config.inactivity_rotation
                                .as_ref()
                                .map(|cfg| cfg.max_oscillation_cycles)
                                .unwrap_or(3);
                            
                            let (target_status, reason) = if demotion_count >= max_cycles as i32 {
                                (
                                    "REJECTED",
                                    format!("Inactivity demotion: wallet inactive for tiered threshold, oscillation limit ({}) reached", max_cycles)
                                )
                            } else {
                                (
                                    "CANDIDATE",
                                    format!("Inactivity demotion: wallet inactive for tiered threshold (demotion #{}/{})", demotion_count + 1, max_cycles)
                                )
                            };

                            tracing::warn!(
                                wallet_address = %wallet_address,
                                target = %target_status,
                                reason = %reason,
                                "Auto-demoting wallet due to inactivity"
                            );

                            // Increment demotion count
                            let _ = self.db.increment_inactivity_demotion_count(wallet_address).await;

                            // Update wallet status
                            match self
                                .db
                                .update_wallet_status_ext(
                                    wallet_address,
                                    target_status,
                                    None,
                                    Some(&reason),
                                )
                                .await
                            {
                                Ok(true) => {
                                    tracing::info!(
                                        wallet_address = %wallet_address,
                                        from = "ACTIVE",
                                        to = %target_status,
                                        "Wallet auto-demoted due to inactivity"
                                    );
                                }
                                Ok(false) => {
                                    tracing::warn!(
                                        wallet_address = %wallet_address,
                                        "Wallet inactivity demotion attempted but wallet not found or already demoted"
                                    );
                                }
                                Err(e) => {
                                    tracing::error!(
                                        wallet_address = %wallet_address,
                                        error = %e,
                                        "Failed to auto-demote wallet due to inactivity"
                                    );
                                }
                            }
                        } else {
                            tracing::debug!(
                                wallet_address = %wallet_address,
                                "Wallet passed inactivity check"
                            );
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Get metrics for a wallet
    pub async fn get_metrics(&self, wallet_address: &str) -> Option<WalletCopyMetrics> {
        let cache = self.metrics_cache.read().await;
        cache.get(wallet_address).cloned()
    }

    /// Check if (and why) a wallet should be auto-demoted.
    /// Returns `Some(Inactivity)` when the wallet has been inactive beyond the
    /// tiered inactivity threshold, or `Some(CopyPnl)` when copy PnL < 70% of
    /// expected (based on original ROI) continuously for 7+ days, else `None`.
    /// The 7-day timer starts when performance first breaches the threshold and is stored in
    /// `breach_started_at`; it is NOT reset on every trade close (that was the old bug).
    pub async fn compute_copy_tier(&self, wallet: &str) -> CopyTier {
        let cfg = match self.config.monitoring.as_ref() {
            Some(m) => m,
            None => return CopyTier::Base,
        };
        if !cfg.wallet_boost_enabled {
            return CopyTier::Base;
        }
        let window_days = cfg.wallet_boost_window_days;
        let window_trades = cfg.wallet_boost_window_trades;
        let from = chrono::Utc::now() - chrono::Duration::days(window_days);
        let from_str = from.format("%Y-%m-%dT%H:%M:%SZ").to_string();

        let trades = match self
            .db
            .get_trades_filtered(
                Some(&from_str),
                None,
                Some("CLOSED"),
                None,
                Some(wallet),
                window_trades as i64,
                0,
            )
            .await
        {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    wallet = %wallet,
                    "compute_copy_tier: query failed -> Base"
                );
                return CopyTier::Base;
            }
        };

        // Most-recent first, then take the window.
        let mut sorted: Vec<_> = trades.into_iter().collect();
        sorted.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        let recent: Vec<_> = sorted.into_iter().take(window_trades as usize).collect();

        let count = recent.len() as u32;
        let net: rust_decimal::Decimal = recent.iter().filter_map(|t| t.net_pnl_sol).sum();
        let wins = recent
            .iter()
            .filter(|t| {
                t.net_pnl_sol
                    .map(|p| p > rust_decimal::Decimal::ZERO)
                    .unwrap_or(false)
            })
            .count();
        let winrate = if count > 0 {
            wins as f64 / count as f64
        } else {
            0.0
        };
        // TradeDetail.created_at is a String (ISO/RFC3339); parse for the
        // recency computation. ISO-8601 strings sort chronologically, so the
        // String `max()` is the most-recent trade.
        let last_trade_str = recent.iter().map(|t| t.created_at.as_str()).max();
        let days_since = last_trade_str
            .and_then(|s| {
                chrono::DateTime::parse_from_rfc3339(s)
                    .ok()
                    .or_else(|| chrono::DateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f%:z").ok())
            })
            .map(|dt| {
                chrono::Utc::now()
                    .signed_duration_since(dt.with_timezone(&chrono::Utc))
                    .num_days()
                    .max(0)
            })
            .unwrap_or(i64::MAX);

        classify_copy_tier(count, net, winrate, days_since, cfg)
    }

    /// Convenience for the position-sizing path: returns `Some(boost_size)` if
    /// the wallet qualifies as BOOSTED, else `None` (floor sizing applies).
    /// Encapsulates both the tier classification and the configured boost size
    /// so callers (selection) don't need MonitoringConfig plumbing.
    pub async fn boost_target_for(&self, wallet: &str) -> Option<rust_decimal::Decimal> {
        let enabled = self
            .config
            .monitoring
            .as_ref()
            .map(|m| m.wallet_boost_enabled)
            .unwrap_or(false);
        if !enabled {
            return None;
        }
        match self.compute_copy_tier(wallet).await {
            CopyTier::Boosted => self
                .config
                .monitoring
                .as_ref()
                .map(|m| m.wallet_boost_size_sol),
            CopyTier::Base => None,
        }
    }

    pub async fn should_demote(&self, wallet_address: &str) -> Option<DemotionReason> {
        // Phase 1: Inactivity-based rotation
        if let Some(monitoring_config) = &self.config.monitoring {
            if monitoring_config.inactivity_rotation_enabled {
                if let Ok(Some(wallet)) = self.db.get_wallet(wallet_address).await {
                    if let Ok(Some(wm)) = self.db.get_wallet_monitoring(wallet_address).await {
                        if let Some(rotation_config) = &monitoring_config.inactivity_rotation {
                            // Determine tiered threshold based on WQS
                            let wqs = wallet.wqs_score.unwrap_or(Decimal::ZERO);
                            let wqs_f64 = wqs.to_f64().unwrap_or(0.0);
                            let threshold_secs = if wqs_f64 >= rotation_config.high_conviction_wqs_threshold {
                                rotation_config.high_conviction_threshold_secs
                            } else if wqs_f64 >= rotation_config.regular_conviction_wqs_threshold {
                                rotation_config.regular_conviction_threshold_secs
                            } else {
                                rotation_config.low_conviction_threshold_secs
                            };

                            // Get last activity timestamp from the most recent
                            // of: speculative signal, actual trade, or promotion.
                            // Using max() avoids demoting wallets whose last_trade_at
                            // is stale (scout hasn't re-analyzed) but are actively
                            // generating signals right now.
                            let spec_signal = wm.last_speculative_signal_at.as_ref()
                                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                                .map(|dt| dt.with_timezone(&chrono::Utc));

                            let last_activity = [spec_signal, wallet.last_trade_at, wallet.promoted_at]
                                .into_iter()
                                .flatten()
                                .max();

                            if let Some(last_activity_dt) = last_activity {
                                let elapsed = chrono::Utc::now().signed_duration_since(last_activity_dt);
                                if elapsed.num_seconds() >= threshold_secs as i64 {
                                    return Some(DemotionReason::Inactivity);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Existing copy-PnL demotion logic
        if self.auto_demote_enabled {
            // Get wallet from database to compare copy vs original performance
            if let Ok(Some(wallet)) = self.db.get_wallet(wallet_address).await {
                if let Some(metrics) = self.get_metrics(wallet_address).await {
                    // FAST net-negative demotion: a wallet with several recent
                    // copies that are clearly net-negative is bad — demote
                    // immediately instead of waiting 7 days. Catches wallets
                    // churning a dying token (e.g., 11 consecutive losses on
                    // one token) within hours. The count guard prevents one
                    // unlucky trade from triggering it.
                    const FAST_DEMOTE_MIN_TRADES: u32 = 5;
                    let fast_demote_net =
                        Decimal::from_str("-0.03").unwrap_or(Decimal::ZERO);
                    if metrics.recent_trade_count >= FAST_DEMOTE_MIN_TRADES
                        && metrics.copy_pnl_7d < fast_demote_net
                    {
                        tracing::warn!(
                            wallet_address = %wallet_address,
                            copy_pnl_7d = %metrics.copy_pnl_7d,
                            recent_trades = metrics.recent_trade_count,
                            "Fast demotion: wallet clearly net-negative over recent copies (churn protection)"
                        );
                        return Some(DemotionReason::CopyPnl);
                    }

                    // Get original wallet ROI (from Scout analysis)
                    let original_roi_7d = wallet.roi_7d.unwrap_or(rust_decimal::Decimal::ZERO);

                    // Calculate expected copy PnL (simplified: assume same ROI)
                    // In reality, we'd need to track original wallet's actual PnL
                    let expected_copy_pnl = original_roi_7d
                        * rust_decimal::Decimal::from_f64_retain(0.01)
                            .unwrap_or(rust_decimal::Decimal::ZERO); // Rough estimate

                    // If copy PnL is significantly worse than expected (less than 70% of expected)
                    let threshold =
                        expected_copy_pnl * Decimal::from_str("0.7").unwrap_or(Decimal::ZERO);
                    if metrics.copy_pnl_7d < threshold {
                        // Use breach_started_at to measure how long performance has been poor.
                        // This timer is set on the first breach and only reset on recovery, so
                        // it accurately reflects continuous underperformance rather than resetting
                        // on every trade close (which was the previous bug).
                        if metrics
                            .breach_started_at
                            .map(|t| t.elapsed().as_secs() >= 7 * 24 * 3600)
                            .unwrap_or(false)
                        {
                            return Some(DemotionReason::CopyPnl);
                        }
                    }
                }
            }
        }
        None
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

#[cfg(test)]
mod tier_tests {
    use super::*;
    use rust_decimal::Decimal;

    fn cfg() -> crate::config::MonitoringConfig {
        let mut c = crate::config::MonitoringConfig::default();
        c.wallet_boost_enabled = true;
        c
    }

    #[test]
    fn boosted_when_all_gates_pass() {
        // count 20>=15, net 0.5>0.01, wr 0.5>=0.4, age 1<=7
        assert_eq!(
            classify_copy_tier(20, Decimal::new(5, 1), 0.50, 1, &cfg()),
            CopyTier::Boosted
        );
    }

    #[test]
    fn base_when_sample_too_small() {
        assert_eq!(
            classify_copy_tier(14, Decimal::new(5, 1), 0.50, 1, &cfg()),
            CopyTier::Base
        );
    }

    #[test]
    fn base_when_net_not_positive_enough() {
        // net 0.01 is not strictly greater than min 0.01
        assert_eq!(
            classify_copy_tier(20, Decimal::new(1, 2), 0.50, 1, &cfg()),
            CopyTier::Base
        );
    }

    #[test]
    fn base_when_winrate_below_threshold() {
        assert_eq!(
            classify_copy_tier(20, Decimal::new(5, 1), 0.39, 1, &cfg()),
            CopyTier::Base
        );
    }

    #[test]
    fn base_when_dormant() {
        // 8 > 7 recency days
        assert_eq!(
            classify_copy_tier(20, Decimal::new(5, 1), 0.50, 8, &cfg()),
            CopyTier::Base
        );
    }

    #[test]
    fn base_when_disabled() {
        let c = crate::config::MonitoringConfig::default(); // wallet_boost_enabled = false
        assert_eq!(
            classify_copy_tier(20, Decimal::new(5, 1), 0.50, 1, &c),
            CopyTier::Base
        );
    }
}
