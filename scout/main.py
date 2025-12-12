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
from typing import List, Optional, Tuple, Dict, Any

# Add parent directory to path for imports
sys.path.insert(0, str(Path(__file__).parent))

from core.db_writer import RosterWriter, WalletRecord, write_roster_atomic
from core.wqs import calculate_wqs, WalletMetrics
from core.analyzer import WalletAnalyzer
from core.models import BacktestConfig, ValidationStatus
from core.validator import PrePromotionValidator, PromotionCriteria
from core.liquidity import LiquidityProvider
from core.auto_merge import auto_merge_roster

# Import config module if available
try:
    from config import ScoutConfig
    CONFIG_AVAILABLE = True
except ImportError:
    CONFIG_AVAILABLE = False
    ScoutConfig = None


# Default configuration (tuned defaults; can be overridden by env/flags)
# Note: WQS thresholds aligned with rescaled 0-100 range (see wqs.py)
DEFAULT_OUTPUT_PATH = "../data/roster_new.db"
DEFAULT_MIN_WQS_ACTIVE = 60.0  # Rescaled from 35.0 (was ~55% of old max, now 60% of 0-100)
DEFAULT_MIN_WQS_CANDIDATE = 30.0  # Rescaled from 25.0 (was ~45% of old max, now 30% of 0-100)
DEFAULT_DISCOVERY_HOURS = 168
DEFAULT_WALLET_TX_LIMIT = 500
DEFAULT_WALLET_TX_MAX_PAGES = 20
DEFAULT_PRIORITY_FEE_SOL = 0.00005
DEFAULT_JITO_TIP_SOL = 0.0001


def _percentile(values: List[float], p: float) -> Optional[float]:
    """Compute percentile with linear interpolation (p in [0, 100])."""
    if not values:
        return None
    xs = sorted(values)
    if len(xs) == 1:
        return float(xs[0])
    p = max(0.0, min(100.0, float(p)))
    k = (len(xs) - 1) * (p / 100.0)
    f = int(k)
    c = min(f + 1, len(xs) - 1)
    if f == c:
        return float(xs[f])
    d = k - f
    return float(xs[f] * (1.0 - d) + xs[c] * d)


def _calibration_report(records: List[WalletRecord], stats: Dict[str, Any]) -> None:
    """Print percentiles and suggested thresholds based on current run."""
    wqs = [r.wqs_score for r in records if r.wqs_score is not None]
    closes = [float(r.trade_count_30d) for r in records if r.trade_count_30d is not None]
    wins = [r.win_rate for r in records if r.win_rate is not None]
    wqs_closers = [
        r.wqs_score
        for r in records
        if r.wqs_score is not None and (r.trade_count_30d or 0) >= 3
    ]

    def fmt(x: Optional[float]) -> str:
        return "n/a" if x is None else f"{x:.2f}"

    print("\n[Scout] Calibration report (from this run)")
    print(f"  Wallets discovered: {stats.get('total', 0)}")
    print(f"  Wallets with metrics: {len(records)}")
    if stats.get("total", 0) and len(records) < stats.get("total", 0):
        print(f"  Wallets missing metrics: {stats.get('total', 0) - len(records)}")

    print("  WQS percentiles:")
    for p in [10, 25, 50, 75, 90, 95]:
        print(f"    p{p}: {fmt(_percentile(wqs, p))}")

    print("  WQS percentiles (wallets with >=3 closes):")
    for p in [10, 25, 50, 75, 90, 95]:
        print(f"    p{p}: {fmt(_percentile(wqs_closers, p))}")

    print("  Close-count (trade_count_30d) percentiles:")
    for p in [10, 25, 50, 75, 90, 95]:
        v = _percentile(closes, p)
        print(f"    p{p}: {fmt(v)}")

    print("  Win-rate percentiles:")
    for p in [10, 25, 50, 75, 90]:
        print(f"    p{p}: {fmt(_percentile(wins, p))}")

    # Suggested thresholds (heuristics) - aligned with rescaled 0-100 WQS
    # Prefer using the subset with >=3 closes so we don't let "no-close" wallets
    # drag thresholds toward zero.
    p75 = _percentile(wqs_closers, 75) or _percentile(wqs, 75) or DEFAULT_MIN_WQS_CANDIDATE
    p90 = _percentile(wqs_closers, 90) or _percentile(wqs, 90) or DEFAULT_MIN_WQS_ACTIVE

    # Thresholds now in 0-100 range
    suggested_candidate = max(30.0, min(70.0, p75))
    suggested_active = max(suggested_candidate + 15.0, min(90.0, p90))

    median_closes = _percentile(closes, 50) or 0.0
    p75_closes = _percentile(closes, 75) or 0.0
    suggested_min_closes = int(max(3.0, min(10.0, median_closes)))

    # Holdout fraction suggestion: try to keep >=5 closes in holdout for a typical wallet.
    # If close counts are low, reduce holdout to preserve minimum holdout size.
    # (Validator still falls back to full set if holdout is too small.)
    suggested_holdout = 0.3
    if median_closes > 0 and median_closes * suggested_holdout < 5:
        suggested_holdout = max(0.15, min(0.3, 5.0 / max(1.0, p75_closes)))

    print("\n  Suggested defaults (heuristics):")
    print(f"    min_wqs_candidate: {suggested_candidate:.1f}")
    print(f"    min_wqs_active:    {suggested_active:.1f}")
    print(f"    min_closes_required_for_promotion: {suggested_min_closes}")
    print(f"    walk_forward_holdout_fraction:     {suggested_holdout:.2f}")


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
        default=float(os.getenv("SCOUT_MIN_WQS_ACTIVE", str(DEFAULT_MIN_WQS_ACTIVE))),
        help=f"Minimum WQS score for ACTIVE status (default: {DEFAULT_MIN_WQS_ACTIVE}, or SCOUT_MIN_WQS_ACTIVE)"
    )
    
    parser.add_argument(
        "--min-wqs-candidate",
        type=float,
        default=float(os.getenv("SCOUT_MIN_WQS_CANDIDATE", str(DEFAULT_MIN_WQS_CANDIDATE))),
        help=f"Minimum WQS score for CANDIDATE status (default: {DEFAULT_MIN_WQS_CANDIDATE}, or SCOUT_MIN_WQS_CANDIDATE)"
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
        "--min-closes-required",
        type=int,
        default=int(os.getenv("SCOUT_MIN_CLOSES_REQUIRED", "10")),
        help="Minimum realized closes (SELLs with PnL) required for promotion (default: 10, or SCOUT_MIN_CLOSES_REQUIRED)",
    )

    parser.add_argument(
        "--walk-forward-min-trades",
        type=int,
        default=int(os.getenv("SCOUT_WALK_FORWARD_MIN_TRADES", "5")),
        help="Minimum realized closes required in walk-forward holdout window (default: 5, or SCOUT_WALK_FORWARD_MIN_TRADES)",
    )
    
    parser.add_argument(
        "--verbose", "-v",
        action="store_true",
        help="Enable verbose output"
    )

    parser.add_argument(
        "--max-wallets",
        type=int,
        default=int(os.getenv("SCOUT_MAX_WALLETS", "50")),
        help="Max wallets to analyze (default: 50, or SCOUT_MAX_WALLETS env var)",
    )

    parser.add_argument(
        "--discovery-hours",
        type=int,
        default=int(os.getenv("SCOUT_DISCOVERY_HOURS", str(DEFAULT_DISCOVERY_HOURS))),
        help=f"Wallet discovery lookback window in hours (default: {DEFAULT_DISCOVERY_HOURS}, or SCOUT_DISCOVERY_HOURS)",
    )

    parser.add_argument(
        "--wallet-tx-limit",
        type=int,
        default=int(os.getenv("SCOUT_WALLET_TX_LIMIT", str(DEFAULT_WALLET_TX_LIMIT))),
        help=f"Max SWAP transactions to fetch per wallet (default: {DEFAULT_WALLET_TX_LIMIT}, or SCOUT_WALLET_TX_LIMIT)",
    )

    parser.add_argument(
        "--wallet-tx-max-pages",
        type=int,
        default=int(os.getenv("SCOUT_WALLET_TX_MAX_PAGES", str(DEFAULT_WALLET_TX_MAX_PAGES))),
        help=f"Max pagination pages per wallet tx fetch (default: {DEFAULT_WALLET_TX_MAX_PAGES}, or SCOUT_WALLET_TX_MAX_PAGES)",
    )

    parser.add_argument(
        "--priority-fee-sol",
        type=float,
        default=float(os.getenv("SCOUT_PRIORITY_FEE_SOL", str(DEFAULT_PRIORITY_FEE_SOL))),
        help=f"Priority fee cost per swap in SOL (default: {DEFAULT_PRIORITY_FEE_SOL}, or SCOUT_PRIORITY_FEE_SOL)",
    )

    parser.add_argument(
        "--jito-tip-sol",
        type=float,
        default=float(os.getenv("SCOUT_JITO_TIP_SOL", str(DEFAULT_JITO_TIP_SOL))),
        help=f"Jito tip cost per swap in SOL (default: {DEFAULT_JITO_TIP_SOL}, or SCOUT_JITO_TIP_SOL)",
    )

    parser.add_argument(
        "--calibration-report",
        action="store_true",
        help="Print calibration percentiles and suggested thresholds",
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

            # Get historical trades once (used for stats + optional backtest)
            trades = analyzer.get_historical_trades(wallet_address, days=30)
            
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
            wallet_stats = analyzer.compute_wallet_trade_stats(trades)
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
                avg_win_sol=wallet_stats.get("avg_win_sol"),
                avg_loss_sol=wallet_stats.get("avg_loss_sol"),
                profit_factor=wallet_stats.get("profit_factor"),
                realized_pnl_30d_sol=wallet_stats.get("realized_pnl_30d_sol"),
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
    
    # Print configuration summary if config module available
    if CONFIG_AVAILABLE and ScoutConfig:
        ScoutConfig.print_config_summary()
        print()
    
    # Initialize components
    try:
        # Ensure env-driven knobs are set for deeper modules (Analyzer/HeliusClient)
        os.environ["SCOUT_DISCOVERY_HOURS"] = str(args.discovery_hours)
        os.environ["SCOUT_WALLET_TX_LIMIT"] = str(args.wallet_tx_limit)
        os.environ["SCOUT_WALLET_TX_MAX_PAGES"] = str(args.wallet_tx_max_pages)
        
        # Get configuration (use config module if available, else fallback to env)
        if CONFIG_AVAILABLE and ScoutConfig:
            liquidity_mode = ScoutConfig.get_liquidity_mode()
            helius_api_key = ScoutConfig.get_helius_api_key()
        else:
            liquidity_mode = os.getenv("SCOUT_LIQUIDITY_MODE", "real").lower()
            helius_api_key = os.getenv("HELIUS_API_KEY")
            if not helius_api_key:
                # Try to extract from RPC URL
                rpc_url = os.getenv("CHIMERA_RPC__PRIMARY_URL") or os.getenv("SOLANA_RPC_URL", "")
                if "api-key=" in rpc_url:
                    helius_api_key = rpc_url.split("api-key=")[1].split("&")[0].split("?")[0]
        
        if liquidity_mode == "simulated":
            print("[Scout] WARNING: Running with simulated liquidity mode - results are non-deterministic!")
            print("[Scout] Set SCOUT_LIQUIDITY_MODE=real and provide BIRDEYE_API_KEY for production use")
        
        analyzer = WalletAnalyzer(
            helius_api_key=helius_api_key,
            discover_wallets=True,  # Enable wallet discovery from on-chain data
            max_wallets=args.max_wallets,
        )
    except Exception as e:
        print(f"[Scout] ERROR: Failed to initialize analyzer: {e}")
        sys.exit(1)
    
    # Initialize validator if not skipping backtest
    validator = None
    if not args.skip_backtest:
        try:
            # Initialize liquidity provider with configuration
            if CONFIG_AVAILABLE and ScoutConfig:
                liquidity_mode = ScoutConfig.get_liquidity_mode()
                cache_ttl = ScoutConfig.get_liquidity_cache_ttl()
                birdeye_key = ScoutConfig.get_birdeye_api_key()
                dexscreener_key = ScoutConfig.get_dexscreener_api_key()
            else:
                liquidity_mode = os.getenv("SCOUT_LIQUIDITY_MODE", "real").lower()
                cache_ttl = int(os.getenv("SCOUT_LIQUIDITY_CACHE_TTL_SECONDS", "60"))
                birdeye_key = os.getenv("BIRDEYE_API_KEY")
                dexscreener_key = os.getenv("DEXSCREENER_API_KEY")
            
            liquidity_provider = LiquidityProvider(
                mode=liquidity_mode,
                cache_ttl_seconds=cache_ttl,
                birdeye_api_key=birdeye_key,
                dexscreener_api_key=dexscreener_key,
            )
            backtest_config = BacktestConfig(
                min_liquidity_shield_usd=args.min_liquidity_shield,
                min_liquidity_spear_usd=args.min_liquidity_spear,
                dex_fee_percent=0.003,
                max_slippage_percent=0.05,
                min_trades_required=5,
                priority_fee_sol_per_trade=args.priority_fee_sol,
                jito_tip_sol_per_trade=args.jito_tip_sol,
                enforce_current_liquidity=os.getenv("SCOUT_ENFORCE_CURRENT_LIQUIDITY", "true").lower() == "true",
            )
            promotion_criteria = PromotionCriteria(
                # Keep WQS threshold aligned with ACTIVE gate (validator only runs for ACTIVE candidates)
                # Note: min_wqs_score should match min_wqs_active (rescaled 0-100 range)
                min_wqs_score=args.min_wqs_active,
                min_trades=5,  # Minimum raw swap events
                min_closes_required=args.min_closes_required,  # Minimum realized closes (SELLs with PnL)
                walk_forward_enabled=True,
                walk_forward_holdout_fraction=0.3,
                walk_forward_min_trades=args.walk_forward_min_trades,
            )
            validator = PrePromotionValidator(
                liquidity_provider=liquidity_provider,
                backtest_config=backtest_config,
                promotion_criteria=promotion_criteria,
            )
            print(f"[Scout] Backtest validation enabled")
            print(f"  Min liquidity (Shield): ${args.min_liquidity_shield:,.0f}")
            print(f"  Min liquidity (Spear): ${args.min_liquidity_spear:,.0f}")
            print(f"  Min closes required: {args.min_closes_required}")
            print(f"  Walk-forward min closes: {args.walk_forward_min_trades}")
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

    if args.calibration_report or args.verbose or args.dry_run:
        _calibration_report(records, stats)
    
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
            
            # Automatically merge roster into main database
            print(f"\n[Scout] Automatically merging roster into main database...")
            merge_success, merge_message = auto_merge_roster(
                roster_path=str(output_path),
                api_url=os.getenv("CHIMERA_API_URL", "http://localhost:8080"),
                operator_container=os.getenv("CHIMERA_OPERATOR_CONTAINER", "chimera-operator"),
                prefer_api=True,
                retries=3,
            )
            
            if merge_success:
                print(f"[Scout] ✓ {merge_message}")
            else:
                print(f"[Scout] ⚠ Automatic merge failed: {merge_message}")
                print(f"[Scout] You can manually merge with:")
                print(f"  kill -HUP $(pgrep chimera_operator)")
                print(f"  OR call POST /api/v1/roster/merge")
        except Exception as e:
            print(f"[Scout] ERROR: Failed to write roster: {e}")
            sys.exit(1)
    
    print(f"\n[Scout] Finished at: {datetime.utcnow().isoformat()}")
    print("=" * 70)


if __name__ == "__main__":
    main()
