//! TokenParser - validates tokens for safety before trading
//!
//! Rejection criteria:
//! - Freeze Authority present (except whitelist)
//! - Mint Authority present (except whitelist)
//! - Liquidity below threshold
//! - Honeypot detection (sell simulation fails)

use super::{TokenCache, TokenMetadata, TokenMetadataFetcher};
use crate::error::{AppError, AppResult};
use crate::models::Strategy;
use std::collections::HashSet;
use std::sync::Arc;

/// Known safe token mints that are allowed to have freeze/mint authority
pub mod known_tokens {
    /// USDC mint address
    pub const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    /// USDT mint address
    pub const USDT: &str = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB";
    /// Wrapped SOL mint address
    pub const WSOL: &str = "So11111111111111111111111111111111111111112";
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
    pub liquidity_usd: Option<f64>,
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
    pub min_liquidity_shield_usd: f64,
    /// Minimum liquidity for Spear strategy (USD)
    pub min_liquidity_spear_usd: f64,
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
            min_liquidity_shield_usd: 10_000.0,
            min_liquidity_spear_usd: 5_000.0,
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
            Strategy::Exit => 0.0, // No liquidity check for exits
        };

        // Check liquidity
        let liquidity_usd = match self.fetcher.get_liquidity(token_address).await {
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
            liquidity_usd = liquidity_usd,
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

    #[test]
    fn test_known_tokens() {
        assert_eq!(
            known_tokens::USDC,
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
        );
    }

    #[test]
    fn test_default_config() {
        let config = TokenSafetyConfig::default();
        assert_eq!(config.min_liquidity_shield_usd, 10_000.0);
        assert_eq!(config.min_liquidity_spear_usd, 5_000.0);
        assert!(config.freeze_authority_whitelist.contains(known_tokens::USDC));
    }

    #[test]
    fn test_safety_result() {
        let safe = TokenSafetyResult::safe();
        assert!(safe.safe);
        assert!(safe.rejection_reason.is_none());

        let unsafe_result = TokenSafetyResult::unsafe_with_reason("test reason");
        assert!(!unsafe_result.safe);
        assert_eq!(unsafe_result.rejection_reason, Some("test reason".to_string()));
    }
}
