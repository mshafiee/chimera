//! MEV protection with dynamic Jito bundle tips
//!
//! Uses Jito bundles for all trades with dynamic tip calculation based on urgency:
//! - EXIT signals: High tip (0.005-0.01 SOL)
//! - Consensus BUY: Medium tip (0.002-0.005 SOL)
//! - Single BUY: Low tip (0.001-0.002 SOL)

use std::sync::Arc;
use crate::config::MevProtectionConfig;
use crate::models::{Signal, Strategy};

/// MEV protection manager
pub struct MevProtection {
    config: Arc<MevProtectionConfig>,
}

impl MevProtection {
    pub fn new(config: Arc<MevProtectionConfig>) -> Self {
        Self { config }
    }

    /// Calculate Jito tip based on signal urgency
    ///
    /// # Arguments
    /// * `signal` - Trading signal
    /// * `is_consensus` - Whether this is a consensus signal (multiple wallets)
    ///
    /// # Returns
    /// Tip amount in SOL
    pub fn calculate_tip(&self, signal: &Signal, is_consensus: bool) -> f64 {
        // EXIT signals get highest priority
        if signal.payload.strategy == Strategy::Exit {
            return self.config.exit_tip_sol;
        }

        // Consensus signals get higher priority (increased tip for consensus)
        if is_consensus {
            // Use higher tip for consensus (1.5x the standard consensus tip)
            return self.config.consensus_tip_sol * 1.5;
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
        let mut rng = rand::thread_rng();
        let delay_ms = rng.gen_range(50..=200);
        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
    }
}
