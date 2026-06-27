//! Tests for ATR-based stop-loss functionality
//!
//! Tests the new ATR-based stop-loss optimization with market regime adjustment.

use chimera_operator::engine::stop_loss::MarketRegime;
use rust_decimal_macros::dec;

#[test]
fn test_market_regime_multipliers() {
    // Test that market regimes return correct ATR multipliers
    let bull = MarketRegime::Bull;
    let bear = MarketRegime::Bear;
    let volatile = MarketRegime::Volatile;
    let neutral = MarketRegime::Neutral;

    assert_eq!(bull.atr_multiplier(), dec!(1.5));
    assert_eq!(bear.atr_multiplier(), dec!(1.0));
    assert_eq!(volatile.atr_multiplier(), dec!(2.0));
    assert_eq!(neutral.atr_multiplier(), dec!(1.25));

    println!("✓ Market regime multipliers correct");
}

#[test]
fn test_market_regime_parsing() {
    // Test that market regimes parse correctly from strings
    assert_eq!(MarketRegime::from_str("BULL"), MarketRegime::Bull);
    assert_eq!(MarketRegime::from_str("BEAR"), MarketRegime::Bear);
    assert_eq!(MarketRegime::from_str("VOLATILE"), MarketRegime::Volatile);
    assert_eq!(MarketRegime::from_str("NEUTRAL"), MarketRegime::Neutral);
    assert_eq!(MarketRegime::from_str("bull"), MarketRegime::Bull);
    assert_eq!(MarketRegime::from_str("bear"), MarketRegime::Bear);
    assert_eq!(MarketRegime::from_str("unknown"), MarketRegime::Neutral);

    println!("✓ Market regime parsing works");
}

#[test]
fn test_market_regime_display() {
    // Test market regime display and ordering
    let regimes = vec![
        MarketRegime::Bull,
        MarketRegime::Bear,
        MarketRegime::Volatile,
        MarketRegime::Neutral,
    ];

    // Verify all regimes have different multipliers
    let multipliers: Vec<_> = regimes.iter().map(|r| r.atr_multiplier()).collect();
    let unique_multipliers: std::collections::HashSet<_> = multipliers.iter().collect();

    assert_eq!(unique_multipliers.len(), 4, "All regimes should have unique multipliers");

    println!("✓ Market regime multipliers are unique");

    // Verify ordering logic
    let bear_mult = MarketRegime::Bear.atr_multiplier();
    let neutral_mult = MarketRegime::Neutral.atr_multiplier();
    let bull_mult = MarketRegime::Bull.atr_multiplier();
    let volatile_mult = MarketRegime::Volatile.atr_multiplier();

    assert!(bear_mult < neutral_mult, "Bear should have tightest stops");
    assert!(neutral_mult < bull_mult, "Neutral should be tighter than bull");
    assert!(bull_mult < volatile_mult, "Bull should be tighter than volatile");

    println!("✓ Market regime multipliers follow expected ordering:");
    println!("  Bear: {}", bear_mult);
    println!("  Neutral: {}", neutral_mult);
    println!("  Bull: {}", bull_mult);
    println!("  Volatile: {}", volatile_mult);
}

#[test]
fn test_atr_formula_logic() {
    // Test ATR-based stop-loss formula logic without full setup
    let entry_price = dec!(100.0);
    let atr_value = dec!(5.0);
    let atr_multiplier = dec!(1.5);
    let regime_multiplier = dec!(1.5);

    // Formula: stop_price = entry_price - (entry_price * atr_value * atr_multiplier * regime_multiplier / 100.0)
    let atr_distance = atr_value * atr_multiplier * regime_multiplier / dec!(100.0);
    let stop_price = entry_price - (entry_price * atr_distance);

    // Verify the stop-loss price is below entry for long positions
    assert!(stop_price < entry_price, "ATR stop-loss should be below entry price");

    // Verify the stop-loss distance is reasonable (not too tight or wide)
    let loss_percent = ((stop_price - entry_price) / entry_price) * dec!(100.0);
    assert!(loss_percent > dec!(-20), "ATR stop-loss should not be extremely tight");
    assert!(loss_percent < dec!(-5), "ATR stop-loss should provide protection");

    println!("✓ ATR formula logic works:");
    println!("  Entry: ${}", entry_price);
    println!("  ATR: {}", atr_value);
    println!("  Stop: ${}", stop_price);
    println!("  Loss: {}%", loss_percent);
}

#[test]
fn test_regime_adjustment_logic() {
    // Test that different regimes produce appropriate stop adjustments
    let entry_price = dec!(100.0);
    let atr_value = dec!(5.0);
    let base_multiplier = dec!(1.5);

    let regimes = vec![
        (MarketRegime::Bull, "Bull"),
        (MarketRegime::Bear, "Bear"),
        (MarketRegime::Volatile, "Volatile"),
        (MarketRegime::Neutral, "Neutral"),
    ];

    let mut stops = Vec::new();

    for (regime, name) in regimes {
        let regime_mult = regime.atr_multiplier();
        let atr_distance = atr_value * base_multiplier * regime_mult / dec!(100.0);
        let stop_price = entry_price - (entry_price * atr_distance);
        stops.push((name, stop_price));
    }

    // Verify regime ordering (bear tightest, volatile widest)
    let bear_stop = stops.iter().find(|(n, _)| *n == "Bear").map(|(_, s)| s).unwrap();
    let neutral_stop = stops.iter().find(|(n, _)| *n == "Neutral").map(|(_, s)| s).unwrap();
    let bull_stop = stops.iter().find(|(n, _)| *n == "Bull").map(|(_, s)| s).unwrap();
    let volatile_stop = stops.iter().find(|(n, _)| *n == "Volatile").map(|(_, s)| s).unwrap();

    assert!(bear_stop > neutral_stop, "Bear should have tightest stops (highest price)");
    assert!(neutral_stop > bull_stop, "Neutral should be tighter than bull");
    assert!(bull_stop > volatile_stop, "Bull should be tighter than volatile");

    println!("✓ Regime adjustment logic works correctly:");
    for (name, stop) in &stops {
        println!("  {}: ${}", name, stop);
    }
}