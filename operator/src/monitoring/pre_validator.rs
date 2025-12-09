//! Pre-execution validation to prevent losses
//!
//! Validates trades before execution:
//! - Price drift check (reject if price moved >5%)
//! - Liquidity validation
//! - Slippage estimation
//! - Token age check

use std::sync::Arc;
use crate::config::AppConfig;
use crate::price_cache::PriceCache;
use anyhow::Result;

/// Validation result
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub valid: bool,
    pub reason: Option<String>,
    pub estimated_slippage: f64,
    pub price_drift_percent: f64,
}

/// Pre-execution validator
pub struct PreValidator {
    config: Arc<AppConfig>,
}

impl PreValidator {
    pub fn new(config: Arc<AppConfig>) -> Self {
        Self { config }
    }

    /// Validate trade before execution
    ///
    /// # Arguments
    /// * `token_address` - Token to trade
    /// * `amount_sol` - Trade size in SOL
    /// * `tracked_price` - Price when tracked wallet traded
    /// * `price_cache` - Price cache for current price
    ///
    /// # Returns
    /// Validation result
    pub async fn validate(
        &self,
        token_address: &str,
        amount_sol: f64,
        tracked_price: Option<f64>,
        price_cache: Arc<PriceCache>,
    ) -> ValidationResult {
        // Get current price
        let current_price = price_cache.get_price_usd(token_address);

        // Check price drift
        let price_drift = if let Some(tracked) = tracked_price {
            if let Some(current) = current_price {
                let drift = ((current - tracked) / tracked).abs() * 100.0;
                if drift > 5.0 {
                    return ValidationResult {
                        valid: false,
                        reason: Some(format!(
                            "Price drifted {:.2}% (max 5%)",
                            drift
                        )),
                        estimated_slippage: 0.0,
                        price_drift_percent: drift,
                    };
                }
                drift
            } else {
                0.0
            }
        } else {
            0.0
        };

        // Estimate slippage (simplified - would need DEX-specific calculation)
        let estimated_slippage = self.estimate_slippage(token_address, amount_sol, price_cache.clone()).await;

        // Check slippage threshold (3%)
        if estimated_slippage > 3.0 {
            return ValidationResult {
                valid: false,
                reason: Some(format!(
                    "Estimated slippage {:.2}% exceeds 3% threshold",
                    estimated_slippage
                )),
                estimated_slippage,
                price_drift_percent: price_drift,
            };
        }

        // Check liquidity (would need to fetch from DEX)
        // For now, assume liquidity check is done elsewhere

        ValidationResult {
            valid: true,
            reason: None,
            estimated_slippage,
            price_drift_percent: price_drift,
        }
    }

    /// Estimate slippage for a trade
    async fn estimate_slippage(
        &self,
        _token_address: &str,
        amount_sol: f64,
        _price_cache: Arc<PriceCache>,
    ) -> f64 {
        // Simplified slippage estimation
        // In production, would:
        // 1. Fetch liquidity from DEX
        // 2. Calculate price impact based on trade size
        // 3. Account for DEX fees

        // Rough estimate: 0.5% base + 0.1% per 0.1 SOL
        let base_slippage = 0.5;
        let size_slippage = (amount_sol / 0.1) * 0.1;
        (base_slippage + size_slippage).min(5.0) // Cap at 5%
    }

    /// Check if token is too new (extra scrutiny for tokens <24h old)
    pub async fn check_token_age(&self, _token_address: &str) -> Result<bool> {
        // Would need to fetch token creation time from on-chain data
        // For now, return true (not too new)
        Ok(true)
    }
}
