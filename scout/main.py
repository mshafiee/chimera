#!/usr/bin/env python3
"""
Chimera Scout - Wallet Intelligence Layer

The Scout runs periodically (via cron) to:
1. Analyze wallet performance from on-chain data
2. Calculate Wallet Quality Scores (WQS)
3. Run backtest validation before promotion
4. Output updated roster to roster_new.db for Operator merge

Usage:
    python main.py                    # Run with default config
    python main.py --output /path/to/roster_new.db
    python main.py --dry-run          # Analyze without writing
    python main.py --skip-backtest    # Skip backtest validation (faster)

The Scout writes to roster_new.db atomically. The Rust Operator then
merges this into the main database via SIGHUP or API call.
"""

import argparse
import os
import sys
from datetime import datetime
from pathlib import Path
from typing import List, Optional, Tuple

# Add parent directory to path for imports
sys.path.insert(0, str(Path(__file__).parent))

from core.db_writer import RosterWriter, WalletRecord, write_roster_atomic
from core.wqs import calculate_wqs, WalletMetrics
from core.analyzer import WalletAnalyzer
from core.models import BacktestConfig, ValidationStatus
from core.validator import PrePromotionValidator
from core.liquidity import LiquidityProvider


# Default configuration
DEFAULT_OUTPUT_PATH = "../data/roster_new.db"
DEFAULT_MIN_WQS_ACTIVE = 70.0
DEFAULT_MIN_WQS_CANDIDATE = 40.0


def parse_args() -> argparse.Namespace:
    """Parse command line arguments."""
    parser = argparse.ArgumentParser(
        description="Chimera Scout - Wallet Intelligence Layer",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    
    parser.add_argument(
        "--output", "-o",
        default=DEFAULT_OUTPUT_PATH,
        help=f"Output path for roster_new.db (default: {DEFAULT_OUTPUT_PATH})"
    )
    
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Analyze wallets without writing to database"
    )
    
    parser.add_argument(
        "--skip-backtest",
        action="store_true",
        help="Skip backtest validation (faster, but less accurate)"
    )
    
    parser.add_argument(
        "--min-wqs-active",
        type=float,
        default=DEFAULT_MIN_WQS_ACTIVE,
        help=f"Minimum WQS score for ACTIVE status (default: {DEFAULT_MIN_WQS_ACTIVE})"
    )
    
    parser.add_argument(
        "--min-wqs-candidate",
        type=float,
        default=DEFAULT_MIN_WQS_CANDIDATE,
        help=f"Minimum WQS score for CANDIDATE status (default: {DEFAULT_MIN_WQS_CANDIDATE})"
    )
    
    parser.add_argument(
        "--min-liquidity-shield",
        type=float,
        default=10000.0,
        help="Minimum liquidity (USD) for Shield strategy (default: 10000)"
    )
    
    parser.add_argument(
        "--min-liquidity-spear",
        type=float,
        default=5000.0,
        help="Minimum liquidity (USD) for Spear strategy (default: 5000)"
    )
    
    parser.add_argument(
        "--verbose", "-v",
        action="store_true",
        help="Enable verbose output"
    )
    
    return parser.parse_args()


def analyze_wallets(
    analyzer: WalletAnalyzer,
    validator: Optional[PrePromotionValidator],
    min_wqs_active: float,
    min_wqs_candidate: float,
    skip_backtest: bool = False,
    verbose: bool = False,
) -> Tuple[List[WalletRecord], dict]:
    """
    Analyze wallets and generate roster records.
    
    Args:
        analyzer: Wallet analyzer instance
        validator: Pre-promotion validator instance
        min_wqs_active: Minimum WQS for ACTIVE status
        min_wqs_candidate: Minimum WQS for CANDIDATE status
        skip_backtest: Skip backtest validation
        verbose: Enable verbose logging
        
    Returns:
        Tuple of (list of WalletRecords, statistics dict)
    """
    records = []
    stats = {
        "total": 0,
        "active": 0,
        "candidate": 0,
        "rejected": 0,
        "backtest_passed": 0,
        "backtest_failed": 0,
        "backtest_skipped": 0,
    }
    
    # Get candidate wallets from analyzer
    candidates = analyzer.get_candidate_wallets()
    stats["total"] = len(candidates)
    
    if verbose:
        print(f"[Scout] Analyzing {len(candidates)} candidate wallets...")
    
    for wallet_address in candidates:
        try:
            # Get wallet metrics
            metrics = analyzer.get_wallet_metrics(wallet_address)
            
            if metrics is None:
                if verbose:
                    print(f"  [!] No metrics for {wallet_address[:8]}...")
                continue
            
            # Calculate WQS
            wqs_score = calculate_wqs(metrics)
            
            # Initial status based on WQS only
            if wqs_score >= min_wqs_active:
                initial_status = "ACTIVE"
            elif wqs_score >= min_wqs_candidate:
                initial_status = "CANDIDATE"
            else:
                initial_status = "REJECTED"
            
            # For wallets that would be ACTIVE, run backtest validation
            final_status = initial_status
            backtest_notes = None
            
            if initial_status == "ACTIVE" and not skip_backtest and validator:
                # Get historical trades
                trades = analyzer.get_historical_trades(wallet_address, days=30)
                
                if trades:
                    # Run backtest validation
                    validation = validator.validate_for_promotion(
                        wallet_address, metrics, trades, strategy="SHIELD"
                    )
                    
                    if validation.passed:
                        final_status = "ACTIVE"
                        stats["backtest_passed"] += 1
                        backtest_notes = f"Backtest PASSED: {validation.notes}"
                        if verbose:
                            print(f"  [ACTIVE] {wallet_address[:8]}... WQS: {wqs_score:.1f} | Backtest: PASSED")
                    else:
                        # Demote to CANDIDATE if backtest fails
                        final_status = "CANDIDATE"
                        stats["backtest_failed"] += 1
                        backtest_notes = f"Backtest FAILED: {validation.reason}"
                        if verbose:
                            print(f"  [CANDIDATE] {wallet_address[:8]}... WQS: {wqs_score:.1f} | Backtest: FAILED ({validation.reason})")
                else:
                    # No trades to backtest, demote to CANDIDATE
                    final_status = "CANDIDATE"
                    stats["backtest_skipped"] += 1
                    backtest_notes = "No historical trades for backtest"
                    if verbose:
                        print(f"  [CANDIDATE] {wallet_address[:8]}... WQS: {wqs_score:.1f} | Backtest: SKIPPED (no trades)")
            
            elif initial_status == "ACTIVE" and skip_backtest:
                stats["backtest_skipped"] += 1
                if verbose:
                    print(f"  [ACTIVE] {wallet_address[:8]}... WQS: {wqs_score:.1f} | Backtest: SKIPPED")
            
            elif verbose:
                print(f"  [{final_status}] {wallet_address[:8]}... WQS: {wqs_score:.1f}")
            
            # Update stats
            if final_status == "ACTIVE":
                stats["active"] += 1
            elif final_status == "CANDIDATE":
                stats["candidate"] += 1
            else:
                stats["rejected"] += 1
            
            # Build notes
            notes_parts = [f"WQS: {wqs_score:.1f}"]
            if backtest_notes:
                notes_parts.append(backtest_notes)
            notes_parts.append(f"Analyzed at {datetime.utcnow().isoformat()}")
            
            # Create wallet record
            record = WalletRecord(
                address=wallet_address,
                status=final_status,
                wqs_score=wqs_score,
                roi_7d=metrics.roi_7d,
                roi_30d=metrics.roi_30d,
                trade_count_30d=metrics.trade_count_30d,
                win_rate=metrics.win_rate,
                max_drawdown_30d=metrics.max_drawdown_30d,
                avg_trade_size_sol=metrics.avg_trade_size_sol,
                last_trade_at=metrics.last_trade_at,
                notes=" | ".join(notes_parts),
            )
            records.append(record)
            
        except Exception as e:
            print(f"[Scout] ERROR analyzing {wallet_address[:8]}...: {e}")
            continue
    
    return records, stats


def main():
    """Main entry point for the Scout."""
    args = parse_args()
    
    print("=" * 70)
    print("Chimera Scout - Wallet Intelligence Layer")
    print(f"Started at: {datetime.utcnow().isoformat()}")
    print("=" * 70)
    
    # Initialize components
    try:
        # Get Helius API key from environment or RPC URL
        helius_api_key = os.getenv("HELIUS_API_KEY")
        if not helius_api_key:
            # Try to extract from RPC URL
            rpc_url = os.getenv("CHIMERA_RPC__PRIMARY_URL") or os.getenv("SOLANA_RPC_URL", "")
            if "api-key=" in rpc_url:
                helius_api_key = rpc_url.split("api-key=")[1].split("&")[0].split("?")[0]
        
        analyzer = WalletAnalyzer(
            helius_api_key=helius_api_key,
            discover_wallets=True,  # Enable wallet discovery from on-chain data
            max_wallets=50,  # Limit to 50 wallets for analysis
        )
    except Exception as e:
        print(f"[Scout] ERROR: Failed to initialize analyzer: {e}")
        sys.exit(1)
    
    # Initialize validator if not skipping backtest
    validator = None
    if not args.skip_backtest:
        try:
            liquidity_provider = LiquidityProvider()
            backtest_config = BacktestConfig(
                min_liquidity_shield_usd=args.min_liquidity_shield,
                min_liquidity_spear_usd=args.min_liquidity_spear,
                dex_fee_percent=0.003,
                max_slippage_percent=0.05,
                min_trades_required=5,
            )
            validator = PrePromotionValidator(
                liquidity_provider=liquidity_provider,
                backtest_config=backtest_config,
            )
            print(f"[Scout] Backtest validation enabled")
            print(f"  Min liquidity (Shield): ${args.min_liquidity_shield:,.0f}")
            print(f"  Min liquidity (Spear): ${args.min_liquidity_spear:,.0f}")
        except Exception as e:
            print(f"[Scout] WARNING: Failed to initialize validator: {e}")
            print("[Scout] Continuing without backtest validation")
    else:
        print("[Scout] Backtest validation: DISABLED")
    
    # Analyze wallets
    print(f"\n[Scout] Analyzing wallets...")
    print(f"  Min WQS for ACTIVE: {args.min_wqs_active}")
    print(f"  Min WQS for CANDIDATE: {args.min_wqs_candidate}")
    
    records, stats = analyze_wallets(
        analyzer,
        validator,
        args.min_wqs_active,
        args.min_wqs_candidate,
        skip_backtest=args.skip_backtest,
        verbose=args.verbose,
    )
    
    # Summary
    print(f"\n[Scout] Analysis complete:")
    print(f"  Total analyzed: {stats['total']}")
    print(f"  ACTIVE: {stats['active']}")
    print(f"  CANDIDATE: {stats['candidate']}")
    print(f"  REJECTED: {stats['rejected']}")
    
    if not args.skip_backtest:
        print(f"\n[Scout] Backtest results:")
        print(f"  Passed: {stats['backtest_passed']}")
        print(f"  Failed: {stats['backtest_failed']}")
        print(f"  Skipped: {stats['backtest_skipped']}")
    
    # Write output
    if args.dry_run:
        print(f"\n[Scout] Dry run mode - not writing to database")
    else:
        output_path = Path(args.output)
        
        # Ensure parent directory exists
        output_path.parent.mkdir(parents=True, exist_ok=True)
        
        print(f"\n[Scout] Writing roster to {output_path}...")
        
        try:
            write_roster_atomic(records, str(output_path))
            print(f"[Scout] Successfully wrote {len(records)} wallets")
            print(f"\n[Scout] To merge with Operator:")
            print(f"  kill -HUP $(pgrep chimera_operator)")
            print(f"  OR call POST /api/v1/roster/merge")
        except Exception as e:
            print(f"[Scout] ERROR: Failed to write roster: {e}")
            sys.exit(1)
    
    print(f"\n[Scout] Finished at: {datetime.utcnow().isoformat()}")
    print("=" * 70)


if __name__ == "__main__":
    main()
