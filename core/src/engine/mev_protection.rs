//! MEV protection with dynamic Jito bundle tips
//!
//! Uses Jito bundles for all trades with dynamic tip calculation based on urgency:
//! - EXIT signals: High tip (0.005-0.01 SOL)
//! - Consensus BUY: Medium tip (0.002-0.005 SOL)
//! - Single BUY: Low tip (0.001-0.002 SOL)

use crate::config::MevProtectionConfig;
use crate::models::{Signal, Strategy};
use rust_decimal::prelude::*;
use std::sync::Arc;

/// MEV protection manager
pub struct MevProtection {
    config: Arc<MevProtectionConfig>,
}

impl MevProtection {
    pub fn new(config: Arc<MevProtectionConfig>) -> Self {
        // A zero/negative tip would silently disable MEV protection (or produce
        // an invalid bundle tip) with no signal at the call site. Surface it at
        // construction so misconfiguration is visible.
        for (name, tip) in [
            ("exit_tip_sol", config.exit_tip_sol),
            ("consensus_tip_sol", config.consensus_tip_sol),
            ("standard_tip_sol", config.standard_tip_sol),
        ] {
            if tip <= Decimal::ZERO {
                tracing::warn!(
                    config_key = name,
                    tip = %tip,
                    "MEV protection tip is non-positive — effective MEV protection disabled; configure a positive tip"
                );
            }
        }
        Self { config }
    }

    /// Calculate Jito tip based on signal urgency.
    ///
    /// Hydra unification (single authority): cluster/consensus buys use
    /// `hydra_cluster_tip_sol(amount) = 0.003 + 10%` clamped to
    /// `[consensus_tip_sol, exit_tip_sol]`. Exits keep the flat exit tip.
    /// Standard (non-consensus) signals keep the flat standard tip.
    pub fn calculate_tip(&self, signal: &Signal, is_consensus: bool) -> Decimal {
        // EXIT signals get highest priority
        if signal.payload.strategy == Strategy::Exit {
            return self.config.exit_tip_sol;
        }

        // Consensus/cluster signals: Hydra unified formula.
        if is_consensus {
            let hydra = super::tip_inlining::hydra_cluster_tip_sol(signal.payload.amount_sol);
            return hydra
                .max(self.config.consensus_tip_sol)
                .min(self.config.exit_tip_sol);
        }

        // Standard signals get low priority
        self.config.standard_tip_sol
    }

    /// Check if Jito bundles should always be used
    pub fn always_use_jito(&self) -> bool {
        self.config.always_use_jito
    }

    /// Add random delay to avoid predictable patterns (50-200ms)
    pub async fn add_random_delay(&self) {
        use rand::Rng;
        let mut rng = rand::rng();
        let delay_ms = rng.random_range(50..=200);
        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Action, SignalPayload};
    use rust_decimal_macros::dec;

    fn test_signal(strategy: Strategy) -> Signal {
        Signal::new(
            SignalPayload {
                strategy,
                token: "BONK".to_string(),
                token_address: Some(
                    "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
                ),
                action: if strategy == Strategy::Exit {
                    Action::Sell
                } else {
                    Action::Buy
                },
                amount_sol: dec!(0.5),
                wallet_address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
                trade_uuid: None,
                exit_fraction: None,
                trial_admission: false,
            },
            12345,
            None,
        )
    }

    #[test]
    fn test_calculate_tip_exit_highest_priority() {
        let config = Arc::new(MevProtectionConfig::default());
        let mev = MevProtection::new(config.clone());
        let tip = mev.calculate_tip(&test_signal(Strategy::Exit), false);
        assert_eq!(tip, config.exit_tip_sol);
        // Exit tip beats the consensus multiplier even when flagged consensus
        assert_eq!(tip, dec!(0.007));
    }

    #[test]
    fn test_calculate_tip_consensus_uses_hydra_formula() {
        let config = Arc::new(MevProtectionConfig::default());
        let mev = MevProtection::new(config.clone());
        // 0.5 SOL → hydra 0.003+0.05=0.053, clamped to exit_tip ceiling 0.007.
        let tip = mev.calculate_tip(&test_signal(Strategy::Shield), true);
        assert_eq!(tip, config.exit_tip_sol);
        assert_eq!(tip, dec!(0.007));
        // Dust size 0.05 SOL → 0.003+0.005=0.008 → still ceiling 0.007.
        // Small size 0.001 → 0.0031 → above consensus floor 0.003, below ceiling.
        let mut small = test_signal(Strategy::Shield);
        small.payload.amount_sol = dec!(0.001);
        assert_eq!(mev.calculate_tip(&small, true), dec!(0.0031));
    }

    #[test]
    fn test_calculate_tip_standard() {
        let config = Arc::new(MevProtectionConfig::default());
        let mev = MevProtection::new(config.clone());
        let tip = mev.calculate_tip(&test_signal(Strategy::Spear), false);
        assert_eq!(tip, config.standard_tip_sol);
        assert_eq!(tip, dec!(0.0015));
    }

    #[test]
    fn test_always_use_jito() {
        let config = Arc::new(MevProtectionConfig::default());
        assert!(MevProtection::new(config).always_use_jito());

        let mut disabled = MevProtectionConfig::default();
        disabled.always_use_jito = false;
        assert!(!MevProtection::new(Arc::new(disabled)).always_use_jito());
    }

    #[test]
    fn test_new_warns_on_non_positive_tips() {
        let mut config = MevProtectionConfig::default();
        config.exit_tip_sol = dec!(0.0);
        config.consensus_tip_sol = dec!(-0.001);
        config.standard_tip_sol = dec!(0.0015);
        // Must not panic; just logs warnings and constructs
        let mev = MevProtection::new(Arc::new(config));
        assert_eq!(mev.calculate_tip(&test_signal(Strategy::Exit), false), dec!(0.0));
    }

    #[tokio::test]
    async fn test_add_random_delay_returns() {
        let config = Arc::new(MevProtectionConfig::default());
        let mev = MevProtection::new(config);
        let start = std::time::Instant::now();
        mev.add_random_delay().await;
        assert!(start.elapsed().as_millis() >= 40);
    }
}
