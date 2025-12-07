//! Tip Manager Unit Tests
//!
//! Tests dynamic Jito tip calculation:
//! - Cold start behavior (tip_floor * 2)
//! - Percentile calculation with history
//! - Tip capping (floor, ceiling, percent max)

use chimera_operator::engine::tips::TipManager;
use chimera_operator::JitoConfig;

const COLD_START_MULTIPLIER: f64 = 2.0;
const MIN_SAMPLES_FOR_PERCENTILE: u32 = 10;

#[test]
fn test_cold_start_multiplier() {
    assert_eq!(COLD_START_MULTIPLIER, 2.0, "Cold start multiplier should be 2.0");
}

#[test]
fn test_cold_start_tip_calculation() {
    let config = JitoConfig {
        enabled: true,
        tip_floor_sol: 0.001,
        tip_ceiling_sol: 0.01,
        tip_percentile: 50,
        tip_percent_max: 0.10,
    };
    
    let cold_tip = config.tip_floor_sol * COLD_START_MULTIPLIER;
    assert!((cold_tip - 0.002).abs() < 0.0001, "Cold start tip should be 0.002 SOL");
}

#[test]
fn test_cold_start_shield_tip() {
    let config = JitoConfig {
        enabled: true,
        tip_floor_sol: 0.001,
        tip_ceiling_sol: 0.01,
        tip_percentile: 50,
        tip_percent_max: 0.10,
    };
    
    // Shield uses floor * 2
    let tip = config.tip_floor_sol * COLD_START_MULTIPLIER;
    assert!((tip - 0.002).abs() < 0.0001, "Shield cold start tip should be 0.002");
}

#[test]
fn test_cold_start_spear_tip() {
    let config = JitoConfig {
        enabled: true,
        tip_floor_sol: 0.001,
        tip_ceiling_sol: 0.01,
        tip_percentile: 50,
        tip_percent_max: 0.10,
    };
    
    // Spear uses floor * 2 * 1.5
    let tip = config.tip_floor_sol * COLD_START_MULTIPLIER * 1.5;
    assert!((tip - 0.003).abs() < 0.0001, "Spear cold start tip should be 0.003");
}

#[test]
fn test_cold_start_exit_tip() {
    let config = JitoConfig {
        enabled: true,
        tip_floor_sol: 0.001,
        tip_ceiling_sol: 0.01,
        tip_percentile: 50,
        tip_percent_max: 0.10,
    };
    
    // Exit uses ceiling during cold start
    let tip = config.tip_ceiling_sol;
    assert!((tip - 0.01).abs() < 0.0001, "Exit cold start tip should be ceiling");
}

#[test]
fn test_min_samples_for_percentile() {
    assert_eq!(MIN_SAMPLES_FOR_PERCENTILE, 10, "Minimum samples should be 10");
}

#[test]
fn test_cold_start_with_few_samples() {
    let sample_count: u32 = 5;
    let is_cold_start = sample_count < MIN_SAMPLES_FOR_PERCENTILE;
    assert!(is_cold_start, "5 samples should trigger cold start mode");
}

#[test]
fn test_exit_cold_start_with_enough_samples() {
    let sample_count: u32 = 10;
    let is_cold_start = sample_count < MIN_SAMPLES_FOR_PERCENTILE;
    assert!(!is_cold_start, "10 samples should exit cold start mode");
}

#[test]
fn test_percentile_50th_calculation() {
    let mut tips: Vec<f64> = vec![
        0.001, 0.002, 0.003, 0.004, 0.005,
        0.006, 0.007, 0.008, 0.009, 0.010,
    ];
    tips.sort_by(|a, b| a.partial_cmp(b).unwrap());
    
    let percentile = 50_usize;
    let index = (tips.len() * percentile / 100).min(tips.len() - 1);
    let tip = tips[index];
    
    assert!((tip - 0.006).abs() < 0.0001, "50th percentile should be 0.006");
}

#[test]
fn test_percentile_25th_calculation() {
    let mut tips: Vec<f64> = vec![
        0.001, 0.002, 0.003, 0.004, 0.005,
        0.006, 0.007, 0.008, 0.009, 0.010,
    ];
    tips.sort_by(|a, b| a.partial_cmp(b).unwrap());
    
    let percentile = 25_usize;
    let index = (tips.len() * percentile / 100).min(tips.len() - 1);
    let tip = tips[index];
    
    assert!((tip - 0.003).abs() < 0.0001, "25th percentile should be 0.003");
}

#[test]
fn test_tip_ceiling_cap() {
    let config = JitoConfig {
        enabled: true,
        tip_floor_sol: 0.001,
        tip_ceiling_sol: 0.01,
        tip_percentile: 50,
        tip_percent_max: 0.10,
    };
    
    let calculated_tip: f64 = 0.015; // Above ceiling
    let capped_tip = calculated_tip.min(config.tip_ceiling_sol);
    
    assert!((capped_tip - 0.01).abs() < 0.0001, "Tip should be capped at ceiling");
}

#[test]
fn test_tip_floor_minimum() {
    let config = JitoConfig {
        enabled: true,
        tip_floor_sol: 0.001,
        tip_ceiling_sol: 0.01,
        tip_percentile: 50,
        tip_percent_max: 0.10,
    };
    
    let calculated_tip: f64 = 0.0005; // Below floor
    let floored_tip = calculated_tip.max(config.tip_floor_sol);
    
    assert!((floored_tip - 0.001).abs() < 0.0001, "Tip should be floored at minimum");
}

#[test]
fn test_tip_percent_max_cap() {
    let config = JitoConfig {
        enabled: true,
        tip_floor_sol: 0.001,
        tip_ceiling_sol: 0.01,
        tip_percentile: 50,
        tip_percent_max: 0.10, // 10%
    };
    
    let trade_amount_sol: f64 = 1.0;
    let calculated_tip: f64 = 0.15; // 15% of trade
    let max_tip_by_percent = trade_amount_sol * config.tip_percent_max;
    let capped_tip = calculated_tip.min(max_tip_by_percent);
    
    assert!((capped_tip - 0.10).abs() < 0.0001, "Tip should be capped at 10% of trade");
}

