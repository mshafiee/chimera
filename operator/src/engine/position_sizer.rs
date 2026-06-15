//! Confidence-based dynamic position sizing
//!
//! Calculates position size based on:
//! - Base size (or Kelly Criterion when enabled)
//! - Confidence multiplier (consensus, WQS, etc.)
//! - Wallet performance multiplier
//! - Portfolio limits

use crate::config::PositionSizingConfig;
use crate::db::DbPool;
use crate::engine::kelly_sizer::KellySizer;
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
use sqlx;
use std::sync::Arc;

/// Position sizer
pub struct PositionSizer {
    db: DbPool,
    config: Arc<PositionSizingConfig>,
    /// Kelly Criterion sizer (active when use_kelly_sizing = true and ≥10 closed trades exist)
    kelly_sizer: Option<Arc<KellySizer>>,
}

/// Position sizing factors
#[derive(Debug, Clone)]
pub struct SizingFactors {
    pub is_consensus: bool,
    pub wallet_wqs: f64, // WQS score (0-100), used for threshold comparisons only
    pub wallet_success_rate: Decimal, // Success rate (0.0-1.0), used in financial calculations
    pub token_age_hours: Option<f64>, // Token age in hours, used for threshold comparisons only
    pub estimated_slippage: Decimal, // Slippage percentage, used in financial calculations
    /// Signal quality score (0.0-1.0)
    pub signal_quality: Option<Decimal>, // Quality score, used in financial calculations
    /// Token 24h volatility percentage (None if unknown)
    pub token_volatility_24h: Option<Decimal>, // Volatility percentage, used in financial calculations
    /// Wallet address for Kelly Criterion lookup
    pub wallet_address: String,
    /// Total trading capital in SOL (for Kelly sizing)
    pub total_capital_sol: Decimal,
    /// Trading strategy — determines per-strategy max position size
    pub strategy: crate::models::Strategy,
    /// Number of wallets in agreement for consensus
    pub consensus_wallet_count: Option<usize>,
    /// Multiplier based on the effective market regime
    pub regime_multiplier: Decimal,
}

impl PositionSizer {
    pub fn new(db: DbPool, config: Arc<PositionSizingConfig>) -> Self {
        let kelly_sizer = if config.use_kelly_sizing {
            Some(Arc::new(KellySizer::new(db.clone())))
        } else {
            None
        };
        Self { db, config, kelly_sizer }
    }

    pub fn off_hours_size_multiplier(&self) -> rust_decimal::Decimal {
        self.config.off_hours_size_multiplier
    }

    /// Calculate position size based on factors.
    ///
    /// Multipliers applied (all multiplicative): confidence (1×–2×), performance (0.8×–1.1×),
    /// token_age (0.5×–1×), slippage (0.7×–1×), quality (0.7×–1.3×), volatility (0.5×–1×),
    /// regime (0.5×–2×). Total range: ~0.06× to ~5.15×. Min/max caps prevent extreme sizes.
    pub async fn calculate_size(&self, factors: SizingFactors) -> Decimal {
        // Kelly Criterion override: derive base size from historical win/loss ratio.
        // Falls back to WQS-scaled sizing when Kelly can't compute (< 10 trades).
        //
        // When Kelly is active we track full_kelly_cap = full_kelly * total_capital so that
        // multiplicative adjustments (confidence, quality, regime) applied below never push the
        // final size past full Kelly — which already maximises long-term growth and exceeding it
        // guarantees ruin over a sufficient sample.
        let mut full_kelly_cap: Option<Decimal> = None;
        let mut size = if let Some(ref kelly) = self.kelly_sizer {
            // Adaptive lookback: prefer the recent 14-day window for wallets that have
            // changed strategy recently. Fall back to 30 days when the 14-day window
            // has fewer than 20 trades — too few data points for reliable Kelly.
            let kelly_result_14d = kelly.calculate_kelly(&factors.wallet_address, factors.strategy, 14).await;
            let use_30d = kelly_result_14d.as_ref().map(|r| r.trade_count < 20).unwrap_or(true);
            let kelly_result = if use_30d {
                kelly.calculate_kelly(&factors.wallet_address, factors.strategy, 30).await
            } else {
                kelly_result_14d
            };
            match kelly_result {
                Ok(result) => {
                    // Apply strategy-specific Kelly fraction: Shield uses a larger fraction
                    // (more conservative signal, wider stops) while Spear uses a smaller
                    // fraction (higher variance, smaller size to bound risk).
                    let kelly_fraction = match factors.strategy {
                        crate::models::Strategy::Shield => self.config.kelly_fraction_shield,
                        crate::models::Strategy::Spear => self.config.kelly_fraction_spear,
                        // Exit signals don't open positions; use Shield default as a safe fallback
                        crate::models::Strategy::Exit => self.config.kelly_fraction_shield,
                    };
                    full_kelly_cap = Some(factors.total_capital_sol * result.full_kelly);
                    let kelly_pct = result.full_kelly * kelly_fraction * result.velocity_multiplier;
                    let kelly_base = factors.total_capital_sol * kelly_pct;
                    tracing::debug!(
                        wallet = %factors.wallet_address,
                        strategy = ?factors.strategy,
                        full_kelly = ?result.full_kelly,
                        kelly_fraction = ?kelly_fraction,
                        kelly_pct = ?kelly_pct,
                        kelly_base_sol = ?kelly_base,
                        "Kelly Criterion base size computed"
                    );
                    // Do NOT apply max(min_size_sol) here when Kelly is active.
                    // A zero kelly_base means non-positive EV — the full_kelly_cap zero-check
                    // below will reject the trade. Clamping up to min_size_sol first would
                    // inflate a negative-EV signal past the zero-cap guard.
                    if kelly_base.is_zero() {
                        kelly_base
                    } else {
                        kelly_base
                            .max(self.config.min_size_sol)
                            .min(self.config.max_size_sol)
                    }
                }
                Err(_) => {
                    // < 10 closed trades: scale base size by WQS quality and sample confidence
                    let trade_count = crate::db::get_closed_trade_count(&self.db, &factors.wallet_address)
                        .await
                        .unwrap_or(0);
                    // Floor at 0.05 (5%) so unproven wallets (0 trades) get a minimal but
                    // non-zero base. The previous 0.2 floor gave new wallets a 20% allocation,
                    // which is too generous for an unvalidated signal source.
                    let confidence = Decimal::from_f64_retain((trade_count as f64 / 10.0).clamp(0.05, 1.0))
                        .unwrap_or(Decimal::ONE);
                    let wqs_factor = Decimal::from_f64_retain(factors.wallet_wqs / 100.0)
                        .unwrap_or(Decimal::from_str("0.5").unwrap_or(Decimal::ONE));
                    // Set a conservative capital cap so the multiplicative chain (regime,
                    // consensus, quality) cannot push an unproven wallet past a modest
                    // fraction of total capital. Scales linearly: 0 trades → 2%, 9 trades → 9.2%.
                    // This mirrors the full_kelly_cap guard used when Kelly is available.
                    let fallback_cap_pct = Decimal::from_f64_retain(
                        (trade_count as f64 / 10.0 * 0.08 + 0.02).min(0.10)
                    ).unwrap_or(dec!(0.02));
                    full_kelly_cap = Some(factors.total_capital_sol * fallback_cap_pct);
                    (self.config.base_size_sol * wqs_factor * confidence)
                        .max(self.config.min_size_sol)
                        .min(self.config.max_size_sol)
                }
            }
        } else {
            // Kelly not enabled: apply WQS + confidence scaling directly
            let trade_count = crate::db::get_closed_trade_count(&self.db, &factors.wallet_address)
                .await
                .unwrap_or(0);
            let confidence = Decimal::from_f64_retain((trade_count as f64 / 10.0).clamp(0.05, 1.0))
                .unwrap_or(Decimal::ONE);
            let wqs_factor = Decimal::from_f64_retain(factors.wallet_wqs / 100.0)
                .unwrap_or(Decimal::from_str("0.5").unwrap_or(Decimal::ONE));
            (self.config.base_size_sol * wqs_factor * confidence)
                .max(self.config.min_size_sol)
                .min(self.config.max_size_sol)
        };

        // Confidence multiplier (using Decimal)
        let confidence_mult = if let Some(count) = factors.consensus_wallet_count {
            if count > 0 {
                let excess = (count - 1).min(4) as i64;
                (Decimal::ONE + Decimal::from_str("0.25").unwrap() * Decimal::from(excess))
                    .min(Decimal::from(2))
            } else {
                Decimal::ONE
            }
        } else if factors.is_consensus {
            self.config.consensus_multiplier
        } else {
            Decimal::ONE
        };

        // Wallet performance multiplier (based on success rate)
        let performance_mult = if factors.wallet_success_rate
            >= Decimal::from_str("0.6").unwrap_or(Decimal::ZERO)
        {
            Decimal::from_str("1.1").unwrap_or(Decimal::ONE)
        } else if factors.wallet_success_rate < Decimal::from_str("0.4").unwrap_or(Decimal::ZERO) {
            Decimal::from_str("0.8").unwrap_or(Decimal::ONE)
        } else {
            Decimal::ONE
        };

        // New token penalty (<24h old)
        let token_age_mult = if let Some(age) = factors.token_age_hours {
            if age < 24.0 {
                Decimal::from_str("0.5").unwrap_or(Decimal::ONE)
            } else {
                Decimal::ONE
            }
        } else {
            Decimal::ONE
        };

        // Slippage degrades size linearly: no penalty at ≤1%, 50% floor at ≥5%.
        // Mirrors the volatility_mult continuous approach — avoids a hard cliff at one
        // threshold (the previous >2% → 0.7× binary hit a 30% reduction instantaneously).
        let slippage_mult = if factors.estimated_slippage <= dec!(1.0) {
            Decimal::ONE
        } else if factors.estimated_slippage >= dec!(5.0) {
            dec!(0.5)
        } else {
            let excess = factors.estimated_slippage - dec!(1.0);
            let penalty = excess / dec!(4.0) * dec!(0.5);
            (Decimal::ONE - penalty).max(dec!(0.5))
        };

        // Signal quality multiplier
        // High quality (>0.9): 1.3x
        // Medium quality (0.7-0.9): 1.0x
        // Low quality (<0.7): 0.7x (shouldn't reach here due to filter)
        let quality_mult = if let Some(quality) = factors.signal_quality {
            if quality >= dec!(0.9) {
                dec!(1.3)
            } else if quality >= dec!(0.7) {
                Decimal::ONE
            } else {
                dec!(0.7)
            }
        } else {
            Decimal::ONE // Default if quality not provided
        };

        // Volatility multiplier (reduce size for high volatility)
        // If volatility > 30%, reduce size proportionally; floor at 0.5x
        let volatility_mult = if let Some(volatility) = factors.token_volatility_24h {
            if volatility > dec!(30.0) {
                // Each 10% above the 30% threshold reduces size by 30%, floored at 50%
                let excess = volatility - dec!(30.0);
                let steps = excess / dec!(10.0);
                let reduction = steps * dec!(0.3);
                (Decimal::ONE - reduction).max(dec!(0.5))
            } else {
                Decimal::ONE
            }
        } else {
            Decimal::ONE // Default if volatility unknown
        };

        // Apply all multipliers using Decimal arithmetic
        size *= confidence_mult;
        size *= performance_mult;
        size *= token_age_mult;
        size *= slippage_mult;
        size *= quality_mult;
        size *= volatility_mult;
        size *= factors.regime_multiplier;

        // When Kelly is active, cap at full Kelly × capital before the strategy_max clamp.
        // Full Kelly already maximises long-term growth; exceeding it guarantees ruin.
        //
        // Zero cap means Kelly (or its fallback) calculated a non-positive EV for this
        // wallet. Reject immediately — trading at min_size_sol in this case causes "death
        // by a thousand cuts" as the engine bleeds capital on negative-EV signals.
        if let Some(cap) = full_kelly_cap {
            if cap < self.config.min_size_sol {
                tracing::warn!(
                    wallet = %factors.wallet_address,
                    strategy = ?factors.strategy,
                    cap = %cap,
                    min_size_sol = %self.config.min_size_sol,
                    "Kelly cap is below min_size_sol (negative EV or insufficient allocation) — rejecting trade"
                );
                return Decimal::ZERO;
            }
            if size > cap {
                tracing::debug!(
                    wallet = %factors.wallet_address,
                    pre_cap_size = %size,
                    full_kelly_cap = %cap,
                    "Clamping size to full Kelly cap after multipliers"
                );
                size = cap;
            }
        }

        // Apply strategy-specific max cap (Barbell: Shield gets larger allocation, Spear smaller)
        let strategy_max = match factors.strategy {
            crate::models::Strategy::Shield => self.config.shield_max_size_sol,
            crate::models::Strategy::Spear => self.config.spear_max_size_sol,
            crate::models::Strategy::Exit => self.config.max_size_sol,
        };

        // Reject dust trades: if strategy_max is below min_size_sol, the resulting size
        // would be unviable — too small to clear DEX tick constraints or survive gas costs.
        // Return zero so the caller can reject the trade cleanly rather than submit a dust tx.
        if strategy_max < self.config.min_size_sol {
            tracing::warn!(
                strategy = ?factors.strategy,
                strategy_max = %strategy_max,
                min_size_sol = %self.config.min_size_sol,
                "Rejecting trade: strategy_max is below min_size_sol — would produce unviable dust trade; check config"
            );
            return Decimal::ZERO;
        }

        size = size.max(self.config.min_size_sol).min(strategy_max);

        size
    }

    /// Get sizing factors for a wallet
    ///
    /// # Arguments
    /// * `wallet_address` - Wallet address to get factors for
    /// * `is_consensus` - Whether this is a consensus signal
    /// * `estimated_slippage` - Estimated slippage percentage
    /// * `token_address` - Optional token address for age calculation
    /// * `helius_client` - Optional Helius client for token age fetching
    /// * `total_capital_sol` - Total trading capital for Kelly sizing
    pub async fn get_sizing_factors(
        &self,
        wallet_address: &str,
        is_consensus: bool,
        estimated_slippage: Decimal,
        token_address: Option<&str>,
        helius_client: Option<&crate::monitoring::HeliusClient>,
        total_capital_sol: Decimal,
    ) -> SizingFactors {
        // Get wallet from database
        let wallet_opt = crate::db::get_wallet_by_address(&self.db, wallet_address).await;
        let wqs = match wallet_opt {
            Ok(Some(w)) => w.wqs_score.unwrap_or(50.0),
            _ => 50.0,
        };

        // Get wallet performance metrics from database
        // Convert success rate percentage to Decimal (0.0-1.0)
        let success_rate =
            match crate::db::get_wallet_copy_performance(&self.db, wallet_address).await {
                Ok(Some(metrics)) => Decimal::from_f64_retain(metrics.signal_success_rate / 100.0)
                    .unwrap_or(Decimal::from_str("0.5").unwrap_or(Decimal::ZERO)),
                _ => Decimal::from_str("0.5").unwrap_or(Decimal::ZERO), // Default fallback if no performance data exists
            };

        // Get token age if token address and Helius client are provided
        let token_age_hours =
            if let (Some(token_addr), Some(helius)) = (token_address, helius_client) {
                match helius.get_token_age_hours(token_addr).await {
                    Ok(age) => age,
                    Err(e) => {
                        tracing::warn!(
                            token = token_addr,
                            error = %e,
                            "Failed to fetch token age, using None"
                        );
                        None
                    }
                }
            } else {
                None
            };

        SizingFactors {
            is_consensus,
            wallet_wqs: wqs,
            wallet_success_rate: success_rate,
            token_age_hours,
            estimated_slippage,
            signal_quality: None,       // Will be set by caller if available
            token_volatility_24h: None, // Will be set by caller if available
            wallet_address: wallet_address.to_string(),
            total_capital_sol,
            strategy: crate::models::Strategy::Shield, // caller can override
            consensus_wallet_count: None,
            regime_multiplier: Decimal::ONE,
        }
    }

    /// Check if we can open a new position (portfolio limits)
    pub async fn can_open_position(&self) -> bool {
        // Count ACTIVE and EXITING positions together — EXITING positions still consume capital
        // until the exit transaction confirms. Ignoring them allows 2× over-deployment.
        let active_count: i64 = match sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM positions WHERE state IN ('ACTIVE', 'EXITING')",
        )
        .fetch_one(&self.db)
        .await
        {
            Ok(count) => count,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to query active positions, allowing trade");
                return true;
            }
        };

        active_count < self.config.max_concurrent_positions as i64
    }
}
