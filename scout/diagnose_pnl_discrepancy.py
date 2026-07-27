#!/usr/bin/env python3
"""
Diagnostic script to investigate PnL discrepancy between analyzer and backtest.

This script fetches actual trade data for the problematic wallet and compares
the ROI calculation from analyzer.py vs the simulated PnL from backtester.py.
"""

import asyncio
import os
import sys
from pathlib import Path

# Add scout directory to path
sys.path.insert(0, str(Path(__file__).parent))

from core.analyzer import WalletAnalyzer
from core.helius_client import HeliusClient
from core.backtester import BacktestSimulator
from core.liquidity import LiquidityProvider
from core.models import BacktestConfig
from decimal import Decimal


async def diagnose_wallet(wallet_address: str):
    """Diagnose PnL discrepancy for a specific wallet."""

    print(f"\n{'='*80}")
    print(f"Diagnosing wallet: {wallet_address}")
    print(f"{'='*80}\n")

    # Initialize components
    helius_api_key = os.getenv("HELIUS_API_KEY")
    if not helius_api_key:
        print("ERROR: HELIUS_API_KEY not set")
        return

    # Initialize Helius client
    helius_client = HeliusClient(api_key=helius_api_key)

    # Fetch trade data
    print("Fetching trade data...")
    trades = await helius_client.get_wallet_transactions(
        wallet_address,
        days=30,
        limit=100
    )

    print(f"✓ Found {len(trades)} transactions")
    print(f"  Trade actions: {[t.action.value for t in trades[:20]]}")

    # Filter to actual trades only
    actual_trades = [t for t in trades if t.action.value in ('BUY', 'SELL')]
    print(f"✓ Filtered to {len(actual_trades)} trades (BUY/SELL only)")

    # Calculate ROI using analyzer (this is what appears in DB)
    print("\n--- Analyzer ROI Calculation ---")
    analyzer = WalletAnalyzer(
        helius_api_key=helius_api_key,
        discovery_hours=168,
        wallet_tx_limit=500,
        wallet_tx_max_pages=20,
    )

    # We need to create a simplified metrics object to test ROI calculation
    # For now, let's just look at the trade data directly
    print(f"\nTrade details (first 10 trades):")
    print(f"{'Idx':<4} {'Action':<6} {'Sol Amount':<12} {'Token Amount':<12} {'PnL SOL':<10} {'Liquidity':<15} {'Price SOL':<10}")
    print("-" * 90)

    for i, t in enumerate(actual_trades[:10]):
        sol_amt = float(t.sol_amount or t.amount_sol or 0)
        token_amt = float(t.token_amount or 0)
        pnl = float(t.pnl_sol or 0)
        liq = float(t.liquidity_at_trade_usd or 0)
        price_sol = float(t.price_sol or 0)

        print(f"{i:<4} {t.action.value:<6} {sol_amt:<12.4f} {token_amt:<12.4f} {pnl:<10.4f} ${liq:<14,.0f} {price_sol:<10.6f}")

    # Calculate total on-chain ROI
    total_buy_sol = sum(t.sol_amount or t.amount_sol or 0 for t in actual_trades if t.action.value == 'BUY')
    total_sell_sol = sum(t.sol_amount or t.amount_sol or 0 for t in actual_trades if t.action.value == 'SELL')
    total_pnl = sum(t.pnl_sol or 0 for t in actual_trades if t.action.value == 'SELL')

    if total_buy_sol > 0:
        on_chain_roi = (total_pnl / total_buy_sol) * 100
        print(f"\nTotal Buy SOL: ${total_buy_sol:.4f}")
        print(f"Total Sell SOL: ${total_sell_sol:.4f}")
        print(f"Total PnL SOL: ${total_pnl:.4f}")
        print(f"📊 On-chain ROI: {on_chain_roi:.2f}%")

    # Now simulate using backtester
    print(f"\n{'='*80}")
    print(f"--- Backtest Simulation ---")
    print(f"{'='*80}\n")

    # Initialize liquidity provider
    liquidity_mode = os.getenv("SCOUT_LIQUIDITY_MODE", "real").lower()
    liquidity_provider = LiquidityProvider(mode=liquidity_mode)

    # Initialize backtester
    backtest_config = BacktestConfig(
        min_liquidity_shield_usd=Decimal('10000'),
        min_liquidity_spear_usd=Decimal('5000'),
        dex_fee_percent=Decimal('0.003'),
        max_slippage_percent=Decimal('0.05'),
        min_trades_required=5,
        priority_fee_sol_per_trade=Decimal('0.00005'),
        jito_tip_sol_per_trade=Decimal('0.0001'),
        enforce_current_liquidity=False,
        simulate_at_size_sol=None,
        entry_delay_slippage_pct=Decimal('0.015'),
        exit_delay_slippage_pct=Decimal('0.010'),
        mev_penalty_pct=Decimal('0.002'),
        lookback_days=30,
    )

    simulator = BacktestSimulator(liquidity_provider, backtest_config)
    result = simulator.simulate_wallet(wallet_address, actual_trades)

    print(f"✓ Simulated {result.simulated_trades} trades")
    print(f"✓ Rejected {result.rejected_trades} trades")
    print(f"✓ Original PnL: ${result.original_pnl_sol:.4f} SOL")
    print(f"✓ Simulated PnL: ${result.simulated_pnl_sol:.4f} SOL")
    print(f"✓ Difference: ${result.pnl_difference_sol:.4f} SOL")

    if result.passed:
        print(f"\n✅ Wallet PASSED backtest!")
    else:
        print(f"\n❌ Wallet FAILED backtest: {result.failure_reason}")

    # Show rejected trades
    if result.rejected_trades > 0:
        print(f"\nRejected trades (showing first 5):")
        rejected_count = 0
        for sim_trade in result.simulated_trades:
            if sim_trade.rejected:
                print(f"\n  Trade {sim_trade.original_trade.token_symbol}:")
                print(f"    Action: {sim_trade.original_trade.action.value}")
                print(f"    Size: {float(sim_trade.original_trade.amount_sol) if sim_trade.original_trade.amount_sol else sim_trade.original_trade.sol_amount:.4f} SOL")
                print(f"    Liquidity: ${sim_trade.current_liquidity_usd:.0f}")
                print(f"    Reason: {sim_trade.rejection_reason}")
                rejected_count += 1
                if rejected_count >= 5:
                    break

    print(f"\n{'='*80}\n")


if __name__ == "__main__":
    # Test with the problematic wallet
    WALLET_ADDRESS = "3uGcxoHV5FCKQGqA77S2HDMMtTjsAcvF3xgXdSZUfVdr"  # 815% ROI, -0.307 SOL backtest

    asyncio.run(diagnose_wallet(WALLET_ADDRESS))
