//! Token safety integration tests
//!
//! These tests validate that the production configuration keeps the safety
//! guards enabled: strict mode for unlisted tokens (`allow_unlisted_heuristic`
//! false), honeypot detection on, and liquidity floors in place.
//!
//! `AppConfig::load_config()` resolves `operator/config.yaml` (the crate's
//! working directory) and silently falls back to built-in defaults if the file
//! is missing — so every test also asserts a value that only the real file
//! sets, proving the file was actually loaded rather than defaults.

use chimera_operator::config::AppConfig;
use rust_decimal_macros::dec;

fn loaded_config() -> AppConfig {
    AppConfig::load_config().expect("config.yaml must load (crate CWD)")
}

#[test]
fn test_production_config_file_is_actually_loaded() {
    let config = loaded_config();
    // Prove the REAL file was loaded, not the silent-default fallback:
    // operator/config.yaml sets min_liquidity_spear_usd: 10000, while the
    // built-in default is 5000. If this fails, load_config resolved defaults
    // and every other assertion in this file is meaningless.
    assert_eq!(
        config.token_safety.min_liquidity_spear_usd,
        dec!(10_000.0),
        "operator/config.yaml sets spear liquidity to $10k — if this fails, \
         load_config fell back to defaults and the file was not resolved"
    );
}

#[test]
fn test_unlisted_heuristic_rejected_in_strict_mode() {
    let config = loaded_config();

    // Ensure strict mode is active (allow_unlisted_heuristic is false)
    assert!(
        !config.token_safety.allow_unlisted_heuristic,
        "allow_unlisted_heuristic must be false in production config for safety"
    );

    println!("✓ Config validation: allow_unlisted_heuristic is correctly set to false (strict mode)");
}

#[test]
fn test_honeypot_detection_enabled() {
    // Flag-level guard: honeypot detection must be enabled. (Behavioral
    // coverage of the detection path itself needs live token metadata and
    // lives in the token-parser unit tests.)
    let config = loaded_config();

    assert!(
        config.token_safety.honeypot_detection_enabled,
        "Honeypot detection must be enabled in production for safety"
    );

    println!("✓ Honeypot detection is enabled (required for production safety)");
}

#[test]
fn test_minimum_liquidity_thresholds() {
    // Policy floors, derived from the single source of truth (operator/config.yaml).
    // Intentional tightening/loosening of the file must be mirrored here.
    let config = loaded_config();

    assert!(
        config.token_safety.min_liquidity_shield_usd >= dec!(10_000.0),
        "Shield minimum liquidity should be at least $10,000 for safety"
    );

    assert!(
        config.token_safety.min_liquidity_spear_usd >= dec!(5_000.0),
        "Spear minimum liquidity should be at least $5,000 for safety"
    );

    println!(
        "✓ Liquidity thresholds: Shield ${:.0}, Spear ${:.0}",
        config.token_safety.min_liquidity_shield_usd,
        config.token_safety.min_liquidity_spear_usd
    );
}

#[cfg(test)]
mod config_validation_tests {
    use super::*;

    #[test]
    fn test_config_file_exists() {
        // Anchor at the crate root: cargo test runs with the package directory
        // as CWD, but this must stay deterministic regardless of invocation.
        let config_path = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/config.yaml"
        ));
        assert!(
            config_path.exists(),
            "operator/config.yaml must exist for token safety tests"
        );
    }

    #[test]
    fn test_config_loads_successfully() {
        let result = AppConfig::load_config();
        assert!(
            result.is_ok(),
            "Config should load successfully: {:?}",
            result.err()
        );
    }
}
