//! Pre-execution validation to prevent losses
//!
//! Validates trades before execution:
//! - Price drift check (reject if price moved >5%)
//! - Liquidity validation
//! - Slippage estimation
//! - Token age check

use crate::config::AppConfig;
use crate::monitoring::HeliusClient;
use crate::price_cache::PriceCache;
use crate::token::TokenMetadataFetcher;
use anyhow::Result;
use rust_decimal::prelude::*;
use std::sync::Arc;

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
    helius_client: Option<Arc<HeliusClient>>,
    token_fetcher: Option<Arc<TokenMetadataFetcher>>,
    /// FIX 1: Price cache for non-blocking validation
    price_cache: Option<Arc<PriceCache>>,
}

impl PreValidator {
    pub fn new(config: Arc<AppConfig>) -> Self {
        Self {
            config,
            helius_client: None,
            token_fetcher: None,
            price_cache: None,
        }
    }

    /// Attach a Helius client to enable token age checks.
    pub fn with_helius(mut self, client: Arc<HeliusClient>) -> Self {
        self.helius_client = Some(client);
        self
    }

    /// Attach a token fetcher to enable liquidity-based slippage estimation.
    pub fn with_token_fetcher(mut self, fetcher: Arc<TokenMetadataFetcher>) -> Self {
        self.token_fetcher = Some(fetcher);
        self
    }

    /// Attach a price cache to enable non-blocking validation.
    /// FIX 1: Required for validate_local() fast path
    pub fn with_price_cache(mut self, price_cache: Arc<PriceCache>) -> Self {
        self.price_cache = Some(price_cache);
        self
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
            match current_price {
                Some(current) if !tracked.is_zero() => {
                    let diff = (current - tracked).abs();
                    let ratio = diff / tracked;
                    let drift = ratio * Decimal::from(100);
                    let max_drift = Decimal::from_str("5.0").unwrap_or(Decimal::ZERO);
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
                }
                None => {
                    // Tracked price exists but cache is cold — cannot verify drift.
                    // Fail-closed: without a current price we cannot confirm the signal
                    // isn't stale, and silently passing would allow drift-check bypass.
                    return ValidationResult {
                        valid: false,
                        reason: Some(
                            "Price drift check unavailable: current price not in cache (cold cache miss)"
                                .to_string(),
                        ),
                        estimated_slippage: Decimal::ZERO,
                        price_drift_percent: Decimal::ZERO,
                    };
                }
                _ => Decimal::ZERO,
            }
        } else {
            Decimal::ZERO
        };

        // Estimate slippage (simplified - would need DEX-specific calculation)
        let estimated_slippage = self
            .estimate_slippage(token_address, amount_sol, price_cache.clone())
            .await;

        // Check slippage threshold (3%)
        let slippage_threshold = Decimal::from_str("3.0").unwrap_or(Decimal::ZERO);
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

        // Check liquidity via DexScreener when token_fetcher is available.
        // Use the lower of shield/spear thresholds to be conservative regardless of strategy.
        if let Some(ref fetcher) = self.token_fetcher {
            let min_liq = self
                .config
                .token_safety
                .min_liquidity_spear_usd
                .min(self.config.token_safety.min_liquidity_shield_usd);
            match fetcher.get_liquidity(token_address).await {
                Ok(liq_usd) if liq_usd < min_liq => {
                    return ValidationResult {
                        valid: false,
                        reason: Some(format!(
                            "Liquidity ${:.0} below minimum ${:.0}",
                            liq_usd.to_f64().unwrap_or(0.0),
                            min_liq.to_f64().unwrap_or(0.0)
                        )),
                        estimated_slippage,
                        price_drift_percent: price_drift,
                    };
                }
                _ => {}
            }
        }

        ValidationResult {
            valid: true,
            reason: None,
            estimated_slippage,
            price_drift_percent: price_drift,
        }
    }

    /// Non-blocking validation using purely local, cached data.
    /// Returns immediately without awaiting external HTTP requests.
    ///
    /// FIX 1: Fast path for trade execution - uses only cached data (O(1) reads)
    /// Returns immediately even if token not in cache (fail-closed).
    ///
    /// # Arguments
    /// * `token_address` - Token to trade
    /// * `amount_sol` - Trade size in SOL
    /// * `tracked_price` - Price when tracked wallet traded
    /// * `price_cache` - Price cache for current price
    ///
    /// # Returns
    /// Result with estimated slippage if valid, error message if invalid
    pub fn validate_local(
        &self,
        token_address: &str,
        amount_sol: Decimal,
        tracked_price: Option<Decimal>,
        price_cache: Arc<PriceCache>,
    ) -> Result<Decimal, String> {
        // 1. Get price from local cache (O(1) read)
        let current_price = match price_cache.get_price_usd(token_address) {
            Some(price) => price,
            None => {
                return Err("Price not found in local cache".to_string());
            }
        };

        // 2. Fast price drift check
        if let Some(tracked) = tracked_price {
            if !tracked.is_zero() {
                let drift = ((current_price - tracked).abs() / tracked) * Decimal::from(100);
                if drift > Decimal::from(5) {
                    return Err(format!("Price drifted {:.2}% (max 5%)", drift));
                }
            }
        }

        // 3. Fast liquidity lookup from local metadata cache
        let liquidity_usd = self
            .token_fetcher
            .as_ref()
            .and_then(|fetcher| fetcher.get_cached_liquidity(token_address))
            .unwrap_or(Decimal::ZERO);

        let min_liquidity = self
            .config
            .token_safety
            .min_liquidity_spear_usd
            .min(self.config.token_safety.min_liquidity_shield_usd);

        if liquidity_usd < min_liquidity {
            return Err(format!(
                "Cached liquidity ${} below minimum ${}",
                liquidity_usd, min_liquidity
            ));
        }

        // 4. Estimate slippage locally using the cached pool liquidity
        let estimated_slippage = self.estimate_slippage_local(amount_sol, liquidity_usd, price_cache);

        Ok(estimated_slippage)
    }

    /// Estimate slippage locally using cached data (no HTTP requests)
    ///
    /// FIX 1: Non-blocking slippage estimation using cached liquidity
    /// Falls back to size-based heuristic if liquidity not cached
    fn estimate_slippage_local(
        &self,
        amount_sol: Decimal,
        liquidity_usd: Decimal,
        price_cache: Arc<PriceCache>,
    ) -> Decimal {
        let max_slippage = Decimal::from(5);

        if liquidity_usd > Decimal::ZERO {
            let sol_price = price_cache
                .get_price_usd(crate::constants::mints::SOL)
                .unwrap_or_else(|| Decimal::from(150));

            let trade_usd = amount_sol * sol_price;

            // Square-root market impact estimation
            let impact = ((trade_usd * Decimal::from(2)) / liquidity_usd);
            return impact.min(max_slippage);
        }

        // Fallback: size-only heuristic (0.5% base + 0.1% per 0.1 SOL)
        let base = Decimal::from_str("0.5").unwrap_or(Decimal::ZERO);
        let size_unit = Decimal::from_str("0.1").unwrap_or(Decimal::ONE);
        let size_part = (amount_sol / size_unit) * Decimal::from_str("0.1").unwrap_or(Decimal::ZERO);
        (base + size_part).min(max_slippage)
    }

    /// Estimate slippage for a trade.
    ///
    /// Uses real pool liquidity when a `token_fetcher` is wired in:
    ///   price_impact ≈ trade_size_usd / (2 * pool_liquidity_usd) * 100
    /// Falls back to a size-based heuristic when liquidity is unavailable.
    async fn estimate_slippage(
        &self,
        token_address: &str,
        amount_sol: Decimal,
        price_cache: Arc<PriceCache>,
    ) -> Decimal {
        let max_slippage = Decimal::from_str("5.0").unwrap_or(Decimal::from(5));

        if let Some(ref fetcher) = self.token_fetcher {
            let sol_price = price_cache
                .get_price_usd(crate::constants::mints::SOL)
                .unwrap_or_else(|| Decimal::from(150));
            let trade_usd = amount_sol * sol_price;

            if let Ok(liq_usd) = fetcher.get_liquidity(token_address).await {
                if liq_usd > Decimal::ZERO {
                    let impact = ((trade_usd * Decimal::from(2)) / liq_usd) * Decimal::from(100);
                    return impact.min(max_slippage);
                }
            }
        }

        // Fallback: size-only heuristic (0.5% base + 0.1% per 0.1 SOL)
        let base = Decimal::from_str("0.5").unwrap_or(Decimal::ZERO);
        let size_unit = Decimal::from_str("0.1").unwrap_or(Decimal::ONE);
        let size_part =
            (amount_sol / size_unit) * Decimal::from_str("0.1").unwrap_or(Decimal::ZERO);
        (base + size_part).min(max_slippage)
    }

    /// Return false if the token is younger than `min_token_age_hours` in config.
    /// Fail-open: unknown age (API failure or no data) → allowed.
    pub async fn check_token_age(&self, token_address: &str) -> Result<bool> {
        let min_age = self.config.token_safety.min_token_age_hours;
        if min_age == 0.0 {
            return Ok(true);
        }

        let client = match &self.helius_client {
            Some(c) => c,
            None => return Ok(true),
        };

        match client.get_token_age_hours(token_address).await {
            Ok(Some(age_hours)) if age_hours < min_age => {
                tracing::warn!(
                    token = token_address,
                    age_hours = age_hours,
                    min_age_hours = min_age,
                    "Token rejected: too new"
                );
                Ok(false)
            }
            Ok(_) => Ok(true),
            Err(e) => {
                tracing::warn!(
                    token = token_address,
                    error = %e,
                    "Token age check failed, allowing trade (fail-open)"
                );
                Ok(true)
            }
        }
    }
}
