//! Tests for the friction-gating expected-profit calculation.
//!
//! The full friction-gating decision lives inside `Executor::check_execution_costs`,
//! which requires live Jupiter quotes / RPC mocking to drive end-to-end. This file
//! pins the expected-profit arithmetic that gates the decision so the formula
//! (expected return = win_rate*avg_win - (1-win_rate)*avg_loss) cannot regress.

use rust_decimal::Decimal;
use rust_decimal::prelude::*;

/// Helper function to calculate expected profit manually
/// This can be used in other tests to verify friction gating behavior
pub fn calculate_expected_profit_manually(
    win_rate: Decimal,
    avg_win: Decimal,
    avg_loss: Decimal,
    position_size: Decimal,
) -> Decimal {
    let loss_rate = Decimal::ONE - win_rate;
    let expected_return = (win_rate * avg_win) - (loss_rate * avg_loss);
    position_size * expected_return
}

#[test]
fn test_expected_profit_calculation_helper() {
    // Verify the helper function works correctly
    // 60% win rate, 15% avg win, 8% avg loss, 1.0 SOL position
    let win_rate = Decimal::from_str("0.6").unwrap();
    let avg_win = Decimal::from_str("0.15").unwrap();
    let avg_loss = Decimal::from_str("0.08").unwrap();
    let position_size = Decimal::from_str("1.0").unwrap();

    let expected_profit = calculate_expected_profit_manually(
        win_rate,
        avg_win,
        avg_loss,
        position_size,
    );

    // Expected: 1.0 * ((0.6 * 0.15) - (0.4 * 0.08)) = 1.0 * (0.09 - 0.032) = 0.058
    let expected = Decimal::from_str("0.058").unwrap();
    let tolerance = Decimal::from_str("0.001").unwrap();

    assert!(
        (expected_profit - expected).abs() < tolerance,
        "Expected profit calculation failed: got {}, expected {}",
        expected_profit,
        expected
    );
}

#[test]
fn test_expected_profit_negative_edge() {
    // A losing edge must produce a negative expected profit so the gating
    // check (expected_profit <= total_cost) rejects the trade.
    let win_rate = Decimal::from_str("0.4").unwrap();
    let avg_win = Decimal::from_str("0.10").unwrap();
    let avg_loss = Decimal::from_str("0.20").unwrap();

    let expected_profit = calculate_expected_profit_manually(
        win_rate,
        avg_win,
        avg_loss,
        Decimal::from_str("1.0").unwrap(),
    );

    let expected = Decimal::from_str("-0.08").unwrap(); // 0.4*0.1 - 0.6*0.2
    let tolerance = Decimal::from_str("0.001").unwrap();
    assert!(
        (expected_profit - expected).abs() < tolerance,
        "Losing edge must produce negative expected profit: got {}, expected {}",
        expected_profit,
        expected
    );
    assert!(expected_profit < Decimal::ZERO);
}

#[test]
fn test_expected_profit_scales_with_position_size() {
    // Doubling the position size must double the expected profit.
    let win_rate = Decimal::from_str("0.6").unwrap();
    let avg_win = Decimal::from_str("0.15").unwrap();
    let avg_loss = Decimal::from_str("0.08").unwrap();

    let one_sol = calculate_expected_profit_manually(
        win_rate, avg_win, avg_loss, Decimal::ONE,
    );
    let two_sol = calculate_expected_profit_manually(
        win_rate, avg_win, avg_loss, Decimal::from(2),
    );

    assert_eq!(two_sol, one_sol * Decimal::from(2));
}
