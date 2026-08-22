//! Shared exit-rule evaluation used by BOTH the live shadow mirror
//! (`shadow_trader`) and the offline price-path replay harness
//! (`bin/replay_exit.rs`). This is the single source of truth for the
//! *stop / recovery / trailing / time* rails, so a grid-search over those rules
//! cannot drift from what the monitor runs.
//!
//! IMPORTANT — profit-target divergence: this function records exactly ONE exit
//! per (position, strategy). It therefore returns a **full exit** on the first
//! crossed profit target (`profit_target_<x>`). The live monitor
//! (`profit_targets.rs`) instead performs **tiered PARTIAL** sells (e.g. 33% at
//! each tier) and lets the remainder trail. So `mirror_main` shadow PnL is a
//! *full-exit proxy* at the first target, NOT a byte-identical replica of live
//! behavior — a grid-search over profit targets here does NOT model the live
//! partial-then-trail path. (`config.targets` is populated in every deployed
//! config: prod `config.yaml` sets `[5.0]`, the code default is
//! `[25,50,100,200]`.)
//!
//! Order mirrors the real position monitor (`stop_loss.rs` + `profit_targets.rs`):
//! hard stop, recovery gate, adaptive stop, wick window, trailing stop, profit
//! targets, tiered time exit.

use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use crate::config::ProfitManagementConfig;
use crate::engine::exit_profile::EffectiveExitParams;

/// Decide whether a position should exit given the current tick.
///
/// Same contract as `ShadowTrader::check_mirror_main` (kept in lockstep).
/// Returns the exit `reason` string, or `None` to hold.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_exit(
    config: &ProfitManagementConfig,
    eff: &EffectiveExitParams,
    entry_price: Decimal,
    current_price: Decimal,
    peak_price: Decimal,
    elapsed_secs: i64,
    strategy: &str,
) -> Option<String> {
    if entry_price.is_zero() {
        return None;
    }

    let pnl_pct = (current_price - entry_price) / entry_price * Decimal::from(100);
    let profit_pct = pnl_pct.max(Decimal::ZERO);
    let loss_pct = pnl_pct.min(Decimal::ZERO);
    let elapsed_secs_u64 = elapsed_secs as u64;

    // 1. Hard stop: absolute floor at -25%.
    if loss_pct <= dec!(-25) {
        return Some("stop_loss".to_string());
    }

    // 2. Recovery gate (selective, Phase 2): cut a below-threshold position
    //    only when it is at/beyond the hard threshold (genuine dump) OR has
    //    stayed below threshold past `recovery_gate_max_secs`.
    if elapsed_secs_u64 > config.recovery_gate_secs
        && loss_pct < config.recovery_gate_threshold
        && (loss_pct <= config.recovery_gate_hard_threshold
            || elapsed_secs_u64 >= config.recovery_gate_max_secs)
    {
        return Some("recovery_gate".to_string());
    }

    // 3. Adaptive stop approximation (flat max in the mirror).
    if loss_pct <= config.max_stop_loss_distance {
        return Some("stop_loss".to_string());
    }

    // 4. Wick window: cap losses during the first wick_protection_secs.
    if elapsed_secs_u64 <= config.wick_protection_secs
        && loss_pct <= config.wick_protection_max_loss_percent
    {
        return Some("stop_loss".to_string());
    }

    // 5. Trailing stop — per-wallet activation/distance when a profile exists.
    if profit_pct >= eff.trailing_activation_pct {
        let trailing_stop_price =
            peak_price * (Decimal::ONE - eff.trailing_distance_pct / Decimal::from(100));
        if current_price <= trailing_stop_price {
            return Some("trailing_stop".to_string());
        }
    }

    // 6. Profit targets. NOTE: `config.targets` is NOT empty in any deployed
    //    config (prod `config.yaml` sets `[5.0]`; code default is
    //    `[25,50,100,200]`), and the live monitor (`profit_targets.rs`) uses
    //    them for tiered PARTIAL sells. Here we emit a FULL exit on the first
    //    crossed target — the single-exit-per-position proxy described in the
    //    module docstring. This branch is live, not dead code; it diverges from
    //    live only in full-vs-partial exit semantics.
    // TODO(perf-parity): if `mirror_main` must predict live PnL, model the
    //    partial-then-trail path instead of a single full exit at the first tier.
    for target in &config.targets {
        if profit_pct >= *target {
            return Some(format!("profit_target_{}", target));
        }
    }

    // 7. Tiered time exit, per-wallet hours when a profile exists.
    let is_spear = strategy == "SPEAR";
    let exit_limit_hours = if profit_pct > dec!(25) {
        eff.high_profit_hours
    } else if profit_pct > dec!(10) {
        eff.medium_profit_hours
    } else if is_spear {
        config.losing_time_exit_hours_spear
    } else {
        config.losing_time_exit_hours_shield
    };

    if elapsed_secs >= exit_limit_hours as i64 * 3600 {
        return Some("time_exit".to_string());
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProfitManagementConfig;
    use crate::engine::exit_profile::EffectiveExitParams;
    use rust_decimal_macros::dec;

    fn cfg() -> ProfitManagementConfig {
        ProfitManagementConfig::default()
    }

    fn eff() -> EffectiveExitParams {
        EffectiveExitParams {
            high_profit_hours: 24,
            medium_profit_hours: 48,
            trailing_activation_pct: dec!(5),
            trailing_distance_pct: dec!(3),
        }
    }

    #[test]
    fn hard_stop_fires() {
        let c = cfg();
        assert_eq!(
            evaluate_exit(&c, &eff(), dec!(1.0), dec!(0.70), dec!(1.0), 60, "SHIELD"),
            Some("stop_loss".to_string())
        );
    }

    #[test]
    fn recovery_gate_fires_on_hard_loss() {
        let c = cfg();
        // below soft threshold AND at/beyond hard threshold -> cut
        assert_eq!(
            evaluate_exit(&c, &eff(), dec!(1.0), dec!(0.95), dec!(1.0), 200, "SHIELD"),
            Some("recovery_gate".to_string())
        );
    }

    #[test]
    fn time_exit_fires_after_losing_hours() {
        let c = cfg();
        let hours = c.losing_time_exit_hours_shield;
        assert_eq!(
            evaluate_exit(&c, &eff(), dec!(1.0), dec!(1.0), dec!(1.0), hours as i64 * 3600, "SHIELD"),
            Some("time_exit".to_string())
        );
    }

    #[test]
    fn no_exit_when_flat_and_recent() {
        let c = cfg();
        assert_eq!(
            evaluate_exit(&c, &eff(), dec!(1.0), dec!(1.0), dec!(1.0), 10, "SHIELD"),
            None
        );
    }
}
