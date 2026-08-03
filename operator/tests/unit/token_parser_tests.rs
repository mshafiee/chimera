//! Token Parser Unit Tests
//!
//! Tests token safety validation:
//! - Freeze/mint authority whitelists (production defaults)
//! - Liquidity thresholds per strategy
//! - TokenSafetyResult construction
//!
//! NOTE: the fast/slow validation paths (TokenParser::fast_check /
//! slow_check) require live RPC metadata and are covered by the integration
//! suite and the in-module tests in parser.rs, not here.

use chimera_operator::token::{TokenSafetyConfig, TokenSafetyResult};
use rust_decimal::Decimal;
use std::str::FromStr;

#[test]
fn test_freeze_authority_whitelist() {
    let config = TokenSafetyConfig::default();

    // USDC has freeze authority but is whitelisted
    assert!(
        config
            .freeze_authority_whitelist
            .contains(chimera_operator::token::known_tokens::USDC),
        "USDC should be in freeze authority whitelist"
    );
    // USDT and WSOL must also be whitelisted — a regression that drops them
    // from the default whitelist would reject their trades.
    assert!(
        config
            .freeze_authority_whitelist
            .contains(chimera_operator::token::known_tokens::USDT),
        "USDT should be in freeze authority whitelist"
    );
    assert!(
        config
            .freeze_authority_whitelist
            .contains(chimera_operator::token::known_tokens::WSOL),
        "WSOL should be in freeze authority whitelist"
    );
}

#[test]
fn test_freeze_authority_rejection() {
    // A token with freeze authority that is NOT on the production whitelist
    // must be rejected. (The rejection decision itself lives in
    // TokenParser::slow_check, which needs RPC metadata; here we pin the
    // whitelist-membership precondition.)
    let config = TokenSafetyConfig::default();
    let unknown_token = "UnknownTokenWithFreezeAuthority";

    assert!(
        !config.freeze_authority_whitelist.contains(unknown_token),
        "unknown token must NOT be whitelisted, so slow_check rejects it"
    );
}

#[test]
fn test_mint_authority_whitelist() {
    let config = TokenSafetyConfig::default();

    for token in [
        chimera_operator::token::known_tokens::USDC,
        chimera_operator::token::known_tokens::USDT,
        chimera_operator::token::known_tokens::WSOL,
    ] {
        assert!(
            config.mint_authority_whitelist.contains(token),
            "{token} should be in mint authority whitelist"
        );
    }
}

#[test]
fn test_liquidity_threshold_shield() {
    let config = TokenSafetyConfig::default();

    assert_eq!(
        config.min_liquidity_shield_usd,
        Decimal::from_str("12000.0").unwrap(),
        "Shield strategy should require $12,000 liquidity (20% buffer over $10k)"
    );
}

#[test]
fn test_liquidity_threshold_spear() {
    let config = TokenSafetyConfig::default();

    assert_eq!(
        config.min_liquidity_spear_usd,
        Decimal::from_str("6000.0").unwrap(),
        "Spear strategy should require $6,000 liquidity (20% buffer over $5k)"
    );
}

#[test]
fn test_safety_result_safe() {
    let result = TokenSafetyResult::safe();
    assert!(result.safe, "Safe result should have safe=true");
    assert!(result.rejection_reason.is_none());
}

#[test]
fn test_safety_result_unsafe() {
    let reason = "Freeze authority detected";
    let result = TokenSafetyResult::unsafe_with_reason(reason);
    assert!(!result.safe, "Unsafe result should have safe=false");
    assert_eq!(result.rejection_reason, Some(reason.to_string()));
}

#[test]
fn test_known_tokens_match_production() {
    // The test constants must match the production mint addresses (the
    // production defaults for the whitelists are built from these).
    use chimera_operator::token::known_tokens as production;
    assert_eq!(known_tokens::USDC, production::USDC);
    assert_eq!(known_tokens::USDT, production::USDT);
    assert_eq!(known_tokens::WSOL, production::WSOL);
}

#[test]
fn test_honeypot_detection_enabled() {
    let config = TokenSafetyConfig::default();
    assert!(
        config.honeypot_detection_enabled,
        "Honeypot detection should be enabled by default"
    );
}

#[test]
fn test_liquidity_zero_rejection() {
    // Zero liquidity is below the configured floor for BOTH strategies.
    let config = TokenSafetyConfig::default();
    let zero = Decimal::ZERO;
    assert!(zero < config.min_liquidity_shield_usd);
    assert!(zero < config.min_liquidity_spear_usd);
}

#[test]
fn test_liquidity_insufficient_rejection() {
    // $5k is below the Shield floor (and the Spear floor).
    let config = TokenSafetyConfig::default();
    let insufficient = Decimal::from(5000u32);
    assert!(insufficient < config.min_liquidity_shield_usd);
    assert!(insufficient < config.min_liquidity_spear_usd);
}

// Local copy kept in sync with the production module (asserted above).
mod known_tokens {
    pub const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    pub const USDT: &str = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB";
    pub const WSOL: &str = "So11111111111111111111111111111111111111112";
}
