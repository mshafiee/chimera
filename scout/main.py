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

from __future__ import annotations

from _version import __version__

import argparse
import json
import logging
import logging.handlers
import math
import os
import sqlite3
import sys
from datetime import datetime, timezone
from decimal import Decimal
from pathlib import Path
from typing import List, Optional, Tuple, Dict, Any, TYPE_CHECKING
import asyncio

if TYPE_CHECKING:
    from core.scout_optimizer import ScoutOptimizer
from dotenv import load_dotenv

load_dotenv(Path(__file__).parent / '.env')

# Fix stdout buffering so output is visible when piped or in long-running processes
sys.stdout.reconfigure(line_buffering=True)

# Add parent directory to path for imports
sys.path.insert(0, str(Path(__file__).parent))

# ruff: noqa: E402
from core.utils import utcnow

from core.db_writer import WalletRecord, write_roster_atomic
from core.wqs import calculate_wqs_with_confidence, \
    _calculate_raw_score, _interpret_trajectory, _compute_wmi
from core.analyzer import WalletAnalyzer
from core.models import BacktestConfig
from core.validator import PrePromotionValidator, PromotionCriteria
from core.liquidity import LiquidityProvider
from core.auto_merge import auto_merge_roster
from core.metrics import get_metrics
from core.cost_estimator import CostEstimator
from core.clustering import cluster_and_dedup
from core.correlation_reader import CorrelationReader
from core.feature_store import FeatureStore
# Import profit tracking for growth monitoring
try:
    from core.realtime_profit_tracker import RealtimeProfitTracker, TrackerConfig
    PROFIT_TRACKER_AVAILABLE = True
except ImportError:
    PROFIT_TRACKER_AVAILABLE = False
    RealtimeProfitTracker = None
    TrackerConfig = None
# Import optimization modules
try:
    from core.scout_optimizer import get_scout_optimizer
    from core.optimized_analyzer import OptimizedWalletAnalyzer
    OPTIMIZATION_AVAILABLE = True
except ImportError:
    OPTIMIZATION_AVAILABLE = False
    print("[Scout] Warning: Optimization modules not available")

# Import stop-loss optimization modules
try:
    from core.stop_loss_optimizer import StopLossOptimizer
    from core.position_manager import PositionManager
    from core.market_regime_detector import MarketRegimeDetector
    STOP_LOSS_OPTIMIZER_AVAILABLE = True
except ImportError:
    STOP_LOSS_OPTIMIZER_AVAILABLE = False
    print("[Scout] Warning: Stop-loss optimization modules not available")

# Import validation reporter for ML monitoring
try:
    from core.validation_reporter import ValidationReporter, AlertConfig
    VALIDATION_REPORTER_AVAILABLE = True
except ImportError:
    VALIDATION_REPORTER_AVAILABLE = False
    ValidationReporter = None
    AlertConfig = None
    print("[Scout] Warning: Validation Reporter not available")

# Import state persistence for cross-session learning
try:
    from core.state_persistence import StatePersistence, PersistenceConfig, CreditHistory, WalletPerformance, ROIMetrics
    STATE_PERSISTENCE_AVAILABLE = True
except ImportError:
    STATE_PERSISTENCE_AVAILABLE = False
    StatePersistence = None
    PersistenceConfig = None
    CreditHistory = None
    WalletPerformance = None
    ROIMetrics = None
    print("[Scout] Warning: State Persistence not available")

# Import signal quality filter for trade quality improvement
try:
    from core.signal_quality_filter import SignalQualityFilter, FilterConfig, TradingSignal
    SIGNAL_QUALITY_FILTER_AVAILABLE = True
except ImportError:
    SIGNAL_QUALITY_FILTER_AVAILABLE = False
    SignalQualityFilter = None
    FilterConfig = None
    TradingSignal = None
    print("[Scout] Warning: Signal Quality Filter not available")

# Import PredictiveBudgetManager for Helius API quota management
try:
    from core.predictive_budget_manager import PredictiveBudgetManager, BudgetCategory, CreditAlertLevel
    BUDGET_MANAGER_AVAILABLE = True
except ImportError:
    BUDGET_MANAGER_AVAILABLE = False
    PredictiveBudgetManager = None
    BudgetCategory = None
    CreditAlertLevel = None
    print("[Scout] Warning: PredictiveBudgetManager not available")

# Import advanced caching for RPC optimization (optional)
try:
    from core.advanced_cache import (
        get_wallet_metrics, set_wallet_metrics,
        get_liquidity_data, set_liquidity_data,
        get_high_wqs_wallet_data, set_high_wqs_wallet_data,
    )
    ADVANCED_CACHE_AVAILABLE = True
except ImportError:
    ADVANCED_CACHE_AVAILABLE = False
    print("[Scout] Warning: Advanced caching not available")

# Import HighConvictionIntegration for WQS-based prioritization
try:
    from integrations.high_conviction_integration import create_high_conviction_integration
    HIGH_CONVICTIION_AVAILABLE = True
except ImportError:
    HIGH_CONVICTIION_AVAILABLE = False
    create_high_conviction_integration = None
    print("[Scout] Warning: High-conviction integration not available")

# Import network features for graph-based wallet intelligence
try:
    from core.network_features import NetworkFeatures
    NETWORK_FEATURES_AVAILABLE = True
except ImportError:
    NETWORK_FEATURES_AVAILABLE = False
    NetworkFeatures = None
    print("[Scout] Warning: Network features not available (requires networkx)")

# Import time-series features for temporal pattern analysis
try:
    from core.time_series_features import TimeSeriesFeatures
    TIME_SERIES_FEATURES_AVAILABLE = True
except ImportError:
    TIME_SERIES_FEATURES_AVAILABLE = False
    TimeSeriesFeatures = None
    print("[Scout] Warning: Time-series features not available")

# Import advanced risk features for sophisticated risk management
try:
    from core.advanced_risk_features import AdvancedRiskFeatures
    ADVANCED_RISK_FEATURES_AVAILABLE = True
except ImportError:
    ADVANCED_RISK_FEATURES_AVAILABLE = False
    AdvancedRiskFeatures = None
    print("[Scout] Warning: Advanced risk features not available")

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
DEFAULT_MIN_WQS_ACTIVE = 75.0  # Must match PromotionCriteria.min_wqs_score in validator.py; config module default is 75.0
DEFAULT_MIN_WQS_CANDIDATE = 50.0  # Raised from 15.0 to maintain gap with new 75.0 ACTIVE threshold
DEFAULT_DISCOVERY_HOURS = 168
DEFAULT_WALLET_TX_LIMIT = 500
DEFAULT_WALLET_TX_MAX_PAGES = 20
DEFAULT_PRIORITY_FEE_SOL = 0.00005
DEFAULT_JITO_TIP_SOL = 0.0001


def setup_logging() -> None:
    """Configure file logging with rotation for Scout."""
    # Create log directory - use /app/data/logs for container permissions
    log_dir = "/app/data/logs"
    try:
        os.makedirs(log_dir, exist_ok=True)
    except Exception as e:
        print(f"Warning: Could not create log directory {log_dir}: {e}")
        # Fall back to stdout only
        return

    log_file = os.path.join(log_dir, "scout.log")

    try:
        # Configure root logger
        logger = logging.getLogger()
        logger.setLevel(logging.INFO)

        # File handler with daily rotation, 7-day retention
        file_handler = logging.handlers.RotatingFileHandler(
            log_file,
            maxBytes=10*1024*1024,  # 10MB per file
            backupCount=7,
            encoding='utf-8'
        )
        file_handler.setLevel(logging.INFO)
        file_formatter = logging.Formatter(
            '%(asctime)s - %(name)s - %(levelname)s - %(message)s',
            datefmt='%Y-%m-%d %H:%M:%S'
        )
        file_handler.setFormatter(file_formatter)

        # Console handler for immediate visibility
        console_handler = logging.StreamHandler()
        console_handler.setLevel(logging.INFO)
        console_formatter = logging.Formatter('[Scout] %(levelname)s: %(message)s')
        console_handler.setFormatter(console_formatter)

        # Add both handlers
        logger.addHandler(file_handler)
        logger.addHandler(console_handler)

        logger.info("File logging configured: %s", log_file)
    except Exception as e:
        print(f"Warning: Could not setup file logging: {e}")


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

    # PnL accuracy metric (from wqs_pnl_correlation table)
    try:
        from core.correlation_reader import CorrelationReader
        cr = CorrelationReader()
        if cr.table_exists():
            pnl_records = cr.get_all_records(min_trades=5)
            profitable = sum(1 for r in pnl_records if r.actual_copy_pnl_30d_sol is not None and r.actual_copy_pnl_30d_sol > 0)
            total_pnl = sum(1 for r in pnl_records if r.actual_copy_pnl_30d_sol is not None)
            if total_pnl > 0:
                accuracy_pct = (profitable / total_pnl) * 100
                print(f"\n  PnL accuracy: {profitable}/{total_pnl} promoted wallets profitable ({accuracy_pct:.1f}%)")
    except Exception:
        pass

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


from core.in_sample import compute_in_sample_metrics
from core.correlation_backfill import backfill_correlation_pnl, write_correlation_record
from core.degradation import check_performance_degradation


async def analyze_wallets(
    analyzer: WalletAnalyzer,
    validator: Optional[PrePromotionValidator],
    min_wqs_active: float,
    min_wqs_candidate: float,
    skip_backtest: bool = False,
    verbose: bool = False,
    optimizer: Optional['ScoutOptimizer'] = None,
    high_conviction: Optional['HighConvictionIntegration'] = None,
) -> Tuple[List[WalletRecord], dict, list]:
    """
    Analyze wallets in parallel and generate roster records.
    """
    # Macro kill switch: emergency pause prevents all new promotions
    if os.getenv("SCOUT_EMERGENCY_PAUSE", "false").lower() == "true":
        print("[Scout] EMERGENCY PAUSE ACTIVE — returning zero promotions")
        empty_stats = {
            "total": 0, "active": 0, "candidate": 0, "rejected": 0,
            "backtest_passed": 0, "backtest_failed": 0, "backtest_skipped": 0,
            "trajectory_demotions": 0, "trajectory_peak_blocks": 0,
        }
        return [], empty_stats, []

    records = []
    stats = {
        "total": 0, "active": 0, "candidate": 0, "rejected": 0,
        "backtest_passed": 0, "backtest_failed": 0, "backtest_skipped": 0,
        "trajectory_demotions": 0, "trajectory_peak_blocks": 0,
    }
    exit_recs: List[Dict[str, Any]] = []
    
    candidates = analyzer.get_candidate_wallets()
    stats["total"] = len(candidates)

    # Apply high-conviction prioritization if enabled
    if high_conviction and HIGH_CONVICTIION_AVAILABLE:
        print("[Scout] Applying high-conviction prioritization...")
        # Get WQS scores for prioritization (use cached metrics if available)
        wqs_scores = {}
        for addr in candidates:
            cached_metrics = analyzer._metrics_cache.get(addr)
            if cached_metrics and hasattr(cached_metrics, 'wqs_score'):
                wqs_scores[addr] = cached_metrics.wqs_score or 0
            else:
                wqs_scores[addr] = 0  # Unknown wallets get lowest priority

        candidates = high_conviction.prioritize_wallets_for_analysis(candidates, wqs_scores)

    print(f"[Scout] Analyzing {len(candidates)} candidate wallets (Parallel, max 10 concurrent)...")

    # Define a single wallet processor function (async)
    async def process_wallet(wallet_address):
        try:
            # Check high-conviction budget before processing
            if high_conviction and HIGH_CONVICTIION_AVAILABLE:
                # For budget checks, we need a preliminary WQS estimate
                # Use cached metrics or default to medium priority
                cached_metrics = analyzer._metrics_cache.get(wallet_address)
                estimated_wqs = cached_metrics.wqs_score if cached_metrics else 50.0

                can_analyze, reason = high_conviction.should_analyze_wallet(wallet_address, estimated_wqs)
                if not can_analyze:
                    print(f"[Scout] Skipping {wallet_address[:8]}...: {reason}")
                    return None

            print(f"[Scout] Starting analysis for {wallet_address[:8]}...")
            metrics = await analyzer.get_wallet_metrics(wallet_address)
            if metrics is None:
                print(f"[Scout] No metrics for {wallet_address[:8]}... (skipped)")
                return None

            print(f"[Scout] Getting trades from cache for {wallet_address[:8]}...")
            trades = analyzer._trades_cache.get(wallet_address, [])
            print(f"[Scout] Got {len(trades)} trades from cache")

            # --- Chronological split for clean WQS ---
            # In-sample: oldest 70% of trades for WQS scoring
            # Holdout: newest 30% (the validator does its own split for backtest)
            MAX_HEURISTIC_BOOST = float(os.getenv("SCOUT_MAX_HEURISTIC_BOOST", "10.0"))
            _SPLIT_ENABLED = os.getenv("SCOUT_SPLIT_WQS_ENABLED", "false").lower() == "true"
            wqs_metrics = metrics
            if _SPLIT_ENABLED and len(trades) >= 10:
                sorted_trades = sorted(trades, key=lambda t: t.timestamp)
                split_idx = int(len(sorted_trades) * 0.70)
                in_sample_trades = sorted_trades[:split_idx]
                _in_sample_m = compute_in_sample_metrics(analyzer, in_sample_trades, metrics)
                if _in_sample_m is not None and (_in_sample_m.trade_count_30d or 0) >= 5:
                    wqs_metrics = _in_sample_m
                    print(f"[Scout] Split WQS: in-sample={len(in_sample_trades)}t, "
                          f"holdout={len(sorted_trades) - split_idx}t, "
                          f"wqs_roi_30d={wqs_metrics.roi_30d:.1f}%")

            print(f"[Scout] Computing WQS for {wallet_address[:8]}...")
            try:
                wqs_result = calculate_wqs_with_confidence(wqs_metrics)
                wqs_score = wqs_result.score
                wqs_confidence = wqs_result.confidence
                print(f"[Scout] WQS calculated: {wqs_score:.1f} (confidence={wqs_confidence:.2f})")
            except Exception as e:
                print(f"[Scout] ✗ ERROR calculating WQS for {wallet_address[:8]}...: {e}")
                import traceback
                traceback.print_exc()
                return None

            # Shadow WQS comparison mode (dual-write: old vs new WQS)
            if os.getenv("SCOUT_WQS_COMPARISON_MODE", "false").lower() == "true":
                try:
                    from core.wqs_comparison import compute_comparison, append_to_log
                    if wqs_metrics is not metrics:
                        comp = compute_comparison(
                            wallet_address, metrics, wqs_metrics,
                            active_threshold=min_wqs_active,
                        )
                        append_to_log(comp)
                        if comp.promoted_by_new_only:
                            print(f"[Scout] SHADOW: {wallet_address[:8]}... promoted by NEW WQS "
                                  f"(old={comp.old_wqs:.1f} → new={comp.new_wqs:.1f})")
                        elif comp.demoted_by_new_only:
                            print(f"[Scout] SHADOW: {wallet_address[:8]}... demoted by NEW WQS "
                                  f"(old={comp.old_wqs:.1f} → new={comp.new_wqs:.1f})")
                        elif abs(comp.delta) > 5.0:
                            print(f"[Scout] SHADOW: {wallet_address[:8]}... WQS delta={comp.delta:+.1f} "
                                  f"(old={comp.old_wqs:.1f} → new={comp.new_wqs:.1f})")
                except Exception:
                    pass

            # Survivorship bias flag (informational)
            survivorship = analyzer._compute_survivorship_flag(trades)
            if survivorship == "FRESH_30D":
                sorted_t = sorted(trades, key=lambda t: t.timestamp)
                span = (sorted_t[-1].timestamp - sorted_t[0].timestamp).days
                print(f"[Scout] {wallet_address[:8]}... FRESH wallet "
                      f"(only {span}d of trade data — survivorship bias risk)")

            # ML-based profitability prediction for growth optimization
            heuristic_boost_used = 0.0
            ml_boost_applied = 0.0
            if optimizer and OPTIMIZATION_AVAILABLE and ScoutConfig.get_wqs_boost_enabled():
                try:
                    # Prepare wallet features for ML prediction (from wqs_metrics)
                    wallet_features = {
                        'roi_7d': wqs_metrics.roi_7d,
                        'roi_30d': wqs_metrics.roi_30d,
                        'win_rate': wqs_metrics.win_rate,
                        'profit_factor': wqs_metrics.profit_factor,
                        'sortino_ratio': wqs_metrics.sortino_ratio,
                        'max_drawdown_30d': wqs_metrics.max_drawdown_30d,
                        'trade_count_30d': wqs_metrics.trade_count_30d,
                        'avg_trade_size_sol': wqs_metrics.avg_trade_size_sol,
                    }

                    # Get ML prediction
                    prediction = optimizer.predict_profitability(wallet_features)

                    print(f"[Scout] ML Prediction: {prediction.profitability_class.value} "
                          f"(expected: {prediction.expected_return_pct:.1f}%, "
                          f"confidence: {prediction.confidence:.1f}, "
                          f"risk: {prediction.risk_score:.1f})")

                    # Apply growth optimization WQS boost (capped by MAX_HEURISTIC_BOOST)
                    if ScoutConfig.get_growth_optimized():
                        expected_return = prediction.expected_return_pct
                        prediction_confidence = prediction.confidence

                        if expected_return > 15 and prediction_confidence > 0.6:
                            boost_raw = min(8.0, expected_return / 4)
                            ml_boost_applied = min(boost_raw, MAX_HEURISTIC_BOOST - heuristic_boost_used)
                            if ml_boost_applied > 0:
                                wqs_score += ml_boost_applied
                                heuristic_boost_used += ml_boost_applied
                                print(f"[Scout] Growth boost: +{ml_boost_applied:.1f} WQS "
                                      f"(heuristic total: {heuristic_boost_used:.1f})")
                        elif expected_return > 10 and prediction_confidence > 0.5:
                            boost_raw = min(4.0, expected_return / 6)
                            ml_boost_applied = min(boost_raw, MAX_HEURISTIC_BOOST - heuristic_boost_used)
                            if ml_boost_applied > 0:
                                wqs_score += ml_boost_applied
                                heuristic_boost_used += ml_boost_applied
                                print(f"[Scout] Growth boost: +{ml_boost_applied:.1f} WQS "
                                      f"(heuristic total: {heuristic_boost_used:.1f})")

                except Exception as e:
                    print(f"[Scout] ML prediction failed: {e}")

            # Log the prediction for downstream matching against actual PnL
            if ml_boost_applied > 0 and optimizer and OPTIMIZATION_AVAILABLE:
                try:
                    from core.prediction_logger import PredictionLogger

                    # Convert expected_return_pct (percentage) to absolute SOL amount
                    # The downstream matcher compares against actual_copy_pnl_30d_sol (in SOL)
                    # Use avg_trade_size_sol as proxy for investment size
                    investment_size_sol = wqs_metrics.avg_trade_size_sol if wqs_metrics.avg_trade_size_sol and wqs_metrics.avg_trade_size_sol > 0 else 1.0
                    expected_return_sol = (prediction.expected_return_pct / 100.0) * investment_size_sol

                    plogger = PredictionLogger()
                    plogger.log_prediction(
                        wallet_address=wallet_address,
                        model_type="simple_ensemble",
                        # Normalize expected_return_pct to SOL units for proper matching
                        # against actual_copy_pnl_30d_sol (downstream comparison is now SOL to SOL)
                        predicted_pnl=expected_return_sol,
                        confidence=prediction.confidence,
                        features={
                            'roi_7d': wqs_metrics.roi_7d,
                            'roi_30d': wqs_metrics.roi_30d,
                            'win_rate': wqs_metrics.win_rate,
                            'profit_factor': wqs_metrics.profit_factor,
                            'trade_count_30d': wqs_metrics.trade_count_30d,
                        },
                    )
                except Exception as e:
                    print(f"[Scout] Prediction logging failed for {wallet_address[:8]}...: {e}")

            # Phase 3c: Determine strategy from archetype (from wqs_metrics)
            _archetype = None
            try:
                _archetype_enum = analyzer.determine_archetype(wqs_metrics, trades)
                _archetype = _archetype_enum.value if _archetype_enum else None
            except Exception:
                _archetype = None
            _strategy = "SPEAR" if _archetype in ("WHALE", "SWING") else "SHIELD"
            
            # Get raw components for correlation tracking (from wqs_metrics)
            raw_components = _calculate_raw_score(wqs_metrics, strategy=_strategy)
            
            # Step 2: Multi-TF trajectory interpretation (from wqs_metrics)
            trajectory = _interpret_trajectory(wqs_metrics.roi_7d, wqs_metrics.roi_30d)
            wmi = _compute_wmi(wqs_metrics.roi_7d, wqs_metrics.roi_30d, wqs_metrics.trade_count_30d)
            
            # Initial Status (with confidence gating for ACTIVE)
            if wqs_score >= min_wqs_active and wqs_confidence >= 0.70:
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
            elif trajectory == "IMPROVING":
                trajectory_boost = min(5.0, MAX_HEURISTIC_BOOST - heuristic_boost_used)
                if trajectory_boost > 0:
                    wqs_score += trajectory_boost
                    heuristic_boost_used += trajectory_boost
                if wqs_score >= min_wqs_active and wqs_confidence >= 0.70:
                    initial_status = "ACTIVE"
                print(f"[Scout] {wallet_address[:8]}... IMPROVING trajectory, "
                      f"+{trajectory_boost:.0f} WQS (heuristic total: {heuristic_boost_used:.1f}, "
                      f"WQS={wqs_score:.1f})")
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
                                  f"roi_7d={wqs_metrics.roi_7d}, roi_30d={wqs_metrics.roi_30d}",
                        "timestamp": utcnow().isoformat(),
                        "recommended_action": "EXIT_ALL",
                    })
            
            # Step 3: Composite alpha decay detection — catch wallets losing their edge
            # even when WQS and trajectory still look fine.
            # Combines win-rate decay, trade-size decay, and token-rotation decay.
            if initial_status == "ACTIVE":
                wr_decay = analyzer._calculate_alpha_decay(trades)
                sz_decay = analyzer._calculate_trade_size_decay(trades)
                rt_decay = analyzer._calculate_token_rotation_decay(trades)

                # Build composite decay from individual values (inlined to avoid
                # redundant calls inside _calculate_composite_decay)
                composite_decay = None
                parts = []
                if wr_decay is not None: parts.append((wr_decay, 0.5))
                if sz_decay is not None: parts.append((sz_decay, 0.3))
                if rt_decay is not None: parts.append((rt_decay, 0.2))
                if parts:
                    total_w = sum(w for _, w in parts)
                    composite_decay = max(0.0, min(1.0, sum(v * w for v, w in parts) / total_w))

                if composite_decay is not None:
                    single_signal_decay = (
                        (wr_decay is not None and wr_decay < 0.30) or
                        (sz_decay is not None and sz_decay < 0.25) or
                        (rt_decay is not None and rt_decay < 0.20)
                    )

                    if composite_decay < 0.60 or single_signal_decay:
                        initial_status = "CANDIDATE"
                        reason = "composite" if composite_decay < 0.60 else "single-signal"
                        print(f"[Scout] {wallet_address[:8]}... {reason} decay detected "
                              f"(composite={composite_decay:.2f}), demoting to CANDIDATE")
                    elif composite_decay < 0.70:
                        print(f"[Scout] {wallet_address[:8]}... borderline decay "
                              f"(composite={composite_decay:.2f}) — flagging for monitoring")
            
            # Performance degradation check: if an ACTIVE wallet shows signs of
            # decay, demote to CANDIDATE regardless of historical WQS.
            # Skip for IMPROVING wallets — they're accelerating, not decaying.
            if initial_status == "ACTIVE" and trajectory != "IMPROVING" and check_performance_degradation(wqs_metrics):
                initial_status = "CANDIDATE"
                print(f"[Scout] {wallet_address[:8]}... WQS={wqs_score:.1f} but "
                      f"degradation detected (7d ROI={wqs_metrics.roi_7d}), demoting to CANDIDATE")
            
            print(f"[Scout] {wallet_address[:8]}... WQS={wqs_score:.1f} Status={initial_status}")
            
            # Validation / Backtest logic
            final_status = initial_status
            backtest_res = {"status": "SKIPPED", "notes": None}

            if initial_status == "ACTIVE" and not skip_backtest and validator:
                # Credit-aware backtest validation check
                can_validate = True
                validation_reason = None

                if optimizer and OPTIMIZATION_AVAILABLE and ScoutConfig.get_credit_tracking_enabled():
                    try:
                        can_validate, validation_reason = optimizer.can_validate_backtest()
                        if not can_validate:
                            print(f"[Scout] Credit budget limit reached: {validation_reason}")
                            print(f"[Scout] Skipping backtest for {wallet_address[:8]}... (credit-aware)")
                            stats["backtest_skipped"] += 1
                            final_status = "CANDIDATE"
                            backtest_res = {"status": "SKIPPED", "notes": f"Credit: {validation_reason}"}
                    except Exception as e:
                        print(f"[Scout] Credit check failed, proceeding with backtest: {e}")

                if trades and can_validate:
                    validation = await validator.validate_for_promotion(
                        wallet_address, wqs_metrics, trades, strategy=_strategy
                    )
                    if validation.passed:
                        backtest_res = {"status": "PASSED", "notes": validation.notes}
                    else:
                        final_status = "CANDIDATE" # Demote
                        backtest_res = {"status": "FAILED", "notes": validation.reason}
                elif not can_validate:
                    # Already handled above by credit check
                    pass
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

            # Extract network features if available
            network_features = None
            if NETWORK_FEATURES_AVAILABLE and NetworkFeatures:
                try:
                    print(f"[Scout] Computing network features for {wallet_address[:8]}...")
                    network_extractor = NetworkFeatures()
                    # For network features, we need the broader context, but we can extract
                    # wallet-level features like token co-holding patterns
                    # Note: Full network analysis requires all wallets, done in post-processing
                    network_features = {
                        "wallet_address": wallet_address,
                        "token_co_holding_score": None,  # Placeholder for individual wallet analysis
                        "centrality_score": None,        # Requires full network graph
                        "sybil_score": None,            # Requires full network graph
                        "cluster_membership": None      # Requires full network graph
                    }
                    print("[Scout] Network features computed")
                except Exception as e:
                    print(f"[Scout] Warning: Network feature extraction failed for {wallet_address[:8]}...: {e}")
                    network_features = None

            # Extract time-series features if available
            time_series_features = None
            if TIME_SERIES_FEATURES_AVAILABLE and TimeSeriesFeatures and trades:
                try:
                    print(f"[Scout] Computing time-series features for {wallet_address[:8]}...")
                    ts_extractor = TimeSeriesFeatures(min_samples=3, max_samples=100)

                    # Build performance history from trades
                    performance_history = []
                    for trade in trades:
                        if trade.get('timestamp') and (trade.get('pnl_sol') or trade.get('pnl')):
                            performance_history.append({
                                'timestamp': trade.get('timestamp'),
                                'pnl_sol': trade.get('pnl_sol', trade.get('pnl', 0.0)),
                                'roi': trade.get('roi', 0.0)
                            })

                    if performance_history:
                        time_series_features = ts_extractor.extract_features(
                            performance_history, feature_set="all"
                        )
                        print("[Scout] Time-series features computed")
                    else:
                        print("[Scout] Insufficient trade history for time-series features")

                except Exception as e:
                    print(f"[Scout] Warning: Time-series feature extraction failed for {wallet_address[:8]}...: {e}")
                    time_series_features = None

            # Extract advanced risk features if available
            advanced_risk_features = None
            if ADVANCED_RISK_FEATURES_AVAILABLE and AdvancedRiskFeatures and trades:
                try:
                    print(f"[Scout] Computing advanced risk features for {wallet_address[:8]}...")
                    risk_extractor = AdvancedRiskFeatures(confidence_levels=[0.90, 0.95, 0.99])

                    # Build trade history for risk analysis
                    trade_history = []
                    for trade in trades:
                        if trade.get('pnl_sol') or trade.get('pnl'):
                            trade_history.append({
                                'pnl_sol': trade.get('pnl_sol', trade.get('pnl', 0.0)),
                                'pnl': trade.get('pnl', 0.0),
                                'timestamp': trade.get('timestamp'),
                            })

                    if len(trade_history) >= 5:  # Minimum sample requirement
                        advanced_risk_features = risk_extractor.extract_features(trade_history)
                        print("[Scout] Advanced risk features computed")
                    else:
                        print("[Scout] Insufficient trade history for advanced risk features (min 5 trades)")

                except Exception as e:
                    print(f"[Scout] Warning: Advanced risk feature extraction failed for {wallet_address[:8]}...: {e}")
                    advanced_risk_features = None

            result = {
                "address": wallet_address,
                "metrics": metrics,
                "wqs": wqs_score,
                "confidence": wqs_confidence,
                "status": final_status,
                "backtest": backtest_res,
                "trades": trades,
                "wallet_stats": wallet_stats,
                "components": raw_components,
                "trajectory": trajectory,
                "wmi": wmi,
                "strategy": _strategy,
                "archetype": _archetype,
                "network_features": network_features,
                "time_series_features": time_series_features,
                "advanced_risk_features": advanced_risk_features,
            }
            
            print(f"[Scout] ✓ Completed {wallet_address[:8]}... (WQS={wqs_score:.1f}, Status={final_status})")
            return result
        except Exception as e:
            print(f"[Scout] ✗ ERROR processing {wallet_address[:8]}...: {e}")
            return None

    if not candidates:
        print("[Scout] No candidate wallets to process")
        return [], stats, []

    # Run in parallel using asyncio (with semaphore for rate limiting)
    semaphore = asyncio.Semaphore(max(1, min(10, len(candidates))))
    
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

    # Clear analyzer caches to free memory before proceeding
    if analyzer:
        await analyzer.clear_all_caches()

    # Post-processing: Network features analysis across all wallets
    network_analysis = None
    if NETWORK_FEATURES_AVAILABLE and NetworkFeatures:
        try:
            print("[Scout] Computing network-wide features...")
            network_extractor = NetworkFeatures()

            # Collect wallet addresses and their trades for network analysis
            wallet_addresses = []
            wallet_trades_map = {}
            for res in results:
                if res and res.get('address'):
                    wallet_addresses.append(res['address'])
                    wallet_trades_map[res['address']] = res.get('trades', [])

            if wallet_addresses:
                # Build network graph from all wallets using batch method
                network_analysis = network_extractor.extract_network_features_batch(
                    wallet_addresses, wallet_trades_map
                )

                # Enhance individual results with network-wide features
                if network_analysis:
                    for res in results:
                        if res and res.get('address'):
                            addr = res['address']
                            if addr in network_analysis:
                                # Update network features with computed values
                                if res.get('network_features'):
                                    res['network_features'].update(network_analysis[addr])
                                else:
                                    res['network_features'] = network_analysis[addr]

                    print(f"[Scout] Network features computed for {len(network_analysis)} wallets")
        except Exception as e:
            print(f"[Scout] Warning: Network-wide analysis failed: {e}")
            import traceback
            traceback.print_exc()

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

        # Add network feature highlights if available
        network_features = res.get('network_features')
        if network_features:
            if network_features.get('sybil_risk') == 'HIGH':
                notes_parts.append(f"⚠️ SYBIL RISK: {network_features.get('sybil_cluster', 'unknown')}")
            elif network_features.get('pagerank_centrality'):
                centrality = network_features.get('pagerank_centrality', 0)
                if centrality > 0.5:
                    notes_parts.append(f"Network: High centrality ({centrality:.2f})")
            elif network_features.get('avg_coholding_with_successful'):
                coholding = network_features.get('avg_coholding_with_successful', 0)
                if coholding > 0.3:
                    notes_parts.append(f"Network: Strong successful co-holding ({coholding:.2f})")

        # Add time-series feature highlights if available
        time_series_features = res.get('time_series_features')
        if time_series_features and time_series_features.get('extraction_success'):
            # RSI indicators
            rsi = time_series_features.get('rsi')
            if rsi:
                if rsi > 70:
                    notes_parts.append(f"Time-series: Overbought (RSI={rsi:.1f})")
                elif rsi < 30:
                    notes_parts.append(f"Time-series: Oversold (RSI={rsi:.1f})")

            # Trend indicators
            if time_series_features.get('trend_up'):
                trend_strength = time_series_features.get('trend_strength', 0)
                notes_parts.append(f"Time-series: Uptrend (strength={trend_strength:.2f})")
            elif time_series_features.get('trend_up') == 0:
                trend_strength = time_series_features.get('trend_strength', 0)
                notes_parts.append(f"Time-series: Downtrend (strength={trend_strength:.2f})")

            # Momentum
            momentum = time_series_features.get('momentum_score')
            if momentum is not None and momentum > 0.7:
                notes_parts.append(f"Time-series: Strong momentum ({momentum:.2f})")
            elif momentum is not None and momentum < 0.3:
                notes_parts.append(f"Time-series: Weak momentum ({momentum:.2f})")

            # Persistence
            if time_series_features.get('persistence'):
                notes_parts.append("Time-series: Performance persistence detected")
            elif time_series_features.get('mean_reverting'):
                notes_parts.append("Time-series: Mean-reverting pattern")

        # Add advanced risk feature highlights if available
        advanced_risk_features = res.get('advanced_risk_features')
        if advanced_risk_features and advanced_risk_features.get('extraction_success'):
            # CVaR (Conditional Value at Risk) - Tail risk measure
            cvar_95 = advanced_risk_features.get('cvar_95')
            if cvar_95:
                if cvar_95 < -0.20:  # More than 20% loss in worst 5% cases
                    notes_parts.append(f"Risk: High tail risk (CVaR95={cvar_95:.1%})")
                elif cvar_95 < -0.10:
                    notes_parts.append(f"Risk: Moderate tail risk (CVaR95={cvar_95:.1%})")

            # Tail risk metrics
            tail_ratio = advanced_risk_features.get('tail_ratio')
            if tail_ratio:
                if tail_ratio < 0.8:  # Poor risk-adjusted returns in tail
                    notes_parts.append(f"Risk: Weak tail protection (ratio={tail_ratio:.2f})")

            # Ulcer Index - Measures downside duration and severity
            ulcer_index = advanced_risk_features.get('ulcer_index')
            if ulcer_index:
                if ulcer_index > 10.0:  # High prolonged drawdown
                    notes_parts.append(f"Risk: High ulcer index ({ulcer_index:.1f})")
                elif ulcer_index > 5.0:
                    notes_parts.append(f"Risk: Moderate ulcer index ({ulcer_index:.1f})")

            # Maximum drawdown duration
            max_dd_duration = advanced_risk_features.get('max_drawdown_duration_days')
            if max_dd_duration:
                if max_dd_duration > 30:  # More than 30 days in drawdown
                    notes_parts.append(f"Risk: Extended drawdown ({max_dd_duration:.0f} days)")
                elif max_dd_duration > 14:
                    notes_parts.append(f"Risk: Notable drawdown ({max_dd_duration:.0f} days)")

            # Risk regime classification
            risk_regime = advanced_risk_features.get('risk_regime')
            if risk_regime:
                if risk_regime == 'high_risk':
                    notes_parts.append("Risk: High volatility regime")
                elif risk_regime == 'low_risk':
                    notes_parts.append("Risk: Stable regime")

        notes_parts.append(f"Analyzed at {utcnow().isoformat()}")

        # Determine archetype
        archetype = res.get('archetype')
        
        record = WalletRecord(
            address=wallet_addr,
            status=status,
            wqs_score=wqs,
            wqs_confidence=res.get('confidence'),
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
    pre_diversion_active = {r.address for r in records if r.status == "ACTIVE"}
    _apply_archetype_diversification(records, min_wqs_active)

    # Post-diversification backtest validation: run validator on each newly
    # promoted wallet. Revert to CANDIDATE if validation fails.
    if validator is not None:
        promoted = [r for r in records if r.status == "ACTIVE" and r.address not in pre_diversion_active]
        if promoted:
            result_by_addr = {}
            for res in results:
                if res:
                    result_by_addr[res.get("address")] = res
            reverted_count = 0
            for r in promoted:
                res = result_by_addr.get(r.address)
                if not res:
                    continue
                trades = res.get("trades", [])
                metrics = res.get("metrics")
                if not trades or not metrics:
                    continue
                try:
                    vresult = await validator.validate_for_promotion(
                        r.address, metrics, trades, strategy=res.get("strategy", "SHIELD"),
                    )
                    if not vresult.passed:
                        r.status = "CANDIDATE"
                        reverted_count += 1
                        print(f"[Scout] Diversification validation: {r.address[:8]}... reverted to CANDIDATE "
                              f"({vresult.reason})")
                except Exception as val_err:
                    print(f"[Scout] Diversification validation error for {r.address[:8]}...: {val_err}")
                    r.status = "CANDIDATE"
                    reverted_count += 1
            if reverted_count > 0:
                print(f"[Scout] Diversification validation: reverted {reverted_count}/{len(promoted)} promoted wallets")
    
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
    
    # Step 5c: Cross-wallet token correlation — demote wallets with >70%
    # shared tokens to prevent correlated risk across the roster.
    if os.getenv("SCOUT_CROSS_WALLET_CORRELATION", "true").lower() == "true":
        try:
            from core.clustering import apply_cross_wallet_token_correlation
            wallet_tokens = {}
            for res in results:
                if not res:
                    continue
                addr = res.get("address")
                trades = res.get("trades", [])
                if addr and trades:
                    wallet_tokens[addr] = {t.token_address for t in trades if hasattr(t, 'token_address')}
            if wallet_tokens:
                funder_map = {}
                for r in records:
                    cid = getattr(r, 'cluster_id', None)
                    if cid and not cid.startswith("__singleton_"):
                        funder_map[r.address] = cid
                demoted = apply_cross_wallet_token_correlation(
                    records, wallet_tokens, funder_map=funder_map or None,
                )
                if demoted > 0:
                    stats_active = [r for r in records if r.status == "ACTIVE"]
                    stats["active"] = len(stats_active)
        except Exception as e:
            print(f"[Scout] Cross-wallet correlation skipped ({e})")
    
    # Step 3c: Write correlation records for promoted ACTIVE wallets
    for res in results:
        if not res or res.get('status') != "ACTIVE":
            continue
        wallet_addr = res['address']
        components = res.get('components')
        strategy = res.get('strategy', 'SHIELD')
        if components:
            write_correlation_record(
                wallet_addr, res['wqs'],
                components.components_json, strategy
            )
    
    # Step 5b: Write exit recommendations to JSON file
    if exit_recs:
        _write_exit_recommendations(exit_recs)
    
    return records, stats, results


def _apply_archetype_diversification(records: List[WalletRecord], min_wqs_active: float) -> None:
    """
    Flexible archetype diversification with diversity scoring.

    This is a SOFT target approach that balances WQS quality with
    archetype diversity, rather than hard quotas that force suboptimal wallets.

    Features:
    - Diversity score calculation (balances WQS vs archetype balance)
    - Opportunity cost estimation for archetype-forced promotions
    - Market condition awareness (can be configured)
    - Per-archetype configurable targets

    Modifies records in-place.
    """
    try:
        from config import ScoutConfig
        diversity_min_pct = ScoutConfig.get_archetype_diversity_min_pct()
        # Check if flexible mode is enabled
        flexible_mode = os.getenv("SCOUT_ARCHETYPE_DIVERSITY_MODE", "flexible").lower() == "flexible"
    except ImportError:
        diversity_min_pct = float(os.getenv("SCOUT_ARCHETYPE_DIVERSITY_MIN_PCT", "0.15"))
        flexible_mode = True  # Default to flexible

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

    # Calculate archetype distribution
    archetype_counts = {arch: len(recs) for arch, recs in active_by_archetype.items()}
    max_count = max(archetype_counts.values()) if archetype_counts else total_active

    # Calculate diversity score (0.0 = completely unbalanced, 1.0 = perfectly balanced)
    # Using normalized entropy calculation, excluding UNKNOWN archetypes
    non_unknown = {arch: count for arch, count in archetype_counts.items() if arch != "UNKNOWN"}
    diversity_score = 0.0
    if len(non_unknown) > 1 and max_count > 0:
        for arch, count in non_unknown.items():
            diversity_score += (count / max_count) * math.log2(len(non_unknown))
        diversity_score = diversity_score / (len(non_unknown) * math.log2(len(non_unknown)))

    # Target archetypes with minimum thresholds (soft targets in flexible mode)
    target_archetypes = {"SCALPER", "SWING", "WHALE"}

    # Per-archetype minimum thresholds (can be overridden by env vars)
    archetype_thresholds = {
        "SCALPER": int(os.getenv("SCOUT_MIN_SCALPER_COUNT", "2")),
        "SWING": int(os.getenv("SCOUT_MIN_SWING_COUNT", "2")),
        "WHALE": int(os.getenv("SCOUT_MIN_WHALE_COUNT", "1")),
    }

    promoted_count = 0
    forced_promotions = []  # Track promotions with opportunity cost

    for arch in target_archetypes:
        current = len(active_by_archetype.get(arch, []))
        min_count = archetype_thresholds.get(arch, 1)

        if current >= min_count:
            continue

        candidates = sorted(
            candidate_by_archetype.get(arch, []),
            key=lambda r: r.wqs_score or 0,
            reverse=True,
        )

        slots_needed = min_count - current

        for i, c in enumerate(candidates[:slots_needed]):
            # Calculate opportunity cost: how much WQS we're sacrificing
            active_wqs_scores = [r.wqs_score or 0 for r in active_records]
            next_best_active = max(active_wqs_scores) if active_wqs_scores else 0
            opportunity_cost = next_best_active - (c.wqs_score or 0)

            # In flexible mode, only promote if opportunity cost is acceptable
            wqs_threshold = min_wqs_active * 0.85  # Slightly lower threshold for diversity

            if flexible_mode:
                # Skip if WQS is too low or opportunity cost is too high
                if (c.wqs_score or 0) < wqs_threshold:
                    continue
                if opportunity_cost > 10.0:  # Don't sacrifice >10 WQS points
                    continue

            # Promote the candidate
            c.status = "ACTIVE"
            promoted_count += 1
            active_by_archetype.setdefault(arch, []).append(c)

            # Track forced promotions with opportunity cost
            forced_promotions.append({
                'address': c.address,
                'archetype': arch,
                'wqs': c.wqs_score,
                'opportunity_cost': opportunity_cost,
            })

    # Log results
    if promoted_count > 0:
        avg_opportunity_cost = sum(p['opportunity_cost'] for p in forced_promotions) / max(1, len(forced_promotions))
        print(f"[Scout] Archetype diversification: promoted {promoted_count} wallets (flexible mode: {flexible_mode})")
        print(f"[Scout]   Diversity score: {diversity_score:.2f}, Avg opportunity cost: {avg_opportunity_cost:.2f} WQS points")
    else:
        print(f"[Scout] Archetype diversification: No promotions needed (diversity score: {diversity_score:.2f})")


def _write_exit_recommendations(exit_recs: List[Dict[str, Any]]) -> None:
    """
    Write exit recommendations to data/exit_recommendations.json atomically
    AND to chimera.db exit_recommendations table with confidence scoring.
    The Operator reads the database table to trigger exits for declining ACTIVE wallets.

    Enhanced features:
    - Confidence scoring (0.0-1.0) based on signal strength
    - Exit throttling to prevent cascade exits
    - Priority-based processing (HIGH > MEDIUM > LOW)
    """
    if not exit_recs:
        return

    output_dir = os.getenv("SCOUT_DATA_DIR", os.path.join(os.path.dirname(__file__), "..", "data"))
    os.makedirs(output_dir, exist_ok=True)
    final_path = os.path.join(output_dir, "exit_recommendations.json")
    tmp_path = final_path + ".tmp"

    # Calculate confidence scores and apply throttling
    max_concurrent_exits = int(os.getenv("SCOUT_MAX_CONCURRENT_EXITS", "3"))
    current_exits = 0

    enhanced_recs = []
    for rec in exit_recs:
        confidence = _calculate_exit_confidence(rec)
        priority = _determine_exit_priority(rec, confidence)

        # Apply throttling for LOW priority exits
        if priority == "LOW" and current_exits >= max_concurrent_exits:
            print(f"[Scout] Throttled LOW priority exit for {rec.get('wallet', '')[:8]}...")
            continue

        enhanced_rec = {
            **rec,
            "confidence": confidence,
            "priority": priority,
            "created_at": utcnow().isoformat(),
        }
        enhanced_recs.append(enhanced_rec)

        if priority in ("HIGH", "MEDIUM"):
            current_exits += 1

    try:
        with open(tmp_path, "w") as f:
            json.dump(enhanced_recs, f, indent=2)
        os.rename(tmp_path, final_path)
        print(f"[Scout] Wrote {len(enhanced_recs)} exit recommendations to {final_path}")
    except Exception as e:
        print(f"[Scout] Failed to write exit recommendations: {e}")

    # Also write to chimera.db with enhanced schema
    db_path = os.getenv("CHIMERA_DB_PATH", "../data/chimera.db")
    conn = None
    try:
        conn = sqlite3.connect(db_path, timeout=10.0)
        conn.execute("PRAGMA journal_mode=WAL;")

        conn.execute("""
            CREATE TABLE IF NOT EXISTS exit_recommendations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                wallet_address TEXT NOT NULL,
                reason TEXT,
                recommended_action TEXT NOT NULL DEFAULT 'EXIT_ALL',
                confidence REAL NOT NULL DEFAULT 0.5,
                priority TEXT NOT NULL DEFAULT 'MEDIUM',
                created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                acknowledged INTEGER NOT NULL DEFAULT 0,
                processed INTEGER NOT NULL DEFAULT 0
            )
        """)

        for rec in enhanced_recs:
            conn.execute(
                """INSERT INTO exit_recommendations
                   (wallet_address, reason, recommended_action, confidence, priority)
                   VALUES (?, ?, ?, ?, ?)""",
                (rec.get("wallet"), rec.get("reason"), rec.get("recommended_action", "EXIT_ALL"),
                 rec.get("confidence", 0.5), rec.get("priority", "MEDIUM")),
            )

        conn.commit()
        print(f"[Scout] Wrote {len(enhanced_recs)} exit recommendations to {db_path} (exit_recommendations table)")

        high_conf = sum(1 for r in enhanced_recs if r.get("confidence", 0) >= 0.7)
        print(f"[Scout] Exit summary: {len(enhanced_recs)} total, {high_conf} high confidence")

    except Exception as e:
        print(f"[Scout] Failed to write exit recommendations to DB: {e}")
    finally:
        if conn:
            conn.close()


def _calculate_exit_confidence(rec: Dict[str, Any]) -> float:
    """
    Calculate confidence score for an exit recommendation.

    Higher confidence for:
    - Multiple negative signals (trajectory, WMI, alpha decay)
    - Strong decline (large negative ROI)
    - Low trade count (easier to exit)

    Returns:
        Confidence score 0.0-1.0
    """
    confidence = 0.5  # Base confidence

    # Boost confidence for multiple reasons
    reason = rec.get("reason", "")
    negative_signals = reason.count("|") + 1  # Count multiple reasons
    confidence += min(0.2, negative_signals * 0.1)

    # Check for strong decline signals
    if any(sig in reason for sig in ["DECLINING", "alpha decay", "drawdown"]):
        confidence += 0.15

    # Check trajectory
    if "DECLINING" in reason:
        confidence += 0.1
    elif "IMPROVING" in reason:
        confidence -= 0.2  # Lower confidence if improving

    # Check WMI (negative momentum = higher confidence)
    if "WMI" in reason and ("negative" in reason.lower() or "degrading" in reason.lower()):
        confidence += 0.1

    return max(0.0, min(1.0, confidence))


def _determine_exit_priority(rec: Dict[str, Any], confidence: float) -> str:
    """
    Determine priority level for exit processing.

    Args:
        rec: Exit recommendation record
        confidence: Confidence score

    Returns:
        Priority level: HIGH, MEDIUM, or LOW
    """
    # HIGH confidence + clear signals = HIGH priority
    if confidence >= 0.8:
        return "HIGH"

    # MEDIUM confidence or moderate signals = MEDIUM priority
    if confidence >= 0.5:
        return "MEDIUM"

    # Low confidence = LOW priority
    return "LOW"


async def main_async():
    """Async main entry point for the Scout."""
    # Setup file logging
    setup_logging()

    args = parse_args()
    
    print("=" * 70)
    print("Chimera Scout - Wallet Intelligence Layer")
    print(f"Started at: {utcnow().isoformat()}")
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
                    from urllib.parse import urlparse, parse_qs
                    helius_api_key = parse_qs(urlparse(rpc_url).query).get("api-key", [None])[0]
        
        if liquidity_mode == "simulated":
            print("[Scout] WARNING: Running with simulated liquidity mode - results are non-deterministic!")
            print("[Scout] Set SCOUT_LIQUIDITY_MODE=real and provide BIRDEYE_API_KEY for production use")

        # Initialize optimization system
        optimizer = None
        if CONFIG_AVAILABLE and ScoutConfig and ScoutConfig.get_optimization_enabled():
            if OPTIMIZATION_AVAILABLE:
                try:
                    optimizer = get_scout_optimizer()
                    if optimizer.initialize():
                        print("[Scout] ✓ Optimization system initialized")
                        optimizer.start_monitoring()
                        print("[Scout] ✓ Production monitoring started")

                        # Check production readiness
                        is_ready, readiness_issues = optimizer.is_production_ready()
                        if not is_ready:
                            print("[Scout] ⚠ Production readiness issues:")
                            for issue in readiness_issues[:3]:
                                print(f"  - {issue}")
                    else:
                        print("[Scout] ⚠ Optimization system initialization failed")
                        optimizer = None
                except Exception as e:
                    print(f"[Scout] ⚠ Failed to initialize optimization: {e}")
                    optimizer = None
            else:
                print("[Scout] Optimization modules not available, using base analyzer")

        # Initialize Stop-Loss Optimizer and Position Manager
        position_manager = None
        stop_loss_optimizer = None
        try:
            from core.stop_loss_optimizer import StopLossOptimizer
            from core.position_manager import PositionManager, set_position_manager
            from core.market_regime_detector import MarketRegimeDetector

            # Initialize stop-loss optimizer
            stop_loss_optimizer = StopLossOptimizer()
            print("[Scout] ✓ Stop-Loss Optimizer initialized")

            # Initialize market regime detector
            regime_detector = MarketRegimeDetector()
            print("[Scout] ✓ Market regime detector initialized")

            # Initialize position manager with stop-loss optimizer
            position_manager = PositionManager(stop_loss_optimizer, regime_detector)
            set_position_manager(position_manager)
            print("[Scout] ✓ Position Manager initialized")

        except Exception as e:
            print(f"[Scout] ⚠ Failed to initialize stop-loss optimization: {e}")
            print("[Scout]   Position management will use default stop-losses")

        # Initialize Validation Reporter for ML monitoring
        validation_reporter = None
        try:
            from core.validation_reporter import ValidationReporter, AlertConfig

            # Load alert configuration from config if available
            alert_config = None
            if CONFIG_AVAILABLE and ScoutConfig.get_validation_enabled():
                alert_config = AlertConfig(
                    webhook_url=ScoutConfig.get_alert_webhook_url(),
                    high_error_threshold=ScoutConfig.get_alert_high_error_threshold(),
                    drift_threshold=ScoutConfig.get_alert_drift_threshold(),
                    low_accuracy_threshold=ScoutConfig.get_alert_low_accuracy_threshold(),
                    alert_dir=ScoutConfig.get_alert_dir(),
                )

            # Initialize validation reporter
            db_path = str(args.output) if args.output else "data/roster_new.db"
            validation_reporter = ValidationReporter(db_path=db_path, alert_config=alert_config)
            print("[Scout] ✓ Validation Reporter initialized")

        except Exception as e:
            print(f"[Scout] ⚠ Failed to initialize Validation Reporter: {e}")
            validation_reporter = None

        # Initialize State Persistence for cross-session learning
        state_persistence = None
        try:
            from core.state_persistence import StatePersistence, PersistenceConfig

            # Load persistence configuration from config if available
            persistence_config = None
            if CONFIG_AVAILABLE and ScoutConfig.get_state_persistence_enabled():
                persistence_config = PersistenceConfig(
                    db_path=ScoutConfig.get_state_persistence_db_path(),
                    max_history_days=ScoutConfig.get_state_persistence_max_days(),
                    backup_enabled=ScoutConfig.get_state_persistence_backup_enabled(),
                    backup_interval_hours=ScoutConfig.get_state_persistence_backup_interval(),
                    vacuum_interval_days=ScoutConfig.get_state_persistence_vacuum_interval(),
                )

            # Initialize state persistence
            state_persistence = StatePersistence(config=persistence_config)
            print("[Scout] ✓ State Persistence initialized")

            # Get database stats
            stats = state_persistence.get_database_stats()
            print(f"[Scout]   Database: {stats['database_path']}")
            print(f"[Scout]   Records: {stats['total_records']}")
            print(f"[Scout]   Database size: {stats['database_size_mb']:.2f} MB")

        except Exception as e:
            print(f"[Scout] ⚠ Failed to initialize State Persistence: {e}")
            state_persistence = None

        # Initialize Signal Quality Filter for trade quality improvement
        signal_quality_filter = None
        try:
            from core.signal_quality_filter import SignalQualityFilter, FilterConfig

            # Load filter configuration from config if available
            filter_config = None
            if CONFIG_AVAILABLE and ScoutConfig.get_signal_quality_filter_enabled():
                filter_config = FilterConfig(
                    WQS_WEIGHT=ScoutConfig.get_wqs_weight(),
                    TIMING_WEIGHT=ScoutConfig.get_timing_weight(),
                    REGIME_WEIGHT=ScoutConfig.get_regime_weight(),
                    ENSEMBLE_WEIGHT=ScoutConfig.get_ensemble_weight(),
                    FRESHNESS_WEIGHT=ScoutConfig.get_freshness_weight(),
                    TOP_PERCENTILE_TARGET=ScoutConfig.get_top_percentile_target(),
                    MIN_PERCENTILE_THRESHOLD=ScoutConfig.get_min_percentile_threshold(),
                    MAX_PERCENTILE_THRESHOLD=ScoutConfig.get_max_percentile_threshold(),
                    WQS_MAX=ScoutConfig.get_wqs_max(),
                    WQS_MIN=ScoutConfig.get_wqs_min(),
                    FRESHNESS_MAX_AGE_SECONDS=ScoutConfig.get_freshness_max_age_seconds(),
                    FRESHNESS_OPTIMAL_AGE_SECONDS=ScoutConfig.get_freshness_optimal_age_seconds(),
                    ENSEMBLE_MIN_CONFIDENCE=ScoutConfig.get_ensemble_min_confidence(),
                    TIMING_MIN_SCORE=ScoutConfig.get_timing_min_score(),
                )

            # Initialize signal quality filter
            signal_quality_filter = SignalQualityFilter(config=filter_config)
            print("[Scout] ✓ Signal Quality Filter initialized")

            # Get filter statistics
            stats = signal_quality_filter.get_filter_stats()
            print(f"[Scout]   Current threshold: top {stats['current_threshold']:.1f}%")
            print(f"[Scout]   Total evaluated: {stats['total_signals_evaluated']}")
            print(f"[Scout]   Execution rate: {stats['execution_rate']:.1f}%")

        except Exception as e:
            print(f"[Scout] ⚠ Failed to initialize Signal Quality Filter: {e}")
            signal_quality_filter = None

        # Initialize PredictiveBudgetManager for Helius API quota management
        budget_manager = None
        if BUDGET_MANAGER_AVAILABLE:
            try:
                # Check if budget tracking is enabled via config or env
                budget_enabled = False
                if CONFIG_AVAILABLE and ScoutConfig:
                    budget_enabled = ScoutConfig.get_budget_tracking_enabled()
                elif os.getenv("SCOUT_BUDGET_TRACKING_ENABLED", "false").lower() == "true":
                    budget_enabled = True

                if budget_enabled:
                    budget_manager = PredictiveBudgetManager()
                    print("[Scout] ✓ PredictiveBudgetManager initialized")

                    # Get current snapshot and alert level
                    snapshot = budget_manager.get_realtime_snapshot()
                    print(f"[Scout]   Credits: {snapshot.credits_remaining:,} / {snapshot.credits_total:,} remaining")
                    print(f"[Scout]   Daily target: {snapshot.daily_target:,} credits")
                    print(f"[Scout]   Alert level: {snapshot.alert_level.value}")

                    # Run 7-day forecast
                    forecast = budget_manager.forecast_credit_needs(horizon_hours=168)  # 7 days
                    print(f"[Scout]   7-day forecast: {forecast.projected_usage:,} credits projected")
                    print(f"[Scout]   Forecast trend: {forecast.trend} (confidence: {forecast.confidence:.1%})")

                    # Generate and display optimization suggestions
                    suggestions = budget_manager.suggest_credit_optimization()
                    if suggestions:
                        print("[Scout] Optimization suggestions:")
                        for suggestion in suggestions[:5]:  # Show top 5
                            print(f"  [{suggestion.priority.upper()}] {suggestion.action}: {suggestion.description}")
                            if suggestion.expected_savings > 0:
                                print(f"     Expected savings: {suggestion.expected_savings:,} credits")
                else:
                    print("[Scout] Budget tracking disabled (enable with SCOUT_BUDGET_TRACKING_ENABLED=true)")

            except Exception as e:
                print(f"[Scout] ⚠ Failed to initialize PredictiveBudgetManager: {e}")
                budget_manager = None

        # Initialize High-Conviction Integration for WQS-based prioritization
        high_conviction = None
        if HIGH_CONVICTIION_AVAILABLE:
            try:
                # Check if high conviction is enabled via config or env
                conviction_enabled = False
                if CONFIG_AVAILABLE and ScoutConfig:
                    conviction_enabled = ScoutConfig.get_high_conviction_enabled()
                elif os.getenv("SCOUT_HIGH_CONVICTION_ENABLED", "false").lower() == "true":
                    conviction_enabled = True

                if conviction_enabled:
                    # Get total credits budget (default to Helius Developer Plan monthly / 30 days)
                    total_credits = int(os.getenv("SCOUT_TOTAL_ANALYSIS_CREDITS", "333333"))  # Daily budget

                    high_conviction = create_high_conviction_integration(
                        total_credits=total_credits,
                        enabled=True
                    )
                    print("[Scout] ✓ High-Conviction Integration initialized")
                    print(f"[Scout]   Analysis budget: {total_credits:,} credits")
                    print(f"[Scout]   High-conviction priority: WQS 70+")
                else:
                    print("[Scout] High-conviction prioritization disabled")

            except Exception as e:
                print(f"[Scout] ⚠ Failed to initialize High-Conviction Integration: {e}")
                high_conviction = None

        # Use async factory for proper wallet discovery
        base_analyzer = await WalletAnalyzer.create(
            helius_api_key=helius_api_key,
            discover_wallets=True,  # Enable wallet discovery from on-chain data
            max_wallets=args.max_wallets,
            budget_manager=budget_manager,  # Pass budget manager for API quota tracking
        )

        # Wrap with optimized analyzer if available
        if optimizer and OPTIMIZATION_AVAILABLE:
            analyzer = OptimizedWalletAnalyzer(base_analyzer, optimizer)
            print("[Scout] ✓ Using optimized wallet analyzer")
        else:
            analyzer = base_analyzer
            print("[Scout] Using base wallet analyzer")
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
                simulate_at_size_sol=Decimal(os.getenv("SCOUT_COPIER_SIZE_SOL", "0.5")),
            )

            # Fetch dynamic fees from Helius if available (overrides static values)
            if helius_api_key and os.getenv("SCOUT_USE_DYNAMIC_FEES", "true").lower() == "true":
                fee_estimator = None
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
                except Exception as e:
                    print(f"[Scout] Warning: Dynamic fee fetch failed ({e}), using static fees")
                finally:
                    if fee_estimator:
                        try:
                            await fee_estimator.close()
                        except Exception:
                            pass  # Non-critical
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
                rugcheck_client=analyzer.rugcheck_client,  # Share RugCheck client to reuse cache
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

    # Initialize profit tracker for growth monitoring
    profit_tracker = None
    if PROFIT_TRACKER_AVAILABLE and os.getenv("SCOUT_PROFIT_TRACKER_ENABLED", "false").lower() == "true":
        try:
            # Get starting capital from environment or use default
            starting_capital = float(os.getenv("SCOUT_STARTING_CAPITAL", "200.0"))
            target_capital = float(os.getenv("SCOUT_TARGET_CAPITAL", "1000.0"))

            tracker_config = TrackerConfig(
                STARTING_CAPITAL=starting_capital,
                TARGET_CAPITAL=target_capital,
            )
            profit_tracker = RealtimeProfitTracker(config=tracker_config)
            print(f"[Scout] ✓ Profit tracker initialized (${starting_capital:.0f} → ${target_capital:.0f})")
        except Exception as e:
            print(f"[Scout] Warning: Failed to initialize profit tracker: {e}")

    # Analyze wallets
    print("\n[Scout] Analyzing wallets...")
    print(f"  Min WQS for ACTIVE: {args.min_wqs_active}")
    print(f"  Min WQS for CANDIDATE: {args.min_wqs_candidate}")
    
    import time
    analysis_start = time.time()
    records, stats, results = await analyze_wallets(
        analyzer,
        validator,
        args.min_wqs_active,
        args.min_wqs_candidate,
        skip_backtest=args.skip_backtest,
        verbose=args.verbose,
        optimizer=optimizer,
        high_conviction=high_conviction,
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

    # Backfill actual copy PnL from wallets table (before correlation summary + calibration)
    try:
        backfill_correlation_pnl(os.getenv("CHIMERA_DB_PATH", "../data/chimera.db"))
    except Exception as e:
        if args.verbose:
            print(f"[Scout] PnL backfill error: {e}")

    # Phase 3a: Print WQS-to-PnL correlation summary (now has backfilled data)
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

    # Step 3e: WQS-to-PnL feedback loop — demote ACTIVE wallets with actual
    # negative copy-trade PnL, and compute rolling accuracy metric.
    try:
        corr_reader = CorrelationReader()
        if corr_reader.table_exists():
            pnl_records = corr_reader.get_all_records(min_trades=5)
            demoted_count = 0
            profitable_count = 0
            total_with_pnl = 0
            for rec in pnl_records:
                if rec.actual_copy_pnl_30d_sol is not None:
                    total_with_pnl += 1
                    if rec.actual_copy_pnl_30d_sol > 0:
                        profitable_count += 1
                    elif rec.actual_copy_pnl_30d_sol < 0:
                        for r in records:
                            if r.address == rec.wallet_address and r.status == "ACTIVE":
                                r.status = "CANDIDATE"
                                demoted_count += 1
                                print(f"[Scout] PnL feedback: demoted {rec.wallet_address[:8]}... "
                                      f"(actual 30d PnL={rec.actual_copy_pnl_30d_sol:.4f} SOL)")
                                break
            if total_with_pnl > 0:
                accuracy_pct = (profitable_count / total_with_pnl) * 100
                print(f"[Scout] PnL accuracy: {profitable_count}/{total_with_pnl} promoted wallets "
                      f"profitable ({accuracy_pct:.1f}%)")
                if demoted_count > 0:
                    print(f"[Scout] PnL feedback: demoted {demoted_count} underperforming wallets")
    except Exception as e:
        if args.verbose:
            print(f"[Scout] PnL feedback loop skipped: {e}")

    # Update profit tracker with realized PnL data
    if profit_tracker and PROFIT_TRACKER_AVAILABLE:
        try:
            corr_reader = CorrelationReader()
            if corr_reader.table_exists():
                pnl_records = corr_reader.get_all_records(min_trades=5)
                total_capital_change = 0.0

                # Update tracker with each wallet's PnL
                for rec in pnl_records:
                    if rec.actual_copy_pnl_30d_sol is not None:
                        # Use WQS score as a proxy for wallet quality (stored in wqs_score_at_promotion)
                        wqs_score = rec.wqs_score_at_promotion or 0.0

                        # Update profit tracker with this wallet's performance
                        profit_tracker.update_profit(
                            trade_id=f"{rec.wallet_address}_30d",
                            pnl=float(rec.actual_copy_pnl_30d_sol),
                            wqs=wqs_score,
                            category="copy_trading"
                        )
                        total_capital_change += float(rec.actual_copy_pnl_30d_sol)

                # Print profit tracking summary
                if pnl_records and total_capital_change != 0:
                    tracker_summary = profit_tracker.get_tracker_summary()
                    velocity = profit_tracker.get_profit_velocity()
                    eta = profit_tracker.get_eta_to_1000()

                    print(f"\n[Scout] === Growth Tracking ===")
                    print(f"  Current capital: ${tracker_summary['capital']['current']:.2f}")
                    print(f"  Total profit: ${tracker_summary['capital']['profit']:.2f} "
                          f"({tracker_summary['capital']['profit_pct']:.1f}%)")
                    print(f"  Growth stage: {tracker_summary['capital']['growth_stage']}")
                    print(f"  Velocity: ${velocity.daily_rate:.2f}/day ({velocity.trend})")
                    if eta.days_remaining < float('inf'):
                        print(f"  ETA to $1,000: {eta.days_remaining:.1f} days (confidence: {eta.confidence:.1%})")
                    else:
                        print(f"  ETA to $1,000: Unable to calculate (negative velocity)")
                    print(f"  Win rate: {tracker_summary['performance']['win_rate']:.1%}")

                    # Check for optimization triggers
                    optimization_actions = profit_tracker.trigger_optimization_if_needed()
                    if optimization_actions:
                        print(f"\n[Scout] Optimization Actions:")
                        for action in optimization_actions:
                            print(f"  [{action.priority.upper()}] {action.action}: {action.description}")

        except Exception as e:
            if args.verbose:
                print(f"[Scout] Profit tracker update failed: {e}")

    # Prediction matching + validation metrics
    if os.getenv("SCOUT_PREDICTION_MATCHING_ENABLED", "true").lower() == "true":
        try:
            from core.prediction_matcher import PredictionMatcher
            from core.validation_metrics import ValidationMetricsCalculator
            matcher = PredictionMatcher()
            matched_count = matcher.match_predictions_to_actuals()
            if matched_count > 0:
                print(f"[Scout] Prediction matching: {matched_count} predictions matched to actual PnL")
                metrics_calc = ValidationMetricsCalculator()
                vm = metrics_calc.calculate_metrics()
                if vm:
                    print(f"[Scout] Model accuracy: RMSE={vm.rmse:.4f}, "
                          f"direction_accuracy={vm.direction_accuracy:.1%}")
            elif os.getenv("SCOUT_PREDICTION_MATCHING_ENABLED") == "true":
                print(f"[Scout] Prediction matching: no unmatched predictions found")
        except Exception as e:
            if args.verbose:
                print(f"[Scout] Prediction matching skipped: {e}")

    # End-of-run outcome summary
    try:
        cr = CorrelationReader()
        if cr.table_exists():
            stats = cr.get_correlation_stats()
            print(f"\n[Scout] === Outcome Summary ===")
            print(f"  Wallets with PnL data: {stats.wallets_with_pnl}/{stats.total_wallets}")
            if stats.wallets_with_pnl > 0:
                print(f"  Mean copy PnL (30d): {stats.mean_pnl_30d_sol:.4f} SOL")
                print(f"  Mean WQS at promotion: {stats.mean_wqs_at_promotion:.1f}")

                # WQS-to-PnL predictiveness check (Pearson correlation)
                records = cr.get_all_records(min_trades=1)
                pairs = [(r.wqs_score_at_promotion, r.actual_copy_pnl_30d_sol)
                         for r in records if r.actual_copy_pnl_30d_sol is not None]
                if len(pairs) >= 5:
                    n = len(pairs)
                    mean_w = sum(p[0] for p in pairs) / n
                    mean_p = sum(p[1] for p in pairs) / n
                    cov = sum((w - mean_w) * (p - mean_p) for w, p in pairs)
                    var_w = sum((w - mean_w) ** 2 for w, _ in pairs)
                    var_p = sum((p - mean_p) ** 2 for _, p in pairs)
                    r = cov / ((var_w * var_p) ** 0.5) if var_w * var_p > 0 else 0.0
                    print(f"  WQS-PnL correlation: r={r:.3f}, n={n}")
                    if metrics:
                        metrics.update_pnl_correlation_metrics(
                            wallets_with_pnl=stats.wallets_with_pnl,
                            mean_pnl_30d=stats.mean_pnl_30d_sol,
                            correlation_r=r,
                        )

                    # Profitability rate vs random baseline
                    profitable = sum(1 for _, p in pairs if p > 0)
                    profit_rate = profitable / n * 100
                    print(f"  Profitability rate: {profit_rate:.1f}% ({profitable}/{n})")
                    print(f"  Baseline (random):  50.0%")
                    if profit_rate > 50:
                        print(f"  WQS beats random by {profit_rate - 50:.1f}pp")
                else:
                    print(f"  WQS-PnL correlation: insufficient data ({len(pairs)} pairs, need 5+)")
                    if metrics:
                        metrics.update_pnl_correlation_metrics(
                            wallets_with_pnl=stats.wallets_with_pnl,
                            mean_pnl_30d=stats.mean_pnl_30d_sol,
                            correlation_r=None,
                        )

                for strat, data in stats.strategy_breakdown.items():
                    mean_pnl = data.get('mean_pnl', 0) or 0
                    print(f"  {strat}: {data.get('count', 0)} wallets, "
                          f"mean PnL={mean_pnl:.4f} SOL")
    except Exception as e:
        if args.verbose:
            print(f"[Scout] Outcome summary skipped: {e}")

    # Phase 6a: Write feature vectors to FeatureStore for downstream ML
    if not args.dry_run:
        try:
            feature_store = FeatureStore()
            feature_dicts = []
            for res in results:
                if not res:
                    continue
                m = res.get('metrics')
                ws = res.get('wallet_stats', {})
                if not m:
                    continue
                last_trade = m.last_trade_at
                days_since = None
                if last_trade:
                    try:
                        lt = datetime.fromisoformat(last_trade.replace("Z", "+00:00"))
                        if lt.tzinfo is None:
                            lt = lt.replace(tzinfo=timezone.utc)
                        days_since = (utcnow() - lt).days
                    except (ValueError, TypeError):
                        pass
                feature_dicts.append({
                    "address": res.get("address"),
                    "status": res.get("status"),
                    "archetype": res.get("archetype"),
                    "wqs_score": float(res.get("wqs", 0)),
                    "roi_7d": m.roi_7d,
                    "roi_30d": m.roi_30d,
                    "trade_count_30d": m.trade_count_30d,
                    "win_rate": m.win_rate,
                    "max_drawdown_30d": m.max_drawdown_30d,
                    "avg_trade_size_sol": m.avg_trade_size_sol,
                    "profit_factor": ws.get("profit_factor"),
                    "sortino_ratio": m.sortino_ratio,
                    "avg_entry_delay_seconds": m.avg_entry_delay_seconds,
                    "is_fresh_wallet": m.is_fresh_wallet,
                    "dex_diversity_score": m.dex_diversity_score,
                    "uses_limit_orders": m.uses_limit_orders,
                    "uses_mev_protection": m.uses_mev_protection,
                    "unique_token_categories": m.unique_token_categories,
                    "mev_risk_score": m.mev_risk_score,
                    "days_since_last_trade": days_since,
                    "parse_rate": m.parse_rate,
                })
            if feature_dicts:
                csv_path = feature_store.append_run(
                    feature_dicts,
                    wmi_scores={r.get("address"): r.get("wmi") for r in results if r},
                )
                print(f"[Scout] Feature store updated: {csv_path} ({len(feature_dicts)} wallets)")
        except Exception as e:
            if args.verbose:
                print(f"[Scout] Feature store skipped: {e}")

    # Print parse health dashboard (always in verbose/dry-run, otherwise only if >0 failures)
    if args.verbose or args.dry_run or stats["total"] > 0:
        analyzer.print_parse_health_dashboard()

    # If overall parse rate across ALL wallets is below threshold, exit non-zero
    # so that cron can alert. Configurable via SCOUT_PARSE_HEALTH_EXIT_FAIL_PCT.
    if analyzer.is_parse_rate_below_threshold():
        exit_pct = float(os.getenv("SCOUT_PARSE_HEALTH_EXIT_FAIL_PCT", "40"))
        print(f"[Scout] ⚠ Overall parse rate < {exit_pct:.0f}% — exiting non-zero for cron alert")
        sys.exit(2)

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
                merge_success, merge_message = await auto_merge_roster(
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

    # Print optimization report if enabled
    if optimizer and OPTIMIZATION_AVAILABLE and ScoutConfig.get_optimization_enabled():
        try:
            print("\n" + "=" * 70)
            print("SCOUT OPTIMIZATION REPORT")
            print("=" * 70)

            # Print comprehensive optimization status
            optimizer.print_optimization_report()

            # Get optimization suggestions
            suggestions = optimizer.get_optimization_suggestions()
            if suggestions:
                print("\nOptimization Suggestions:")
                for i, suggestion in enumerate(suggestions[:5], 1):
                    print(f"  {i}. {suggestion}")

            # Check production health
            if ScoutConfig.get_production_monitoring_enabled():
                health = optimizer.check_production_health()
                print(f"\nProduction Health Status: {health.get('overall_status', 'UNKNOWN')}")
                if health.get('overall_status') != 'healthy':
                    print("  WARNING: Production issues detected - review monitoring data")

            print("=" * 70)

        except Exception as e:
            print(f"[Scout] WARNING: Optimization report generation failed: {e}")

    # Print budget summary if enabled
    if budget_manager and BUDGET_MANAGER_AVAILABLE:
        try:
            # Record daily usage for forecasting
            try:
                from core.predictive_budget_manager import BudgetCategory
                snapshot = budget_manager.get_realtime_snapshot()
                daily_credits = snapshot.daily_used
                category_breakdown = {
                    BudgetCategory.DISCOVERY: 0,
                    BudgetCategory.ANALYSIS: 0,
                    BudgetCategory.VALIDATION: 0,
                    BudgetCategory.ENRICHMENT: 0,
                    BudgetCategory.MONITORING: 0,
                }
                # The analyzer has been tracking usage by category during the run
                # We could get this from analyzer if we exposed the method, but for now just record total
                budget_manager.record_daily_usage(daily_credits, category_breakdown)
            except Exception as record_err:
                print(f"[Scout] Warning: Failed to record daily usage: {record_err}")

            print("\n" + "=" * 70)
            print("BUDGET MANAGEMENT REPORT")
            print("=" * 70)

            # Get daily summary
            summary = budget_manager.get_daily_summary()

            # Print snapshot
            snapshot = summary.get('snapshot', {})
            print(f"Credit Usage:")
            print(f"  Credits Used: {snapshot.get('credits_used', 0):,}")
            print(f"  Credits Remaining: {snapshot.get('credits_remaining', 0):,}")
            print(f"  Usage Percentage: {snapshot.get('usage_percentage', 0):.1f}%")
            print(f"  Daily Target: {snapshot.get('daily_target', 0):,}")
            print(f"  Daily Used: {snapshot.get('daily_used', 0):,}")
            print(f"  Alert Level: {snapshot.get('alert_level', 'unknown').upper()}")

            # Print allocations
            print(f"\nBudget Allocations:")
            for category, allocation in summary.get('allocations', {}).items():
                print(f"  {category}: {allocation}")

            # Print category performance
            print(f"\nCategory Performance:")
            for category, perf in summary.get('category_performance', {}).items():
                print(f"  {category}:")
                print(f"    Credits Used: {perf.get('credits_used', 0):,}")
                print(f"    Operations: {perf.get('operations', 0)}")
                print(f"    ROI: {perf.get('roi', 0):.2f}")

            # Print forecast
            print(f"\nForecasts:")
            forecast = summary.get('forecast', {})
            print(f"  24h Projected: {forecast.get('24h_projected', 0):,} credits")
            print(f"  7d Projected: {forecast.get('7d_projected', 0):,} credits")

            # Get and display optimization suggestions
            suggestions = budget_manager.suggest_credit_optimization()
            if suggestions:
                print(f"\nBudget Optimization Suggestions:")
                for suggestion in suggestions[:5]:  # Show top 5
                    print(f"  [{suggestion.priority.upper()}] {suggestion.action}")
                    print(f"     {suggestion.description}")
                    if suggestion.expected_savings > 0:
                        print(f"     Expected Savings: {suggestion.expected_savings:,} credits")

            print("=" * 70)

        except Exception as e:
            print(f"[Scout] WARNING: Budget report generation failed: {e}")

    # Print high-conviction allocation report if enabled
    if high_conviction and HIGH_CONVICTIION_AVAILABLE:
        try:
            high_conviction.print_allocation_report()
        except Exception as e:
            print(f"[Scout] WARNING: High-conviction report generation failed: {e}")

    # Clean up resources
    try:
        if analyzer and hasattr(analyzer, 'shutdown'):
            await analyzer.shutdown()
            print("[Scout] Cleaned up all resources")
        if 'liquidity_provider' in locals() and liquidity_provider:
            try:
                await liquidity_provider.close()
            except Exception:
                pass  # Non-critical
    except Exception as e:
        if args.verbose:
            print(f"[Scout] Warning during cleanup: {e}")

    print(f"\n[Scout] Finished at: {utcnow().isoformat()}")
    print("=" * 70)


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


if __name__ == "__main__":
    main()
