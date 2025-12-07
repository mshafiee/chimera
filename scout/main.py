#!/usr/bin/env python3
"""
Chimera Scout - Wallet Intelligence Layer

The Scout runs periodically (via cron) to:
1. Analyze wallet performance from on-chain data
2. Calculate Wallet Quality Scores (WQS)
3. Output updated roster to roster_new.db for Operator merge

Usage:
    python main.py                    # Run with default config
    python main.py --output /path/to/roster_new.db
    python main.py --dry-run          # Analyze without writing

The Scout writes to roster_new.db atomically. The Rust Operator then
merges this into the main database via SIGHUP or API call.
"""

import argparse
import os
import sys
from datetime import datetime
from pathlib import Path
from typing import List, Optional

# Add parent directory to path for imports
sys.path.insert(0, str(Path(__file__).parent))

from core.db_writer import RosterWriter, WalletRecord, write_roster_atomic
from core.wqs import calculate_wqs, WalletMetrics
from core.analyzer import WalletAnalyzer


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
        "--verbose", "-v",
        action="store_true",
        help="Enable verbose output"
    )
    
    return parser.parse_args()


def analyze_wallets(
    analyzer: WalletAnalyzer,
    min_wqs_active: float,
    min_wqs_candidate: float,
    verbose: bool = False
) -> List[WalletRecord]:
    """
    Analyze wallets and generate roster records.
    
    Args:
        analyzer: Wallet analyzer instance
        min_wqs_active: Minimum WQS for ACTIVE status
        min_wqs_candidate: Minimum WQS for CANDIDATE status
        verbose: Enable verbose logging
        
    Returns:
        List of WalletRecord objects
    """
    records = []
    
    # Get candidate wallets from analyzer
    candidates = analyzer.get_candidate_wallets()
    
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
            
            # Determine status based on WQS
            if wqs_score >= min_wqs_active:
                status = "ACTIVE"
            elif wqs_score >= min_wqs_candidate:
                status = "CANDIDATE"
            else:
                status = "REJECTED"
            
            if verbose:
                print(f"  [{status}] {wallet_address[:8]}... WQS: {wqs_score:.1f}")
            
            # Create wallet record
            record = WalletRecord(
                address=wallet_address,
                status=status,
                wqs_score=wqs_score,
                roi_7d=metrics.roi_7d,
                roi_30d=metrics.roi_30d,
                trade_count_30d=metrics.trade_count_30d,
                win_rate=metrics.win_rate,
                max_drawdown_30d=metrics.max_drawdown_30d,
                avg_trade_size_sol=metrics.avg_trade_size_sol,
                last_trade_at=metrics.last_trade_at,
                notes=f"WQS calculated at {datetime.utcnow().isoformat()}",
            )
            records.append(record)
            
        except Exception as e:
            print(f"[Scout] ERROR analyzing {wallet_address[:8]}...: {e}")
            continue
    
    return records


def main():
    """Main entry point for the Scout."""
    args = parse_args()
    
    print("=" * 60)
    print("Chimera Scout - Wallet Intelligence Layer")
    print(f"Started at: {datetime.utcnow().isoformat()}")
    print("=" * 60)
    
    # Initialize analyzer
    # Note: In production, this would connect to RPC/API to fetch on-chain data
    try:
        analyzer = WalletAnalyzer()
    except Exception as e:
        print(f"[Scout] ERROR: Failed to initialize analyzer: {e}")
        sys.exit(1)
    
    # Analyze wallets
    print(f"\n[Scout] Analyzing wallets...")
    print(f"  Min WQS for ACTIVE: {args.min_wqs_active}")
    print(f"  Min WQS for CANDIDATE: {args.min_wqs_candidate}")
    
    records = analyze_wallets(
        analyzer,
        args.min_wqs_active,
        args.min_wqs_candidate,
        verbose=args.verbose
    )
    
    # Summary
    active_count = sum(1 for r in records if r.status == "ACTIVE")
    candidate_count = sum(1 for r in records if r.status == "CANDIDATE")
    rejected_count = sum(1 for r in records if r.status == "REJECTED")
    
    print(f"\n[Scout] Analysis complete:")
    print(f"  ACTIVE: {active_count}")
    print(f"  CANDIDATE: {candidate_count}")
    print(f"  REJECTED: {rejected_count}")
    print(f"  Total: {len(records)}")
    
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
            print(f"\n[Scout] Send SIGHUP to Operator to merge:")
            print(f"  kill -HUP $(pgrep chimera_operator)")
            print(f"  OR call POST /api/v1/roster/merge")
        except Exception as e:
            print(f"[Scout] ERROR: Failed to write roster: {e}")
            sys.exit(1)
    
    print(f"\n[Scout] Finished at: {datetime.utcnow().isoformat()}")
    print("=" * 60)


if __name__ == "__main__":
    main()
