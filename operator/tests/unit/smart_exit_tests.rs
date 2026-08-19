//! Unit tests for the smart-exit decision helpers.
//!
//! Phase 1 of the smart-exit redesign: a pure, testable decision function that
//! tells a protective exit rail whether to DEFER (hold) because the live sell
//! fill is materially worse than the latest price-cache reading, instead of
//! selling into a bad fill — with a catastrophic-drop override and a fail-safe
//! on missing quote data.

use chimera_operator::engine::smart_exit::should_defer_exit;
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;

const DEFAULT_HARD_STOP_FLOOR: Decimal = dec!(-25);
const DEFAULT_SKEW_PCT: Decimal = dec!(5);

#[test]
fn catastrophic_loss_never_defers() {
    // At or beyond the hard-stop floor the position is being dumped; we accept
    // the fill rather than risk bag-holding. Never defer at or past -25.
    assert!(!should_defer_exit(
        dec!(-30),
        Some(dec!(-29)),
        DEFAULT_HARD_STOP_FLOOR,
        DEFAULT_SKEW_PCT
    ));
    assert!(!should_defer_exit(
        dec!(-25),
        Some(dec!(-24)),
        DEFAULT_HARD_STOP_FLOOR,
        DEFAULT_SKEW_PCT
    ));
}

#[test]
fn missing_live_quote_failsafe_exits() {
    // No live fill data -> cannot prove the fill is bad -> fail-safe: exit now.
    assert!(!should_defer_exit(
        dec!(-5),
        None,
        DEFAULT_HARD_STOP_FLOOR,
        DEFAULT_SKEW_PCT
    ));
}

#[test]
fn live_fill_about_equal_cache_does_not_defer() {
    // Live loss (-6) vs cache loss (-5): gap of 1 is within the 5% skew band ->
    // the fill is not materially worse -> do not defer.
    assert!(!should_defer_exit(
        dec!(-5),
        Some(dec!(-6)),
        DEFAULT_HARD_STOP_FLOOR,
        DEFAULT_SKEW_PCT
    ));
}

#[test]
fn live_fill_much_worse_defers() {
    // Cache says -3%, live fill would realize -12%: gap of 9% exceeds the 5%
    // skew -> the exit is selling into a bad fill -> defer (hold for a better
    // fill or the next tick), as long as we're not past the catastrophic floor.
    assert!(should_defer_exit(
        dec!(-3),
        Some(dec!(-12)),
        DEFAULT_HARD_STOP_FLOOR,
        DEFAULT_SKEW_PCT
    ));
}

#[test]
fn live_fill_better_than_cache_does_not_defer() {
    // Live fill (-4) is BETTER than the cache reading (-8): no reason to defer.
    assert!(!should_defer_exit(
        dec!(-8),
        Some(dec!(-4)),
        DEFAULT_HARD_STOP_FLOOR,
        DEFAULT_SKEW_PCT
    ));
}

#[test]
fn edge_equal_skew_does_not_defer() {
    // Gap exactly equal to the skew threshold is NOT "materially worse"
    // (strictly-greater comparison) -> do not defer.
    assert!(!should_defer_exit(
        dec!(-5),
        Some(dec!(-10)),
        DEFAULT_HARD_STOP_FLOOR,
        dec!(5)
    ));
}

#[test]
fn catastrophic_takes_precedence_over_skew() {
    // Even if the live fill is much worse (-40 vs -30), a position already past
    // the catastrophic floor must never be deferred — exit immediately.
    assert!(!should_defer_exit(
        dec!(-30),
        Some(dec!(-40)),
        DEFAULT_HARD_STOP_FLOOR,
        DEFAULT_SKEW_PCT
    ));
}
