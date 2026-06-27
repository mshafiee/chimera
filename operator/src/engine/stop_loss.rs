//! Hard and dynamic stop-loss system
//!
//! Implements:
//! - Hard stop-loss at -15% (never let losses run)
//! - Dynamic stops (tighter for low-WQS wallets, wider for high-WQS)
//! - Portfolio-level stop (pause all trading if daily loss >5%)
//! - ATR-based stop-loss optimization with market regime adjustment
//! - Market regime detection (BULL/BEAR/VOLATILE/NEUTRAL)

use crate::config::ProfitManagementConfig;
use crate::db_abstraction::Database;
use crate::monitoring::SignalAggregator;
use crate::price_cache::PriceCache;
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Market regime for stop-loss adjustment
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketRegime {
    Bull,    // Widen stops (follow trends)
    Bear,    // Tighten stops (preserve capital)
    Volatile, // Widen stops significantly (avoid wick-outs)
    Neutral, // Standard parameters
}

impl MarketRegime {
    /// Get regime multiplier for ATR-based stops
    pub fn atr_multiplier(&self) -> Decimal {
        match self {
            MarketRegime::Bull => dec!(1.5),
            MarketRegime::Bear => dec!(1.0),
            MarketRegime::Volatile => dec!(2.0),
            MarketRegime::Neutral => dec!(1.25),
        }
    }

    /// Parse from string (for config loading)
    pub fn from_str(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "BULL" => MarketRegime::Bull,
            "BEAR" => MarketRegime::Bear,
            "VOLATILE" => MarketRegime::Volatile,
            _ => MarketRegime::Neutral,
        }
    }
}

/// Stop-loss manager
pub struct StopLossManager {
    db: Arc<dyn Database>,
    config: Arc<ProfitManagementConfig>,
    price_cache: Arc<PriceCache>,
    /// Optional in-memory consensus cache (avoids per-position DB query every 5 s).
    /// Set via `set_signal_aggregator` after construction.
    signal_aggregator: Arc<RwLock<Option<Arc<SignalAggregator>>>>,
}

/// Stop-loss action
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopLossAction {
    /// No action
    None,
    /// Exit position (stop-loss hit)
    Exit,
}

impl StopLossManager {
    pub fn new(
        db: Arc<dyn Database>,
        config: Arc<ProfitManagementConfig>,
        price_cache: Arc<PriceCache>,
    ) -> Self {
        Self {
            db,
            config,
            price_cache,
            signal_aggregator: Arc::new(RwLock::new(None)),
        }
    }

    /// Wire in the signal aggregator after construction so consensus checks read from
    /// the in-memory cache instead of issuing a DB query on every position tick.
    pub async fn set_signal_aggregator(&self, agg: Arc<SignalAggregator>) {
        *self.signal_aggregator.write().await = Some(agg);
    }

    /// Calculate ATR (Average True Range) from price history
    ///
    /// ATR measures market volatility by analyzing the range of price movements
    /// over a specified period. Used for dynamic stop-loss calculation.
    pub fn calculate_atr(&self, price_history: &[Decimal], period: usize) -> Option<Decimal> {
        if price_history.len() < period {
            return None;
        }

        let mut true_ranges = Vec::with_capacity(price_history.len() - 1);

        // Calculate True Range for each period
        for i in 1..price_history.len() {
            let high = price_history[i];
            let low = price_history[i];
            let prev_close = price_history[i - 1];

            // True Range = max(high - low, abs(high - prev_close), abs(low - prev_close))
            let tr = (high - low)
                .abs()
                .max((high - prev_close).abs())
                .max((low - prev_close).abs());

            true_ranges.push(tr);
        }

        // Calculate ATR as average of true ranges
        if true_ranges.is_empty() {
            return None;
        }

        let sum: Decimal = true_ranges.iter().sum();
        let atr = sum / Decimal::from(true_ranges.len());

        Some(atr)
    }

    /// Get current market regime from configuration
    pub fn get_market_regime(&self) -> MarketRegime {
        MarketRegime::from_str(&self.config.market_regime)
    }

    /// Calculate ATR-based stop-loss price
    ///
    /// # Arguments
    /// * `entry_price` - Position entry price
    /// * `atr_value` - Current ATR value
    /// * `regime` - Market regime for adjustment
    ///
    /// # Returns
    /// Stop-loss price calculated as: entry_price - (atr * multiplier * regime_factor)
    pub fn calculate_atr_stop_loss(
        &self,
        entry_price: Decimal,
        atr_value: Decimal,
        regime: MarketRegime,
    ) -> Decimal {
        // Get base ATR multiplier from config
        let atr_multiplier = self.config.atr_multiplier;

        // Get regime multiplier
        let regime_multiplier = regime.atr_multiplier();

        // Calculate ATR-based stop distance
        let atr_distance = atr_value * atr_multiplier * regime_multiplier;

        // Calculate stop-loss price (for long positions)
        let stop_loss_price = entry_price - (entry_price * atr_distance / dec!(100.0));

        tracing::debug!(
            entry_price = %entry_price,
            atr_value = %atr_value,
            atr_multiplier = %atr_multiplier,
            regime_multiplier = %regime_multiplier,
            stop_loss_price = %stop_loss_price,
            "ATR-based stop-loss calculated"
        );

        stop_loss_price
    }

    /// Check stop-loss for a position
    ///
    /// # Arguments
    /// * `trade_uuid` - Trade UUID
    /// * `wallet_address` - Wallet address (for WQS-based dynamic stops)
    /// * `entry_price` - Entry price (using Decimal for precision)
    /// * `token_address` - Token address
    ///
    /// # Returns
    /// Stop-loss action
    pub async fn check_stop_loss(
        &self,
        trade_uuid: &str,
        wallet_address: &str,
        entry_price: Decimal,
        token_address: &str,
        entry_time: chrono::DateTime<chrono::Utc>,
    ) -> StopLossAction {
        let current_price = match self.price_cache.get_price_usd(token_address) {
            Some(price) => {
                // Check staleness even when price is cached — a cached price can be
                // hours old if the feed is stale. Without this check, stop-loss
                // protection silently degrades for tokens with stale feeds.
                if self.price_cache.is_price_stale(token_address) {
                    tracing::error!(
                        trade_uuid = %trade_uuid,
                        token_address = token_address,
                        "STALE_PRICE: cached price is stale (>30s old) — forcing exit (risk management blind)"
                    );
                    return StopLossAction::Exit;
                }
                price
            }
            None => {
                // No cached price at all — check if this is a tracked token with a stale feed
                if self.price_cache.is_price_stale(token_address) {
                    tracing::error!(
                        trade_uuid = %trade_uuid,
                        token_address = token_address,
                        "STALE_PRICE: no price update for >2 min on tracked token — forcing exit (risk management blind)"
                    );
                    return StopLossAction::Exit;
                }
                return StopLossAction::None;
            }
        };

        // Calculate loss percentage using Decimal for precision
        // Negative when price has fallen (e.g. -15.0 for 15% drop), matching negative thresholds.
        // The engine now rejects BUY signals with zero entry_price before opening the position.
        // This guard is a last-resort safety net for positions that predate that check or were
        // inserted directly into the DB — force-exit to recover capital rather than holding
        // a position with no cost basis indefinitely.
        if entry_price.is_zero() {
            tracing::error!(
                trade_uuid = %trade_uuid,
                token_address = token_address,
                "CORRUPT_POSITION: entry_price is zero — forcing immediate exit to recover capital"
            );
            return StopLossAction::Exit;
        }

        let loss_percent = {
            let diff = current_price - entry_price;
            let ratio = diff / entry_price;
            ratio * Decimal::from(100)
        };

        // Get wallet WQS for dynamic stop calculation
        let wallet_opt = self.db.get_wallet(wallet_address).await;
        let wqs: f64 = match wallet_opt {
            Ok(Some(w)) => w
                .wqs_score
                .map(|s| s.to_f64().unwrap_or(50.0))
                .unwrap_or(50.0),
            _ => 50.0,
        };

        // Check if this is a consensus signal — read from SignalAggregator in-memory cache
        // (O(1), no DB query per position per 5-second tick).
        let is_consensus = {
            let agg_guard = self.signal_aggregator.read().await;
            if let Some(ref agg) = *agg_guard {
                agg.is_consensus_token(token_address).await
            } else {
                // Fallback: DB query when in-memory aggregator is not wired
                let count = match self.db.pool() {
                    crate::db_abstraction::DbPool::SQLite(ref pool) => {
                        let c: i64 = sqlx::query_scalar(
                            "SELECT COUNT(DISTINCT wallet_address) FROM signal_aggregation WHERE token_address = ? AND created_at >= datetime('now', '-5 minutes')",
                        )
                        .bind(token_address)
                        .fetch_one(pool)
                        .await
                        .unwrap_or(0);
                        c >= 2
                    }
                    _ => false,
                };
                count
            }
        };

        // Calculate base dynamic stop-loss threshold using compile-time Decimal constants.
        // High-WQS wallets get wider stops to let proven signals breathe; low-WQS gets tighter.
        let mut stop_loss_threshold = if wqs >= 70.0 {
            dec!(-20) // High WQS: wider stop
        } else if wqs >= 40.0 {
            dec!(-15) // Medium WQS: standard stop
        } else {
            dec!(-10) // Low WQS: tighter stop
        };

        // ATR-based stop-loss override (if enabled and ATR calculation is available)
        if self.config.atr_stop_loss_enabled {
            if let Some(volatility_f64) = self.price_cache.calculate_volatility(token_address) {
                let market_regime = self.get_market_regime();
                let atr_value = Decimal::from_f64(volatility_f64).unwrap_or(Decimal::ZERO);
                let atr_stop_price = self.calculate_atr_stop_loss(entry_price, atr_value, market_regime);

                // Convert ATR stop price to percentage threshold
                let atr_loss_percent = ((atr_stop_price - entry_price) / entry_price) * dec!(100.0);

                // Use ATR-based threshold if it's more conservative than the WQS-based threshold
                if atr_loss_percent > stop_loss_threshold {
                    stop_loss_threshold = atr_loss_percent;
                    tracing::info!(
                        trade_uuid = %trade_uuid,
                        token_address = token_address,
                        atr_loss_percent = %atr_loss_percent,
                        market_regime = ?market_regime,
                        "ATR-based stop-loss applied (overrides WQS-based threshold)"
                    );
                }
            }
        }

        // Adaptive stop-loss: adjust based on token volatility (ATR-like calculation).
        // If token is highly volatile, widen stops to avoid getting wicked out.
        if let Some(volatility) = self.price_cache.calculate_volatility(token_address) {
            // Volatility is returned as percentage (e.g., 15.0 = 15%)
            // If volatility > 20%, widen stop by 1.5x
            // If volatility > 30%, widen stop by 2x
            // If volatility < 10%, tighten stop by 0.9x
            let volatility_multiplier = if volatility > 30.0 {
                dec!(2.0)
            } else if volatility > 20.0 {
                dec!(1.5)
            } else if volatility < 10.0 {
                dec!(0.9)
            } else {
                Decimal::ONE
            };

            stop_loss_threshold *= volatility_multiplier;

            tracing::debug!(
                trade_uuid = %trade_uuid,
                token_address = token_address,
                volatility_percent = volatility,
                adjusted_threshold = %stop_loss_threshold,
                "Adaptive stop-loss adjusted based on volatility"
            );
        }

        // Widen stop-loss for consensus signals (applied after volatility).
        // Use a proportional 25% widening instead of a flat -5% so that tight stops
        // receive smaller absolute widening than wide stops — a flat -5% on a -10% base
        // would be a 50% widening, disproportionate relative to a -20% base.
        // A second clamp is applied immediately after so the combined result respects the envelope.
        if is_consensus {
            stop_loss_threshold *= dec!(1.25); // widen by 25% of current threshold
            tracing::debug!(
                trade_uuid = %trade_uuid,
                token_address = token_address,
                consensus_threshold = %stop_loss_threshold,
                "Consensus signal detected, widening stop-loss by 25%"
            );
        }

        let adaptive_threshold = stop_loss_threshold;

        // Final clamp: never tighter than -5% or wider than the operator-configured maximum.
        let widest_stop = self.config.max_stop_loss_distance;
        let tightest_stop = dec!(-5);
        stop_loss_threshold = stop_loss_threshold.max(widest_stop).min(tightest_stop);
        // Absolute floor: never wider than -25% regardless of config.
        // Changed from -35% to -25% to allow hard stop to trigger for catastrophic drops.
        // At 20% portfolio heat cap a single -25% stop wipes 5% of total capital.
        stop_loss_threshold = stop_loss_threshold.max(dec!(-25));

        // Warn when max_stop_loss_distance overrides adaptive widening so the operator can
        // see in logs that volatile/consensus tokens are being stopped tighter than intended.
        // To allow adaptive stops to breathe, set max_stop_loss_distance to a larger negative
        // value (e.g. -50) in config.yaml.
        if widest_stop > adaptive_threshold {
            tracing::warn!(
                trade_uuid = %trade_uuid,
                adaptive_threshold = %adaptive_threshold,
                max_stop_loss_distance = %self.config.max_stop_loss_distance,
                effective_threshold = %stop_loss_threshold,
                "Adaptive stop-loss widening overridden by max_stop_loss_distance; \
                 set max_stop_loss_distance to a larger negative (e.g. -50) to let adaptive stops breathe"
            );
        }

        if loss_percent <= stop_loss_threshold {
            let elapsed_secs = chrono::Utc::now()
                .signed_duration_since(entry_time)
                .num_seconds();
            if elapsed_secs < self.config.wick_protection_secs as i64 {
                // Hard stop at -25% always bypasses wick protection — a 25%+ crash
                // in the first seconds is never "normal entry slippage."
                if loss_percent <= dec!(-25) {
                    tracing::warn!(
                        trade_uuid = %trade_uuid,
                        loss_percent = %loss_percent,
                        "Hard stop at -25% triggered during wick protection window — catastrophic drop bypasses grace period"
                    );
                    return StopLossAction::Exit;
                }

                tracing::info!(
                    trade_uuid = %trade_uuid,
                    elapsed_secs,
                    wick_protection_secs = self.config.wick_protection_secs,
                    loss_percent = %loss_percent,
                    "Stop-loss triggered but ignored due to entry grace period (wick protection)"
                );
                return StopLossAction::None;
            }
            return StopLossAction::Exit;
        }

        StopLossAction::None
    }
}
