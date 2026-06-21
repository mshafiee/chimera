"""
Dynamic Strategy Allocator for Aggressive Growth

This module implements capital allocation strategies optimized for rapid growth
from $200 → $1,000 to afford Helius Business Plan upgrade.

Aggressive Allocation Strategy (User-Selected):
- <$300: 60% Shield / 40% Spear
- $300-500: 50% Shield / 50% Spear
- $500-800: 40% Shield / 60% Spear
- $800+: 30% Shield / 70% Spear (Maximum growth mode)

Features:
- Dynamic allocation adjustment based on capital growth stage
- Market regime-aware allocation (BULL/BEAR/VOLATILE/NEUTRAL)
- Risk-adjusted position sizing
- Portfolio diversification constraints
- Real-time strategy rebalancing
- Integration with circuit breaker protection
"""

import os
import time
import logging
from datetime import datetime
from typing import Dict, List, Optional, Tuple, Any
from dataclasses import dataclass, field
from enum import Enum
import threading
import json
from pathlib import Path

logger = logging.getLogger(__name__)


class MarketRegime(Enum):
    """Market regime types."""
    BULL = "bull"          # Strong upward momentum
    BEAR = "bear"          # Downward momentum
    VOLATILE = "volatile"  # High volatility
    NEUTRAL = "neutral"    # Sideways/ranging


class StrategyType(Enum):
    """Strategy types."""
    SHIELD = "shield"      # Low-risk preservation
    SPEAR = "spear"        # High-reward asymmetric upside


@dataclass
class StrategyAllocation:
    """Current strategy allocation."""
    shield_pct: float      # Shield allocation percentage (0-1)
    spear_pct: float       # Spear allocation percentage (0-1)
    timestamp: float = field(default_factory=time.time)
    regime: MarketRegime = MarketRegime.NEUTRAL
    capital: float = 200.0
    growth_stage: str = "early"  # early, mid, growth, final


@dataclass
class AllocatorConfig:
    """Configuration for strategy allocator."""

    # Capital thresholds for growth stages
    STAGE_EARLY_MAX: float = 300.0       # $200-300
    STAGE_MID_MAX: float = 500.0         # $300-500
    STAGE_GROWTH_MAX: float = 800.0      # $500-800
    STAGE_FINAL_MIN: float = 800.0       # $800+

    # Aggressive allocation percentages (from user selection)
    EARLY_SHIELD: float = 0.60   # 60%
    EARLY_SPEAR: float = 0.40    # 40%

    MID_SHIELD: float = 0.50     # 50%
    MID_SPEAR: float = 0.50      # 50%

    GROWTH_SHIELD: float = 0.40  # 40%
    GROWTH_SPEAR: float = 0.60   # 60%

    FINAL_SHIELD: float = 0.30   # 30%
    FINAL_SPEAR: float = 0.70    # 70%

    # Regime-based adjustments
    BULL_SPEAR_BONUS: float = 0.10     # +10% Spear in bull market
    BEAR_SPEAR_PENALTY: float = -0.15  # -15% Spear in bear market
    VOLATILE_SPEAR_PENALTY: float = -0.10  # -10% Spear in volatile

    # Diversification constraints
    MAX_SINGLE_POSITION: float = 0.20   # Max 20% of capital in one position
    MIN_POSITION_SIZE: float = 0.02     # Min 2% of capital per position
    MAX_CORRELATION_EXPOSURE: float = 0.50  # Max 50% in correlated assets

    # Rebalancing settings
    REBALANCE_THRESHOLD: float = 0.05   # Rebalance if allocation drifts by 5%
    MIN_REBALANCE_INTERVAL: int = 3600  # Minimum 1 hour between rebalances


class StrategyAllocator:
    """
    Dynamic strategy allocator for aggressive growth.

    Implements capital allocation optimized for rapid growth from $200 → $1,000:
    - Progressive Spear allocation as capital grows
    - Market regime-aware adjustments
    - Risk-managed position sizing
    - Automatic rebalancing
    """

    def __init__(self, config: Optional[AllocatorConfig] = None):
        """Initialize the strategy allocator."""
        self._config = config or AllocatorConfig()
        self._allocation = StrategyAllocation()
        self._last_rebalance = 0.0
        self._lock = threading.Lock()

        # Load previous allocation if available
        self._load_allocation()

        logger.info("Strategy Allocator initialized")
        logger.info(f"  Current allocation: Shield {self._allocation.shield_pct*100:.0f}%, Spear {self._allocation.spear_pct*100:.0f}%")
        logger.info(f"  Capital: ${self._allocation.capital:.0f}")
        logger.info(f"  Growth stage: {self._allocation.growth_stage}")

    def _load_allocation(self):
        """Load previous allocation from disk."""
        try:
            alloc_file = Path(os.getenv("SCOUT_STRATEGY_ALLOC_FILE",
                                        "/tmp/strategy_allocation.json"))
            if alloc_file.exists():
                with open(alloc_file, 'r') as f:
                    data = json.load(f)

                # Check if allocation is from today
                alloc_time = data.get('timestamp', 0)
                alloc_datetime = datetime.fromtimestamp(alloc_time)
                if alloc_datetime.date() == datetime.now().date():
                    self._allocation = StrategyAllocation(
                        shield_pct=data.get('shield_pct', 0.60),
                        spear_pct=data.get('spear_pct', 0.40),
                        timestamp=alloc_time,
                        regime=MarketRegime(data.get('regime', 'neutral')),
                        capital=data.get('capital', 200.0),
                        growth_stage=data.get('growth_stage', 'early'),
                    )
                    logger.info(f"Loaded previous allocation from {alloc_datetime.strftime('%H:%M')}")
        except Exception as e:
            logger.warning(f"Failed to load allocation: {e}")

    def _save_allocation(self):
        """Save current allocation to disk."""
        try:
            alloc_file = Path(os.getenv("SCOUT_STRATEGY_ALLOC_FILE",
                                        "/tmp/strategy_allocation.json"))
            alloc_file.parent.mkdir(parents=True, exist_ok=True)

            data = {
                'shield_pct': self._allocation.shield_pct,
                'spear_pct': self._allocation.spear_pct,
                'timestamp': self._allocation.timestamp,
                'regime': self._allocation.regime.value,
                'capital': self._allocation.capital,
                'growth_stage': self._allocation.growth_stage,
            }

            with open(alloc_file, 'w') as f:
                json.dump(data, f, indent=2)
        except Exception as e:
            logger.warning(f"Failed to save allocation: {e}")

    def get_growth_stage(self, capital: float) -> str:
        """Determine growth stage based on capital."""
        if capital < self._config.STAGE_EARLY_MAX:
            return "early"
        elif capital < self._config.STAGE_MID_MAX:
            return "mid"
        elif capital < self._config.STAGE_GROWTH_MAX:
            return "growth"
        else:
            return "final"

    def get_base_allocation(self, capital: float) -> Tuple[float, float]:
        """
        Get base allocation for capital (aggressive strategy).

        Returns:
            Tuple of (shield_pct, spear_pct)
        """
        stage = self.get_growth_stage(capital)

        if stage == "early":
            return self._config.EARLY_SHIELD, self._config.EARLY_SPEAR
        elif stage == "mid":
            return self._config.MID_SHIELD, self._config.MID_SPEAR
        elif stage == "growth":
            return self._config.GROWTH_SHIELD, self._config.GROWTH_SPEAR
        else:  # final
            return self._config.FINAL_SHIELD, self._config.FINAL_SPEAR

    def apply_regime_adjustment(self, shield: float, spear: float,
                                regime: MarketRegime) -> Tuple[float, float]:
        """Apply market regime adjustments to allocation."""
        if regime == MarketRegime.BULL:
            # Increase Spear exposure in bull market
            spear = min(0.80, spear + self._config.BULL_SPEAR_BONUS)
            shield = 1.0 - spear
        elif regime == MarketRegime.BEAR:
            # Decrease Spear exposure in bear market
            spear = max(0.0, spear + self._config.BEAR_SPEAR_PENALTY)
            shield = 1.0 - spear
        elif regime == MarketRegime.VOLATILE:
            # Decrease Spear in volatile conditions
            spear = max(0.10, spear + self._config.VOLATILE_SPEAR_PENALTY)
            shield = 1.0 - spear
        # NEUTRAL: no adjustment

        return shield, spear

    def calculate_allocation(self, capital: float,
                           regime: MarketRegime = MarketRegime.NEUTRAL,
                           force_rebalance: bool = False) -> StrategyAllocation:
        """
        Calculate optimal allocation for current capital and market regime.

        Args:
            capital: Current capital
            regime: Current market regime
            force_rebalance: Force rebalance regardless of threshold

        Returns:
            StrategyAllocation with recommended allocation
        """
        with self._lock:
            # Get base aggressive allocation
            shield, spear = self.get_base_allocation(capital)

            # Apply regime adjustments
            shield, spear = self.apply_regime_adjustment(shield, spear, regime)

            # Check if rebalance is needed
            needs_rebalance = force_rebalance or self._needs_rebalance(shield, spear)

            if needs_rebalance:
                self._allocation = StrategyAllocation(
                    shield_pct=shield,
                    spear_pct=spear,
                    timestamp=time.time(),
                    regime=regime,
                    capital=capital,
                    growth_stage=self.get_growth_stage(capital),
                )
                self._last_rebalance = time.time()
                self._save_allocation()

                logger.info("Strategy allocation updated:")
                logger.info(f"  Capital: ${capital:.0f} ({self._allocation.growth_stage} stage)")
                logger.info(f"  Regime: {regime.value}")
                logger.info(f"  Shield: {shield*100:.0f}%")
                logger.info(f"  Spear: {spear*100:.0f}%")

            return self._allocation

    def _needs_rebalance(self, new_shield: float, new_spear: float) -> bool:
        """Check if allocation drift exceeds rebalance threshold."""
        time_since_rebalance = time.time() - self._last_rebalance
        if time_since_rebalance < self._config.MIN_REBALANCE_INTERVAL:
            return False

        shield_drift = abs(new_shield - self._allocation.shield_pct)
        spear_drift = abs(new_spear - self._allocation.spear_pct)

        return max(shield_drift, spear_drift) > self._config.REBALANCE_THRESHOLD

    def calculate_position_size(self, capital: float, wallet_confidence: float,
                               strategy: StrategyType,
                               num_positions: int = 1) -> float:
        """
        Calculate position size using Kelly-inspired sizing with growth multipliers.

        Args:
            capital: Total capital
            wallet_confidence: WQS-based confidence (0-1)
            strategy: Strategy type (SHIELD or SPEAR)
            num_positions: Number of concurrent positions

        Returns:
            Position size in capital units
        """
        # Get strategy allocation
        shield_alloc, spear_alloc = self.get_base_allocation(capital)
        strategy_capital = spear_alloc * capital if strategy == StrategyType.SPEAR else shield_alloc * capital

        # Apply Kelly-inspired sizing with growth multiplier
        growth_stage = self.get_growth_stage(capital)
        if growth_stage == "early":
            growth_multiplier = 0.3  # Conservative early stage
        elif growth_stage == "mid":
            growth_multiplier = 0.5
        elif growth_stage == "growth":
            growth_multiplier = 0.8
        else:  # final
            growth_multiplier = 1.0  # Full Kelly

        # Base Kelly fraction
        kelly_fraction = wallet_confidence * growth_multiplier

        # Calculate position size
        position_size = strategy_capital * kelly_fraction / num_positions

        # Apply constraints
        max_position = capital * self._config.MAX_SINGLE_POSITION
        min_position = capital * self._config.MIN_POSITION_SIZE

        position_size = max(min_position, min(position_size, max_position))

        return position_size

    def validate_portfolio(self, positions: List[Dict[str, Any]],
                          capital: float) -> Tuple[bool, List[str]]:
        """
        Validate portfolio meets diversification constraints.

        Args:
            positions: List of position dictionaries with 'amount' and 'correlation_group'
            capital: Total capital

        Returns:
            Tuple of (is_valid, list_of_issues)
        """
        issues = []
        total_exposure = sum(p.get('amount', 0) for p in positions)

        # Check total exposure
        if total_exposure > capital:
            issues.append(f"Total exposure ${total_exposure:.2f} exceeds capital ${capital:.2f}")

        # Check single position size
        for i, pos in enumerate(positions):
            amount = pos.get('amount', 0)
            if amount > capital * self._config.MAX_SINGLE_POSITION:
                issues.append(f"Position {i} (${amount:.2f}) exceeds max size (${capital * self._config.MAX_SINGLE_POSITION:.2f})")
            if amount < capital * self._config.MIN_POSITION_SIZE and amount > 0:
                issues.append(f"Position {i} (${amount:.2f}) below min size (${capital * self._config.MIN_POSITION_SIZE:.2f})")

        # Check correlation exposure
        correlation_groups = {}
        for pos in positions:
            group = pos.get('correlation_group', 'default')
            correlation_groups[group] = correlation_groups.get(group, 0) + pos.get('amount', 0)

        for group, exposure in correlation_groups.items():
            max_correlated = capital * self._config.MAX_CORRELATION_EXPOSURE
            if exposure > max_correlated:
                issues.append(f"Correlation group '{group}' exposure ${exposure:.2f} exceeds max ${max_correlated:.2f}")

        return len(issues) == 0, issues

    def get_current_allocation(self) -> StrategyAllocation:
        """Get current strategy allocation."""
        with self._lock:
            return self._allocation

    def update_regime(self, regime: MarketRegime):
        """Update market regime and recalculate if needed."""
        with self._lock:
            if self._allocation.regime != regime:
                logger.info(f"Market regime changed: {self._allocation.regime.value} → {regime.value}")
                self.calculate_allocation(
                    self._allocation.capital,
                    regime,
                    force_rebalance=True
                )

    def get_allocation_summary(self) -> Dict[str, Any]:
        """Get summary of current allocation."""
        with self._lock:
            capital = self._allocation.capital
            shield_amount = capital * self._allocation.shield_pct
            spear_amount = capital * self._allocation.spear_pct

            return {
                "capital": capital,
                "growth_stage": self._allocation.growth_stage,
                "regime": self._allocation.regime.value,
                "shield_allocation_pct": self._allocation.shield_pct * 100,
                "spear_allocation_pct": self._allocation.spear_pct * 100,
                "shield_amount": shield_amount,
                "spear_amount": spear_amount,
                "target_capital": 1000.0,
                "progress_to_target": (capital / 1000.0) * 100,
                "last_rebalance": self._last_rebalance,
            }

    def print_allocation_report(self):
        """Print comprehensive allocation report."""
        summary = self.get_allocation_summary()

        print("\n" + "="*70)
        print("STRATEGY ALLOCATOR - ALLOCATION REPORT")
        print("="*70)

        print("\nGrowth Progress:")
        print(f"  Current Capital: ${summary['capital']:.2f}")
        print(f"  Target Capital: ${summary['target_capital']:.2f}")
        print(f"  Progress: {summary['progress_to_target']:.1f}%")
        print(f"  Growth Stage: {summary['growth_stage'].capitalize()}")

        print("\nMarket Conditions:")
        print(f"  Regime: {summary['regime'].upper()}")

        print("\nStrategy Allocation:")
        print(f"  Shield: {summary['shield_allocation_pct']:.0f}% (${summary['shield_amount']:.2f})")
        print(f"  Spear: {summary['spear_allocation_pct']:.0f}% (${summary['spear_amount']:.2f})")

        if summary['last_rebalance'] > 0:
            rebalance_ago = (time.time() - summary['last_rebalance']) / 60
            print(f"\nLast rebalance: {rebalance_ago:.0f} minutes ago")

        # Growth targets
        print("\nGrowth Targets:")
        print(f"  Early ($300): {100 * min(summary['capital'] / 300, 1.0):.0f}%")
        print(f"  Mid ($500): {100 * min(summary['capital'] / 500, 1.0):.0f}%")
        print(f"  Growth ($800): {100 * min(summary['capital'] / 800, 1.0):.0f}%")
        print(f"  Final ($1,000): {100 * min(summary['capital'] / 1000, 1.0):.0f}%")

        print("="*70 + "\n")


# Global singleton instance
_allocator: Optional[StrategyAllocator] = None
_allocator_lock = threading.Lock()


def get_strategy_allocator() -> StrategyAllocator:
    """Get the global strategy allocator singleton."""
    global _allocator

    with _allocator_lock:
        if _allocator is None:
            _allocator = StrategyAllocator()

    return _allocator


def reset_strategy_allocator():
    """Reset the global strategy allocator (mainly for testing)."""
    global _allocator

    with _allocator_lock:
        if _allocator:
            del _allocator
        _allocator = None


if __name__ == "__main__":
    # Test the strategy allocator
    allocator = get_strategy_allocator()

    # Test allocations at different capital levels
    print("Testing allocations at different capital levels:")
    for capital in [200, 300, 500, 800, 1000]:
        alloc = allocator.calculate_allocation(capital, MarketRegime.NEUTRAL)
        print(f"  ${capital}: Shield {alloc.shield_pct*100:.0f}%, Spear {alloc.spear_pct*100:.0f}%, Stage {alloc.growth_stage}")

    # Print full report
    allocator.print_allocation_report()
