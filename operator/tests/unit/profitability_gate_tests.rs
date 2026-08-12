//! Unit tests for the profitability gate decision table
//! (`signal_pipeline::profitability_gate_blocks`).
//!
//! These lock the fail-closed contract: a live entry BUY is blocked unless the
//! verdict is GO; everything else (paper, devnet, exits, no verdict,
//! INCONCLUSIVE, STOP, unknown) is allowed-or-blocked exactly as documented.

use chimera_operator::config::TradeMode;
use chimera_operator::engine::signal_pipeline::profitability_gate_blocks;
use chimera_operator::models::{Action, Strategy};

#[test]
fn live_go_proceeds() {
    assert_eq!(
        profitability_gate_blocks(TradeMode::Live, Action::Buy, Strategy::Shield, "GO"),
        None
    );
}

#[test]
fn live_no_verdict_fails_closed() {
    // The crucial fail-closed case: no verdict computed yet → block.
    assert!(profitability_gate_blocks(TradeMode::Live, Action::Buy, Strategy::Shield, "").is_some());
}

#[test]
fn live_inconclusive_fails_closed() {
    assert!(profitability_gate_blocks(TradeMode::Live, Action::Buy, Strategy::Spear, "INCONCLUSIVE").is_some());
}

#[test]
fn live_stop_fails_closed() {
    assert!(profitability_gate_blocks(TradeMode::Live, Action::Buy, Strategy::Shield, "STOP").is_some());
}

#[test]
fn live_unknown_verdict_fails_closed() {
    assert!(profitability_gate_blocks(TradeMode::Live, Action::Buy, Strategy::Shield, "WAT").is_some());
}

#[test]
fn paper_mode_never_blocks_even_without_verdict() {
    // Paper must accumulate evidence regardless of verdict.
    assert_eq!(
        profitability_gate_blocks(TradeMode::Paper, Action::Buy, Strategy::Shield, ""),
        None
    );
    assert_eq!(
        profitability_gate_blocks(TradeMode::Paper, Action::Buy, Strategy::Shield, "STOP"),
        None
    );
}

#[test]
fn devnet_mode_never_blocks() {
    assert_eq!(
        profitability_gate_blocks(TradeMode::Devnet, Action::Buy, Strategy::Shield, ""),
        None
    );
}

#[test]
fn exits_are_never_blocked_even_in_live_without_verdict() {
    // Protective sells must always proceed.
    assert_eq!(
        profitability_gate_blocks(TradeMode::Live, Action::Sell, Strategy::Exit, ""),
        None
    );
}

#[test]
fn live_exit_strategy_buy_is_not_gated() {
    // An Exit-strategy BUY (a position-management action, not a new entry) is
    // treated as an exit and not gated.
    assert_eq!(
        profitability_gate_blocks(TradeMode::Live, Action::Buy, Strategy::Exit, ""),
        None
    );
}
