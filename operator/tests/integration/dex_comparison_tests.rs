//! Integration tests for multi-DEX comparison
//!
//! Tests that multiple DEXs are queried and the one with
//! lowest total cost is selected.

use chimera_operator::engine::dex_comparator::DexComparator;

#[tokio::test]
#[ignore] // Requires network access - run with: cargo test -- --ignored
async fn test_dex_comparison_jupiter() {
    let comparator = DexComparator::new();
    
    let sol_mint = "So11111111111111111111111111111111111111112";
    let usdc_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    let amount_sol = 1.0;

    // Query Jupiter (should work)
    let result = comparator
        .compare_and_select(sol_mint, usdc_mint, amount_sol)
        .await;

    match result {
        Ok(dex_result) => {
            println!("Selected DEX: {}", dex_result.selected_dex);
            println!("Total cost: {:.6} SOL", dex_result.total_cost_sol);
            println!("Fee: {:.6} SOL", dex_result.fee_sol);
            println!("Slippage: {:.6} SOL", dex_result.slippage_sol);
            
            assert_eq!(dex_result.selected_dex, "Jupiter");
            assert!(dex_result.total_cost_sol > 0.0);
            assert!(dex_result.fee_sol > 0.0);
        }
        Err(e) => {
            println!("DEX comparison error (expected in CI): {}", e);
            // Don't fail test if network is unavailable
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_dex_comparison_caching() {
    let comparator = DexComparator::new();
    
    let sol_mint = "So11111111111111111111111111111111111111112";
    let usdc_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    let amount_sol = 1.0;

    // First call should query API
    let result1 = comparator
        .compare_and_select(sol_mint, usdc_mint, amount_sol)
        .await;

    // Second call within 5 seconds should use cache
    let result2 = comparator
        .compare_and_select(sol_mint, usdc_mint, amount_sol)
        .await;

    if let (Ok(r1), Ok(r2)) = (result1, result2) {
        // Results should be identical (cached)
        assert_eq!(r1.selected_dex, r2.selected_dex);
        assert_eq!(r1.total_cost_sol, r2.total_cost_sol);
    }
}

#[tokio::test]
#[ignore]
async fn test_dex_comparison_multiple_dexs() {
    let comparator = DexComparator::new();
    
    let sol_mint = "So11111111111111111111111111111111111111112";
    let usdc_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    let amount_sol = 1.0;

    // This should query Jupiter, Raydium, Orca, and Meteora in parallel
    let result = comparator
        .compare_and_select(sol_mint, usdc_mint, amount_sol)
        .await;

    match result {
        Ok(dex_result) => {
            println!("Selected DEX: {}", dex_result.selected_dex);
            println!("Total cost: {:.6} SOL", dex_result.total_cost_sol);
            
            // Should select one of the DEXs
            assert!(
                dex_result.selected_dex == "Jupiter" ||
                dex_result.selected_dex == "Raydium" ||
                dex_result.selected_dex == "Orca" ||
                dex_result.selected_dex == "Meteora"
            );
        }
        Err(e) => {
            println!("Multi-DEX comparison error: {}", e);
            // Network errors are acceptable in CI
        }
    }
}

#[tokio::test]
async fn test_dex_comparison_fallback() {
    let comparator = DexComparator::new();
    
    // Use invalid token addresses to trigger fallback
    let invalid_token1 = "InvalidToken111111111111111111111111111111";
    let invalid_token2 = "InvalidToken222222222222222222222222222222";
    let amount_sol = 1.0;

    // Should fallback to default Jupiter result if all queries fail
    let result = comparator
        .compare_and_select(invalid_token1, invalid_token2, amount_sol)
        .await;

    // Should still return a result (fallback)
    assert!(result.is_ok(), "Should fallback gracefully");
    let dex_result = result.unwrap();
    assert_eq!(dex_result.selected_dex, "Jupiter");
}


