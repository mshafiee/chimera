"""
Adaptive WQS Weight Calibrator (Phase 3b)

Uses linear regression on wqs_pnl_correlation data to periodically
recalibrate WQS component weights. Components that correlate well with
actual copy-trade profitability get higher weight; noise components
get deweighted.

Pre-seeded defaults are loaded from data/wqs_default_weights.json to
avoid the cold-start problem (all-1.0 weights before any calibration
records exist). On early calibration runs (< 10 records) a Bayesian
prior blends 60% seeded + 40% regression to prevent noise-driven swings.

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

from .weight_seed import load_seeded_weights

logger = logging.getLogger(__name__)

# Default WQS component weights — loaded from pre-seeded file, not hardcoded
DEFAULT_WQS_WEIGHTS = load_seeded_weights()


class AdaptiveWeightCalibrator:
    """
    Periodically recalibrates WQS component weights based on actual
    copy-trading PnL data from the wqs_pnl_correlation table.

    Weights are clamped to [0.5, 2.0] to prevent radical swings.

    Cold start improvements:
    - MIN_SAMPLES reduced to 3 for faster initial adaptation
    - Confidence-weighted blending for gradual transition
    - Warm start with pre-seeded historical calibration
    """

    MIN_WEIGHT = 0.5
    MAX_WEIGHT = 2.0
    MIN_SAMPLES = 3  # Reduced from 5 for faster initial adaptation
    BAYESIAN_THRESHOLD = 10  # Below this, blend with seeded prior
    WARM_START_SAMPLES = 15  # Number of samples for full confidence in adaptive weights

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
            adjusted = 1.0 + corr
            new_weights[comp_name] = max(self.MIN_WEIGHT, min(self.MAX_WEIGHT, adjusted))

        # Confidence-weighted blend: gradually transition from seeded to adaptive weights
        # based on data confidence (sample size between MIN_SAMPLES and WARM_START_SAMPLES)
        n_records = len(records)

        if n_records < self.WARM_START_SAMPLES:
            seeded = load_seeded_weights()

            # Calculate confidence score (0.0 to 1.0) based on sample size
            # 0.0 at MIN_SAMPLES, 1.0 at WARM_START_SAMPLES
            if n_records <= self.MIN_SAMPLES:
                confidence = 0.0
            else:
                confidence = (n_records - self.MIN_SAMPLES) / (self.WARM_START_SAMPLES - self.MIN_SAMPLES)
                confidence = min(1.0, max(0.0, confidence))

            # Use confidence to weight seeded vs adaptive weights
            # Low confidence = more seeded weight, High confidence = more adaptive weight
            seeded_weight = 1.0 - confidence * 0.7  # Starts at 1.0, goes down to 0.3
            adaptive_weight = 1.0 - seeded_weight

            blended = {}
            for key in set(list(current_weights.keys()) + list(new_weights.keys())):
                seed_w = seeded.get(key, 1.0)
                reg_w = new_weights.get(key, seed_w)

                # Apply confidence-weighted blend
                if n_records < self.BAYESIAN_THRESHOLD:
                    # Extra conservative for very early runs
                    blended[key] = seed_w * 0.7 + reg_w * 0.3
                else:
                    # Confidence-weighted blend
                    blended[key] = seed_w * seeded_weight + reg_w * adaptive_weight

            logger.info(
                "Confidence-weighted blend applied: %.0f%% seeded + %.0f%% adaptive (confidence: %.2f, %d records)",
                seeded_weight * 100, adaptive_weight * 100, confidence, n_records,
            )
            return blended

        # Standard EMA blend (30% new, 70% old) for mature data
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
