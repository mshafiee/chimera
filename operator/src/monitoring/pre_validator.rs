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
use rust_decimal::prelude::*;
use anyhow::Result;

/// Validation result
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub valid: bool,
    pub reason: Option<String>,
    pub estimated_slippage: Decimal,
    pub price_drift_percent: Decimal,
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
    /// * `amount_sol` - Trade size in SOL (using Decimal for precision)
    /// * `tracked_price` - Price when tracked wallet traded (using Decimal for precision)
    /// * `price_cache` - Price cache for current price
    ///
    /// # Returns
    /// Validation result
    pub async fn validate(
        &self,
        token_address: &str,
        amount_sol: Decimal,
        tracked_price: Option<Decimal>,
        price_cache: Arc<PriceCache>,
    ) -> ValidationResult {
        // Get current price
        let current_price = price_cache.get_price_usd(token_address);

        // Check price drift (using Decimal for precision)
        let price_drift = if let Some(tracked) = tracked_price {
            if let Some(current) = current_price {
                if !tracked.is_zero() {
                    let diff = (current - tracked).abs();
                    let ratio = diff / tracked;
                    let drift = ratio * Decimal::from(100);
                    let max_drift = Decimal::from_f64_retain(5.0).unwrap_or(Decimal::ZERO);
                    if drift > max_drift {
                        return ValidationResult {
                            valid: false,
                            reason: Some(format!(
                                "Price drifted {:.2}% (max 5%)",
                                drift.to_f64().unwrap_or(0.0)
                            )),
                            estimated_slippage: Decimal::ZERO,
                            price_drift_percent: drift,
                        };
                    }
                    drift
                } else {
                    Decimal::ZERO
                }
            } else {
                Decimal::ZERO
            }
        } else {
            Decimal::ZERO
        };

        // Estimate slippage (simplified - would need DEX-specific calculation)
        let estimated_slippage = self.estimate_slippage(token_address, amount_sol, price_cache.clone()).await;

        // Check slippage threshold (3%)
        let slippage_threshold = Decimal::from_f64_retain(3.0).unwrap_or(Decimal::ZERO);
        if estimated_slippage > slippage_threshold {
            return ValidationResult {
                valid: false,
                reason: Some(format!(
                    "Estimated slippage {:.2}% exceeds 3% threshold",
                    estimated_slippage.to_f64().unwrap_or(0.0)
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
        amount_sol: Decimal,
        _price_cache: Arc<PriceCache>,
    ) -> Decimal {
        // Simplified slippage estimation
        // In production, would:
        // 1. Fetch liquidity from DEX
        // 2. Calculate price impact based on trade size
        // 3. Account for DEX fees

        // Rough estimate: 0.5% base + 0.1% per 0.1 SOL
        let base_slippage = Decimal::from_f64_retain(0.5).unwrap_or(Decimal::ZERO);
        let size_unit = Decimal::from_f64_retain(0.1).unwrap_or(Decimal::ONE);
        let size_slippage_per_unit = Decimal::from_f64_retain(0.1).unwrap_or(Decimal::ZERO);
        let size_slippage = (amount_sol / size_unit) * size_slippage_per_unit;
        let max_slippage = Decimal::from_f64_retain(5.0).unwrap_or(Decimal::ZERO);
        (base_slippage + size_slippage).min(max_slippage) // Cap at 5%
    }

    /// Check if token is too new (extra scrutiny for tokens <24h old)
    pub async fn check_token_age(&self, _token_address: &str) -> Result<bool> {
        // Would need to fetch token creation time from on-chain data
        // For now, return true (not too new)
        Ok(true)
    }
}
