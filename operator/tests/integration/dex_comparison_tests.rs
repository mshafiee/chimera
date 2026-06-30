//! Integration tests for multi-DEX route selection.
//!
//! The CI-safe test (`test_slippage_model_*`) is non-network and asserts the
//! unified slippage model behaves — this is what actually gates the on-chain
//! `slippageBps`. The real route-selection tests are gated behind `#[ignore]`
//! and require a Jupiter API key (set `CHIMERA_JUPITER__API_KEY`), since live
//! routing depends on real liquidity that cannot be meaningfully mocked here.

use chimera_operator::engine::slippage;
use chimera_operator::Strategy;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

/// A deep pool: a 1 SOL trade against $500k liquidity should produce a tight,
/// sub-1% expected impact and a proportionally small (but buffered) tolerance.
#[test]
fn test_slippage_model_deep_pool() {
    let fb = slippage::FallbackTiers {
        small_fraction: dec!(0.005),
        large_fraction: dec!(0.01),
        threshold_sol: dec!(0.5),
    };
    // 1 SOL @ $100 = $100 trade vs $500k liquidity → raw 100/(2×500000) = 0.0001
    // → clamped UP to the impact floor (0.001) — deep pools get a small fixed impact.
    let est = slippage::estimate(
        Strategy::Spear,
        None,
        dec!(1),
        Some(dec!(500_000)),
        Some(dec!(100)),
        fb,
    );
    assert_eq!(est.expected_fraction, dec!(0.001), "deep pool clamps to the impact floor");
    // tolerance = (0.001×2 + 0.003) × 1e4 = 50 bps (within Spear [30,300]).
    assert_eq!(est.tolerance_bps, 50);
}

/// A thin pool (memecoin-grade): a 0.5 SOL trade against $5k liquidity should
/// produce a ~0.5% expected impact and a wider tolerance.
#[test]
fn test_slippage_model_thin_pool() {
    let fb = slippage::FallbackTiers {
        small_fraction: dec!(0.005),
        large_fraction: dec!(0.01),
        threshold_sol: dec!(0.5),
    };
    // 0.5 SOL @ $100 = $50 vs $5k → 50/(2×5000) = 0.005 (0.5%)
    let est = slippage::estimate(
        Strategy::Spear,
        None,
        dec!(0.5),
        Some(dec!(5_000)),
        Some(dec!(100)),
        fb,
    );
    assert_eq!(est.expected_fraction, dec!(0.005));
    // tolerance = 0.005×2 + 0.003 = 0.013 → 130 bps
    assert_eq!(est.tolerance_bps, 130);
}

/// Jupiter's real impact always wins; Shield clamps to its tight ceiling.
#[test]
fn test_slippage_model_jupiter_overrides_and_shield_clamps() {
    let fb = slippage::FallbackTiers {
        small_fraction: dec!(0.005),
        large_fraction: dec!(0.01),
        threshold_sol: dec!(0.5),
    };
    let est =
        slippage::estimate(Strategy::Shield, Some(dec!(5.0)), dec!(1), None, None, fb);
    // 5% real impact, but Shield tolerance caps at 100 bps.
    assert_eq!(est.expected_fraction, dec!(0.05));
    assert_eq!(est.tolerance_bps, 100);
}

#[tokio::test]
#[ignore] // Requires network + CHIMERA_JUPITER__API_KEY: cargo test -- --ignored
async fn test_route_selection_real() {
    use chimera_operator::engine::dex_comparator::DexComparator;

    if chimera_operator::jupiter::api_key().is_none() {
        eprintln!("Skipping: no Jupiter API key installed");
        return;
    }

    let comparator = DexComparator::new().expect("Failed to create DexComparator");
    let sol_mint = "So11111111111111111111111111111111111111112";
    let usdc_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    let sel = comparator
        .select_route(sol_mint, usdc_mint, 1_000_000_000, 50)
        .await
        .expect("route selection should succeed with a live key");

    // A real quote always carries a non-zero outAmount.
    let out = sel
        .quote
        .get("outAmount")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u64>().ok())
        .expect("real quote has outAmount");
    assert!(out > 0);
    assert!(sel.total_cost_sol >= Decimal::ZERO);
    println!(
        "selected_dex={} out={} fee_sol={} slippage_sol={}",
        sel.selected_dex, out, sel.fee_sol, sel.slippage_sol
    );
}

#[tokio::test]
#[ignore] // Requires network + CHIMERA_JUPITER__API_KEY
async fn test_route_selection_caching() {
    use chimera_operator::engine::dex_comparator::DexComparator;

    if chimera_operator::jupiter::api_key().is_none() {
        eprintln!("Skipping: no Jupiter API key installed");
        return;
    }

    let comparator = DexComparator::new().expect("Failed to create DexComparator");
    let sol_mint = "So11111111111111111111111111111111111111112";
    let usdc_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    let r1 = comparator
        .select_route(sol_mint, usdc_mint, 1_000_000_000, 50)
        .await
        .expect("first selection");
    let r2 = comparator
        .select_route(sol_mint, usdc_mint, 1_000_000_000, 50)
        .await
        .expect("cached selection");
    // Cached within TTL → identical selection.
    assert_eq!(r1.selected_dex, r2.selected_dex);
}
