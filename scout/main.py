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
import json
import os
import sqlite3
import sys
from datetime import datetime, timezone
from decimal import Decimal
from pathlib import Path
from typing import List, Optional, Tuple, Dict, Any
import asyncio

# Add parent directory to path for imports
sys.path.insert(0, str(Path(__file__).parent))

from core.db_writer import WalletRecord, write_roster_atomic
from core.wqs import calculate_wqs, calculate_wqs_with_confidence, \
    _calculate_raw_score, _interpret_trajectory, _compute_wmi, RawScoreComponents
from core.analyzer import WalletAnalyzer
from core.models import BacktestConfig
from core.validator import PrePromotionValidator, PromotionCriteria
from core.liquidity import LiquidityProvider
from core.auto_merge import auto_merge_roster
from core.metrics import get_metrics
from core.cost_estimator import CostEstimator
from core.clustering import cluster_and_dedup
from core.correlation_reader import CorrelationReader

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
DEFAULT_MIN_WQS_ACTIVE = 65.0  # Must match PromotionCriteria.min_wqs_score in validator.py; config module default is 60.0
DEFAULT_MIN_WQS_CANDIDATE = 15.0  # Lowered from 20.0 to capture more emerging wallets during discovery
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
        default=int(os.getenv("SCOUT_MAX_WALLETS", "250")),
        help="Max wallets to analyze (default: 250, or SCOUT_MAX_WALLETS env var; set to 200-500 for paid Helius plans)",
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


async def analyze_wallets(
    analyzer: WalletAnalyzer,
    validator: Optional[PrePromotionValidator],
    min_wqs_active: float,
    min_wqs_candidate: float,
    skip_backtest: bool = False,
    verbose: bool = False,
) -> Tuple[List[WalletRecord], dict]:
    """
    Analyze wallets in parallel and generate roster records.
    """
    records = []
    stats = {
        "total": 0, "active": 0, "candidate": 0, "rejected": 0,
        "backtest_passed": 0, "backtest_failed": 0, "backtest_skipped": 0,
        "trajectory_demotions": 0, "trajectory_peak_blocks": 0,
    }
    exit_recs: List[Dict[str, Any]] = []
    
    candidates = analyzer.get_candidate_wallets()
    stats["total"] = len(candidates)
    
    print(f"[Scout] Analyzing {len(candidates)} candidate wallets (Parallel, max 10 concurrent)...")

    # Define a single wallet processor function (async)
    async def process_wallet(wallet_address):
        try:
            print(f"[Scout] Starting analysis for {wallet_address[:8]}...")
            metrics = await analyzer.get_wallet_metrics(wallet_address)
            if metrics is None:
                print(f"[Scout] No metrics for {wallet_address[:8]}... (skipped)")
                return None
            
            print(f"[Scout] Computing WQS for {wallet_address[:8]}...")
            try:
                wqs_score = calculate_wqs(metrics)
                print(f"[Scout] WQS calculated: {wqs_score:.1f}")
            except Exception as e:
                print(f"[Scout] ✗ ERROR calculating WQS for {wallet_address[:8]}...: {e}")
                import traceback
                traceback.print_exc()
                return None
            
            print(f"[Scout] Getting trades from cache for {wallet_address[:8]}...")
            # Get trades from cache (already fetched during metrics calculation)
            trades = analyzer._trades_cache.get(wallet_address, [])
            print(f"[Scout] Got {len(trades)} trades from cache")

            # Phase 3c: Determine strategy from archetype
            _archetype = None
            try:
                _archetype_enum = analyzer.determine_archetype(metrics, trades)
                _archetype = _archetype_enum.value if _archetype_enum else None
            except Exception:
                _archetype = None
            _strategy = "SPEAR" if _archetype in ("WHALE", "SWING") else "SHIELD"
            
            # Get raw components for correlation tracking
            raw_components = _calculate_raw_score(metrics, strategy=_strategy)
            
            # Step 2: Multi-TF trajectory interpretation
            trajectory = _interpret_trajectory(metrics.roi_7d, metrics.roi_30d)
            wmi = _compute_wmi(metrics.roi_7d, metrics.roi_30d, metrics.trade_count_30d)
            
            # Initial Status
            if wqs_score >= min_wqs_active:
                initial_status = "ACTIVE"
            elif wqs_score >= min_wqs_candidate:
                initial_status = "CANDIDATE"
            else:
                initial_status = "REJECTED"
            
            # Step 2: Trajectory-based status adjustments
            if trajectory == "PEAKED" and initial_status in ("ACTIVE", "CANDIDATE"):
                initial_status = "CANDIDATE"
                stats["trajectory_peak_blocks"] += 1
                print(f"[Scout] {wallet_address[:8]}... PEAKED trajectory, "
                      f"blocking promotion (WQS={wqs_score:.1f})")
            elif trajectory == "DECLINING":
                if initial_status == "ACTIVE":
                    initial_status = "CANDIDATE"
                    stats["trajectory_demotions"] += 1
                    print(f"[Scout] {wallet_address[:8]}... DECLINING trajectory, "
                          f"demoting to CANDIDATE (WQS={wqs_score:.1f}, WMI={wmi:.2f})")
                # Step 5: Exit recommendation for DECLINING wallets with high WQS
                if wqs_score >= min_wqs_active:
                    exit_recs.append({
                        "wallet": wallet_address,
                        "reason": f"WMI={wmi:.2f}, trajectory=DECLINING, "
                                  f"roi_7d={metrics.roi_7d}, roi_30d={metrics.roi_30d}",
                        "timestamp": datetime.utcnow().isoformat(),
                        "recommended_action": "EXIT_ALL",
                    })
            
            # Performance degradation check: if an ACTIVE wallet shows signs of
            # decay, demote to CANDIDATE regardless of historical WQS.
            if initial_status == "ACTIVE" and _check_performance_degradation(metrics):
                initial_status = "CANDIDATE"
                print(f"[Scout] {wallet_address[:8]}... WQS={wqs_score:.1f} but "
                      f"degradation detected (7d ROI={metrics.roi_7d}), demoting to CANDIDATE")
            
            print(f"[Scout] {wallet_address[:8]}... WQS={wqs_score:.1f} Status={initial_status}")
            
            # Validation / Backtest logic
            final_status = initial_status
            backtest_res = {"status": "SKIPPED", "notes": None}
            
            if initial_status == "ACTIVE" and not skip_backtest and validator:
                if trades:
                    validation = await validator.validate_for_promotion(
                        wallet_address, metrics, trades, strategy=_strategy
                    )
                    if validation.passed:
                        backtest_res = {"status": "PASSED", "notes": validation.notes}
                    else:
                        final_status = "CANDIDATE" # Demote
                        backtest_res = {"status": "FAILED", "notes": validation.reason}
                else:
                    final_status = "CANDIDATE"
                    backtest_res = {"status": "SKIPPED", "notes": "No trades"}
            
            print(f"[Scout] Computing wallet stats for {wallet_address[:8]}...")
            try:
                wallet_stats = analyzer.compute_wallet_trade_stats(trades)
                print("[Scout] Wallet stats computed")
            except Exception as e:
                print(f"[Scout] ✗ ERROR computing wallet stats for {wallet_address[:8]}...: {e}")
                import traceback
                traceback.print_exc()
                return None
            
            result = {
                "address": wallet_address,
                "metrics": metrics,
                "wqs": wqs_score,
                "status": final_status,
                "backtest": backtest_res,
                "trades": trades,
                "wallet_stats": wallet_stats,
                "components": raw_components,
                "trajectory": trajectory,
                "wmi": wmi,
                "strategy": _strategy,
                "archetype": _archetype,
            }
            
            # MEMORY FIX: Clear analyzer cache for this wallet immediately
            # We have extracted everything we need into 'result'
            analyzer.clear_wallet_cache(wallet_address)
            print(f"[Scout] ✓ Completed {wallet_address[:8]}... (WQS={wqs_score:.1f}, Status={final_status})")
            return result
        except Exception as e:
            print(f"[Scout] ✗ ERROR processing {wallet_address[:8]}...: {e}")
            # Ensure cleanup happens even on error
            analyzer.clear_wallet_cache(wallet_address)
            return None

    # Run in parallel using asyncio (with semaphore for rate limiting)
    semaphore = asyncio.Semaphore(min(10, len(candidates)))
    
    async def process_with_semaphore(wallet_address):
        async with semaphore:
            try:
                # Add timeout per wallet: 5 minutes max
                return await asyncio.wait_for(process_wallet(wallet_address), timeout=300)
            except asyncio.TimeoutError:
                print(f"[Scout] TIMEOUT: {wallet_address[:8]}... took >5 minutes, skipping")
                return None
    
    # Process all wallets concurrently
    print(f"[Scout] Creating {len(candidates)} concurrent tasks...")
    tasks = [asyncio.ensure_future(process_with_semaphore(w)) for w in candidates]
    print("[Scout] Waiting for all tasks to complete...")
    # Use explicit per-task exception handling instead of return_exceptions=True
    # so that individual wallet failures don't silently drop exceptions.
    for i, task in enumerate(tasks):
        addr = candidates[i] if i < len(candidates) else "?"
        task.add_done_callback(
            lambda t, a=addr: print(
                f"[Scout] ✗ Task crashed for {a[:8]}: {t.exception()}"
            ) if t.exception() and not t.cancelled() else None
        )
    done, _ = await asyncio.wait(tasks)
    results = []
    for task in done:
        try:
            results.append(task.result())
        except Exception as e:
            print(f"[Scout] ✗ Wallet task failed: {e}")
            results.append(None)
    print("[Scout] All tasks completed, processing results...")
    
    for res in results:
        if isinstance(res, Exception):
            if verbose:
                print(f"[Scout] ERROR: {res}")
            continue
        if not res:
            continue
        
        # Unpack results and update stats
        wallet_addr = res['address']
        wqs = res['wqs']
        status = res['status']
        
        # Update counters
        if status == "ACTIVE":
            stats["active"] += 1
        elif status == "CANDIDATE":
            stats["candidate"] += 1
        else:
            stats["rejected"] += 1
        
        bt_status = res['backtest']['status']
        if bt_status == "PASSED":
            stats["backtest_passed"] += 1
        elif bt_status == "FAILED":
            stats["backtest_failed"] += 1
        elif bt_status == "SKIPPED" and status == "ACTIVE":
            stats["backtest_skipped"] += 1

        # Console output
        print(f"  [{status}] {wallet_addr[:8]}... WQS: {wqs:.1f}")

        # Build record
        notes_parts = [f"WQS: {wqs:.1f}"]
        if res['backtest']['notes']:
            notes_parts.append(f"Backtest: {res['backtest']['notes']}")
        notes_parts.append(f"Analyzed at {datetime.utcnow().isoformat()}")

        # Determine archetype
        archetype = res.get('archetype')
        
        record = WalletRecord(
            address=wallet_addr,
            status=status,
            wqs_score=wqs,
            roi_7d=res['metrics'].roi_7d,
            roi_30d=res['metrics'].roi_30d,
            trade_count_30d=res['metrics'].trade_count_30d,
            win_rate=res['metrics'].win_rate,
            max_drawdown_30d=res['metrics'].max_drawdown_30d,
            avg_trade_size_sol=res['metrics'].avg_trade_size_sol,
            avg_win_sol=res['wallet_stats'].get("avg_win_sol"),
            avg_loss_sol=res['wallet_stats'].get("avg_loss_sol"),
            profit_factor=res['wallet_stats'].get("profit_factor"),
            realized_pnl_30d_sol=res['wallet_stats'].get("realized_pnl_30d_sol"),
            last_trade_at=res['metrics'].last_trade_at,
            notes=" | ".join(notes_parts),
            archetype=archetype,
            avg_entry_delay_seconds=res['metrics'].avg_entry_delay_seconds,
        )
        records.append(record)
    
    # Archetype diversification: ensure each trading style has minimum representation
    # among ACTIVE wallets. Prevents a homogeneous roster (e.g., all scalpers).
    _apply_archetype_diversification(records, min_wqs_active)
    
    # Wallet clustering/deduplication: group wallets by shared funder and keep
    # only the top-WQS wallet per cluster to prevent correlated risk.
    if os.getenv("SCOUT_CLUSTER_DEDUP", "true").lower() == "true":
        try:
            records = await cluster_and_dedup(records, helius_client=analyzer.helius_client)
        except Exception as e:
            print(f"[Scout] Clustering dedup skipped ({e})")
    
    # Step 4: Cluster ensemble scoring — penalize wallets in losing clusters
    if os.getenv("SCOUT_CLUSTER_ENSEMBLE", "true").lower() == "true" and len([r for r in records if r.status == "ACTIVE"]) > 1:
        try:
            from core.cluster_ensemble import compute_cluster_scores, apply_cluster_adjustment
            
            cluster_data = {}
            for r in records:
                cid = getattr(r, 'cluster_id', None) or r.address
                cluster_data[r.address] = {"cluster_id": cid}
            
            cluster_metrics = compute_cluster_scores(
                [{"address": r.address, "wqs_score": r.wqs_score,
                  "roi_30d": r.roi_30d, "profit_factor": r.profit_factor}
                 for r in records],
                cluster_data
            )
            
            adjusted_count = 0
            for r in records:
                old_score = r.wqs_score
                r.wqs_score = apply_cluster_adjustment(
                    r.wqs_score, r.address,
                    cluster_data=cluster_data,
                    cluster_metrics=cluster_metrics,
                )
                if r.wqs_score != old_score:
                    adjusted_count += 1
            if adjusted_count > 0:
                print(f"[Scout] Cluster ensemble: adjusted {adjusted_count} wallet WQS scores")
        except Exception as e:
            print(f"[Scout] Cluster ensemble skipped ({e})")
    
    # Step 3c: Write correlation records for promoted ACTIVE wallets
    for res in results:
        if not res or res.get('status') != "ACTIVE":
            continue
        wallet_addr = res['address']
        components = res.get('components')
        strategy = res.get('strategy', 'SHIELD')
        if components:
            _write_correlation_record(
                wallet_addr, res['wqs'],
                components.components_json, strategy
            )
    
    # Step 5b: Write exit recommendations to JSON file
    if exit_recs:
        _write_exit_recommendations(exit_recs)
    
    return records, stats


def _apply_archetype_diversification(records: List[WalletRecord], min_wqs_active: float) -> None:
    """
    Stratified selection: ensure each trader archetype (SCALPER, SWING, WHALE)
    gets at least the configured minimum fraction of ACTIVE slots.
    
    Promotes the highest-WQS CANDIDATE wallets of underrepresented archetypes
    to ACTIVE, up to the minimum quota. This prevents Scout from producing
    a homogeneous roster that amplifies correlated risk.
    
    Modifies records in-place.
    """
    try:
        from config import ScoutConfig
        diversity_min_pct = ScoutConfig.get_archetype_diversity_min_pct()
    except ImportError:
        diversity_min_pct = float(os.getenv("SCOUT_ARCHETYPE_DIVERSITY_MIN_PCT", "0.2"))
    
    if diversity_min_pct <= 0:
        return
    
    active_records = [r for r in records if r.status == "ACTIVE"]
    if len(active_records) < 3 or len(active_records) > 100:
        return
    
    candidate_records = [r for r in records if r.status == "CANDIDATE"]
    
    active_by_archetype: Dict[str, List[WalletRecord]] = {}
    candidate_by_archetype: Dict[str, List[WalletRecord]] = {}
    
    for r in active_records:
        arch = r.archetype or "UNKNOWN"
        active_by_archetype.setdefault(arch, []).append(r)
    
    for r in candidate_records:
        arch = r.archetype or "UNKNOWN"
        candidate_by_archetype.setdefault(arch, []).append(r)
    
    total_active = len(active_records)
    min_per_archetype = int(max(1, total_active * diversity_min_pct))
    
    target_archetypes = {"SCALPER", "SWING", "WHALE"}
    promoted_count = 0
    
    for arch in target_archetypes:
        current = len(active_by_archetype.get(arch, []))
        if current >= min_per_archetype:
            continue
        
        candidates = sorted(
            candidate_by_archetype.get(arch, []),
            key=lambda r: r.wqs_score or 0,
            reverse=True,
        )
        
        slots_needed = min_per_archetype - current
        for c in candidates[:slots_needed]:
            if (c.wqs_score or 0) >= min_wqs_active * 0.85:
                c.status = "ACTIVE"
                promoted_count += 1
                active_by_archetype.setdefault(arch, []).append(c)
    
    if promoted_count > 0:
        print(f"[Scout] Archetype diversification: promoted {promoted_count} CANDIDATE wallets to ACTIVE "
              f"(min {diversity_min_pct*100:.0f}% per archetype, {min_per_archetype} slot(s) each)")


def _check_performance_degradation(metrics) -> bool:
    """
    Detect when a previously-ACTIVE wallet's recent performance has degraded.
    
    Returns True if:
    - 7d ROI is negative AND last trade was > 7 days ago (stale + negative trend)
    - 7d ROI is significantly negative (< -15%) regardless of recency (sharp decline)
    """
    seven_d_roi = metrics.roi_7d
    last_trade = metrics.last_trade_at

    if seven_d_roi is not None and seven_d_roi < 0:
        if last_trade:
            try:
                last_trade_dt = datetime.fromisoformat(last_trade.replace("Z", "+00:00"))
                now = datetime.now(timezone.utc)
                if last_trade_dt.tzinfo is None:
                    now = now.replace(tzinfo=None)
                days_since = (now - last_trade_dt).days
                if days_since > 7:
                    return True
            except (ValueError, TypeError):
                pass
        
        if seven_d_roi < -15.0:
            return True

    return False


def _write_correlation_record(
    wallet_address: str,
    wqs_score: float,
    components_json_str: str,
    strategy: str,
) -> None:
    """
    INSERT OR REPLACE into wqs_pnl_correlation table in the MAIN database.
    
    Writes the fields the Scout owns: wallet_address, wqs_score_at_promotion,
    wqs_components_json, promoted_at, strategy, last_updated_at.
    Actual PnL fields (actual_copy_pnl_*) stay NULL until the Operator UPDATEs them.
    """
    db_path = os.getenv("CHIMERA_DB_PATH", "../data/chimera.db")
    try:
        conn = sqlite3.connect(db_path, timeout=10.0)
        conn.execute("PRAGMA journal_mode=WAL;")
        now = datetime.utcnow().isoformat()
        conn.execute(
            """INSERT OR REPLACE INTO wqs_pnl_correlation
               (wallet_address, wqs_score_at_promotion, wqs_components_json,
                promoted_at, strategy, last_updated_at)
               VALUES (?, ?, ?, ?, ?, ?)""",
            (wallet_address, wqs_score, components_json_str, now, strategy, now),
        )
        conn.commit()
        conn.close()
    except sqlite3.OperationalError:
        pass


def _write_exit_recommendations(exit_recs: List[Dict[str, Any]]) -> None:
    """
    Write exit recommendations to data/exit_recommendations.json atomically.
    The Operator reads this file to trigger exits for declining ACTIVE wallets.
    """
    output_dir = os.getenv("SCOUT_DATA_DIR", os.path.join(os.path.dirname(__file__), "..", "data"))
    os.makedirs(output_dir, exist_ok=True)
    final_path = os.path.join(output_dir, "exit_recommendations.json")
    tmp_path = final_path + ".tmp"
    
    try:
        with open(tmp_path, "w") as f:
            json.dump(exit_recs, f, indent=2)
        os.rename(tmp_path, final_path)
        print(f"[Scout] Wrote {len(exit_recs)} exit recommendations to {final_path}")
    except Exception as e:
        print(f"[Scout] Failed to write exit recommendations: {e}")


async def main_async():
    """Async main entry point for the Scout."""
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
        
        # Use async factory for proper wallet discovery
        analyzer = await WalletAnalyzer.create(
            helius_api_key=helius_api_key,
            discover_wallets=True,  # Enable wallet discovery from on-chain data
            max_wallets=args.max_wallets,
        )
    except Exception as e:
        print(f"[Scout] ERROR: Failed to initialize analyzer: {e}")
        import traceback
        traceback.print_exc()
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
                enforce_current_liquidity=os.getenv("SCOUT_ENFORCE_CURRENT_LIQUIDITY", "true").lower() == "true",  # Default True for promotion safety
                simulate_at_size_sol=Decimal(os.getenv("SCOUT_COPIER_SIZE_SOL", "0")) if os.getenv("SCOUT_COPIER_SIZE_SOL") else None,
            )

            # Fetch dynamic fees from Helius if available (overrides static values)
            if helius_api_key and os.getenv("SCOUT_USE_DYNAMIC_FEES", "true").lower() == "true":
                try:
                    fee_estimator = CostEstimator(helius_api_key=helius_api_key)
                    dyn_prio, dyn_jito = await fee_estimator.get_all_estimates(strategy="SHIELD")
                    dyn_prio_float = float(dyn_prio)
                    dyn_jito_float = float(dyn_jito)
                    if dyn_prio_float > 0:
                        backtest_config.priority_fee_sol_per_trade = dyn_prio
                        print(f"[Scout] Dynamic priority fee (p75 Shield): {dyn_prio_float:.8f} SOL")
                    if dyn_jito_float > 0:
                        backtest_config.jito_tip_sol_per_trade = dyn_jito
                        print(f"[Scout] Dynamic Jito tip: {dyn_jito_float:.8f} SOL")
                    backtest_config.use_dynamic_fees = True
                    print("[Scout] Dynamic fee estimation enabled (source: Helius getPriorityFeeEstimate)")
                    await fee_estimator.close()
                except Exception as e:
                    print(f"[Scout] Warning: Dynamic fee fetch failed ({e}), using static fees")
            promotion_criteria = PromotionCriteria(
                # Keep WQS threshold aligned with ACTIVE gate (validator only runs for ACTIVE candidates)
                # Note: min_wqs_score should match min_wqs_active (rescaled 0-100 range)
                min_wqs_score=args.min_wqs_active,
                min_trades=5,  # Minimum raw swap events
                min_close_ratio=0.4,  # At least 40% of trades must be SELLs with PnL
                walk_forward_enabled=True,
                walk_forward_holdout_fraction=0.3,
                walk_forward_min_trades=args.walk_forward_min_trades,
            )
            validator = PrePromotionValidator(
                liquidity_provider=liquidity_provider,
                backtest_config=backtest_config,
                promotion_criteria=promotion_criteria,
            )
            print("[Scout] Backtest validation enabled")
            print(f"  Min liquidity (Shield): ${args.min_liquidity_shield:,.0f}")
            print(f"  Min liquidity (Spear): ${args.min_liquidity_spear:,.0f}")
            print("  Min close ratio: 0.4 (40% of trades must be SELLs with PnL)")
            print(f"  Walk-forward min closes: {args.walk_forward_min_trades}")
        except Exception as e:
            print(f"[Scout] WARNING: Failed to initialize validator: {e}")
            print("[Scout] Continuing without backtest validation")
    else:
        print("[Scout] Backtest validation: DISABLED")
    
    # Initialize metrics if enabled
    metrics = get_metrics()
    if metrics:
        metrics.start_server()
    
    # Analyze wallets
    print("\n[Scout] Analyzing wallets...")
    print(f"  Min WQS for ACTIVE: {args.min_wqs_active}")
    print(f"  Min WQS for CANDIDATE: {args.min_wqs_candidate}")
    
    import time
    analysis_start = time.time()
    records, stats = await analyze_wallets(
        analyzer,
        validator,
        args.min_wqs_active,
        args.min_wqs_candidate,
        skip_backtest=args.skip_backtest,
        verbose=args.verbose,
    )

    analysis_duration = time.time() - analysis_start
    
    # Update metrics
    if metrics:
        metrics.update_wqs_metrics(records)
        metrics.update_archetype_counts(records)
        metrics.increment_wallets_analyzed(stats['total'])
        metrics.record_analysis_duration(analysis_duration)
        
        # Calculate total unrealized PnL from records (if available in future)
        # For now, this would require adding unrealized_pnl to WalletRecord
        # total_unrealized = sum(r.total_unrealized_loss_sol or 0.0 for r in records if r.status == "ACTIVE")
        # metrics.update_unrealized_pnl(total_unrealized)
    
    if args.calibration_report or args.verbose or args.dry_run:
        _calibration_report(records, stats)

    # Phase 3a: Print WQS-to-PnL correlation summary (if data exists)
    try:
        corr_reader = CorrelationReader()
        if corr_reader.table_exists():
            corr_reader.print_correlation_summary()
    except Exception as e:
        if args.verbose:
            print(f"[Scout] Correlation reader skipped: {e}")

    # Step 3d: Adaptive weights calibration
    try:
        from core.adaptive_weights import AdaptiveWeightCalibrator
        calibrator = AdaptiveWeightCalibrator()
        new_weights = calibrator.calibrate_if_needed()
        if new_weights:
            print(f"[Scout] Adaptive weights calibrated: {len(new_weights)} components updated")
    except Exception as e:
        if args.verbose:
            print(f"[Scout] Adaptive weights skipped: {e}")

    # Print parse health dashboard (always in verbose/dry-run, otherwise only if >0 failures)
    if args.verbose or args.dry_run or stats["total"] > 0:
        analyzer.print_parse_health_dashboard()

    # Summary
    print("\n[Scout] Analysis complete:")
    print(f"  Total analyzed: {stats['total']}")
    print(f"  ACTIVE: {stats['active']}")
    print(f"  CANDIDATE: {stats['candidate']}")
    print(f"  REJECTED: {stats['rejected']}")
    if stats.get('trajectory_demotions', 0) > 0 or stats.get('trajectory_peak_blocks', 0) > 0:
        print(f"  Trajectory demotions: {stats['trajectory_demotions']}")
        print(f"  Peak blocks: {stats['trajectory_peak_blocks']}")
    
    if not args.skip_backtest:
        print("\n[Scout] Backtest results:")
        print(f"  Passed: {stats['backtest_passed']}")
        print(f"  Failed: {stats['backtest_failed']}")
        print(f"  Skipped: {stats['backtest_skipped']}")
    
    # Write output
    if args.dry_run:
        print("\n[Scout] Dry run mode - not writing to database")
    else:
        output_path = Path(args.output)
        
        # Ensure parent directory exists
        output_path.parent.mkdir(parents=True, exist_ok=True)
        
        print(f"\n[Scout] Writing roster to {output_path}...")
        
        try:
            write_roster_atomic(records, str(output_path))
            print(f"[Scout] Successfully wrote {len(records)} wallets")
            
            # Automatically merge roster into main database
            print("\n[Scout] Automatically merging roster into main database...")
            
            # NEW CODE: Wrap in try/except to prevent crash if Operator is down
            try:
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
                    print("[Scout] Non-fatal error: Roster is saved on disk.")
            except Exception as merge_err:
                print(f"[Scout] ⚠ Exception during auto-merge: {merge_err}")
                print("[Scout] Non-fatal error: Roster is saved on disk.")
        except Exception as e:
            print(f"[Scout] ERROR: Failed to write roster: {e}")
            sys.exit(1)


def main():
    """Main entry point for the Scout (sync wrapper for async main)."""
    try:
        asyncio.run(main_async())
    except KeyboardInterrupt:
        print("\n[Scout] Interrupted by user")
        sys.exit(1)
    except Exception as e:
        print(f"[Scout] Fatal error: {e}")
        import traceback
        traceback.print_exc()
        sys.exit(1)
    
    print(f"\n[Scout] Finished at: {datetime.utcnow().isoformat()}")
    print("=" * 70)


if __name__ == "__main__":
    main()
