"""
Signal Quality Filter for Top-Percentile Signal Filtering

This module implements top-percentile signal quality filtering to only execute
the highest-quality signals, maximizing profitability under Developer Plan constraints.

Quality Factors (weighted):
- Wallet Quality (WQS): 30% weight
- Timing Score: 25% weight
- Market Regime Alignment: 20% weight
- Ensemble Prediction Confidence: 15% weight
- Signal Freshness: 10% weight

Features:
- Top-percentile signal filtering (default: top 20%)
- Dynamic thresholding based on PnL performance
- Multi-factor quality scoring
- Real-time threshold adjustment
"""

import os
import time
import logging
from datetime import datetime, timedelta
from typing import Dict, List, Optional, Tuple, Any, Callable
from dataclasses import dataclass, field
from enum import Enum
import threading
import json
from pathlib import Path
from collections import deque

logger = logging.getLogger(__name__)


class SignalQuality(Enum):
    """Signal quality indicators."""
    EXCELLENT = "excellent"  # Top 10%
    HIGH = "high"           # Top 10-20%
    GOOD = "good"           # Top 20-40%
    MEDIUM = "medium"       # Top 40-60%
    LOW = "low"             # Top 60-80%
    POOR = "poor"           # Bottom 20%


class ExecuteDecision(Enum):
    """Execution decision for a signal."""
    EXECUTE = "execute"              # Should execute now
    DELAY = "delay"                # Delay execution
    SKIP = "skip"                   # Skip this signal
    HOLD = "hold"                  # Hold for more info


@dataclass
class TradingSignal:
    """Trading signal to evaluate."""
    wallet_address: str
    token_address: str
    wqs_score: float
    timing_score: float  # 0-1 from signal timing optimizer
    market_regime: str  # BULL/BEAR/VOLATILE/NEUTRAL
    ensemble_confidence: float  # 0-1 from ensemble predictor
    signal_age_seconds: int
    pnl_prediction: float  # Predicted PnL from ensemble
    timestamp: float = field(default_factory=time.time)


@dataclass
class QualityScore:
    """Quality score for a trading signal."""
    signal: TradingSignal
    overall_score: float  # 0-1
    quality_level: SignalQuality
    percentile: float  # 0-100
    component_scores: Dict[str, float]
    decision: ExecuteDecision
    reason: str
    delay_seconds: int = 0
    timestamp: float = field(default_factory=time.time)


@dataclass
class FilterConfig:
    """Configuration for signal quality filter."""

    # Quality factor weights
    WQS_WEIGHT: float = 0.30
    TIMING_WEIGHT: float = 0.25
    REGIME_WEIGHT: float = 0.20
    ENSEMBLE_WEIGHT: float = 0.15
    FRESHNESS_WEIGHT: float = 0.10

    # Percentile thresholds
    TOP_PERCENTILE_TARGET: float = 20.0  # Top 20% execute
    MIN_PERCENTILE_THRESHOLD: float = 10.0  # Never drop below top 10%
    MAX_PERCENTILE_THRESHOLD: float = 40.0  # Never exceed top 40%

    # WQS normalization
    WQS_MAX: float = 100.0
    WQS_MIN: float = 0.0

    # Timing thresholds
    TIMING_EXCELLENT: float = 0.8  # Top 20% timing
    TIMING_GOOD: float = 0.6
    TIMING_POOR: float = 0.3

    # Regime alignment scores
    REGIME_ALIGNMENT: Dict[str, Dict[str, float]] = field(default_factory=lambda: {
        "BULL": {"SPEAR": 1.0, "SHIELD": 0.7},
        "BEAR": {"SPEAR": 0.3, "SHIELD": 1.0},
        "VOLATILE": {"SPEAR": 0.5, "SHIELD": 0.9},
        "NEUTRAL": {"SPEAR": 0.7, "SHIELD": 0.7},
    })

    # Ensemble confidence thresholds
    ENSEMBLE_HIGH: float = 0.7
    ENSEMBLE_MEDIUM: float = 0.5
    ENSEMBLE_LOW: float = 0.3

    # Signal freshness
    FRESH_SECONDS: int = 300       # 5 minutes = fresh
    STALE_SECONDS: int = 1800      # 30 minutes = stale

    # Threshold adjustment
    ADAPTIVE_THRESHOLD: bool = True
    THRESHOLD_ADJUSTMENT_INTERVAL: int = 3600  # 1 hour
    PERFORMANCE_LOOKBACK_TRADES: int = 20

    # State persistence
    STATE_FILE: str = "signal_quality_filter_state.json"


class SignalQualityFilter:
    """
    Top-percentile signal quality filter.

    Strategy:
    - Calculate multi-factor quality score (0-1)
    - Rank signals by percentile
    - Only execute top X% (dynamic based on performance)
    - Adjust threshold based on recent PnL

    Features:
    - Multi-factor quality scoring
    - Dynamic percentile thresholding
    - Performance-based threshold adjustment
    - Real-time quality tracking
    """

    def __init__(self, config: Optional[FilterConfig] = None):
        """Initialize the signal quality filter."""
        self._config = config or FilterConfig()
        self._lock = threading.Lock()

        # Current percentile threshold
        self._current_threshold = self._config.TOP_PERCENTILE_TARGET

        # Signal history for percentile calculation
        self._signal_history: deque = deque(maxlen=1000)

        # PnL history for threshold adjustment
        self._pnl_history: deque = deque(maxlen=100)

        # Performance tracking
        self._executed_count = 0
        self._skipped_count = 0
        self._total_signals = 0

        # Last threshold adjustment time
        self._last_threshold_adjustment = time.time()

        # Load state if available
        self._load_state()

        logger.info("SignalQualityFilter initialized")

    def calculate_signal_percentile(self, signal: TradingSignal) -> float:
        """
        Calculate quality percentile for a signal.

        Args:
            signal: Trading signal to evaluate

        Returns:
            Percentile (0-100) - higher is better
        """
        with self._lock:
            # Calculate component scores
            component_scores = self._calculate_component_scores(signal)

            # Calculate weighted overall score
            overall = (
                component_scores['wqs'] * self._config.WQS_WEIGHT +
                component_scores['timing'] * self._config.TIMING_WEIGHT +
                component_scores['regime'] * self._config.REGIME_WEIGHT +
                component_scores['ensemble'] * self._config.ENSEMBLE_WEIGHT +
                component_scores['freshness'] * self._config.FRESHNESS_WEIGHT
            )

            # Convert to percentile using historical distribution
            percentile = self._score_to_percentile(overall)

            return percentile

    def _calculate_component_scores(self, signal: TradingSignal) -> Dict[str, float]:
        """Calculate individual component scores."""
        scores = {}

        # WQS score (normalized 0-1)
        wqs_normalized = (signal.wqs_score - self._config.WQS_MIN) / max(
            0.001, self._config.WQS_MAX - self._config.WQS_MIN
        )
        scores['wqs'] = max(0.0, min(1.0, wqs_normalized))

        # Timing score
        scores['timing'] = signal.timing_score

        # Regime alignment (assume SPEAR strategy by default)
        regime = signal.market_regime.upper() if signal.market_regime else "NEUTRAL"
        regime_scores = self._config.REGIME_ALIGNMENT.get(regime, {})
        # Default to 0.5 for unknown regime
        scores['regime'] = regime_scores.get("SPEAR", 0.5)

        # Ensemble confidence
        scores['ensemble'] = signal.ensemble_confidence

        # Signal freshness (1.0 = fresh, 0.0 = stale)
        if signal.signal_age_seconds <= self._config.FRESH_SECONDS:
            scores['freshness'] = 1.0
        elif signal.signal_age_seconds >= self._config.STALE_SECONDS:
            scores['freshness'] = 0.0
        else:
            # Linear decay
            decay = (signal.signal_age_seconds - self._config.FRESH_SECONDS) / max(
                1, self._config.STALE_SECONDS - self._config.FRESH_SECONDS
            )
            scores['freshness'] = max(0.0, 1.0 - decay)

        return scores

    def _score_to_percentile(self, score: float) -> float:
        """
        Convert raw score to percentile using historical distribution.

        Uses a simplified approach for now - can be enhanced with actual
        percentile calculation from history.
        """
        if not self._signal_history:
            # No history, use linear mapping
            return score * 100

        # Extract historical scores
        historical_scores = [s.overall_score for s in self._signal_history]

        if not historical_scores:
            return score * 100

        # Count how many historical scores are below this score
        below_count = sum(1 for s in historical_scores if s < score)
        percentile = (below_count / len(historical_scores)) * 100

        return min(100.0, max(0.0, percentile))

    def should_execute_signal(self, signal: TradingSignal) -> QualityScore:
        """
        Determine if a signal should be executed.

        Args:
            signal: Trading signal to evaluate

        Returns:
            QualityScore with decision and reasoning
        """
        with self._lock:
            self._total_signals += 1

            # Calculate component scores
            component_scores = self._calculate_component_scores(signal)

            # Calculate overall score
            overall = (
                component_scores['wqs'] * self._config.WQS_WEIGHT +
                component_scores['timing'] * self._config.TIMING_WEIGHT +
                component_scores['regime'] * self._config.REGIME_WEIGHT +
                component_scores['ensemble'] * self._config.ENSEMBLE_WEIGHT +
                component_scores['freshness'] * self._config.FRESHNESS_WEIGHT
            )

            # Calculate percentile
            percentile = self._score_to_percentile(overall)

            # Determine quality level
            if percentile >= 90:
                quality = SignalQuality.EXCELLENT
            elif percentile >= 80:
                quality = SignalQuality.HIGH
            elif percentile >= 60:
                quality = SignalQuality.GOOD
            elif percentile >= 40:
                quality = SignalQuality.MEDIUM
            elif percentile >= 20:
                quality = SignalQuality.LOW
            else:
                quality = SignalQuality.POOR

            # Make execution decision
            threshold = self._current_threshold
            if percentile >= (100 - threshold):
                decision = ExecuteDecision.EXECUTE
                self._executed_count += 1
                reason = f"Top {threshold:.0f}% quality (percentile: {percentile:.1f})"
            else:
                decision = ExecuteDecision.SKIP
                self._skipped_count += 1
                reason = f"Below top {threshold:.0f}% threshold (percentile: {percentile:.1f})"

            # Create quality score
            quality_score = QualityScore(
                signal=signal,
                overall_score=overall,
                quality_level=quality,
                percentile=percentile,
                component_scores=component_scores,
                decision=decision,
                reason=reason,
            )

            # Add to history
            self._signal_history.append(quality_score)

            # Check if threshold adjustment is needed
            if self._config.ADAPTIVE_THRESHOLD:
                self._check_threshold_adjustment()

            logger.debug(
                f"Signal quality: {quality.value} ({percentile:.1f}th percentile), "
                f"decision: {decision.value}"
            )

            return quality_score

    def get_top_percentile_threshold(self) -> float:
        """Get current top percentile threshold."""
        with self._lock:
            return self._current_threshold

    def update_threshold_based_on_performance(self, pnl_history: List[float]) -> None:
        """
        Update threshold based on recent PnL performance.

        Args:
            pnl_history: List of recent PnL values
        """
        with self._lock:
            if not pnl_history or len(pnl_history) < self._config.PERFORMANCE_LOOKBACK_TRADES:
                return

            self._pnl_history.clear()
            self._pnl_history.extend(pnl_history)

            # Calculate metrics
            wins = sum(1 for pnl in pnl_history if pnl > 0)
            win_rate = wins / len(pnl_history)
            avg_pnl = sum(pnl_history) / len(pnl_history)

            # Adjust threshold based on performance
            old_threshold = self._current_threshold

            if win_rate > 0.6 and avg_pnl > 0:
                # Good performance: loosen threshold (allow more signals)
                self._current_threshold = min(
                    self._config.MAX_PERCENTILE_THRESHOLD,
                    self._current_threshold * 1.1
                )
            elif win_rate < 0.4 or avg_pnl < 0:
                # Poor performance: tighten threshold (fewer, better signals)
                self._current_threshold = max(
                    self._config.MIN_PERCENTILE_THRESHOLD,
                    self._current_threshold * 0.9
                )

            if old_threshold != self._current_threshold:
                logger.info(
                    f"Adjusted threshold: {old_threshold:.1f}% -> {self._current_threshold:.1f}% "
                    f"(win_rate: {win_rate:.2f}, avg_pnl: ${avg_pnl:.2f})"
                )

                self._save_state()

    def _check_threshold_adjustment(self) -> None:
        """Check and perform threshold adjustment if needed."""
        now = time.time()
        if now - self._last_threshold_adjustment < self._config.THRESHOLD_ADJUSTMENT_INTERVAL:
            return

        if len(self._pnl_history) >= self._config.PERFORMANCE_LOOKBACK_TRADES:
            pnl_list = list(self._pnl_history)
            self.update_threshold_based_on_performance(pnl_list)

        self._last_threshold_adjustment = now

    def record_execution_result(self, signal: TradingSignal, pnl: float) -> None:
        """
        Record the result of an executed signal.

        Args:
            signal: The signal that was executed
            pnl: Profit/loss from the trade
        """
        with self._lock:
            self._pnl_history.append(pnl)

    def get_filter_stats(self) -> Dict[str, Any]:
        """Get filter statistics."""
        with self._lock:
            return {
                'current_threshold': self._current_threshold,
                'total_signals': self._total_signals,
                'executed_count': self._executed_count,
                'skipped_count': self._skipped_count,
                'execution_rate': (
                    self._executed_count / max(1, self._total_signals)
                ),
                'avg_quality_score': (
                    sum(s.overall_score for s in self._signal_history) / max(1, len(self._signal_history))
                ),
                'recent_win_rate': self._calculate_recent_win_rate(),
            }

    def _calculate_recent_win_rate(self) -> float:
        """Calculate win rate from recent PnL history."""
        if not self._pnl_history:
            return 0.0

        wins = sum(1 for pnl in self._pnl_history if pnl > 0)
        return wins / len(self._pnl_history)

    def get_quality_distribution(self) -> Dict[str, int]:
        """Get distribution of signal qualities in history."""
        with self._lock:
            distribution = {level.value: 0 for level in SignalQuality}

            for score in self._signal_history:
                distribution[score.quality_level.value] += 1

            return distribution

    def reset_statistics(self) -> None:
        """Reset filter statistics."""
        with self._lock:
            self._executed_count = 0
            self._skipped_count = 0
            self._total_signals = 0
            self._signal_history.clear()
            self._pnl_history.clear()

            logger.info("Reset filter statistics")

    def _load_state(self) -> None:
        """Load state from disk."""
        state_file = Path(self._config.STATE_FILE)
        if not state_file.exists():
            return

        try:
            with open(state_file, 'r') as f:
                data = json.load(f)

            self._current_threshold = data.get('current_threshold', self._config.TOP_PERCENTILE_TARGET)
            self._executed_count = data.get('executed_count', 0)
            self._skipped_count = data.get('skipped_count', 0)
            self._total_signals = data.get('total_signals', 0)

            logger.info(f"Loaded state from {state_file}")

        except Exception as e:
            logger.warning(f"Failed to load state: {e}")

    def _save_state(self) -> None:
        """Save state to disk."""
        try:
            data = {
                'current_threshold': self._current_threshold,
                'executed_count': self._executed_count,
                'skipped_count': self._skipped_count,
                'total_signals': self._total_signals,
                'last_save': time.time(),
            }

            state_file = Path(self._config.STATE_FILE)
            with open(state_file, 'w') as f:
                json.dump(data, f, indent=2)

        except Exception as e:
            logger.warning(f"Failed to save state: {e}")

    def save_state(self) -> None:
        """Public method to save state."""
        with self._lock:
            self._save_state()
