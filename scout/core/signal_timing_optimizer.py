"""
Signal Timing Optimizer for Optimal Entry/Exit Execution

This module implements timing optimization for signal execution to maximize
profitability while minimizing adverse selection and slippage.

Timing Factors:
- Time-of-day effects (avoid low-liquidity periods)
- Market condition filters (delay during extreme volatility)
- Wallet momentum filters (only follow positive momentum wallets)
- SOL price momentum (follow the trend)
- Volume spike detection (execute on volume breakouts)

Features:
- Time-of-day optimization (avoid low-liquidity hours)
- Market regime-based timing adjustments
- Wallet momentum scoring and filtering
- SOL price trend following
- Volume spike detection and execution
- Signal aging and decay calculation
"""

import os
import time
import logging
from datetime import datetime, timedelta, time as dt_time
from typing import Dict, List, Optional, Tuple, Any
from dataclasses import dataclass, field
from enum import Enum
import threading
from pathlib import Path

logger = logging.getLogger(__name__)


class TimeOfDayQuality(Enum):
    """Quality of time for signal execution."""
    EXCELLENT = 1  # High volume, high volatility
    GOOD = 2       # Normal conditions
    POOR = 3       # Low volume, low volatility
    AVOID = 4      # Very low liquidity, high risk


class SignalQuality(Enum):
    """Signal quality indicators."""
    HIGH = "high"           # Strong momentum, good timing
    MEDIUM = "medium"       # Decent signal, moderate timing
    LOW = "low"            # Weak signal, poor timing
    REJECT = "reject"      # Should not execute


@dataclass
class TimingConfig:
    """Configuration for timing optimization."""

    # Time-of-day quality thresholds (UTC)
    EXCELLENT_HOURS = [(14, 18), (20, 24)]  # 9AM-1PM & 4PM-8PM UTC (US market hours)
    POOR_HOURS = [(0, 6), (10, 13)]         # Midnight-6AM & 5AM-8AM UTC

    # Momentum thresholds
    POSITIVE_MOMENTUM_THRESHOLD: float = 0.02  # 2% positive momentum required
    NEGATIVE_MOMENTUM_THRESHOLD: float = -0.05 # -5% negative momentum triggers reject

    # Volume thresholds
    VOLUME_SPIKE_MULTIPLIER: float = 2.0  # 2x normal volume = spike
    MIN_VOLUME_THRESHOLD: float = 1000     # Minimum volume for execution

    # Signal aging
    SIGNAL_FRESHNESS_SECONDS: int = 300    # 5 minutes = fresh
    SIGNAL_STALE_SECONDS: int = 1800       # 30 minutes = stale

    # Market condition adjustments
    HIGH_VOLATILE_THRESHOLD: float = 0.10  # 10% volatility = high
    EXTREME_VOLATILE_THRESHOLD: float = 0.15  # 15% volatility = extreme

    # SOL price momentum
    SOL_TREND_LOOKBACK: int = 60           # 60 minutes for trend calculation
    SOL_TREND_STRENGTH_THRESHOLD: float = 0.01  # 1% trend strength required


@dataclass
class TimingScore:
    """Timing score for a signal."""
    overall_score: float  # 0-1
    time_quality: TimeOfDayQuality
    momentum_score: float
    volume_score: float
    freshness_score: float
    market_condition_score: float
    sol_trend_score: float
    recommendation: SignalQuality
    reason: str
    delay_seconds: int = 0
    timestamp: float = field(default_factory=time.time)


class SignalTimingOptimizer:
    """
    Signal timing optimizer for maximizing execution profitability.

    Implements:
    - Time-of-day quality assessment
    - Wallet momentum calculation
    - Volume spike detection
    - Market condition analysis
    - Signal freshness evaluation
    - SOL price trend analysis
    """

    def __init__(self, config: Optional[TimingConfig] = None):
        """Initialize the timing optimizer."""
        self._config = config or TimingConfig()
        self._lock = threading.Lock()

        # Cache for wallet momentum data
        self._momentum_cache: Dict[str, Tuple[float, float]] = {}

        # SOL price history for trend calculation
        self._sol_price_history: List[Tuple[float, float]] = []

        logger.info("Signal Timing Optimizer initialized")

    def get_time_of_day_quality(self, timestamp: Optional[float] = None) -> TimeOfDayQuality:
        """
        Assess time-of-day quality for signal execution.

        Args:
            timestamp: Unix timestamp (default: now)

        Returns:
            TimeOfDayQuality rating
        """
        if timestamp is None:
            timestamp = time.time()

        dt = datetime.utcfromtimestamp(timestamp)
        hour = dt.hour

        # Check excellent hours
        for start, end in self._config.EXCELLENT_HOURS:
            if start <= hour < end:
                return TimeOfDayQuality.EXCELLENT

        # Check poor hours
        for start, end in self._config.POOR_HOURS:
            if start <= hour < end:
                return TimeOfDayQuality.POOR

        # Check avoid hours (late night US time / early morning UTC)
        if 6 <= hour < 10:
            return TimeOfDayQuality.AVOID

        return TimeOfDayQuality.GOOD

    def calculate_wallet_momentum(self, wallet_returns: List[float]) -> float:
        """
        Calculate wallet momentum score from recent returns.

        Args:
            wallet_returns: List of recent trade returns

        Returns:
            Momentum score (-1 to 1)
        """
        if not wallet_returns:
            return 0.0

        # Weight recent returns more heavily
        weights = [i + 1 for i in range(len(wallet_returns))]
        weighted_sum = sum(r * w for r, w in zip(wallet_returns, weights))
        total_weight = sum(weights)

        momentum = weighted_sum / total_weight if total_weight > 0 else 0.0

        # Normalize to -1 to 1 range
        return max(-1.0, min(1.0, momentum))

    def detect_volume_spike(self, current_volume: float, avg_volume: float) -> bool:
        """
        Detect if current volume is a spike.

        Args:
            current_volume: Current trading volume
            avg_volume: Average volume

        Returns:
            True if volume spike detected
        """
        if avg_volume <= 0:
            return False

        ratio = current_volume / avg_volume
        return ratio >= self._config.VOLUME_SPIKE_MULTIPLIER

    def calculate_signal_freshness(self, signal_time: float,
                                  current_time: Optional[float] = None) -> float:
        """
        Calculate signal freshness score.

        Args:
            signal_time: When signal was generated
            current_time: Current time (default: now)

        Returns:
            Freshness score (0-1, 1 = fresh)
        """
        if current_time is None:
            current_time = time.time()

        age = current_time - signal_time

        if age <= self._config.SIGNAL_FRESHNESS_SECONDS:
            return 1.0  # Fresh
        elif age >= self._config.SIGNAL_STALE_SECONDS:
            return 0.0  # Stale
        else:
            # Linear decay
            age_range = self._config.SIGNAL_STALE_SECONDS - self._config.SIGNAL_FRESHNESS_SECONDS
            normalized_age = (age - self._config.SIGNAL_FRESHNESS_SECONDS) / age_range
            return 1.0 - normalized_age

    def calculate_sol_trend(self, sol_price: float) -> float:
        """
        Calculate SOL price trend.

        Args:
            sol_price: Current SOL price

        Returns:
            Trend score (-1 to 1, positive = uptrend)
        """
        # Add to price history
        self._sol_price_history.append((time.time(), sol_price))

        # Keep only recent history
        cutoff = time.time() - self._config.SOL_TREND_LOOKBACK * 60
        self._sol_price_history = [(t, p) for t, p in self._sol_price_history if t > cutoff]

        if len(self._sol_price_history) < 2:
            return 0.0

        # Calculate trend
        oldest_price = self._sol_price_history[0][1]
        newest_price = self._sol_price_history[-1][1]

        if oldest_price <= 0:
            return 0.0

        trend = (newest_price - oldest_price) / oldest_price

        # Normalize to -1 to 1
        return max(-1.0, min(1.0, trend / 0.05))  # 5% move = full trend

    def evaluate_timing(
        self,
        wallet_address: str,
        wallet_returns: List[float],
        signal_time: float,
        current_volume: Optional[float] = None,
        avg_volume: Optional[float] = None,
        sol_price: Optional[float] = None,
        volatility: Optional[float] = None,
    ) -> TimingScore:
        """
        Evaluate overall timing quality for a signal.

        Args:
            wallet_address: Wallet address
            wallet_returns: Recent wallet trade returns
            signal_time: When signal was generated
            current_volume: Current trading volume
            avg_volume: Average volume for comparison
            sol_price: Current SOL price
            volatility: Market volatility

        Returns:
            TimingScore with recommendation
        """
        # Time of day quality
        time_quality = self.get_time_of_day_quality(signal_time)
        time_score = {
            TimeOfDayQuality.EXCELLENT: 1.0,
            TimeOfDayQuality.GOOD: 0.75,
            TimeOfDayQuality.POOR: 0.5,
            TimeOfDayQuality.AVOID: 0.0,
        }[time_quality]

        # Wallet momentum
        momentum = self.calculate_wallet_momentum(wallet_returns)

        # Volume spike detection
        if current_volume and avg_volume:
            is_spike = self.detect_volume_spike(current_volume, avg_volume)
            volume_score = 1.0 if is_spike else 0.5
        else:
            volume_score = 0.5  # Neutral

        # Signal freshness
        freshness = self.calculate_signal_freshness(signal_time)

        # Market condition (volatility-based)
        if volatility:
            if volatility > self._config.EXTREME_VOLATILE_THRESHOLD:
                market_score = 0.0  # Too volatile
            elif volatility > self._config.HIGH_VOLATILE_THRESHOLD:
                market_score = 0.5  # Caution
            else:
                market_score = 1.0  # Normal
        else:
            market_score = 1.0

        # SOL trend
        if sol_price:
            sol_trend = self.calculate_sol_trend(sol_price)
            sol_score = (sol_trend + 1) / 2  # Convert to 0-1
        else:
            sol_trend = 0.0
            sol_score = 0.5  # Neutral

        # Calculate overall score
        weights = {
            'time': 0.2,
            'momentum': 0.25,
            'volume': 0.15,
            'freshness': 0.15,
            'market': 0.15,
            'sol_trend': 0.1,
        }

        overall = (
            time_score * weights['time'] +
            (momentum + 1) / 2 * weights['momentum'] +
            volume_score * weights['volume'] +
            freshness * weights['freshness'] +
            market_score * weights['market'] +
            sol_score * weights['sol_trend']
        )

        # Determine recommendation
        if overall >= 0.7:
            quality = SignalQuality.HIGH
            reason = "Excellent timing conditions"
            delay = 0
        elif overall >= 0.5:
            quality = SignalQuality.MEDIUM
            reason = "Acceptable timing with some caution"
            delay = 60  # Delay 1 minute
        elif overall >= 0.3:
            quality = SignalQuality.LOW
            reason = "Poor timing, consider skipping"
            delay = 300  # Delay 5 minutes
        else:
            quality = SignalQuality.REJECT
            reason = "Poor timing conditions, reject signal"
            delay = -1  # Do not execute

        return TimingScore(
            overall_score=overall,
            time_quality=time_quality,
            momentum_score=momentum,
            volume_score=volume_score,
            freshness_score=freshness,
            market_condition_score=market_score,
            sol_trend_score=sol_trend,
            recommendation=quality,
            reason=reason,
            delay_seconds=delay,
        )

    def should_execute_signal(
        self,
        wallet_address: str,
        wallet_returns: List[float],
        signal_time: float,
        **kwargs
    ) -> Tuple[bool, str, int]:
        """
        Quick check if signal should be executed.

        Args:
            wallet_address: Wallet address
            wallet_returns: Recent wallet returns
            signal_time: When signal was generated
            **kwargs: Additional parameters for evaluate_timing

        Returns:
            Tuple of (should_execute, reason, delay_seconds)
        """
        score = self.evaluate_timing(
            wallet_address=wallet_address,
            wallet_returns=wallet_returns,
            signal_time=signal_time,
            **kwargs
        )

        if score.recommendation == SignalQuality.REJECT:
            return False, score.reason, -1
        elif score.recommendation == SignalQuality.LOW:
            return False, score.reason, score.delay_seconds
        else:
            return True, score.reason, score.delay_seconds

    def get_timing_summary(self, score: TimingScore) -> Dict[str, Any]:
        """Get summary of timing score."""
        return {
            "overall_score": score.overall_score * 100,
            "recommendation": score.recommendation.value,
            "reason": score.reason,
            "delay_seconds": score.delay_seconds,
            "time_quality": score.time_quality.value,
            "momentum_score": score.momentum_score,
            "volume_score": score.volume_score,
            "freshness_score": score.freshness_score * 100,
            "market_condition_score": score.market_condition_score * 100,
            "sol_trend_score": score.sol_trend_score,
        }

    def print_timing_report(self, score: TimingScore):
        """Print comprehensive timing report."""
        summary = self.get_timing_summary(score)

        print("\n" + "="*70)
        print("SIGNAL TIMING REPORT")
        print("="*70)

        print(f"\nRecommendation: {summary['recommendation'].upper()}")
        print(f"Overall Score: {summary['overall_score']:.0f}/100")
        print(f"Reason: {summary['reason']}")

        if summary['delay_seconds'] > 0:
            print(f"Delay: {summary['delay_seconds']} seconds")
        elif summary['delay_seconds'] < 0:
            print("Action: DO NOT EXECUTE")

        print(f"\nComponent Scores:")
        print(f"  Time Quality: {summary['time_quality'].capitalize()}")
        print(f"  Wallet Momentum: {summary['momentum_score']:+.2f}")
        print(f"  Volume Spike: {summary['volume_score']*100:.0f}%")
        print(f"  Freshness: {summary['freshness_score']:.0f}%")
        print(f"  Market Conditions: {summary['market_condition_score']:.0f}%")
        print(f"  SOL Trend: {summary['sol_trend_score']:+.2f}")

        print("="*70 + "\n")


# Global singleton instance
_optimizer: Optional[SignalTimingOptimizer] = None
_optimizer_lock = threading.Lock()


def get_signal_timing_optimizer() -> SignalTimingOptimizer:
    """Get the global signal timing optimizer singleton."""
    global _optimizer

    with _optimizer_lock:
        if _optimizer is None:
            _optimizer = SignalTimingOptimizer()

    return _optimizer


if __name__ == "__main__":
    # Test the signal timing optimizer
    optimizer = get_signal_timing_optimizer()

    # Test timing evaluation
    print("Testing signal timing evaluation:")
    score = optimizer.evaluate_timing(
        wallet_address="test_wallet",
        wallet_returns=[0.05, 0.03, -0.02, 0.08, 0.04],
        signal_time=time.time(),
        current_volume=5000,
        avg_volume=2000,
        sol_price=150.0,
        volatility=0.08,
    )

    optimizer.print_timing_report(score)
