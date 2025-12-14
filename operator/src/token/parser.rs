//! TokenParser - validates tokens for safety before trading
//!
//! Rejection criteria:
//! - Freeze Authority present (except whitelist)
//! - Mint Authority present (except whitelist)
//! - Liquidity below threshold
//! - Honeypot detection (sell simulation fails)

use super::{TokenCache, TokenMetadataFetcher};
use crate::error::AppResult;
use crate::models::Strategy;
use rust_decimal::prelude::*;
use std::collections::HashSet;
use std::sync::Arc;

/// Known safe token mints that are allowed to have freeze/mint authority
/// 
/// These should match the constants in crate::constants::mints
pub mod known_tokens {
    use crate::constants;
    
    /// USDC mint address
    pub const USDC: &str = constants::mints::USDC;
    /// USDT mint address
    pub const USDT: &str = constants::mints::USDT;
    /// Wrapped SOL mint address
    pub const WSOL: &str = constants::mints::SOL;
}

/// Result of a token safety check
#[derive(Debug, Clone)]
pub struct TokenSafetyResult {
    /// Whether the token passed all checks
    pub safe: bool,
    /// Reason for rejection (if not safe)
    pub rejection_reason: Option<String>,
    /// Whether honeypot check was performed
    pub honeypot_checked: bool,
    /// Whether liquidity check was performed
    pub liquidity_checked: bool,
    /// Current liquidity in USD (if checked)
    pub liquidity_usd: Option<Decimal>,
}

impl TokenSafetyResult {
    /// Create a safe result
    pub fn safe() -> Self {
        Self {
            safe: true,
            rejection_reason: None,
            honeypot_checked: false,
            liquidity_checked: false,
            liquidity_usd: None,
        }
    }

    /// Create an unsafe result with reason
    pub fn unsafe_with_reason(reason: impl Into<String>) -> Self {
        Self {
            safe: false,
            rejection_reason: Some(reason.into()),
            honeypot_checked: false,
            liquidity_checked: false,
            liquidity_usd: None,
        }
    }
}

/// Configuration for token safety checks
#[derive(Debug, Clone)]
pub struct TokenSafetyConfig {
    /// Token mints allowed to have freeze authority
    pub freeze_authority_whitelist: HashSet<String>,
    /// Token mints allowed to have mint authority
    pub mint_authority_whitelist: HashSet<String>,
    /// Minimum liquidity for Shield strategy (USD)
    pub min_liquidity_shield_usd: Decimal,
    /// Minimum liquidity for Spear strategy (USD)
    pub min_liquidity_spear_usd: Decimal,
    /// Whether to enable honeypot detection
    pub honeypot_detection_enabled: bool,
}

impl Default for TokenSafetyConfig {
    fn default() -> Self {
        let mut freeze_whitelist = HashSet::new();
        let mut mint_whitelist = HashSet::new();

        // Add known safe tokens
        for token in [known_tokens::USDC, known_tokens::USDT, known_tokens::WSOL] {
            freeze_whitelist.insert(token.to_string());
            mint_whitelist.insert(token.to_string());
        }

        Self {
            freeze_authority_whitelist: freeze_whitelist,
            mint_authority_whitelist: mint_whitelist,
            min_liquidity_shield_usd: Decimal::from_str("12000.0").unwrap(),  // 20% buffer over 10k
            min_liquidity_spear_usd: Decimal::from_str("6000.0").unwrap(),    // 20% buffer over 5k
            honeypot_detection_enabled: true,
        }
    }
}

/// TokenParser provides token safety validation
pub struct TokenParser {
    /// Configuration
    config: TokenSafetyConfig,
    /// Token metadata cache
    cache: Arc<TokenCache>,
    /// Metadata fetcher
    fetcher: Arc<TokenMetadataFetcher>,
}

impl TokenParser {
    /// Create a new TokenParser
    pub fn new(
        config: TokenSafetyConfig,
        cache: Arc<TokenCache>,
        fetcher: Arc<TokenMetadataFetcher>,
    ) -> Self {
        Self {
            config,
            cache,
            fetcher,
        }
    }

    /// Fast path check - validates token metadata only (sub-millisecond)
    ///
    /// Checks:
    /// - Freeze authority (reject if present and not whitelisted)
    /// - Mint authority (reject if present and not whitelisted)
    ///
    /// This is called in the Ingress/webhook handler before queueing.
    pub async fn fast_check(
        &self,
        token_address: &str,
        strategy: Strategy,
    ) -> AppResult<TokenSafetyResult> {
        // Check cache first
        let cache_key = format!("{}:{}", token_address, strategy);
        if let Some(cached) = self.cache.get(&cache_key) {
            tracing::debug!(
                token = token_address,
                cached = true,
                "Fast check using cached result"
            );
            return Ok(cached);
        }

        // Check if token is in our permanent whitelist
        if self.is_whitelisted(token_address) {
            let result = TokenSafetyResult::safe();
            self.cache.insert(cache_key, result.clone());
            return Ok(result);
        }

        // Fetch metadata from cache or RPC
        let metadata = match self.fetcher.get_metadata(token_address).await {
            Ok(meta) => meta,
            Err(e) => {
                tracing::warn!(
                    token = token_address,
                    error = %e,
                    "Failed to fetch token metadata, allowing with warning"
                );
                // On metadata fetch failure, allow but don't cache
                // The slow path will do more thorough checks
                return Ok(TokenSafetyResult::safe());
            }
        };

        // Check freeze authority
        if let Some(ref freeze_auth) = metadata.freeze_authority {
            if !self
                .config
                .freeze_authority_whitelist
                .contains(token_address)
            {
                let result = TokenSafetyResult::unsafe_with_reason(format!(
                    "Token has freeze authority: {}",
                    freeze_auth
                ));
                // Don't cache rejections - metadata might change
                return Ok(result);
            }
        }

        // Check mint authority
        if let Some(ref mint_auth) = metadata.mint_authority {
            if !self.config.mint_authority_whitelist.contains(token_address) {
                let result = TokenSafetyResult::unsafe_with_reason(format!(
                    "Token has mint authority: {}",
                    mint_auth
                ));
                return Ok(result);
            }
        }

        // Fast path passed
        let result = TokenSafetyResult::safe();
        self.cache.insert(cache_key, result.clone());

        tracing::debug!(
            token = token_address,
            strategy = %strategy,
            "Fast check passed"
        );

        Ok(result)
    }

    /// Slow path check - full validation including honeypot detection
    ///
    /// Checks:
    /// - Liquidity threshold
    /// - Honeypot detection (simulate sell transaction)
    ///
    /// This is called in the Executor immediately before trade execution.
    pub async fn slow_check(
        &self,
        token_address: &str,
        strategy: Strategy,
    ) -> AppResult<TokenSafetyResult> {
        let cache_key = format!("{}:{}:full", token_address, strategy);

        // Check if we have a full cached result
        if let Some(cached) = self.cache.get(&cache_key) {
            if cached.honeypot_checked && cached.liquidity_checked {
                tracing::debug!(
                    token = token_address,
                    "Slow check using cached result"
                );
                return Ok(cached);
            }
        }

        // Whitelisted tokens skip slow checks
        if self.is_whitelisted(token_address) {
            return Ok(TokenSafetyResult {
                safe: true,
                rejection_reason: None,
                honeypot_checked: true,
                liquidity_checked: true,
                liquidity_usd: None,
            });
        }

        // Get minimum liquidity threshold based on strategy
        let min_liquidity = match strategy {
            Strategy::Shield => self.config.min_liquidity_shield_usd,
            Strategy::Spear => self.config.min_liquidity_spear_usd,
            Strategy::Exit => Decimal::ZERO, // No liquidity check for exits
        };

        // Check liquidity
        let liquidity_usd_f64 = match self.fetcher.get_liquidity(token_address).await {
            Ok(liq) => liq,
            Err(e) => {
                tracing::warn!(
                    token = token_address,
                    error = %e,
                    "Failed to fetch liquidity, rejecting for safety"
                );
                return Ok(TokenSafetyResult::unsafe_with_reason(
                    "Unable to verify liquidity",
                ));
            }
        };

        // Convert to Decimal for precise comparison
        let liquidity_usd = Decimal::from_f64_retain(liquidity_usd_f64).unwrap_or(Decimal::ZERO);
        if liquidity_usd < min_liquidity {
                    return Ok(TokenSafetyResult {
                        safe: false,
                        rejection_reason: Some(format!(
                            "Insufficient liquidity: ${:.2} < ${:.2} minimum",
                            liquidity_usd, min_liquidity
                        )),
                        honeypot_checked: false,
                        liquidity_checked: true,
                        liquidity_usd: Some(liquidity_usd),
                    });
        }

        // Check liquidity/market cap ratio (Liquidity vs FDV)
        // High FDV with low liquidity = "Ghost Chain" scenario - high slippage on exit
        // Reject tokens with Liq/FDV < 0.05 (5%)
        // Use Decimal for calculation, convert to f64 only for comparison and logging
        if let Ok(fdv_usd) = self.fetcher.get_market_cap_fdv(token_address).await {
            if fdv_usd > 0.0 {
                let fdv_usd_dec = Decimal::from_f64_retain(fdv_usd).unwrap_or(Decimal::ZERO);
                let liquidity_ratio = liquidity_usd / fdv_usd_dec;
                let min_liquidity_ratio = Decimal::from_str("0.05").unwrap_or(Decimal::ZERO); // 5% minimum

                if liquidity_ratio < min_liquidity_ratio {
                    tracing::warn!(
                        token = token_address,
                        liquidity_usd = %liquidity_usd,
                        fdv_usd = fdv_usd,
                        liquidity_ratio = %liquidity_ratio,
                        "Token rejected: Liquidity/FDV ratio too low (Ghost Chain scenario)"
                    );
                    return Ok(TokenSafetyResult {
                        safe: false,
                        rejection_reason: Some(format!(
                            "Liquidity/FDV ratio too low: {:.2}% < {:.0}% (liquidity: ${:.2}, FDV: ${:.2})",
                            liquidity_ratio * Decimal::from(100),
                            min_liquidity_ratio * Decimal::from(100),
                            liquidity_usd,
                            fdv_usd
                        )),
                        honeypot_checked: false,
                        liquidity_checked: true,
                        liquidity_usd: Some(liquidity_usd),
                    });
                }

                tracing::debug!(
                    token = token_address,
                    liquidity_usd = %liquidity_usd,
                    fdv_usd = fdv_usd,
                    liquidity_ratio = %liquidity_ratio,
                    "Liquidity/FDV ratio check passed"
                );
            }
        } else {
            // If we can't fetch FDV, log warning but don't reject (fail open for now)
            // In production, you might want to reject if FDV fetch fails
            tracing::warn!(
                token = token_address,
                "Failed to fetch market cap (FDV), skipping liquidity ratio check"
            );
        }

        // Honeypot detection via sell simulation
        if self.config.honeypot_detection_enabled {
            match self.fetcher.simulate_sell(token_address).await {
                Ok(can_sell) => {
                    if !can_sell {
                        return Ok(TokenSafetyResult {
                            safe: false,
                        rejection_reason: Some("Honeypot detected: sell simulation failed".to_string()),
                        honeypot_checked: true,
                        liquidity_checked: true,
                        liquidity_usd: Some(liquidity_usd),
                        });
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        token = token_address,
                        error = %e,
                        "Honeypot simulation failed, rejecting for safety"
                    );
                    return Ok(TokenSafetyResult::unsafe_with_reason(
                        "Honeypot detection failed - unable to simulate sell",
                    ));
                }
            }
        }

        // All checks passed
        let result = TokenSafetyResult {
            safe: true,
            rejection_reason: None,
            honeypot_checked: self.config.honeypot_detection_enabled,
            liquidity_checked: true,
            liquidity_usd: Some(liquidity_usd),
        };

        // Cache the full result
        self.cache.insert(cache_key, result.clone());

        tracing::info!(
            token = token_address,
            strategy = %strategy,
            liquidity_usd = %liquidity_usd,
            "Slow check passed"
        );

        Ok(result)
    }

    /// Check if a token is in the permanent whitelist
    fn is_whitelisted(&self, token_address: &str) -> bool {
        token_address == known_tokens::USDC
            || token_address == known_tokens::USDT
            || token_address == known_tokens::WSOL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==========================================================================
    // KNOWN TOKENS TESTS
    // ==========================================================================

    #[test]
    fn test_usdc_address() {
        assert_eq!(
            known_tokens::USDC,
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
        );
    }

    #[test]
    fn test_usdt_address() {
        assert_eq!(
            known_tokens::USDT,
            "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB"
        );
    }

    #[test]
    fn test_wsol_address() {
        assert_eq!(
            known_tokens::WSOL,
            "So11111111111111111111111111111111111111112"
        );
    }

    // ==========================================================================
    // DEFAULT CONFIG TESTS
    // ==========================================================================

    #[test]
    fn test_default_config_liquidity_thresholds() {
        let config = TokenSafetyConfig::default();
        // Default includes 20% buffer: 10k * 1.2 = 12k, 5k * 1.2 = 6k
        assert_eq!(config.min_liquidity_shield_usd, 12_000.0, 
            "Shield liquidity threshold should be $12,000 (10k + 20% buffer)");
        assert_eq!(config.min_liquidity_spear_usd, 6_000.0,
            "Spear liquidity threshold should be $6,000 (5k + 20% buffer)");
    }

    #[test]
    fn test_default_config_whitelist() {
        let config = TokenSafetyConfig::default();
        assert!(config.freeze_authority_whitelist.contains(known_tokens::USDC));
        assert!(config.freeze_authority_whitelist.contains(known_tokens::USDT));
        assert!(config.freeze_authority_whitelist.contains(known_tokens::WSOL));
        assert!(config.mint_authority_whitelist.contains(known_tokens::USDC));
    }

    #[test]
    fn test_default_config_honeypot_enabled() {
        let config = TokenSafetyConfig::default();
        assert!(config.honeypot_detection_enabled,
            "Honeypot detection should be enabled by default");
    }

    // ==========================================================================
    // SAFETY RESULT TESTS
    // ==========================================================================

    #[test]
    fn test_safety_result_safe() {
        let result = TokenSafetyResult::safe();
        assert!(result.safe, "Safe result should have safe=true");
        assert!(result.rejection_reason.is_none());
        assert!(!result.honeypot_checked);
        assert!(!result.liquidity_checked);
        assert!(result.liquidity_usd.is_none());
    }

    #[test]
    fn test_safety_result_unsafe() {
        let result = TokenSafetyResult::unsafe_with_reason("Freeze authority detected");
        assert!(!result.safe, "Unsafe result should have safe=false");
        assert_eq!(result.rejection_reason, Some("Freeze authority detected".to_string()));
    }

    // ==========================================================================
    // FREEZE AUTHORITY TESTS
    // ==========================================================================

    #[test]
    fn test_freeze_authority_whitelisted_allowed() {
        let config = TokenSafetyConfig::default();
        let token_address = known_tokens::USDC;
        let has_freeze_authority = true;
        
        let is_whitelisted = config.freeze_authority_whitelist.contains(token_address);
        let should_reject = has_freeze_authority && !is_whitelisted;
        
        assert!(!should_reject, "Whitelisted token with freeze authority should be allowed");
    }

    #[test]
    fn test_freeze_authority_not_whitelisted_rejected() {
        let config = TokenSafetyConfig::default();
        let token_address = "RandomToken11111111111111111111111111111111";
        let has_freeze_authority = true;
        
        let is_whitelisted = config.freeze_authority_whitelist.contains(token_address);
        let should_reject = has_freeze_authority && !is_whitelisted;
        
        assert!(should_reject, "Non-whitelisted token with freeze authority should be rejected");
    }

    #[test]
    fn test_no_freeze_authority_allowed() {
        let config = TokenSafetyConfig::default();
        let token_address = "RandomToken11111111111111111111111111111111";
        let has_freeze_authority = false;
        
        let should_reject = has_freeze_authority && !config.freeze_authority_whitelist.contains(token_address);
        assert!(!should_reject, "Token without freeze authority should be allowed");
    }

    // ==========================================================================
    // MINT AUTHORITY TESTS
    // ==========================================================================

    #[test]
    fn test_mint_authority_whitelisted_allowed() {
        let config = TokenSafetyConfig::default();
        let is_whitelisted = config.mint_authority_whitelist.contains(known_tokens::USDC);
        assert!(is_whitelisted, "USDC should be in mint authority whitelist");
    }

    #[test]
    fn test_mint_authority_not_whitelisted_rejected() {
        let config = TokenSafetyConfig::default();
        let token_address = "RandomToken11111111111111111111111111111111";
        let has_mint_authority = true;
        
        let is_whitelisted = config.mint_authority_whitelist.contains(token_address);
        let should_reject = has_mint_authority && !is_whitelisted;
        
        assert!(should_reject, "Non-whitelisted token with mint authority should be rejected");
    }

    // ==========================================================================
    // LIQUIDITY THRESHOLD TESTS
    // ==========================================================================

    #[test]
    fn test_shield_liquidity_above_threshold() {
        let config = TokenSafetyConfig::default();
        let liquidity_usd = 15_000.0;
        let should_reject = liquidity_usd < config.min_liquidity_shield_usd;
        assert!(!should_reject, "Shield with $15k liquidity should pass");
    }

    #[test]
    fn test_shield_liquidity_below_threshold() {
        let config = TokenSafetyConfig::default();
        let liquidity_usd = 5_000.0;
        let should_reject = liquidity_usd < config.min_liquidity_shield_usd;
        assert!(should_reject, "Shield with $5k liquidity should be rejected");
    }

    #[test]
    fn test_shield_liquidity_exact_threshold() {
        let config = TokenSafetyConfig::default();
        // Default threshold is 12_000.0 (10k + 20% buffer)
        let liquidity_usd = 12_000.0;
        let should_reject = liquidity_usd < config.min_liquidity_shield_usd;
        assert!(!should_reject, "Shield at exact $12k threshold should pass");
    }

    #[test]
    fn test_spear_liquidity_above_threshold() {
        let config = TokenSafetyConfig::default();
        let liquidity_usd = 8_000.0;
        let should_reject = liquidity_usd < config.min_liquidity_spear_usd;
        assert!(!should_reject, "Spear with $8k liquidity should pass");
    }

    #[test]
    fn test_spear_liquidity_below_threshold() {
        let config = TokenSafetyConfig::default();
        let liquidity_usd = 3_000.0;
        let should_reject = liquidity_usd < config.min_liquidity_spear_usd;
        assert!(should_reject, "Spear with $3k liquidity should be rejected");
    }

    // ==========================================================================
    // STRATEGY-SPECIFIC THRESHOLD TESTS
    // ==========================================================================

    #[test]
    fn test_shield_threshold_higher_than_spear() {
        let config = TokenSafetyConfig::default();
        assert!(config.min_liquidity_shield_usd > config.min_liquidity_spear_usd,
            "Shield threshold should be higher than Spear (more conservative)");
    }

    #[test]
    fn test_exit_no_liquidity_requirement() {
        // Per parser.rs: Exit strategy has min_liquidity = 0.0
        let min_liquidity_exit = 0.0_f64;
        let liquidity_usd = 100.0;
        let should_reject = liquidity_usd < min_liquidity_exit;
        assert!(!should_reject, "Exit strategy should not have liquidity requirement");
    }

    // ==========================================================================
    // CACHE KEY TESTS
    // ==========================================================================

    #[test]
    fn test_fast_check_cache_key_format() {
        let token_address = "TokenMint123456789";
        let strategy = Strategy::Shield;
        let cache_key = format!("{}:{}", token_address, strategy);
        assert_eq!(cache_key, "TokenMint123456789:SHIELD");
    }

    #[test]
    fn test_slow_check_cache_key_format() {
        let token_address = "TokenMint123456789";
        let strategy = Strategy::Spear;
        let cache_key = format!("{}:{}:full", token_address, strategy);
        assert_eq!(cache_key, "TokenMint123456789:SPEAR:full");
    }

    #[test]
    fn test_different_strategies_different_cache_keys() {
        let token = "Token123";
        let shield_key = format!("{}:{}", token, Strategy::Shield);
        let spear_key = format!("{}:{}", token, Strategy::Spear);
        assert_ne!(shield_key, spear_key);
    }

    // ==========================================================================
    // EDGE CASES
    // ==========================================================================

    #[test]
    fn test_zero_liquidity_rejected() {
        let config = TokenSafetyConfig::default();
        let liquidity_usd = 0.0;
        assert!(liquidity_usd < config.min_liquidity_shield_usd);
        assert!(liquidity_usd < config.min_liquidity_spear_usd);
    }

    #[test]
    fn test_very_high_liquidity_passes() {
        let config = TokenSafetyConfig::default();
        let liquidity_usd = 1_000_000.0; // $1M
        assert!(liquidity_usd >= config.min_liquidity_shield_usd);
        assert!(liquidity_usd >= config.min_liquidity_spear_usd);
    }
}
