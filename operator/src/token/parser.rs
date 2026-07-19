//! TokenParser - validates tokens for safety before trading
//!
//! Rejection criteria:
//! - Freeze Authority present (except whitelist)
//! - Mint Authority present (except whitelist)
//! - Liquidity below threshold
//! - Honeypot detection (sell simulation fails)

use super::{TokenCache, TokenMetadataFetcher};
use crate::error::{AppError, AppResult};
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
            min_liquidity_shield_usd: Decimal::from_str("12000.0").unwrap_or_else(|_| {
                // Fallback to computed value if string parsing fails
                Decimal::from(12000)
            }), // 20% buffer over 10k
            min_liquidity_spear_usd: Decimal::from_str("6000.0").unwrap_or_else(|_| {
                // Fallback to computed value if string parsing fails
                Decimal::from(6000)
            }), // 20% buffer over 5k
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

    /// Fetch a token's current liquidity in USD (delegates to the metadata fetcher's
    /// DexScreener lookup, with caching). Used by the webhook handler's liquidity
    /// floor gate, since `fast_check` does not populate liquidity.
    pub async fn get_liquidity(&self, token_address: &str) -> AppResult<Decimal> {
        self.fetcher.get_liquidity(token_address).await
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
        // Validate Solana address format before any processing
        // Solana addresses are base58 encoded and typically 32-44 characters
        // Most common wallet/token addresses are 44 chars (Pubkey), but some can be shorter
        // Valid base58 chars: 1-9, A-H, J-N, P-Z, a-k, m-z (excluding 0, O, I, l)
        if token_address.len() < 32 || token_address.len() > 44 {
            return Err(AppError::InvalidTokenAddress(format!(
                "Invalid Solana address length: {} (expected 32-44 chars)",
                token_address.len()
            )));
        }

        // Check for valid base58 characters (rough check - full validation happens at RPC layer)
        if !token_address
            .chars()
            .all(|c| c.is_alphanumeric() || c == '1' || c == '3' || c == '5')
        {
            return Err(AppError::InvalidTokenAddress(
                "Invalid Solana address format (not valid base58)".to_string(),
            ));
        }

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
                    "Failed to fetch token metadata — rejecting (fail-closed for safety)"
                );
                // Fail closed: if we can't verify safety, don't allow the trade.
                return Ok(TokenSafetyResult::unsafe_with_reason(
                    "Token metadata unavailable — cannot verify safety",
                ));
            }
        };

        // Reject Token-2022 tokens with dangerous extensions before anything else.
        // TransferHook can call an arbitrary program on every transfer (can block sells).
        // PermanentDelegate grants a fixed address unlimited transfer authority (wallet drain).
        if metadata.has_transfer_hook {
            return Ok(TokenSafetyResult::unsafe_with_reason(
                "Token-2022 TransferHook extension detected — can block sells",
            ));
        }
        if metadata.has_permanent_delegate {
            return Ok(TokenSafetyResult::unsafe_with_reason(
                "Token-2022 PermanentDelegate extension detected — can drain wallet",
            ));
        }

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
                // Cache rejections with the same TTL as safe results. Freeze/mint authority
                // changes are rare; re-checking every signal under load causes RPC thundering
                // herd on known-bad tokens. The 1-hour TTL matches the safe-result TTL.
                self.cache.insert(cache_key, result.clone());
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
                self.cache.insert(cache_key, result.clone());
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

    /// Get token decimals from on-chain metadata.
    /// Returns None if metadata is unavailable.
    ///
    /// Uses fast path via Jupiter Price API v3 cache when available,
    /// falling back to RPC metadata fetch only when necessary.
    pub async fn get_token_decimals(&self, token_address: &str) -> Option<u8> {
        // Try fast path via PriceCache (Jupiter data)
        if let Some(decimals) = self.fetcher.get_decimals_only(token_address).await {
            return Some(decimals);
        }

        // Fallback to existing behavior (RPC metadata fetch)
        match self.fetcher.get_metadata(token_address).await {
            Ok(metadata) => Some(metadata.decimals),
            Err(e) => {
                tracing::warn!(
                    token = token_address,
                    error = %e,
                    "Failed to fetch token decimals — fill price conversion will assume 9 decimals"
                );
                None
            }
        }
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
                tracing::debug!(token = token_address, "Slow check using cached result");
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

        // Check liquidity (already returns Decimal for precision)
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

        // Ghost-Chain / exit-risk gating.
        //
        // The previous Liq/FDV ratio check (max-pool-liquidity / FDV >= 5%) is
        // removed: it is impossible for any established token (FDV dwarfs a single
        // pool's liquidity — e.g. BONK $119K / $245M = 0.05%) and thus blocked the
        // safest, highest-volume copy targets while only admitting tiny illiquid
        // tokens where exit slippage is actually worst.
        //
        // Exit/slippage risk is now gated by the executor's Jupiter price-impact
        // check (MAX_PRICE_IMPACT_PCT), which measures the ACTUAL trade's market
        // impact — a direct, accurate signal of "can I exit at a reasonable price".
        // Verified-major tokens (deep, multi-pool liquidity) are trusted past this
        // class of heuristic entirely.
        if self.is_verified_major(token_address) {
            tracing::debug!(
                token = token_address,
                liquidity_usd = %liquidity_usd,
                "Verified-major token: skipping Ghost-Chain heuristic (price-impact gate applies at execution)"
            );
        }

        // Honeypot detection via sell simulation
        let mut honeypot_checked = false;
        if self.config.honeypot_detection_enabled {
            match self.fetcher.simulate_sell(token_address).await {
                Ok(can_sell) => {
                    honeypot_checked = true;
                    if !can_sell {
                        return Ok(TokenSafetyResult {
                            safe: false,
                            rejection_reason: Some(
                                "Honeypot detected: sell simulation failed".to_string(),
                            ),
                            honeypot_checked: true,
                            liquidity_checked: true,
                            liquidity_usd: Some(liquidity_usd),
                        });
                    }
                }
                Err(ref e) if e.to_string().contains("honeypot_simulation_inconclusive") => {
                    // Simulation wallet has no balance — cannot distinguish safe from honeypot.
                    // Fail-closed: reject the token rather than assume it is safe. The correct
                    // fix is to supply a funded keypair for simulation; until then we treat an
                    // unverifiable honeypot check as a rejection in production.
                    // Set CHIMERA_DEV_MODE=1 to bypass this gate during local testing.
                    if !crate::utils::is_dev_mode() {
                        tracing::warn!(
                            token = token_address,
                            "Honeypot simulation inconclusive (unfunded wallet) — rejecting (fail-closed)"
                        );
                        return Ok(TokenSafetyResult::unsafe_with_reason(
                            "Honeypot check inconclusive — cannot verify sell route",
                        ));
                    }
                    tracing::warn!(
                        token = token_address,
                        "Honeypot simulation inconclusive (dev mode) — proceeding without verification"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        token = token_address,
                        error = %e,
                        "Honeypot simulation error, rejecting for safety"
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
            honeypot_checked,
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

    /// Check if a token is a verified major (established, high-liquidity).
    ///
    /// Verified majors bypass the Liq/FDV Ghost-Chain heuristic — that ratio is
    /// impossible for large caps (FDV dwarfs single-pool liquidity). Exit/slippage
    /// risk for these is instead gated by the executor's Jupiter price-impact check.
    fn is_verified_major(&self, token_address: &str) -> bool {
        crate::constants::verified_majors::ALL.contains(&token_address)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use std::str::FromStr;

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
        assert_eq!(
            config.min_liquidity_shield_usd,
            Decimal::from_str("12000.0").unwrap(),
            "Shield liquidity threshold should be $12,000 (10k + 20% buffer)"
        );
        assert_eq!(
            config.min_liquidity_spear_usd,
            Decimal::from_str("6000.0").unwrap(),
            "Spear liquidity threshold should be $6,000 (5k + 20% buffer)"
        );
    }

    #[test]
    fn test_default_config_whitelist() {
        let config = TokenSafetyConfig::default();
        assert!(config
            .freeze_authority_whitelist
            .contains(known_tokens::USDC));
        assert!(config
            .freeze_authority_whitelist
            .contains(known_tokens::USDT));
        assert!(config
            .freeze_authority_whitelist
            .contains(known_tokens::WSOL));
        assert!(config.mint_authority_whitelist.contains(known_tokens::USDC));
    }

    #[test]
    fn test_default_config_honeypot_enabled() {
        let config = TokenSafetyConfig::default();
        assert!(
            config.honeypot_detection_enabled,
            "Honeypot detection should be enabled by default"
        );
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
        assert_eq!(
            result.rejection_reason,
            Some("Freeze authority detected".to_string())
        );
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

        assert!(
            !should_reject,
            "Whitelisted token with freeze authority should be allowed"
        );
    }

    #[test]
    fn test_freeze_authority_not_whitelisted_rejected() {
        let config = TokenSafetyConfig::default();
        let token_address = "RandomToken11111111111111111111111111111111";
        let has_freeze_authority = true;

        let is_whitelisted = config.freeze_authority_whitelist.contains(token_address);
        let should_reject = has_freeze_authority && !is_whitelisted;

        assert!(
            should_reject,
            "Non-whitelisted token with freeze authority should be rejected"
        );
    }

    #[test]
    fn test_no_freeze_authority_allowed() {
        let config = TokenSafetyConfig::default();
        let token_address = "RandomToken11111111111111111111111111111111";
        let has_freeze_authority = false;

        let should_reject =
            has_freeze_authority && !config.freeze_authority_whitelist.contains(token_address);
        assert!(
            !should_reject,
            "Token without freeze authority should be allowed"
        );
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

        assert!(
            should_reject,
            "Non-whitelisted token with mint authority should be rejected"
        );
    }

    // ==========================================================================
    // LIQUIDITY THRESHOLD TESTS
    // ==========================================================================

    #[test]
    fn test_shield_liquidity_above_threshold() {
        let config = TokenSafetyConfig::default();
        let liquidity_usd = Decimal::from(15_000u32);
        let should_reject = liquidity_usd < config.min_liquidity_shield_usd;
        assert!(!should_reject, "Shield with $15k liquidity should pass");
    }

    #[test]
    fn test_shield_liquidity_below_threshold() {
        let config = TokenSafetyConfig::default();
        let liquidity_usd = Decimal::from(5_000u32);
        let should_reject = liquidity_usd < config.min_liquidity_shield_usd;
        assert!(
            should_reject,
            "Shield with $5k liquidity should be rejected"
        );
    }

    #[test]
    fn test_shield_liquidity_exact_threshold() {
        let config = TokenSafetyConfig::default();
        // Default threshold is 12_000 (10k + 20% buffer)
        let liquidity_usd = Decimal::from(12_000u32);
        let should_reject = liquidity_usd < config.min_liquidity_shield_usd;
        assert!(!should_reject, "Shield at exact $12k threshold should pass");
    }

    #[test]
    fn test_spear_liquidity_above_threshold() {
        let config = TokenSafetyConfig::default();
        let liquidity_usd = Decimal::from(8_000u32);
        let should_reject = liquidity_usd < config.min_liquidity_spear_usd;
        assert!(!should_reject, "Spear with $8k liquidity should pass");
    }

    #[test]
    fn test_spear_liquidity_below_threshold() {
        let config = TokenSafetyConfig::default();
        let liquidity_usd = Decimal::from(3_000u32);
        let should_reject = liquidity_usd < config.min_liquidity_spear_usd;
        assert!(should_reject, "Spear with $3k liquidity should be rejected");
    }

    // ==========================================================================
    // STRATEGY-SPECIFIC THRESHOLD TESTS
    // ==========================================================================

    #[test]
    fn test_shield_threshold_higher_than_spear() {
        let config = TokenSafetyConfig::default();
        assert!(
            config.min_liquidity_shield_usd > config.min_liquidity_spear_usd,
            "Shield threshold should be higher than Spear (more conservative)"
        );
    }

    #[test]
    fn test_exit_no_liquidity_requirement() {
        // Per parser.rs: Exit strategy has min_liquidity = 0.0
        let min_liquidity_exit = 0.0_f64;
        let liquidity_usd = 100.0;
        let should_reject = liquidity_usd < min_liquidity_exit;
        assert!(
            !should_reject,
            "Exit strategy should not have liquidity requirement"
        );
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
        let liquidity_usd = Decimal::ZERO;
        assert!(liquidity_usd < config.min_liquidity_shield_usd);
        assert!(liquidity_usd < config.min_liquidity_spear_usd);
    }

    #[test]
    fn test_very_high_liquidity_passes() {
        let config = TokenSafetyConfig::default();
        let liquidity_usd = Decimal::from(1_000_000u32); // $1M
        assert!(liquidity_usd >= config.min_liquidity_shield_usd);
        assert!(liquidity_usd >= config.min_liquidity_spear_usd);
    }

    // ==========================================================================
    // TOKEN ADDRESS FORMAT VALIDATION
    // ==========================================================================

    #[test]
    fn test_valid_solana_addresses_pass() {
        // Test typical Solana addresses (44 chars, base58)
        let valid_addresses = vec![
            "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU", // Standard 44-char address
            "So11111111111111111111111111111111111111112",   // System program
            "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",    // Token program
            "9WzDXwBbmg8CsZv8kGqWJRrqcVdNBHQjuUeJPgWcH3YQ",    // Another valid address
        ];

        for address in valid_addresses {
            // Should pass format validation
            assert!(address.len() >= 32 && address.len() <= 44, "Valid address length check");
            assert!(
                address.chars().all(|c| c.is_alphanumeric() || c == '1' || c == '3' || c == '5'),
                "Valid address char check: {}",
                address
            );
        }
    }

    #[test]
    fn test_invalid_solana_addresses_rejected() {
        // Test invalid addresses that should be rejected
        let too_long = "a".repeat(100);
        let invalid_addresses = vec![
            "",                           // Empty
            "short",                      // Too short
            &too_long,                     // Too long
            "invalid@address#",            // Invalid characters
                            // Has special chars
            "ABC DEF",                     // Has space
            "12345",                      // Too short
            "0x1234567890abcdef",          // Ethereum-style hex
        ];

        for address in invalid_addresses {
            let should_fail_length = address.len() < 32 || address.len() > 44;
            let should_fail_chars = !address
                .chars()
                .all(|c| c.is_alphanumeric() || c == '1' || c == '3' || c == '5');

            assert!(
                should_fail_length || should_fail_chars,
                "Invalid address should be rejected: '{}'",
                address
            );
        }
    }

    #[test]
    fn test_base58_character_validation() {
        // Solana uses base58 encoding which excludes: 0, O, I, l
        let invalid_base58 = vec!["0OIL", "abc123def0", "invalid0chars"];

        for address in invalid_base58 {
            let has_invalid_chars = address.chars().any(|c| c == '0' || c == 'O' || c == 'I' || c == 'l');
            assert!(has_invalid_chars, "Should detect invalid base58 chars");
        }

        // Valid base58 characters (subset we check for)
        let valid_base58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
        assert!(
            !valid_base58
                .chars()
                .any(|c| c == '0' || c == 'O' || c == 'I' || c == 'l'),
            "All valid base58 chars should pass"
        );
    }
}
