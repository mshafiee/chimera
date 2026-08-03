//! Simple verification test for 24-hour cache TTL defaults

use chimera_operator::config::{default_token_cache_ttl, TokenSafetyConfig};

#[test]
fn test_default_cache_ttl_is_24_hours() {
    let ttl = default_token_cache_ttl();
    assert_eq!(ttl, 86400, "Default cache TTL should be 24 hours (86400 seconds)");
}

#[test]
fn test_default_token_safety_config_has_24_hour_ttl() {
    let config = TokenSafetyConfig::default();
    assert_eq!(
        config.cache_ttl_seconds, 86400,
        "TokenSafetyConfig default should have 24-hour cache TTL"
    );
}
