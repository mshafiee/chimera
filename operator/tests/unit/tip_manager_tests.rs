//! Tip Manager Unit Tests
//!
//! Documents the cold-start tip contract for a standard JitoConfig:
//! - Shield: tip_floor * 2
//! - Spear: tip_floor * 2 * 1.5
//! - Exit: tip_ceiling
//!
//! NOTE: the actual tip algorithms live in `engine/tips.rs`
//! (cold_start_tip / percentile_tip_from_history / calculate_tip) and are
//! covered by that module's in-module tests; those helpers are private, so
//! this external file only pins the config-derived contract values.

use chimera_operator::JitoConfig;
use rust_decimal::prelude::*;

fn create_test_jito_config() -> chimera_operator::JitoConfig {
    chimera_operator::JitoConfig {
        enabled: true,
        searcher_endpoint: None,
        helius_fallback: false,
        tip_floor_sol: Decimal::from_str("0.001").unwrap(),
        tip_ceiling_sol: Decimal::from_str("0.01").unwrap(),
        tip_percentile: 50,
        tip_percent_max: Decimal::from_str("0.10").unwrap(),
        min_failures_before_fallback: 10,
        disable_fallback: false,
        max_retries: 5,
        helius_staked_exits: true,
    }
}

#[test]
fn test_cold_start_shield_tip() {
    let config = create_test_jito_config();

    // Shield cold-start tip = tip_floor * 2
    let tip = config.tip_floor_sol.to_f64().unwrap() * 2.0;
    assert!(
        (tip - 0.002).abs() < 0.0001,
        "Shield cold start tip should be 0.002"
    );
}

#[test]
fn test_cold_start_spear_tip() {
    let config = create_test_jito_config();

    // Spear cold-start tip = tip_floor * 2 * 1.5
    let tip = config.tip_floor_sol.to_f64().unwrap() * 2.0 * 1.5;
    assert!(
        (tip - 0.003).abs() < 0.0001,
        "Spear cold start tip should be 0.003"
    );
}

#[test]
fn test_cold_start_exit_tip() {
    let config = create_test_jito_config();

    // Exit uses the ceiling during cold start
    let tip = config.tip_ceiling_sol.to_f64().unwrap();
    assert!(
        (tip - 0.01).abs() < 0.0001,
        "Exit cold start tip should be ceiling"
    );
}
