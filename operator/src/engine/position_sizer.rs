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
use rust_decimal::prelude::*;
use sqlx;

/// Position sizer
pub struct PositionSizer {
    db: DbPool,
    config: Arc<PositionSizingConfig>,
}

/// Position sizing factors
#[derive(Debug, Clone)]
pub struct SizingFactors {
    pub is_consensus: bool,
    pub wallet_wqs: f64,  // WQS score (0-100), used for threshold comparisons only
    pub wallet_success_rate: Decimal,  // Success rate (0.0-1.0), used in financial calculations
    pub token_age_hours: Option<f64>,  // Token age in hours, used for threshold comparisons only
    pub estimated_slippage: Decimal,  // Slippage percentage, used in financial calculations
    /// Signal quality score (0.0-1.0)
    pub signal_quality: Option<Decimal>,  // Quality score, used in financial calculations
    /// Token 24h volatility percentage (None if unknown)
    pub token_volatility_24h: Option<Decimal>,  // Volatility percentage, used in financial calculations
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
    /// Position size in SOL (using Decimal for precision)
    pub async fn calculate_size(&self, factors: SizingFactors) -> Decimal {
        let mut size = self.config.base_size_sol;

        // Confidence multiplier (using Decimal)
        let confidence_mult = if factors.is_consensus {
            self.config.consensus_multiplier
        } else {
            Decimal::ONE
        };

        // High WQS multiplier (>80) - use Decimal constants to avoid f64 precision issues
        let wqs_mult = if factors.wallet_wqs >= 80.0 {
            Decimal::from_str("1.2").unwrap_or(Decimal::ONE)
        } else {
            Decimal::ONE
        };

        // Wallet performance multiplier (based on success rate)
        let performance_mult = if factors.wallet_success_rate >= Decimal::from_str("0.6").unwrap_or(Decimal::ZERO) {
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

        // High slippage penalty (>2%)
        let slippage_mult = if factors.estimated_slippage > Decimal::from_str("2.0").unwrap_or(Decimal::ZERO) {
            Decimal::from_str("0.7").unwrap_or(Decimal::ONE)
        } else {
            Decimal::ONE
        };

        // Signal quality multiplier
        // High quality (>0.9): 1.3x
        // Medium quality (0.7-0.9): 1.0x
        // Low quality (<0.7): 0.7x (shouldn't reach here due to filter)
        let quality_mult = if let Some(quality) = factors.signal_quality {
            let high_threshold = Decimal::from_str("0.9").unwrap_or(Decimal::ZERO);
            let medium_threshold = Decimal::from_str("0.7").unwrap_or(Decimal::ZERO);
            if quality >= high_threshold {
                Decimal::from_str("1.3").unwrap_or(Decimal::ONE)
            } else if quality >= medium_threshold {
                Decimal::ONE
            } else {
                Decimal::from_str("0.7").unwrap_or(Decimal::ONE)
            }
        } else {
            Decimal::ONE  // Default if quality not provided
        };

        // Volatility multiplier (reduce size for high volatility)
        // If volatility > 30%, reduce size proportionally
        // Use Decimal arithmetic to avoid f64 precision issues
        let volatility_mult = if let Some(volatility) = factors.token_volatility_24h {
            let threshold = Decimal::from_str("30.0").unwrap_or(Decimal::ZERO);
            if volatility > threshold {
                let step = Decimal::from_str("10.0").unwrap_or(Decimal::ONE);
                let reduction_rate = Decimal::from_str("0.3").unwrap_or(Decimal::ZERO);
                let min_mult = Decimal::from_str("0.5").unwrap_or(Decimal::ONE);
                
                // Calculate: (volatility - 30) / 10 * 0.3
                let excess = volatility - threshold;
                let steps = excess / step;
                let reduction = steps * reduction_rate;
                let multiplier = Decimal::ONE - reduction;
                multiplier.max(min_mult) // Minimum 50% of base size
            } else {
                Decimal::ONE
            }
        } else {
            Decimal::ONE  // Default if volatility unknown
        };

        // Apply all multipliers using Decimal arithmetic
        size = size * confidence_mult;
        size = size * wqs_mult;
        size = size * performance_mult;
        size = size * token_age_mult;
        size = size * slippage_mult;
        size = size * quality_mult;
        size = size * volatility_mult;

        // Apply min/max bounds
        size = size.max(self.config.min_size_sol);
        size = size.min(self.config.max_size_sol);

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
    pub async fn get_sizing_factors(
        &self,
        wallet_address: &str,
        is_consensus: bool,
        estimated_slippage: Decimal,
        token_address: Option<&str>,
        helius_client: Option<&crate::monitoring::HeliusClient>,
    ) -> SizingFactors {
        // Get wallet from database
        let wallet_opt = crate::db::get_wallet_by_address(&self.db, wallet_address).await;
        let wqs = match wallet_opt {
            Ok(Some(w)) => w.wqs_score.unwrap_or(50.0),
            _ => 50.0,
        };

        // Get wallet performance metrics from database
        // Convert success rate percentage to Decimal (0.0-1.0)
        let success_rate = match crate::db::get_wallet_copy_performance(&self.db, wallet_address).await {
            Ok(Some(metrics)) => {
                Decimal::from_f64_retain(metrics.signal_success_rate / 100.0)
                    .unwrap_or(Decimal::from_str("0.5").unwrap_or(Decimal::ZERO))
            },
            _ => Decimal::from_str("0.5").unwrap_or(Decimal::ZERO), // Default fallback if no performance data exists
        };

        // Get token age if token address and Helius client are provided
        let token_age_hours = if let (Some(token_addr), Some(helius)) = (token_address, helius_client) {
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
            signal_quality: None,  // Will be set by caller if available
            token_volatility_24h: None,  // Will be set by caller if available
        }
    }

    /// Check if we can open a new position (portfolio limits)
    pub async fn can_open_position(&self) -> bool {
        // Query database for current active position count
        let active_count: i64 = match sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM positions WHERE state = 'ACTIVE'"
        )
        .fetch_one(&self.db)
        .await
        {
            Ok(count) => count,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to query active positions, allowing trade");
                // On error, allow trade but log warning
                return true;
            }
        };
        
        active_count < self.config.max_concurrent_positions as i64
    }
}
