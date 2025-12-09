//! Confidence-based dynamic position sizing
//!
//! Calculates position size based on:
//! - Base size
//! - Confidence multiplier (consensus, WQS, etc.)
//! - Wallet performance multiplier
//! - Portfolio limits

use std::sync::Arc;
use crate::config::PositionSizingConfig;
use crate::db::DbPool;

/// Position sizer
pub struct PositionSizer {
    db: DbPool,
    config: Arc<PositionSizingConfig>,
}

/// Position sizing factors
#[derive(Debug, Clone)]
pub struct SizingFactors {
    pub is_consensus: bool,
    pub wallet_wqs: f64,
    pub wallet_success_rate: f64,
    pub token_age_hours: Option<f64>,
    pub estimated_slippage: f64,
}

impl PositionSizer {
    pub fn new(db: DbPool, config: Arc<PositionSizingConfig>) -> Self {
        Self { db, config }
    }

    /// Calculate position size based on factors
    ///
    /// Formula: base_size * confidence_multiplier * wallet_performance_multiplier
    ///
    /// # Arguments
    /// * `factors` - Sizing factors
    ///
    /// # Returns
    /// Position size in SOL
    pub async fn calculate_size(&self, factors: SizingFactors) -> f64 {
        let mut size = self.config.base_size_sol;

        // Confidence multiplier
        let confidence_mult = if factors.is_consensus {
            self.config.consensus_multiplier
        } else {
            1.0
        };

        // High WQS multiplier (>80)
        let wqs_mult = if factors.wallet_wqs >= 80.0 {
            1.2
        } else {
            1.0
        };

        // Wallet performance multiplier (based on success rate)
        let performance_mult = if factors.wallet_success_rate >= 0.6 {
            1.1
        } else if factors.wallet_success_rate < 0.4 {
            0.8
        } else {
            1.0
        };

        // New token penalty (<24h old)
        let token_age_mult = if let Some(age) = factors.token_age_hours {
            if age < 24.0 {
                0.5
            } else {
                1.0
            }
        } else {
            1.0
        };

        // High slippage penalty (>2%)
        let slippage_mult = if factors.estimated_slippage > 2.0 {
            0.7
        } else {
            1.0
        };

        // Apply all multipliers
        size *= confidence_mult;
        size *= wqs_mult;
        size *= performance_mult;
        size *= token_age_mult;
        size *= slippage_mult;

        // Apply min/max bounds
        size = size.max(self.config.min_size_sol);
        size = size.min(self.config.max_size_sol);

        size
    }

    /// Get sizing factors for a wallet
    pub async fn get_sizing_factors(
        &self,
        wallet_address: &str,
        is_consensus: bool,
        estimated_slippage: f64,
    ) -> SizingFactors {
        // Get wallet from database
        let wallet_opt = crate::db::get_wallet_by_address(&self.db, wallet_address).await;
        let wqs = match wallet_opt {
            Ok(Some(w)) => w.wqs_score.unwrap_or(50.0),
            _ => 50.0,
        };

        // Get wallet performance metrics (would need to implement)
        // For now, use default success rate
        let success_rate = 0.5; // TODO: Get from wallet_performance tracker

        SizingFactors {
            is_consensus,
            wallet_wqs: wqs,
            wallet_success_rate: success_rate,
            token_age_hours: None, // TODO: Fetch token age
            estimated_slippage,
        }
    }

    /// Check if we can open a new position (portfolio limits)
    pub async fn can_open_position(&self) -> bool {
        // TODO: Query database for current position count
        // For now, assume we can open
        true
    }
}
