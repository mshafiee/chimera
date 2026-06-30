//! Unified slippage estimation & on-chain tolerance.
//!
//! Replaces the two disconnected slippage pipelines (the executor's
//! liquidity-aware sqrt-impact model used only for cost bookkeeping, and the
//! comparator-derived `[30,150]` clamp fed to Jupiter's `slippageBps`).
//!
//! [`estimate`] computes a single best estimate of expected price impact and a
//! strategy-specific Jupiter `slippageBps` tolerance (`expected + buffer`).
//! The same estimate is used for:
//!   - the on-chain tolerance passed to Jupiter's quote (executor → builder), and
//!   - cost bookkeeping / gating in the executor.
//!
//! # Model
//! Preference order for the expected-impact fraction:
//!   1. Jupiter's `priceImpactPct` from a live quote (most accurate), else
//!   2. square-root market impact: `trade_usd / (2 × liquidity_usd)`, clamped to
//!      `[0.1%, 15%]`, else
//!   3. config-based size-tier fallback.
//!
//! The tolerance is `expected × BUFFER_MULT + MIN_BUFFER` (≈ 2× the expected
//! impact plus a 30 bps absolute cushion), then clamped to per-strategy bounds:
//!   - Shield (tight):  [10, 100] bps   (0.1% – 1%)
//!   - Spear (wider):   [30, 300] bps   (0.3% – 3%)
//!   - Exit (generous): [50, 1500] bps  (0.5% – 15%)
//!
//! Setting tolerance ≈ expected impact (the old behaviour) leaves almost no
//! adverse-move buffer; reverting + MEV exposure rises sharply. A 2× buffer is
//! the standard heuristic for memecoin-grade liquidity.

use crate::models::Strategy;
use rust_decimal::prelude::*;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

/// Buffer = `BUFFER_MULT × expected + MIN_BUFFER`. A 2× multiplier gives the
/// on-chain tolerance ~2× the expected impact plus an absolute floor — the
/// standard heuristic for memecoin-grade liquidity, where the quote→land
/// latency and volatility can push actual impact above the quoted value. A
/// tighter buffer risks on-chain reverts (and in the V0/legacy-fallback
/// `[tip, swap]` bundle, a reverted swap with a landed tip = direct fund loss).
const BUFFER_MULT: f64 = 2.0;
/// Minimum absolute buffer (30 bps) added on top of the relative buffer.
const MIN_BUFFER: Decimal = dec!(0.003);

/// Min/max expected-impact clamp applied to the liquidity-aware estimate.
const LIQ_IMPACT_FLOOR: Decimal = dec!(0.001); // 0.1%
const LIQ_IMPACT_CEIL: Decimal = dec!(0.15); // 15%

/// Per-strategy `slippageBps` bounds.
#[derive(Debug, Clone, Copy)]
pub struct SlippageBounds {
    pub floor_bps: u16,
    pub ceil_bps: u16,
}

impl Strategy {
    /// Strategy-specific Jupiter tolerance bounds.
    pub fn slippage_bounds(self) -> SlippageBounds {
        match self {
            // Tight: capital-preservation strategy, reject high-impact entries.
            Strategy::Shield => SlippageBounds {
                floor_bps: 10,
                ceil_bps: 100,
            },
            // Wider: speculative entries on thinner books.
            Strategy::Spear => SlippageBounds {
                floor_bps: 30,
                ceil_bps: 300,
            },
            // Generous: exits must fill even under stress.
            Strategy::Exit => SlippageBounds {
                floor_bps: 50,
                ceil_bps: 1500,
            },
        }
    }
}

/// Result of a slippage estimate.
#[derive(Debug, Clone, Copy)]
pub struct SlippageEstimate {
    /// Expected price-impact fraction (e.g. `0.015` = 1.5%).
    pub expected_fraction: Decimal,
    /// Jupiter on-chain tolerance (`slippageBps`), clamped to strategy bounds.
    pub tolerance_bps: u16,
}

impl SlippageEstimate {
    /// Cost (in SOL) of the expected impact for a trade of `amount_sol`.
    pub fn expected_cost_sol(&self, amount_sol: Decimal) -> Decimal {
        amount_sol * self.expected_fraction
    }
}

/// Inputs needed for the config-based size-tier fallback.
#[derive(Debug, Clone, Copy)]
pub struct FallbackTiers {
    pub small_fraction: Decimal,
    pub large_fraction: Decimal,
    pub threshold_sol: Decimal,
}

impl FallbackTiers {
    /// Pick the fallback fraction for a given trade size.
    pub fn fraction_for(&self, amount_sol: Decimal) -> Decimal {
        if amount_sol < self.threshold_sol {
            self.small_fraction
        } else {
            self.large_fraction
        }
    }
}

/// Compute the expected-impact fraction, preferring the most accurate source.
///
/// - `jupiter_impact_pct`: Jupiter's `priceImpactPct` as a **percent**
///   (`1.5` = 1.5%). When present it is the authoritative value.
/// - `liquidity_usd` / `sol_price_usd`: used for the square-root estimate.
/// - `fallback`: size-tier fractions when neither of the above is available.
pub fn expected_fraction(
    jupiter_impact_pct: Option<Decimal>,
    amount_sol: Decimal,
    liquidity_usd: Option<Decimal>,
    sol_price_usd: Option<Decimal>,
    fallback: FallbackTiers,
) -> Decimal {
    if let Some(pct) = jupiter_impact_pct {
        // pct is a percent (1.5 = 1.5%); convert to fraction.
        return pct / Decimal::from(100);
    }

    if let (Some(liq_usd), Some(sol_price)) = (liquidity_usd, sol_price_usd) {
        if sol_price > Decimal::ZERO && liq_usd > Decimal::ZERO {
            let trade_usd = amount_sol * sol_price;
            // Square-root market impact approximation, clamped to a sane range.
            let est = (trade_usd / (Decimal::from(2) * liq_usd))
                .max(LIQ_IMPACT_FLOOR)
                .min(LIQ_IMPACT_CEIL);
            return est;
        }
    }

    fallback.fraction_for(amount_sol)
}

/// Full estimate: expected impact + strategy-clamped Jupiter tolerance.
pub fn estimate(
    strategy: Strategy,
    jupiter_impact_pct: Option<Decimal>,
    amount_sol: Decimal,
    liquidity_usd: Option<Decimal>,
    sol_price_usd: Option<Decimal>,
    fallback: FallbackTiers,
) -> SlippageEstimate {
    let expected = expected_fraction(
        jupiter_impact_pct,
        amount_sol,
        liquidity_usd,
        sol_price_usd,
        fallback,
    );

    let bounds = strategy.slippage_bounds();

    // tolerance = expected × BUFFER_MULT + MIN_BUFFER
    let tolerance_fraction = expected * Decimal::from_f64(BUFFER_MULT).unwrap_or(Decimal::ONE)
        + MIN_BUFFER;
    let raw_bps = (tolerance_fraction * Decimal::from(10_000u64))
        .to_u16()
        .unwrap_or(bounds.ceil_bps);

    let tolerance_bps = raw_bps.clamp(bounds.floor_bps, bounds.ceil_bps);

    SlippageEstimate {
        expected_fraction: expected,
        tolerance_bps,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn fallback() -> FallbackTiers {
        FallbackTiers {
            small_fraction: dec!(0.005),
            large_fraction: dec!(0.01),
            threshold_sol: dec!(0.5),
        }
    }

    #[test]
    fn jupiter_impact_is_preferred_and_buffers_to_2x() {
        // 0.5% impact → expected 0.005, tolerance = 0.005×2 + 0.003 = 0.013 → 130 bps
        // (within Spear [30,300], so unclamped — confirms the 2× buffer over impact).
        let est = estimate(Strategy::Spear, Some(dec!(0.5)), dec!(1), None, None, fallback());
        assert_eq!(est.expected_fraction, dec!(0.005));
        assert_eq!(est.tolerance_bps, 130);
    }

    #[test]
    fn shield_clamps_to_tight_ceiling() {
        // 5% impact on Shield → tolerance = 0.05×2+0.003 = 0.103 → 1030 bps, ceiling 100.
        let est = estimate(Strategy::Shield, Some(dec!(5.0)), dec!(1), None, None, fallback());
        assert_eq!(est.tolerance_bps, 100);
    }

    #[test]
    fn exit_is_generous() {
        let est = estimate(Strategy::Exit, Some(dec!(10.0)), dec!(1), None, None, fallback());
        // 0.1×2 + 0.003 = 0.203 → 2030 bps, clamped to Exit ceiling 1500.
        assert_eq!(est.tolerance_bps, 1500);
    }

    #[test]
    fn liquidity_sqrt_estimate_used_without_jupiter_impact() {
        // 0.5 SOL @ $100 = $50 trade; $5k liquidity → 50/(2×5000) = 0.005 (0.5%)
        let est = estimate(
            Strategy::Spear,
            None,
            dec!(0.5),
            Some(dec!(5000)),
            Some(dec!(100)),
            fallback(),
        );
        assert_eq!(est.expected_fraction, dec!(0.005));
        // tolerance = 0.005×2 + 0.003 = 0.013 → 130 bps
        assert_eq!(est.tolerance_bps, 130);
    }

    #[test]
    fn fallback_tier_when_no_data() {
        // No impact, no liquidity → small-tier fallback (0.005)
        let est = estimate(Strategy::Spear, None, dec!(0.1), None, None, fallback());
        assert_eq!(est.expected_fraction, dec!(0.005));
    }

    #[test]
    fn fallback_large_tier_above_threshold() {
        let est = estimate(Strategy::Spear, None, dec!(1.0), None, None, fallback());
        assert_eq!(est.expected_fraction, dec!(0.01));
    }

    #[test]
    fn bounds_are_respected_for_each_strategy() {
        // Zero impact: tolerance = 0 + 30 bps buffer = 30 bps raw.
        let shield = estimate(Strategy::Shield, Some(dec!(0)), dec!(1), None, None, fallback());
        // 30 bps is within Shield [10,100] → unchanged.
        assert_eq!(shield.tolerance_bps, 30);
        let spear = estimate(Strategy::Spear, Some(dec!(0)), dec!(1), None, None, fallback());
        // 30 bps == Spear floor.
        assert_eq!(spear.tolerance_bps, 30);
        let exit = estimate(Strategy::Exit, Some(dec!(0)), dec!(1), None, None, fallback());
        // 30 bps < Exit floor 50 → clamped up to 50.
        assert_eq!(exit.tolerance_bps, 50);

        // Ceilings: a huge impact clamps to each strategy's ceiling.
        let shield_hi = estimate(Strategy::Shield, Some(dec!(50)), dec!(1), None, None, fallback());
        assert_eq!(shield_hi.tolerance_bps, 100); // Shield ceiling
        let exit_hi = estimate(Strategy::Exit, Some(dec!(50)), dec!(1), None, None, fallback());
        assert_eq!(exit_hi.tolerance_bps, 1500); // Exit ceiling
    }

    // --- Validation-suite tests for the 🛡️ safety slippage path (P1-4) ---
    // These mirror the plan's "slippage: assert slippageBps tracks expected
    // impact + buffer for a thin pool and a deep pool" check, as pure-math
    // unit tests that need no network.

    /// Deep pool ($500k): a 1 SOL trade's raw sqrt impact (0.01%) is below the
    /// LIQ_IMPACT_FLOOR (0.1%), so `expected_fraction` clamps UP to the floor
    /// and the tolerance is a small fixed value within Spear bounds. Asserting
    /// the exact clamped value (not a trivial `<=`) confirms the deep-pool
    /// behaviour rather than masking it behind the floor.
    #[test]
    fn deep_pool_slippage_is_small_and_buffered() {
        // 1 SOL @ $100 = $100 vs $500k liquidity → raw 100/(2×500_000) = 0.0001,
        // clamped up to LIQ_IMPACT_FLOOR = 0.001.
        let est = estimate(
            Strategy::Spear,
            None,
            dec!(1),
            Some(dec!(500_000)),
            Some(dec!(100)),
            fallback(),
        );
        assert_eq!(est.expected_fraction, LIQ_IMPACT_FLOOR, "deep pool clamps to the impact floor");
        // tolerance = (0.001×2 + 0.003) × 1e4 = 50 bps (within Spear [30,300]).
        assert_eq!(est.tolerance_bps, 50);
    }

    /// Thin pool ($5k): a 0.5 SOL trade has ~0.5% impact (inside the clamp band,
    /// so the sqrt formula is exercised); tolerance widens to 2×+buffer.
    #[test]
    fn thin_pool_slippage_widens_within_ceiling() {
        // 0.5 SOL @ $100 = $50 vs $5k → 50/(2×5000) = 0.005 (0.5%)
        let est = estimate(
            Strategy::Spear,
            None,
            dec!(0.5),
            Some(dec!(5_000)),
            Some(dec!(100)),
            fallback(),
        );
        assert_eq!(est.expected_fraction, dec!(0.005));
        // tolerance = 0.005×2 + 0.003 = 0.013 → 130 bps.
        assert_eq!(est.tolerance_bps, 130);
    }

    /// Invariant: tolerance always equals clamp(expected × BUFFER_MULT + buffer,
    /// bounds) — it tracks the expected impact plus a buffer, and never escapes
    /// the strategy bounds. Tested across the full impact range for every
    /// strategy, using the real `BUFFER_MULT` constant (so a future edit to the
    /// multiplier is caught).
    #[test]
    fn tolerance_tracks_expected_plus_buffer_within_bounds() {
        let mult = Decimal::from(BUFFER_MULT as u32);
        for &strategy in &[Strategy::Shield, Strategy::Spear, Strategy::Exit] {
            let bounds = strategy.slippage_bounds();
            for impact_pct in [dec!(0), dec!(0.1), dec!(1), dec!(3), dec!(10), dec!(50)] {
                let est = estimate(strategy, Some(impact_pct), dec!(1), None, None, fallback());
                // (1) Always within bounds.
                assert!(
                    (bounds.floor_bps..=bounds.ceil_bps).contains(&est.tolerance_bps),
                    "{:?} impact={}% tolerance {} outside [{},{}]",
                    strategy,
                    impact_pct,
                    est.tolerance_bps,
                    bounds.floor_bps,
                    bounds.ceil_bps
                );
                // (2) tolerance == clamp(round((expected × BUFFER_MULT + buffer) × 1e4), bounds).
                let raw = ((est.expected_fraction * mult + MIN_BUFFER) * Decimal::from(10_000u64))
                    .to_u16()
                    .unwrap_or(bounds.ceil_bps);
                let expected = raw.clamp(bounds.floor_bps, bounds.ceil_bps);
                assert_eq!(
                    est.tolerance_bps, expected,
                    "{:?} impact={}% tolerance mismatch",
                    strategy, impact_pct
                );
                // (3) For sub-ceiling impacts the buffer is present: tolerance
                // strictly exceeds the raw impact (in bps). High impacts that
                // clamp to the ceiling are exempt (covered above).
                let impact_bps = (est.expected_fraction * Decimal::from(10_000u64))
                    .to_u16()
                    .unwrap_or(0);
                if raw <= bounds.ceil_bps {
                    assert!(
                        est.tolerance_bps > impact_bps,
                        "{:?} impact={}% tolerance {} lacks buffer over impact_bps {}",
                        strategy,
                        impact_pct,
                        est.tolerance_bps,
                        impact_bps
                    );
                }
            }
        }
    }
}

