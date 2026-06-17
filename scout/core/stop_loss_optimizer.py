"""
Dynamic Stop-Loss Optimizer with ATR and Market Regime Adjustment

This module implements dynamic stop-loss optimization based on:
- Average True Range (ATR) volatility measurement
- Market regime adjustment (BULL/BEAR/VOLATILE/NEUTRAL)
- Growth-stage aware stop levels
- Trailing stop implementation
- Risk-adjusted stop placement

ATR-Based Stop Loss Formula:
stop_loss = entry_price - (ATR * multiplier)

Market Regime Multipliers:
- BULL: 1.5x (wider stops to avoid premature exits)
- BEAR: 1.0x (tighter stops for capital preservation)
- VOLATILE: 2.0x (widest stops to accommodate volatility)
- NEUTRAL: 1.25x (standard stops)

Features:
- ATR-based dynamic stop calculation
- Market regime-adjusted multipliers
- Trailing stop implementation
- Growth-stage aware stop levels
- Risk/reward optimization
- Stop-loss alerting
"""

import os
import time
import logging
from datetime import datetime, timedelta
from typing import Dict, List, Optional, Tuple, Any
from dataclasses import dataclass, field
from enum import Enum
import threading
import json
from pathlib import Path

logger = logging.getLogger(__name__)


class MarketRegime(Enum):
    """Market regime types for stop adjustment."""
    BULL = "bull"          # Strong upward momentum
    BEAR = "bear"          # Downward momentum
    VOLATILE = "volatile"  # High volatility
    NEUTRAL = "neutral"    # Sideways/ranging


class StopType(Enum):
    """Types of stop losses."""
    FIXED = "fixed"              # Fixed percentage stop
    ATR = "atr"                  # ATR-based stop
    TRAILING = "trailing"        # Trailing stop
    TRAILING_ATR = "trailing_atr"  # ATR-based trailing stop


@dataclass
class StopLossConfig:
    """Configuration for stop-loss optimization."""

    # ATR calculation
    ATR_PERIOD: int = 14          # 14-period ATR (standard)
    ATR_MULTIPLIER_DEFAULT: float = 1.5

    # Market regime multipliers
    BULL_MULTIPLIER: float = 1.5
    BEAR_MULTIPLIER: float = 1.0
    VOLATILE_MULTIPLIER: float = 2.0
    NEUTRAL_MULTIPLIER: float = 1.25

    # Fixed stop percentages (fallback)
    DEFAULT_FIXED_STOP: float = 0.05    # 5%
    AGGRESSIVE_FIXED_STOP: float = 0.08  # 8% (for aggressive growth)

    # Trailing stop settings
    TRAILING_ACTIVATE_PCT: float = 0.02  # Activate when 2% in profit
    TRAILING_DISTANCE_PCT: float = 0.03   # Trail by 3%

    # Growth stage adjustments
    EARLY_STAGE_TIGHTEN: float = 0.8     # Tighten stops by 20% in early stage
    FINAL_STAGE_WIDEN: float = 1.2       # Widen stops by 20% in final stage

    # Risk/reward optimization
    MIN_RISK_REWARD: float = 2.0    # Minimum 2:1 risk/reward ratio
    MAX_RISK_PER_TRADE: float = 0.03  # Maximum 3% risk per trade


@dataclass
class StopLossOrder:
    """Stop-loss order with metadata."""
    entry_price: float
    stop_price: float
    stop_type: StopType
    regime: MarketRegime
    atr_value: Optional[float] = None
    multiplier_used: float = 1.0
    distance_pct: float = 0.0
    risk_amount: float = 0.0
    risk_pct: float = 0.0
    reward_target: Optional[float] = None
    risk_reward_ratio: Optional[float] = None
    is_trailing: bool = False
    trailing_high: Optional[float] = None
    created_at: float = field(default_factory=time.time)
    updated_at: float = field(default_factory=time.time)


class StopLossOptimizer:
    """
    Dynamic stop-loss optimizer with ATR and market regime adjustment.

    Implements:
    - ATR-based stop calculation
    - Market regime-adjusted multipliers
    - Trailing stop implementation
    - Growth-stage aware adjustments
    - Risk/reward optimization
    """

    def __init__(self, config: Optional[StopLossConfig] = None):
        """Initialize the stop-loss optimizer."""
        self._config = config or StopLossConfig()
        self._regime = MarketRegime.NEUTRAL
        self._lock = threading.Lock()

        # ATR calculation cache
        self._atr_cache: Dict[str, Tuple[float, float]] = {}  # symbol -> (atr, timestamp)

        logger.info("Stop-Loss Optimizer initialized")
        logger.info(f"  Default ATR multiplier: {self._config.ATR_MULTIPLIER_DEFAULT}")
        logger.info(f"  Market regime: {self._regime.value}")

    def calculate_atr(self, prices: List[float], period: int = 14) -> float:
        """
        Calculate Average True Range (ATR).

        Args:
            prices: List of closing prices
            period: ATR period (default 14)

        Returns:
            ATR value
        """
        if len(prices) < period + 1:
            # Not enough data, use simple range
            if len(prices) >= 2:
                return max(prices) - min(prices)
            return 0.0

        # Calculate true ranges
        true_ranges = []
        for i in range(1, len(prices)):
            high = prices[i]  # Simplified (using close as high)
            low = prices[i]   # Simplified (using close as low)
            prev_close = prices[i - 1]

            tr = max(
                high - low,
                abs(high - prev_close),
                abs(low - prev_close)
            )
            true_ranges.append(tr)

        # Calculate ATR
        if len(true_ranges) >= period:
            return sum(true_ranges[-period:]) / period
        else:
            return sum(true_ranges) / len(true_ranges)

    def get_regime_multiplier(self, regime: MarketRegime) -> float:
        """Get ATR multiplier for market regime."""
        if regime == MarketRegime.BULL:
            return self._config.BULL_MULTIPLIER
        elif regime == MarketRegime.BEAR:
            return self._config.BEAR_MULTIPLIER
        elif regime == MarketRegime.VOLATILE:
            return self._config.VOLATILE_MULTIPLIER
        else:  # NEUTRAL
            return self._config.NEUTRAL_MULTIPLIER

    def calculate_atr_stop(
        self,
        entry_price: float,
        atr_value: float,
        regime: MarketRegime = MarketRegime.NEUTRAL,
        growth_stage: str = "mid",
    ) -> StopLossOrder:
        """
        Calculate ATR-based stop loss.

        Args:
            entry_price: Entry price
            atr_value: Current ATR value
            regime: Market regime
            growth_stage: Growth stage (early/mid/growth/final)

        Returns:
            StopLossOrder with calculated stop
        """
        # Get regime multiplier
        regime_multiplier = self.get_regime_multiplier(regime)

        # Apply growth stage adjustment
        if growth_stage == "early":
            regime_multiplier *= self._config.EARLY_STAGE_TIGHTEN
        elif growth_stage == "final":
            regime_multiplier *= self._config.FINAL_STAGE_WIDEN

        # Calculate stop price
        atr_distance = atr_value * self._config.ATR_MULTIPLIER_DEFAULT * regime_multiplier
        stop_price = entry_price - atr_distance

        # Calculate metrics
        distance_pct = atr_distance / entry_price if entry_price > 0 else 0
        risk_amount = entry_price - stop_price

        return StopLossOrder(
            entry_price=entry_price,
            stop_price=stop_price,
            stop_type=StopType.ATR,
            regime=regime,
            atr_value=atr_value,
            multiplier_used=self._config.ATR_MULTIPLIER_DEFAULT * regime_multiplier,
            distance_pct=distance_pct,
            risk_amount=risk_amount,
            risk_pct=distance_pct,
        )

    def calculate_fixed_stop(
        self,
        entry_price: float,
        stop_pct: Optional[float] = None,
        regime: MarketRegime = MarketRegime.NEUTRAL,
        growth_stage: str = "mid",
    ) -> StopLossOrder:
        """
        Calculate fixed percentage stop loss.

        Args:
            entry_price: Entry price
            stop_pct: Stop percentage (default uses config)
            regime: Market regime
            growth_stage: Growth stage

        Returns:
            StopLossOrder with calculated stop
        """
        if stop_pct is None:
            # Use aggressive fixed stop for growth mode
            stop_pct = self._config.AGGRESSIVE_FIXED_STOP if growth_stage in ["growth", "final"] else self._config.DEFAULT_FIXED_STOP

        # Apply regime adjustment
        multiplier = self.get_regime_multiplier(regime) / self._config.NEUTRAL_MULTIPLIER
        adjusted_pct = stop_pct * multiplier

        stop_price = entry_price * (1 - adjusted_pct)
        distance_pct = adjusted_pct
        risk_amount = entry_price - stop_price

        return StopLossOrder(
            entry_price=entry_price,
            stop_price=stop_price,
            stop_type=StopType.FIXED,
            regime=regime,
            multiplier_used=multiplier,
            distance_pct=distance_pct,
            risk_amount=risk_amount,
            risk_pct=distance_pct,
        )

    def calculate_trailing_stop(
        self,
        entry_price: float,
        current_price: float,
        atr_value: Optional[float] = None,
        trail_pct: Optional[float] = None,
        regime: MarketRegime = MarketRegime.NEUTRAL,
    ) -> StopLossOrder:
        """
        Calculate trailing stop loss.

        Args:
            entry_price: Original entry price
            current_price: Current market price
            atr_value: ATR value (optional, for ATR-based trailing)
            trail_pct: Trail percentage (optional)
            regime: Market regime

        Returns:
            StopLossOrder with trailing stop
        """
        if atr_value and trail_pct is None:
            # ATR-based trailing stop
            multiplier = self.get_regime_multiplier(regime)
            trail_distance = atr_value * self._config.ATR_MULTIPLIER_DEFAULT * multiplier
            stop_price = current_price - trail_distance
            stop_type = StopType.TRAILING_ATR
        else:
            # Percentage-based trailing stop
            if trail_pct is None:
                trail_pct = self._config.TRAILING_DISTANCE_PCT
            stop_price = current_price * (1 - trail_pct)
            stop_type = StopType.TRAILING

        distance_pct = (current_price - stop_price) / current_price if current_price > 0 else 0
        risk_amount = current_price - stop_price

        return StopLossOrder(
            entry_price=entry_price,
            stop_price=stop_price,
            stop_type=stop_type,
            regime=regime,
            atr_value=atr_value,
            distance_pct=distance_pct,
            risk_amount=risk_amount,
            risk_pct=distance_pct,
            is_trailing=True,
            trailing_high=current_price,
        )

    def optimize_risk_reward(
        self,
        entry_price: float,
        stop_loss: StopLossOrder,
        target_price: Optional[float] = None,
        min_risk_reward: float = 2.0,
    ) -> StopLossOrder:
        """
        Optimize stop loss for minimum risk/reward ratio.

        Args:
            entry_price: Entry price
            stop_loss: Calculated stop loss
            target_price: Target exit price (optional)
            min_risk_reward: Minimum risk/reward ratio (default 2.0)

        Returns:
            Optimized StopLossOrder
        """
        if target_price is None:
            # Set default target at 2:1 risk/reward
            risk_amount = entry_price - stop_loss.stop_price
            target_price = entry_price + (risk_amount * min_risk_reward)

        # Calculate risk/reward
        risk = entry_price - stop_loss.stop_price
        reward = target_price - entry_price
        risk_reward = reward / risk if risk > 0 else 0

        # Adjust stop if risk/reward is insufficient
        if risk_reward < min_risk_reward and risk > 0:
            # Tighten stop to improve ratio
            new_risk = reward / min_risk_reward
            new_stop = entry_price - new_risk

            stop_loss.stop_price = max(new_stop, entry_price * 0.9)  # Don't go below 10% stop
            stop_loss.risk_amount = entry_price - stop_loss.stop_price
            stop_loss.risk_pct = stop_loss.risk_amount / entry_price

        stop_loss.reward_target = target_price
        stop_loss.risk_reward_ratio = reward / (entry_price - stop_loss.stop_price) if (entry_price - stop_loss.stop_price) > 0 else 0

        return stop_loss

    def set_regime(self, regime: MarketRegime):
        """Update market regime for future calculations."""
        with self._lock:
            old_regime = self._regime
            self._regime = regime
            if old_regime != regime:
                logger.info(f"Market regime updated: {old_regime.value} → {regime.value}")

    def get_stop_summary(self, stop_order: StopLossOrder) -> Dict[str, Any]:
        """Get summary of stop loss order."""
        return {
            "entry_price": stop_order.entry_price,
            "stop_price": stop_order.stop_price,
            "stop_type": stop_order.stop_type.value,
            "regime": stop_order.regime.value,
            "distance_pct": stop_order.distance_pct * 100,
            "risk_amount": stop_order.risk_amount,
            "risk_pct": stop_order.risk_pct * 100,
            "reward_target": stop_order.reward_target,
            "risk_reward_ratio": stop_order.risk_reward_ratio,
            "is_trailing": stop_order.is_trailing,
            "trailing_high": stop_order.trailing_high,
        }

    def print_stop_report(self, stop_order: StopLossOrder):
        """Print comprehensive stop loss report."""
        summary = self.get_stop_summary(stop_order)

        print("\n" + "="*70)
        print("STOP-LOSS ORDER REPORT")
        print("="*70)

        print(f"\nEntry Price: ${summary['entry_price']:.4f}")
        print(f"Stop Price: ${summary['stop_price']:.4f}")
        print(f"Stop Type: {summary['stop_type'].upper()}")
        print(f"Market Regime: {summary['regime'].upper()}")

        print(f"\nRisk Metrics:")
        print(f"  Distance: {summary['distance_pct']:.2f}%")
        print(f"  Risk Amount: ${summary['risk_amount']:.4f}")
        print(f"  Risk %: {summary['risk_pct']:.2f}%")

        if summary['reward_target']:
            print(f"\nTarget:")
            print(f"  Target Price: ${summary['reward_target']:.4f}")
            print(f"  Risk/Reward: 1:{summary['risk_reward_ratio']:.2f}")

        if summary['is_trailing']:
            print(f"\nTrailing:")
            print(f"  Trailing High: ${summary['trailing_high']:.4f}")

        print("="*70 + "\n")


# Global singleton instance
_optimizer: Optional[StopLossOptimizer] = None
_optimizer_lock = threading.Lock()


def get_stop_loss_optimizer() -> StopLossOptimizer:
    """Get the global stop-loss optimizer singleton."""
    global _optimizer

    with _optimizer_lock:
        if _optimizer is None:
            _optimizer = StopLossOptimizer()

    return _optimizer


def reset_stop_loss_optimizer():
    """Reset the global stop-loss optimizer (mainly for testing)."""
    global _optimizer

    with _optimizer_lock:
        if _optimizer:
            del _optimizer
        _optimizer = None


if __name__ == "__main__":
    # Test the stop-loss optimizer
    optimizer = get_stop_loss_optimizer()

    # Test ATR-based stop
    print("Testing ATR-based stop loss:")
    entry = 100.0
    atr = 2.0
    stop = optimizer.calculate_atr_stop(entry, atr, MarketRegime.NEUTRAL)
    print(f"  Entry ${entry}, ATR ${atr}: Stop ${stop.stop_price:.2f} ({stop.distance_pct*100:.1f}%)")

    # Test fixed stop
    print("\nTesting fixed stop loss:")
    stop = optimizer.calculate_fixed_stop(entry, 0.05, MarketRegime.BULL)
    print(f"  Entry ${entry}, 5% stop: Stop ${stop.stop_price:.2f} ({stop.distance_pct*100:.1f}%)")

    # Test trailing stop
    print("\nTesting trailing stop:")
    current = 105.0
    stop = optimizer.calculate_trailing_stop(entry, current, atr, MarketRegime.VOLATILE)
    print(f"  Entry ${entry}, Current ${current}: Stop ${stop.stop_price:.2f}")

    # Print full report
    optimizer.print_stop_report(stop)
