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

    /// Calculate Jito tip based on signal urgency
    ///
    /// # Arguments
    /// * `signal` - Trading signal
    /// * `is_consensus` - Whether this is a consensus signal (multiple wallets)
    ///
    /// # Returns
    /// Tip amount in SOL (using Decimal for precision)
    pub fn calculate_tip(&self, signal: &Signal, is_consensus: bool) -> Decimal {
        // EXIT signals get highest priority
        if signal.payload.strategy == Strategy::Exit {
            return self.config.exit_tip_sol;
        }

        // Consensus signals get higher priority (increased tip for consensus)
        if is_consensus {
            // Use higher tip for consensus (1.5x the standard consensus tip)
            let consensus_tip_multiplier: Decimal = Decimal::new(15, 1);
            return self.config.consensus_tip_sol * consensus_tip_multiplier;
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
