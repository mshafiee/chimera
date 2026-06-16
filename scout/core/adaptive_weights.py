"""
Adaptive WQS Weight Calibrator (Phase 3b)

Uses linear regression on wqs_pnl_correlation data to periodically
recalibrate WQS component weights. Components that correlate well with
actual copy-trade profitability get higher weight; noise components
get deweighted.

Usage:
    calibrator = AdaptiveWeightCalibrator(db_path="data/chimera.db")
    new_weights = calibrator.calibrate()
    # new_weights is a dict of component_name -> weight_multiplier
"""

import json
import logging
import os
from pathlib import Path
from typing import Dict, List, Optional, Tuple

logger = logging.getLogger(__name__)

# Default WQS component weights (multipliers applied to component scores)
DEFAULT_WQS_WEIGHTS = {
    "roi_score": 1.0,
    "win_rate_score": 1.0,
    "pf_score": 1.0,
    "sortino_score": 1.0,
    "drawdown_penalty": 1.0,
    "activity_score": 1.0,
    "recency_score": 1.0,
    "martingale_penalty": 1.0,
    "pf_wr_penalty": 1.0,
    "token_diversity_score": 1.0,
    "dex_diversity_score": 1.0,
    "smart_money_score": 1.0,
    "smart_money_removal": 1.0,
    "entry_delay_score": 1.0,
    "pump_spike_penalty": 1.0,
    "consistency_score": 1.0,
    "sniper_penalty": 1.0,
    "insider_penalty": 1.0,
    "scam_penalty": 1.0,
    "mev_risk_penalty": 1.0,
}


class AdaptiveWeightCalibrator:
    """
    Periodically recalibrates WQS component weights based on actual
    copy-trading PnL data from the wqs_pnl_correlation table.

    Weights are clamped to [0.5, 2.0] to prevent radical swings.
    """

    MIN_WEIGHT = 0.5
    MAX_WEIGHT = 2.0
    MIN_SAMPLES = 10  # Minimum correlation records for calibration

    def __init__(self, db_path: Optional[str] = None):
        if db_path is None:
            db_path = os.getenv("CHIMERA_DB_PATH", "../data/chimera.db")
        self.db_path = Path(db_path)
        self._weights_cache_file = Path(db_path).parent / "wqs_adaptive_weights.json"

    def get_current_weights(self) -> Dict[str, float]:
        """Load weights from cache file, falling back to defaults."""
        if self._weights_cache_file.exists():
            try:
                with open(self._weights_cache_file) as f:
                    cached = json.load(f)
                    if isinstance(cached, dict) and cached:
                        return {k: float(v) for k, v in cached.items()}
            except (json.JSONDecodeError, ValueError):
                pass
        return dict(DEFAULT_WQS_WEIGHTS)

    def calibrate(
        self,
        strategy: str = "SHIELD",
        min_correlation: float = 0.05,
    ) -> Optional[Dict[str, float]]:
        """
        Run linear regression on component scores vs actual PnL.

        Returns updated weight dictionary, or None if insufficient data.
        """
        from .correlation_reader import CorrelationReader

        reader = CorrelationReader(str(self.db_path))
        records = reader.get_all_records(strategy=strategy, min_trades=1)

        if len(records) < self.MIN_SAMPLES:
            logger.info(
                f"Adaptive weights: insufficient data ({len(records)} < {self.MIN_SAMPLES} records)"
            )
            return None

        # Collect: for each component, list of (component_score, actual_pnl_30d) pairs
        component_pairs: Dict[str, List[Tuple[float, float]]] = {}
        pnl_vals = []

        for r in records:
            if r.actual_copy_pnl_30d_sol is None or r.wqs_components_json is None:
                continue
            try:
                comp_data = json.loads(r.wqs_components_json)
                if not isinstance(comp_data, dict):
                    continue
                for key, val in comp_data.items():
                    if not isinstance(val, (int, float)):
                        continue
                    component_pairs.setdefault(key, []).append((float(val), r.actual_copy_pnl_30d_sol))
                pnl_vals.append(r.actual_copy_pnl_30d_sol)
            except (json.JSONDecodeError, ValueError):
                continue

        if not pnl_vals:
            return None

        # Compute per-component regression coefficients
        current_weights = self.get_current_weights()
        component_corrs: Dict[str, float] = {}

        from .correlation_reader import CorrelationReader as CR

        for comp_name, pairs in component_pairs.items():
            if len(pairs) < 5:
                continue
            xs = [p[0] for p in pairs]
            ys = [p[1] for p in pairs]
            corr = CR._pearson_correlation(xs, ys)
            if abs(corr) >= min_correlation:
                component_corrs[comp_name] = corr

        if not component_corrs:
            return None

        # Scale correlations to weight multipliers
        # Positive correlation → weight >= 1.0, negative → weight <= 1.0
        new_weights = dict(current_weights)

        for comp_name, corr in component_corrs.items():
            # Map correlation [-1.0, 1.0] to weight [0.5, 2.0]
            # corr=0 → weight=1.0, corr=0.3 → weight=1.3, corr=-0.3 → weight=0.7
            adjusted = 1.0 + corr
            new_weights[comp_name] = max(self.MIN_WEIGHT, min(self.MAX_WEIGHT, adjusted))

        # Blend with current weights (EMA-style, 30% new, 70% old)
        blended = {}
        for key in set(list(current_weights.keys()) + list(new_weights.keys())):
            old_w = current_weights.get(key, 1.0)
            new_w = new_weights.get(key, old_w)
            blended[key] = old_w * 0.7 + new_w * 0.3

        return blended

    def save_weights(self, weights: Dict[str, float]) -> None:
        """Persist calibrated weights to cache file."""
        self._weights_cache_file.parent.mkdir(parents=True, exist_ok=True)
        with open(self._weights_cache_file, "w") as f:
            json.dump(weights, f, indent=2)
        logger.info(f"Adaptive weights saved to {self._weights_cache_file}")

    def should_calibrate(self, run_interval: int = 10) -> bool:
        """
        Determine if calibration should run based on run count.

        Uses a simple counter file to track run number.
        """
        counter_file = Path(self.db_path).parent / "wqs_calibration_counter.txt"
        try:
            if counter_file.exists():
                count = int(counter_file.read_text().strip())
            else:
                count = 0
        except (ValueError, FileNotFoundError):
            count = 0

        count += 1
        counter_file.write_text(str(count))

        return count % run_interval == 0

    def calibrate_if_needed(self, strategy: str = "SHIELD") -> Optional[Dict[str, float]]:
        """Run calibration if schedule says it's time."""
        if self.should_calibrate():
            weights = self.calibrate(strategy=strategy)
            if weights:
                self.save_weights(weights)
                return weights
        return None


def get_effective_wqs_weights() -> Dict[str, float]:
    """Convenience: get current effective WQS weights from cache or defaults."""
    calibrator = AdaptiveWeightCalibrator()
    return calibrator.get_current_weights()
