//! Smart-exit decision helpers.
//!
//! Phase 1 of the smart-exit redesign. The core insight (production data): the
//! shadow `mirror_main` exit predicts 46.6% win / +4.1% avg on admitted signals,
//! but real closes realize 21% win / −0.36 SOL — a realize-vs-price gap. A big
//! part of that is protective exit rails (stop-loss / recovery gate) firing on
//! the price-CACHE reading and exiting when the LIVE sell fill is much worse.
//!
//! `should_defer_exit` is a pure decision: given the cache-implied loss and the
//! live-fill-implied loss, should the exit rail DEFER (hold for a better fill /
//! next tick) rather than sell into a bad fill?
//!
//! Safety rules:
//! - A catastrophic loss (at/beyond the hard-stop floor, e.g. −25%) NEVER defers
//!   — a true dump is sold into the live fill before it can run further.
//! - A missing live fill (quote unavailable) does NOT defer — fail-safe: exit.
//! - Otherwise defer only when the live fill is *materially* worse than the
//!   cache reading, i.e. the gap exceeds `skew_pct` (strictly).

use rust_decimal::Decimal;

/// Decide whether a protective exit should be deferred.
///
/// # Arguments
/// * `loss_pct_cache` — loss implied by the latest price-cache reading (negative).
/// * `loss_pct_live` — loss implied by the live Jupiter sell fill (negative), or
///   `None` when no quote is available.
/// * `hard_stop_floor_pct` — catastrophic floor (negative), e.g. −25.
/// * `skew_pct` — minimum gap between live and cache loss (positive) before the
///   fill counts as "materially worse" and triggers a defer.
///
/// # Returns
/// `true` to defer (hold), `false` to exit now.
pub fn should_defer_exit(
    loss_pct_cache: Decimal,
    loss_pct_live: Option<Decimal>,
    hard_stop_floor_pct: Decimal,
    skew_pct: Decimal,
) -> bool {
    // Catastrophic override: a true dump past the floor is sold immediately.
    if loss_pct_cache <= hard_stop_floor_pct {
        return false;
    }
    // Fail-safe: no live fill means we cannot prove the fill is bad.
    let Some(loss_pct_live) = loss_pct_live else {
        return false;
    };
    // Defer only when the live loss is deeper than the cache loss by more than
    // the skew band (e.g. cache -3%, live -12%: gap 9% > 5% -> defer).
    let gap = loss_pct_cache - loss_pct_live;
    gap > skew_pct
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn cache_loss_equal_hard_stop_floor_defers_false() {
        assert!(!should_defer_exit(
            dec!(-25),
            Some(dec!(-20)),
            dec!(-25),
            dec!(5)
        ));
    }
}
