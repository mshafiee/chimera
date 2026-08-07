//! Confidence-based dynamic position sizing
//!
//! Calculates position size based on:
//! - Base size (or Kelly Criterion when enabled)
//! - Confidence multiplier (consensus, WQS, etc.)
//! - Wallet performance multiplier
//! - Portfolio limits

use crate::config::PositionSizingConfig;
use crate::db_abstraction::Database;
use crate::engine::kelly_sizer::KellySizer;
use crate::error::AppResult;
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
use std::sync::Arc;

/// Position sizer
pub struct PositionSizer {
    db: Arc<dyn Database>,
    config: Arc<PositionSizingConfig>,
    /// Kelly Criterion sizer (active when use_kelly_sizing = true and ≥10 closed trades exist)
    kelly_sizer: Option<Arc<KellySizer>>,
}

/// Position sizing factors
#[derive(Debug, Clone)]
pub struct SizingFactors {
    pub is_consensus: bool,
    pub wallet_wqs: f64, // WQS score (0-100), used for threshold comparisons only
    pub wqs_confidence: Option<f64>, // Scout statistical confidence (0.0-1.0)
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
    /// Optional WQS-based max size cap. When set, the final position size
    /// is clamped to this value (for low-WQS wallet micro-positions).
    pub wqs_capped_max_size: Option<Decimal>,
    /// Optional per-wallet copy-performance boost target (SOL). When set, the
    /// final size starts from this value (still capped by strategy_max and
    /// floored by min_size_sol). Set by selection for wallets whose recent copy
    /// trades qualify them as BOOSTED.
    pub boost_target_sol: Option<Decimal>,
    /// Token address for the conviction-size cap (75th percentile of the
    /// token's own recent entry sizes). None disables the token-based cap.
    pub token_address: Option<String>,
}

impl PositionSizer {
    pub fn new(db: Arc<dyn Database>, config: Arc<PositionSizingConfig>) -> Self {
        let kelly_sizer = if config.use_kelly_sizing {
            Some(Arc::new(KellySizer::new(db.clone())))
        } else {
            None
        };
        Self {
            db,
            config,
            kelly_sizer,
        }
    }

    pub fn off_hours_size_multiplier(&self) -> rust_decimal::Decimal {
        self.config.off_hours_size_multiplier
    }

    /// Calculate position size based on factors.
    ///
    /// Multipliers applied (all multiplicative): confidence (1×–1.5×), performance (0.8×–1.1×),
    /// token_age (0.5×–1×), slippage (0.7×–1×), quality (0.7×–1.3×), volatility (0.5×–1×),
    /// regime (0.5×–2×). Total range: ~0.06× to ~4.4×. Min/max caps prevent extreme sizes.
    pub async fn calculate_size(&self, factors: SizingFactors) -> AppResult<Decimal> {
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
            let kelly_result_14d = kelly
                .calculate_kelly(&factors.wallet_address, factors.strategy, 14)
                .await;
            let use_30d = kelly_result_14d
                .as_ref()
                .map(|r| r.trade_count < 20)
                .unwrap_or(true);
            let kelly_result = if use_30d {
                kelly
                    .calculate_kelly(&factors.wallet_address, factors.strategy, 30)
                    .await
            } else {
                kelly_result_14d
            };
            match kelly_result {
                Ok(result) => {
                    // Uniform kelly_fraction (25%) for both strategies.
                    // Spear risk is already bounded by spear_max_size_sol (0.5 SOL).
                    // A per-strategy fraction caused modest-edge Spear signals to drop
                    // below min_size_sol and silently reject, defeating the strategy.
                    let kelly_fraction = self.config.kelly_fraction;
                    full_kelly_cap = Some(factors.total_capital_sol * result.full_kelly);
                    let kelly_pct =
                        (result.full_kelly * kelly_fraction * result.velocity_multiplier)
                            .min(dec!(0.25));
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
                    // < 15 closed trades: scale base size by WQS quality and sample confidence.
                    // Uses the same 15-trade minimum as Kelly Criterion for consistency.
                    let trade_count = self
                        .db
                        .get_closed_trade_count_for_wallet(&factors.wallet_address)
                        .await?;
                    let confidence = if trade_count >= 15 {
                        Decimal::from_f64_retain((trade_count as f64 / 15.0).clamp(0.05, 1.0))
                            .unwrap_or(dec!(0.05))
                    } else {
                        let conf_f64 = factors.wqs_confidence.unwrap_or(0.50).clamp(0.35, 1.0);
                        Decimal::from_f64_retain(conf_f64).unwrap_or(dec!(0.50))
                    };
                    let wqs_factor = Decimal::from_f64_retain(factors.wallet_wqs / 100.0)
                        .unwrap_or(Decimal::from_str("0.5").unwrap_or(dec!(0.5)));
                    // Set a conservative capital cap so the multiplicative chain (regime,
                    // consensus, quality) cannot push an unproven wallet past a modest
                    // fraction of total capital. Scales linearly: 0 trades → 2%, 14 trades → 9.5%.
                    // Uses 15-trade denominator to match Kelly's minimum threshold.
                    let fallback_cap_pct = if trade_count >= 15 {
                        Decimal::from_f64_retain(
                            (trade_count as f64 / 15.0 * 0.075 + 0.02).min(0.10),
                        )
                        .unwrap_or(dec!(0.02))
                    } else {
                        let conf_f64 = factors.wqs_confidence.unwrap_or(0.50).clamp(0.35, 1.0);
                        Decimal::from_f64_retain((conf_f64 * 0.075 + 0.02).min(0.10))
                            .unwrap_or(dec!(0.075))
                    };
                    full_kelly_cap = Some(factors.total_capital_sol * fallback_cap_pct);
                    // Do NOT clamp to min_size_sol here — the fallback cap already
                    // constrains unproven wallets. Clamping up would inflate a
                    // negative-EV or unproven signal past the conservative cap.
                    (self.config.base_size_sol * wqs_factor * confidence)
                        .min(self.config.max_size_sol)
                }
            }
        } else {
            // Kelly not enabled: apply WQS + confidence scaling directly
            // Uses 15-trade denominator to match Kelly's minimum threshold
            let trade_count = self
                .db
                .get_closed_trade_count_for_wallet(&factors.wallet_address)
                .await?;
            let confidence = if trade_count >= 15 {
                Decimal::from_f64_retain((trade_count as f64 / 15.0).clamp(0.05, 1.0))
                    .unwrap_or(dec!(0.05))
            } else {
                let conf_f64 = factors.wqs_confidence.unwrap_or(0.50).clamp(0.35, 1.0);
                Decimal::from_f64_retain(conf_f64).unwrap_or(dec!(0.50))
            };
            let wqs_factor = Decimal::from_f64_retain(factors.wallet_wqs / 100.0)
                .unwrap_or(Decimal::from_str("0.5").unwrap_or(dec!(0.5)));
            (self.config.base_size_sol * wqs_factor * confidence).min(self.config.max_size_sol)
        };

        // Confidence multiplier (using Decimal)
        // Consensus adds 0.15 per excess wallet beyond the first, capped at 1.5×.
        // Previously 0.25 per wallet capped at 2.0×, which combined with regime
        // multiplier (up to 1.5×) created correlation concentration risk.
        let confidence_mult = if let Some(count) = factors.consensus_wallet_count {
            if count > 0 {
                let excess = (count - 1).min(3) as i64;
                (Decimal::ONE
                    + Decimal::from_str("0.15").unwrap_or(Decimal::from(15) / Decimal::from(100))
                        * Decimal::from(excess))
                .min(Decimal::from_str("1.5").unwrap_or(Decimal::from(3) / Decimal::from(2)))
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

        // Hybrid sizing: eliminate multiplier drift by averaging boosts and penalties separately.
        // Pure multiplication causes conservative factors to compound (e.g., 0.8⁷ ≈ 0.21x),
        // resulting in severe under-allocation on profitable signals.
        //
        // Solution: Average boost multipliers (≥1.0x) and penalty multipliers (≤1.0x) separately,
        // then multiply the results. This prevents drift while preserving expressiveness.
        //
        // Benefits:
        // - Conservative factors (0.8x each) now average to 0.8x total, not 0.8⁷ ≈ 0.21x
        // - Strong signals still get meaningful boosts (average 1.2x - 1.3x)
        // - Severe penalties (new token, high slippage) still reduce size significantly
        // - Market regime conditions remain multiplicative (they're structural, not signal-specific)

        // Boost multipliers (≥ 1.0x) - signal strength indicators
        let boost_multiplier = (
            confidence_mult.max(Decimal::ONE) +     // consensus boost: 1.0x - 1.5x
            performance_mult.max(Decimal::ONE) +    // performance boost: 1.0x - 1.1x
            quality_mult.max(Decimal::ONE)
            // quality boost: 1.0x - 1.3x
        ) / dec!(3.0); // Average boosts (1.0x - 1.3x range)

        // Penalty multipliers (≤ 1.0x) - risk adjustment factors.
        // performance_mult (0.8x for success rate < 0.4) and quality_mult
        // (0.7x for low quality) are penalties too — without them the boost
        // average would clamp them back to 1.0x and underperforming wallets /
        // low-quality signals would never be de-sized.
        let penalty_multiplier = (
            token_age_mult.min(Decimal::ONE) +       // age penalty: 0.5x - 1.0x
            slippage_mult.min(Decimal::ONE) +       // slippage penalty: 0.5x - 1.0x
            volatility_mult.min(Decimal::ONE) +      // volatility penalty: 0.5x - 1.0x
            performance_mult.min(Decimal::ONE) +     // performance penalty: 0.8x - 1.0x
            quality_mult.min(Decimal::ONE)
            // quality penalty: 0.7x - 1.0x
        ) / dec!(5.0); // Average penalties (0.5x - 1.0x range)

        // Apply hybrid sizing with regime multiplicative (special case - market conditions)
        size = size * boost_multiplier * penalty_multiplier * factors.regime_multiplier;

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
                return Ok(Decimal::ZERO);
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
            return Ok(Decimal::ZERO);
        }

        // Per-wallet copy-performance boost: a proven wallet's boost target
        // overrides the computed size. Still bounded by strategy_max (next
        // line) and the min_size_sol floor, so a misconfigured boost cannot
        // exceed strategy caps or breach the floor.
        if let Some(boost) = factors.boost_target_sol {
            size = boost;
        }

        size = size.max(self.config.min_size_sol).min(strategy_max);

        // WQS-based micro-position cap for low-conviction wallets.
        // Applied after strategy max to ensure unproven wallets trade small.
        // A cap below min_size_sol would be overridden by the hard floor below
        // (a pointless, silently-ineffective cap) — skip it explicitly.
        if let Some(wqs_cap) = factors.wqs_capped_max_size {
            if wqs_cap < self.config.min_size_sol {
                tracing::warn!(
                    wqs_cap = %wqs_cap,
                    min_size_sol = %self.config.min_size_sol,
                    "WQS cap below min_size_sol — skipping cap to avoid dust trade"
                );
            } else if size > wqs_cap {
                tracing::debug!(
                    wallet_wqs = %factors.wallet_wqs,
                    original_size = %size,
                    capped_size = %wqs_cap,
                    "Applying WQS-based micro-position cap"
                );
                size = wqs_cap;
            }
        }

        // Conviction-size cap (Phase 5, 2026-08-07): never be the "large entry"
        // on a token — clamp to the 75th percentile of the token's own recent
        // entry sizes (7d), falling back to the default cap (0.25 SOL) when
        // history is too thin. Keeps the bot small and invisible so copy-traders
        // don't pile onto our entry as exit liquidity (arxiv 2601.08641).
        if self.config.conviction_size_cap_enabled {
            if let Some(ref token) = factors.token_address {
                let cap = self
                    .token_conviction_cap_sol(token)
                    .await
                    .unwrap_or(self.config.conviction_size_default_cap_sol);
                if cap < self.config.min_size_sol {
                    tracing::warn!(
                        token = %token,
                        conviction_cap = %cap,
                        min_size_sol = %self.config.min_size_sol,
                        "Conviction cap below min_size_sol — skipping cap to avoid dust trade"
                    );
                } else if size > cap {
                    tracing::debug!(
                        token = %token,
                        original_size = %size,
                        capped_size = %cap,
                        "Applying conviction-size cap (75th percentile of token entries)"
                    );
                    size = cap;
                }
            }
        }

        // Hard floor guarantee: the returned size can never fall below
        // min_size_sol regardless of any earlier cap (strategy max, WQS
        // micro-position cap, or multiplicative shrinkage). Sub-floor positions
        // incur a fixed round-trip cost (~1.2% at 0.25 SOL) that turns marginal
        // winners into net losses. Negative-EV signals are rejected earlier
        // (Kelly zero-cap / dust checks) and never reach this point.
        size = size.max(self.config.min_size_sol);

        Ok(size)
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
    ) -> AppResult<SizingFactors> {
        // Get wallet from database
        let wallet_opt = self.db.get_wallet(wallet_address).await?;
        // Neutral WQS default is 50: a missing score must NOT collapse to 0
        // (wqs_factor = 0 → base size 0 → clamped up to min_size_sol, minting
        // a minimum-size order for an unproven wallet).
        let wqs = match &wallet_opt {
            Some(w) => w.wqs_score.unwrap_or(dec!(50)).to_f64().unwrap_or(50.0),
            None => 50.0,
        };
        let wqs_confidence = match &wallet_opt {
            Some(w) => w.wqs_confidence.and_then(|d| d.to_f64()),
            None => None,
        };

        // Get wallet performance metrics from database
        // Convert success rate percentage to Decimal (0.0-1.0)
        // Default to 0.4 for unproven/stale wallets — produces a 0.8× performance
        // penalty rather than neutral 1.0×, reflecting the uncertainty of no data.
        // A DB failure propagates (fail-safe) instead of silently trading on
        // stale/absent data at a default.
        let success_rate = match self.db.get_wallet_copy_performance(wallet_address).await {
            Ok(Some(metrics)) => metrics.signal_success_rate / rust_decimal::Decimal::from(100),
            Ok(None) => {
                rust_decimal::Decimal::from_str("0.4").unwrap_or(rust_decimal::Decimal::ZERO)
            }
            Err(e) => return Err(e),
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

        Ok(SizingFactors {
            is_consensus,
            wallet_wqs: wqs,
            wqs_confidence,
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
            wqs_capped_max_size: None,
            boost_target_sol: None,
            token_address: token_address.map(|s| s.to_string()),
        })
    }

    /// 75th percentile of the token's own entry sizes over the last 7 days
    /// (nearest-rank, 1-based index ceil(0.75*n)). `None` when the token has
    /// fewer than 3 historical entries — caller falls back to the default cap.
    async fn token_conviction_cap_sol(&self, token_address: &str) -> Option<Decimal> {
        use crate::db_abstraction::DbPool;
        let DbPool::PostgreSQL(pool) = self.db.pool();
        let rows: Vec<Decimal> = sqlx::query_scalar(
            "SELECT entry_amount_sol FROM positions
             WHERE token_address = $1 AND opened_at > NOW() - INTERVAL '7 days'
             ORDER BY entry_amount_sol",
        )
        .bind(token_address)
        .fetch_all(&mut *pool.clone().acquire().await.ok()?)
        .await
        .ok()?;
        let n = rows.len();
        if n < 3 {
            return None;
        }
        let idx = (0.75_f64 * n as f64).ceil() as usize - 1; // nearest-rank, 0-based
        Some(rows[idx.min(n - 1)])
    }

    /// Check if we can open a new position (portfolio limits)
    pub async fn can_open_position(&self) -> bool {
        // Count ACTIVE and EXITING positions together — EXITING positions still consume capital
        // until the exit transaction confirms. Ignoring them allows 2× over-deployment.
        let active_count: i64 = match self.db.get_active_positions().await {
            Ok(positions) => positions.len() as i64,
            Err(e) => {
                tracing::error!(error = %e, "Failed to query active positions, rejecting trade for safety");
                return false; // Fail-safe: reject trade on DB error to prevent unlimited position opening
            }
        };

        active_count < self.config.max_concurrent_positions as i64
    }
}
