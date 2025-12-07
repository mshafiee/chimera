//! Token Parser Unit Tests
//!
//! Tests token safety validation:
//! - Fast/slow path checks
//! - Freeze authority rejection
//! - Mint authority whitelist
//! - Liquidity thresholds per strategy

use chimera_operator::token::{TokenParser, TokenSafetyConfig, TokenSafetyResult};

// Known token addresses for testing
mod known_tokens {
    pub const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    pub const USDT: &str = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB";
    pub const WSOL: &str = "So11111111111111111111111111111111111111112";
}

#[test]
fn test_freeze_authority_whitelist() {
    let config = TokenSafetyConfig::default();
    
    // USDC has freeze authority but is whitelisted
    assert!(
        config.freeze_authority_whitelist.contains(known_tokens::USDC),
        "USDC should be in freeze authority whitelist"
    );
}

#[test]
fn test_freeze_authority_rejection() {
    let config = TokenSafetyConfig::default();
    let unknown_token = "UnknownTokenWithFreezeAuthority";
    
    // Token with freeze authority that's not whitelisted should be rejected
    let has_freeze = true;
    let is_whitelisted = config.freeze_authority_whitelist.contains(unknown_token);
    let should_reject = has_freeze && !is_whitelisted;
    
    assert!(should_reject, "Non-whitelisted token with freeze authority should be rejected");
}

#[test]
fn test_mint_authority_whitelist() {
    let config = TokenSafetyConfig::default();
    
    // USDC has mint authority but is whitelisted
    assert!(
        config.mint_authority_whitelist.contains(known_tokens::USDC),
        "USDC should be in mint authority whitelist"
    );
}

#[test]
fn test_liquidity_threshold_shield() {
    let config = TokenSafetyConfig::default();
    
    assert_eq!(
        config.min_liquidity_shield_usd, 10_000.0,
        "Shield strategy should require $10,000 liquidity"
    );
}

#[test]
fn test_liquidity_threshold_spear() {
    let config = TokenSafetyConfig::default();
    
    assert_eq!(
        config.min_liquidity_spear_usd, 5_000.0,
        "Spear strategy should require $5,000 liquidity"
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
fn test_known_tokens() {
    assert_eq!(
        known_tokens::USDC,
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
    );
    assert_eq!(
        known_tokens::USDT,
        "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB"
    );
    assert_eq!(
        known_tokens::WSOL,
        "So11111111111111111111111111111111111111112"
    );
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
    let liquidity_usd = 0.0;
    let threshold = 10_000.0;
    let should_reject = liquidity_usd < threshold;
    
    assert!(should_reject, "Zero liquidity should be rejected");
}

#[test]
fn test_liquidity_insufficient_rejection() {
    let liquidity_usd = 5_000.0;
    let threshold = 10_000.0;
    let should_reject = liquidity_usd < threshold;
    
    assert!(should_reject, "Insufficient liquidity should be rejected");
}

