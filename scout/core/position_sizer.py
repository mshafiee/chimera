"""
Kelly Criterion Position Sizer with Growth-Stage Optimization

.. DEPRECATED::
   This module is a standalone duplicate of position sizing functionality.
   Active implementation: ``scout/core/strategy_allocator.py`` - which already
   implements Kelly Criterion position sizing with growth-stage optimization and
   is integrated into the analysis pipeline.

This module implements position sizing based on the Kelly Criterion with
growth-stage multipliers for aggressive capital growth.

Kelly Criterion Formula:
f* = (bp - q) / b

Where:
- f* = fraction of capital to wager
- b = odds received on the bet (win/loss ratio)
- p = probability of winning
- q = probability of losing (1 - p)

Growth Stage Multipliers (Aggressive Strategy):
- <$300: 0.3x (conservative early stage)
- $300-500: 0.5x
- $500-800: 0.8x
- $800+: 1.0x (full Kelly for maximum growth)

Features:
- Kelly Criterion calculation with safety multipliers
- Growth-stage adjusted position sizing
- Risk-adjusted sizing based on WQS confidence
- Portfolio-level position limits
- Volatility-aware sizing adjustments
- Integration with strategy allocator
"""

import time
import logging
from typing import Dict, List, Optional, Tuple, Any
from dataclasses import dataclass, field
from enum import Enum
import threading

logger = logging.getLogger(__name__)


class GrowthStage(Enum):
    """Growth stages for position sizing."""
    EARLY = "early"       # <$300: 0.3x multiplier
    MID = "mid"           # $300-500: 0.5x multiplier
    GROWTH = "growth"     # $500-800: 0.8x multiplier
    FINAL = "final"       # $800+: 1.0x multiplier


@dataclass
class PositionSizingConfig:
    """Configuration for position sizing."""

    # Growth stage capital thresholds
    STAGE_EARLY_MAX: float = 300.0
    STAGE_MID_MAX: float = 500.0
    STAGE_GROWTH_MAX: float = 800.0

    # Growth stage multipliers (aggressive strategy)
    EARLY_MULTIPLIER: float = 0.3
    MID_MULTIPLIER: float = 0.5
    GROWTH_MULTIPLIER: float = 0.8
    FINAL_MULTIPLIER: float = 1.0

    # Kelly Criterion parameters
    DEFAULT_WIN_RATE: float = 0.55
    DEFAULT_WIN_LOSS_RATIO: float = 2.0  # Average win is 2x average loss

    # Safety limits
    MAX_POSITION_PCT: float = 0.20      # Max 20% of capital in single position
    MIN_POSITION_PCT: float = 0.02      # Min 2% of capital per position
    MAX_TOTAL_EXPOSURE: float = 0.90    # Max 90% total exposure
    KELLY_FRACTION_CAP: float = 0.25     # Cap Kelly fraction at 25%

    # Risk adjustments
    VOLATILITY_PENALTY: float = 0.5     # Reduce size by 50% in high volatility
    LOW_CONFIDENCE_PENALTY: float = 0.7  # Reduce size by 30% for low confidence


@dataclass
class PositionSize:
    """Calculated position size with metadata."""
    size_usd: float
    size_pct: float
    kelly_fraction: float
    growth_multiplier: float
    confidence: float
    stage: GrowthStage
    timestamp: float = field(default_factory=time.time)


class PositionSizer:
    """
    Kelly Criterion-based position sizer with growth-stage optimization.

    Implements:
    - Kelly Criterion calculation
    - Growth-stage multipliers for aggressive growth
    - Risk-adjusted sizing based on WQS confidence
    - Portfolio-level limits
    - Volatility-aware adjustments
    """

    def __init__(self, config: Optional[PositionSizingConfig] = None):
        """Initialize the position sizer."""
        self._config = config or PositionSizingConfig()
        self._lock = threading.Lock()

        logger.info("Position Sizer initialized")
        logger.info(f"  Max position: {self._config.MAX_POSITION_PCT*100:.0f}% of capital")
        logger.info(f"  Kelly cap: {self._config.KELLY_FRACTION_CAP*100:.0f}%")

    def get_growth_stage(self, capital: float) -> GrowthStage:
        """Determine growth stage based on capital."""
        if capital < self._config.STAGE_EARLY_MAX:
            return GrowthStage.EARLY
        elif capital < self._config.STAGE_MID_MAX:
            return GrowthStage.MID
        elif capital < self._config.STAGE_GROWTH_MAX:
            return GrowthStage.GROWTH
        else:
            return GrowthStage.FINAL

    def get_growth_multiplier(self, stage: GrowthStage) -> float:
        """Get growth multiplier for stage."""
        if stage == GrowthStage.EARLY:
            return self._config.EARLY_MULTIPLIER
        elif stage == GrowthStage.MID:
            return self._config.MID_MULTIPLIER
        elif stage == GrowthStage.GROWTH:
            return self._config.GROWTH_MULTIPLIER
        else:  # FINAL
            return self._config.FINAL_MULTIPLIER

    def calculate_kelly_fraction(self, win_rate: float, win_loss_ratio: float) -> float:
        """
        Calculate Kelly Criterion fraction.

        Args:
            win_rate: Probability of winning (0-1)
            win_loss_ratio: Average win / average loss ratio

        Returns:
            Kelly fraction (0-1)
        """
        if win_loss_ratio <= 0:
            return 0.0

        lose_rate = 1.0 - win_rate
        kelly = (win_rate * win_loss_ratio - lose_rate) / win_loss_ratio

        # Cap at maximum Kelly fraction
        return max(0.0, min(kelly, self._config.KELLY_FRACTION_CAP))

    def calculate_position_size(
        self,
        capital: float,
        win_rate: float,
        win_loss_ratio: float = 2.0,
        confidence: float = 0.5,
        volatility_adjustment: float = 1.0,
        strategy_capital: Optional[float] = None,
    ) -> PositionSize:
        """
        Calculate optimal position size using Kelly Criterion.

        Args:
            capital: Total available capital
            win_rate: Historical win rate (0-1)
            win_loss_ratio: Average win/loss ratio
            confidence: WQS-based confidence (0-1)
            volatility_adjustment: Volatility penalty (0-1, lower = more penalty)
            strategy_capital: Capital allocated to this strategy (defaults to total)

        Returns:
            PositionSize with calculated size and metadata
        """
        with self._lock:
            # Determine growth stage
            stage = self.get_growth_stage(capital)
            growth_multiplier = self.get_growth_multiplier(stage)

            # Calculate Kelly fraction
            kelly_fraction = self.calculate_kelly_fraction(win_rate, win_loss_ratio)

            # Apply growth multiplier
            adjusted_fraction = kelly_fraction * growth_multiplier

            # Apply confidence adjustment
            if confidence < 0.5:
                adjusted_fraction *= self._config.LOW_CONFIDENCE_PENALTY

            # Apply volatility adjustment
            if volatility_adjustment < 0.8:
                adjusted_fraction *= self._config.VOLATILITY_PENALTY

            # Calculate position size
            base_capital = strategy_capital or capital
            position_size_usd = base_capital * adjusted_fraction
            position_size_pct = position_size_usd / capital if capital > 0 else 0

            # Apply safety limits
            position_size_pct = max(
                self._config.MIN_POSITION_PCT,
                min(position_size_pct, self._config.MAX_POSITION_PCT)
            )
            position_size_usd = capital * position_size_pct

            return PositionSize(
                size_usd=position_size_usd,
                size_pct=position_size_pct,
                kelly_fraction=kelly_fraction,
                growth_multiplier=growth_multiplier,
                confidence=confidence,
                stage=stage,
            )

    def calculate_portfolio_sizes(
        self,
        capital: float,
        wallet_predictions: List[Tuple[str, float, float]],
        strategy_capital: Optional[float] = None,
    ) -> Dict[str, PositionSize]:
        """
        Calculate position sizes for multiple wallets.

        Args:
            capital: Total capital
            wallet_predictions: List of (wallet, win_rate, confidence) tuples
            strategy_capital: Capital allocated to strategy

        Returns:
            Dictionary mapping wallet to PositionSize
        """
        sizes = {}

        # Calculate individual sizes
        for wallet, win_rate, confidence in wallet_predictions:
            size = self.calculate_position_size(
                capital=capital,
                win_rate=win_rate,
                win_loss_ratio=self._config.DEFAULT_WIN_LOSS_RATIO,
                confidence=confidence,
                strategy_capital=strategy_capital,
            )
            sizes[wallet] = size

        # Normalize to ensure we don't exceed total exposure
        total_exposure = sum(s.size_pct for s in sizes.values())
        if total_exposure > self._config.MAX_TOTAL_EXPOSURE:
            scale_factor = self._config.MAX_TOTAL_EXPOSURE / total_exposure
            for wallet in sizes:
                sizes[wallet].size_pct *= scale_factor
                sizes[wallet].size_usd *= scale_factor

        return sizes

    def calculate_risk_adjusted_size(
        self,
        capital: float,
        wqs_score: float,
        historical_roi: float,
        max_drawdown: float,
        volatility: float = 0.0,
    ) -> PositionSize:
        """
        Calculate position size adjusted for risk metrics.

        Args:
            capital: Total capital
            wqs_score: Wallet Quality Score (0-100)
            historical_roi: Historical ROI (decimal)
            max_drawdown: Maximum historical drawdown (decimal)
            volatility: Recent volatility (decimal)

        Returns:
            PositionSize with risk-adjusted sizing
        """
        # Convert WQS to win rate estimate
        win_rate = min(0.9, max(0.5, wqs_score / 100.0))

        # Estimate win/loss ratio from ROI and drawdown
        if max_drawdown > 0:
            win_loss_ratio = historical_roi / max_drawdown
        else:
            win_loss_ratio = self._config.DEFAULT_WIN_LOSS_RATIO

        # Volatility adjustment (0-1 scale)
        vol_penalty = max(0.5, 1.0 - volatility)

        return self.calculate_position_size(
            capital=capital,
            win_rate=win_rate,
            win_loss_ratio=win_loss_ratio,
            confidence=wqs_score / 100.0,
            volatility_adjustment=vol_penalty,
        )

    def get_sizing_summary(self, capital: float) -> Dict[str, Any]:
        """Get sizing summary for current capital."""
        stage = self.get_growth_stage(capital)
        multiplier = self.get_growth_multiplier(stage)

        # Calculate example sizes
        high_confidence = self.calculate_position_size(
            capital=capital,
            win_rate=0.65,
            confidence=0.8,
        )
        medium_confidence = self.calculate_position_size(
            capital=capital,
            win_rate=0.55,
            confidence=0.6,
        )
        low_confidence = self.calculate_position_size(
            capital=capital,
            win_rate=0.45,
            confidence=0.4,
        )

        return {
            "capital": capital,
            "growth_stage": stage.value,
            "growth_multiplier": multiplier,
            "max_position_pct": self._config.MAX_POSITION_PCT * 100,
            "min_position_pct": self._config.MIN_POSITION_PCT * 100,
            "kelly_cap": self._config.KELLY_FRACTION_CAP * 100,
            "high_confidence_example": {
                "win_rate": 0.65,
                "confidence": 0.8,
                "size_usd": high_confidence.size_usd,
                "size_pct": high_confidence.size_pct * 100,
            },
            "medium_confidence_example": {
                "win_rate": 0.55,
                "confidence": 0.6,
                "size_usd": medium_confidence.size_usd,
                "size_pct": medium_confidence.size_pct * 100,
            },
            "low_confidence_example": {
                "win_rate": 0.45,
                "confidence": 0.4,
                "size_usd": low_confidence.size_usd,
                "size_pct": low_confidence.size_pct * 100,
            },
        }

    def print_sizing_report(self, capital: float):
        """Print comprehensive sizing report."""
        summary = self.get_sizing_summary(capital)

        print("\n" + "="*70)
        print("POSITION SIZER - SIZING REPORT")
        print("="*70)

        print(f"\nCapital: ${summary['capital']:.2f}")
        print(f"Growth Stage: {summary['growth_stage'].capitalize()}")
        print(f"Growth Multiplier: {summary['growth_multiplier']:.1f}x")

        print("\nSafety Limits:")
        print(f"  Max Position: {summary['max_position_pct']:.0f}% of capital")
        print(f"  Min Position: {summary['min_position_pct']:.0f}% of capital")
        print(f"  Kelly Cap: {summary['kelly_cap']:.0f}%")

        print("\nExample Position Sizes:")
        print("  High Confidence (65% win rate, 80% confidence):")
        print(f"    Size: ${summary['high_confidence_example']['size_usd']:.2f} ({summary['high_confidence_example']['size_pct']:.1f}%)")
        print("  Medium Confidence (55% win rate, 60% confidence):")
        print(f"    Size: ${summary['medium_confidence_example']['size_usd']:.2f} ({summary['medium_confidence_example']['size_pct']:.1f}%)")
        print("  Low Confidence (45% win rate, 40% confidence):")
        print(f"    Size: ${summary['low_confidence_example']['size_usd']:.2f} ({summary['low_confidence_example']['size_pct']:.1f}%)")

        print("="*70 + "\n")


# Global singleton instance
_sizer: Optional[PositionSizer] = None
_sizer_lock = threading.Lock()


def get_position_sizer() -> PositionSizer:
    """Get the global position sizer singleton."""
    global _sizer

    with _sizer_lock:
        if _sizer is None:
            _sizer = PositionSizer()

    return _sizer


def reset_position_sizer():
    """Reset the global position sizer (mainly for testing)."""
    global _sizer

    with _sizer_lock:
        if _sizer:
            del _sizer
        _sizer = None


if __name__ == "__main__":
    # Test the position sizer
    sizer = get_position_sizer()

    # Test at different capital levels
    print("Testing position sizing at different capital levels:")
    for capital in [200, 300, 500, 800, 1000]:
        summary = sizer.get_sizing_summary(capital)
        print(f"  ${capital}: {summary['growth_stage']} stage, {summary['growth_multiplier']:.1f}x multiplier")

    # Print full report at $500 capital
    sizer.print_sizing_report(500)
