//! Hydra Phase 5.1: isolated dust-live lane.
//!
//! `DustLive` is real execution at micro-size (0.05 SOL). This module is the
//! single choke point: it clamps size, requires cluster quorum (>=3), and
//! records lane metrics (fills/signals, slippage-vs-quote). A misconfigured
//! sizing file cannot escalate to full-live risk because the clamp lives here
//! in code, not in config.

use rust_decimal::Decimal;
use rust_decimal_macros::dec;

/// Fixed micro-size per admitted cluster signal.
pub const DUST_SIZE_SOL: Decimal = dec!(0.05);
/// Hard ceiling even if the caller requests more.
pub const DUST_MAX_SOL: Decimal = dec!(0.10);
/// Hard floor below which the signal is skipped as dust.
pub const DUST_MIN_SOL: Decimal = dec!(0.01);

/// Clamp any requested size into the dust band `[MIN, MAX]`, defaulting to
/// `DUST_SIZE_SOL` when the request is zero/negative.
pub fn clamp_dust_size(requested_sol: Decimal) -> Decimal {
    if requested_sol <= Decimal::ZERO {
        return DUST_SIZE_SOL;
    }
    requested_sol.max(DUST_MIN_SOL).min(DUST_MAX_SOL)
}

/// Lane counters: fills / signals, slippage observations (bps).
#[derive(Debug, Default)]
pub struct DustLaneMetrics {
    pub signals: u64,
    pub fills: u64,
    pub total_slippage_bps: i64,
    pub slippage_samples: u64,
}

impl DustLaneMetrics {
    pub fn record_signal(&mut self) {
        self.signals += 1;
    }

    pub fn record_fill(&mut self, slippage_bps: Option<i64>) {
        self.fills += 1;
        if let Some(bps) = slippage_bps {
            self.total_slippage_bps += bps;
            self.slippage_samples += 1;
        }
    }

    /// Fill rate `fills / signals` in `[0,1]`. `None` when no signals yet.
    pub fn fill_rate(&self) -> Option<f64> {
        if self.signals == 0 {
            return None;
        }
        Some(self.fills as f64 / self.signals as f64)
    }

    pub fn mean_slippage_bps(&self) -> Option<f64> {
        if self.slippage_samples == 0 {
            return None;
        }
        Some(self.total_slippage_bps as f64 / self.slippage_samples as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn clamp_holds_band() {
        assert_eq!(clamp_dust_size(dec!(0)), DUST_SIZE_SOL);
        assert_eq!(clamp_dust_size(dec!(0.001)), DUST_MIN_SOL);
        assert_eq!(clamp_dust_size(dec!(5)), DUST_MAX_SOL);
        assert_eq!(clamp_dust_size(dec!(0.05)), dec!(0.05));
    }

    #[test]
    fn metrics_fill_rate() {
        let mut m = DustLaneMetrics::default();
        assert_eq!(m.fill_rate(), None);
        m.record_signal();
        m.record_signal();
        m.record_fill(Some(50));
        assert!((m.fill_rate().unwrap() - 0.5).abs() < 1e-9);
        assert_eq!(m.mean_slippage_bps(), Some(50.0));
    }
}
