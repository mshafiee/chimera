//! Shadow-proof evidence rule (2026-08-30).
//!
//! Single source of truth for "this wallet's own shadow book proves copy-trading
//! edge". Used by (a) the wallet-performance demotion paths and (b) the dune
//! on-chain audit, so that no secondary heuristic (Helius round-trip counting,
//! inactivity timers) can demote a wallet whose trailing deduped mirror_main
//! book demonstrates positive expectancy. Established by the 2026-08-29/30
//! backtest: the shadow-proven roster policy simulated +120.46 SOL over 60d
//! vs −1.44 SOL status quo, concentrated in books (132Tkgf5YE, 12kNFpfihj)
//! that secondary heuristics kept locking out.

use crate::db_abstraction::ShadowKellyStats;

/// True when the trailing deduped mirror_main shadow book holds at least
/// `min_samples` exits with positive gross expectancy.
///
/// `expectancy` is the per-trade gross return fraction:
/// `win_rate * avg_win − (1 − win_rate) * avg_loss`.
pub fn shadow_proven_edge(stats: &ShadowKellyStats, min_samples: i64) -> bool {
    if stats.samples < min_samples {
        return false;
    }
    let expectancy =
        stats.win_rate * stats.avg_win - (rust_decimal::Decimal::ONE - stats.win_rate) * stats.avg_loss;
    expectancy > rust_decimal::Decimal::ZERO
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn stats(samples: i64, win: &str, avg_win: &str, avg_loss: &str) -> ShadowKellyStats {
        ShadowKellyStats {
            samples,
            win_rate: rust_decimal::Decimal::from_str(win).unwrap(),
            avg_win: rust_decimal::Decimal::from_str(avg_win).unwrap(),
            avg_loss: rust_decimal::Decimal::from_str(avg_loss).unwrap(),
        }
    }

    #[test]
    fn test_proven_with_positive_expectancy_and_samples() {
        // 0.8*0.20 − 0.2*0.02 = 0.156 > 0, samples 25 >= 20.
        assert!(shadow_proven_edge(&stats(25, "0.8", "0.20", "0.02"), 20));
    }

    #[test]
    fn test_not_proven_below_sample_floor() {
        assert!(!shadow_proven_edge(&stats(19, "0.8", "0.20", "0.02"), 20));
    }

    #[test]
    fn test_not_proven_with_negative_expectancy() {
        // 0.8*0.01 − 0.2*0.03 = 0.002 − wait: 0.008 − 0.006 = 0.002 > 0 — use a
        // clearly negative book: 0.2 win rate, tiny wins, big losses.
        let s = stats(50, "0.2", "0.01", "0.05");
        assert!(!shadow_proven_edge(&s, 20));
    }

    #[test]
    fn test_not_proven_at_exact_zero_expectancy() {
        // 0.5*0.04 − 0.5*0.04 = 0 → strict > 0 required.
        let s = stats(30, "0.5", "0.04", "0.04");
        assert!(!shadow_proven_edge(&s, 20));
    }

    #[test]
    fn test_proven_with_no_loss_history() {
        // avg_loss 0 (never lost): expectancy = win_rate * avg_win > 0.
        assert!(shadow_proven_edge(&stats(22, "0.9", "0.05", "0"), 20));
    }
}
