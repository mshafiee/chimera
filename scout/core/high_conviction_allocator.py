"""
High-Conviction Credit Allocator for Dynamic WQS-Based Allocation

This module implements dynamic credit allocation that heavily favors high-WQS wallets
to maximize profitability under Helius Developer Plan constraints.

Allocation Strategy:
- WQS 70-100: 2.5x credit multiplier (70% of budget)
- WQS 50-70: 1.0x credit multiplier (20% of budget)
- WQS 30-50: 0.3x credit multiplier (8% of budget)
- WQS < 30: 0.1x credit multiplier (2% of budget)

Features:
- WQS-based credit multipliers
- Dynamic allocation based on conviction level
- Emerging wallet budget management
- Real-time rebalancing based on performance
"""

import time
import logging
from typing import Dict, Optional, Any
from dataclasses import dataclass, field
from enum import Enum
import threading
import json
from pathlib import Path

logger = logging.getLogger(__name__)


class ConvictionLevel(Enum):
    """Wallet conviction levels based on WQS."""
    VERY_HIGH = "very_high"    # WQS 80-100
    HIGH = "high"              # WQS 70-80
    MEDIUM = "medium"          # WQS 50-70
    EMERGING = "emerging"      # WQS 30-50
    LOW = "low"                # WQS < 30


@dataclass
class AllocationResult:
    """Result of credit allocation request."""
    wallet_address: str
    wqs_score: float
    conviction_level: ConvictionLevel
    credits_allocated: int
    multiplier_used: float
    reason: str
    timestamp: float = field(default_factory=time.time)


@dataclass
class RebalanceResult:
    """Result of allocation rebalancing."""
    previous_allocations: Dict[ConvictionLevel, float]
    new_allocations: Dict[ConvictionLevel, float]
    credits_moved: int
    reason: str
    timestamp: float = field(default_factory=time.time)


@dataclass
class AllocatorConfig:
    """Configuration for high-conviction allocator."""

    # WQS thresholds for conviction levels
    WQS_VERY_HIGH_MIN: float = 80.0
    WQS_HIGH_MIN: float = 70.0
    WQS_MEDIUM_MIN: float = 50.0
    WQS_EMERGING_MIN: float = 30.0

    # Credit multipliers
    VERY_HIGH_MULTIPLIER: float = 3.0
    HIGH_MULTIPLIER: float = 2.5
    MEDIUM_MULTIPLIER: float = 1.0
    EMERGING_MULTIPLIER: float = 0.3
    LOW_MULTIPLIER: float = 0.1

    # Allocation targets (percentage of total budget)
    VERY_HIGH_TARGET: float = 0.30   # 30%
    HIGH_TARGET: float = 0.40       # 40%
    MEDIUM_TARGET: float = 0.20      # 20%
    EMERGING_TARGET: float = 0.08    # 8%
    LOW_TARGET: float = 0.02         # 2%

    # Minimum allocations to prevent starvation
    MIN_EMERGING_ALLOCATION: int = 1000   # Minimum credits for emerging wallets
    MIN_LOW_ALLOCATION: int = 500        # Minimum credits for low WQS

    # Rebalancing settings
    REBALANCE_INTERVAL_SECONDS: int = 3600  # 1 hour
    DEVIATION_THRESHOLD: float = 0.10        # 10% deviation triggers rebalance

    # Performance-based adjustment
    PERFORMANCE_LOOKBACK_TRADES: int = 20    # Minimum trades for performance assessment
    PERFORMANCE_BOOST_MULTIPLIER: float = 1.5  # Boost multiplier for good performers

    # State persistence
    STATE_FILE: str = "high_conviction_allocator_state.json"


class HighConvictionAllocator:
    """
    High-conviction credit allocator for WQS-based dynamic allocation.

    Strategy:
    - Allocate 70% of budget to WQS 70+ wallets
    - Allocate 20% of budget to WQS 50-70 wallets
    - Allocate 8% of budget to WQS 30-50 wallets
    - Allocate 2% of budget to WQS < 30 wallets

    Features:
    - WQS-based credit multipliers
    - Dynamic rebalancing based on performance
    - Emerging wallet budget protection
    - Cross-session state persistence
    """

    def __init__(self, config: Optional[AllocatorConfig] = None):
        """Initialize the high-conviction allocator."""
        self._config = config or AllocatorConfig()
        self._lock = threading.Lock()

        # Total available credits (will be updated externally)
        self._total_credits = 0

        # Current allocations by conviction level
        self._allocations: Dict[ConvictionLevel, int] = {
            ConvictionLevel.VERY_HIGH: 0,
            ConvictionLevel.HIGH: 0,
            ConvictionLevel.MEDIUM: 0,
            ConvictionLevel.EMERGING: 0,
            ConvictionLevel.LOW: 0,
        }

        # Credits consumed by conviction level
        self._consumed: Dict[ConvictionLevel, int] = {
            level: 0 for level in ConvictionLevel
        }

        # Performance tracking by wallet
        self._wallet_performance: Dict[str, Dict[str, Any]] = {}

        # Last rebalance time
        self._last_rebalance = time.time()

        # Load state if available
        self._load_state()

        logger.info("HighConvictionAllocator initialized")

    def set_total_credits(self, total: int) -> None:
        """Set total available credits for allocation."""
        with self._lock:
            self._total_credits = total
            self._rebalance_initial()

    def _rebalance_initial(self) -> None:
        """Initial rebalancing based on configured targets."""
        if self._total_credits == 0:
            return

        self._allocations = {
            ConvictionLevel.VERY_HIGH: int(self._total_credits * self._config.VERY_HIGH_TARGET),
            ConvictionLevel.HIGH: int(self._total_credits * self._config.HIGH_TARGET),
            ConvictionLevel.MEDIUM: int(self._total_credits * self._config.MEDIUM_TARGET),
            ConvictionLevel.EMERGING: int(self._total_credits * self._config.EMERGING_TARGET),
            ConvictionLevel.LOW: int(self._total_credits * self._config.LOW_TARGET),
        }

        logger.info(f"Initial allocations: {self._allocations}")

    def get_conviction_level(self, wqs_score: float) -> ConvictionLevel:
        """Get conviction level for a WQS score."""
        if wqs_score >= self._config.WQS_VERY_HIGH_MIN:
            return ConvictionLevel.VERY_HIGH
        elif wqs_score >= self._config.WQS_HIGH_MIN:
            return ConvictionLevel.HIGH
        elif wqs_score >= self._config.WQS_MEDIUM_MIN:
            return ConvictionLevel.MEDIUM
        elif wqs_score >= self._config.WQS_EMERGING_MIN:
            return ConvictionLevel.EMERGING
        else:
            return ConvictionLevel.LOW

    def calculate_credit_multiplier(self, wqs_score: float) -> float:
        """Calculate credit multiplier for a WQS score."""
        level = self.get_conviction_level(wqs_score)

        multipliers = {
            ConvictionLevel.VERY_HIGH: self._config.VERY_HIGH_MULTIPLIER,
            ConvictionLevel.HIGH: self._config.HIGH_MULTIPLIER,
            ConvictionLevel.MEDIUM: self._config.MEDIUM_MULTIPLIER,
            ConvictionLevel.EMERGING: self._config.EMERGING_MULTIPLIER,
            ConvictionLevel.LOW: self._config.LOW_MULTIPLIER,
        }

        return multipliers.get(level, 1.0)

    def allocate_analysis_credits(
        self, wallet_address: str, wqs_score: float, base_credits: int = 100
    ) -> AllocationResult:
        """
        Allocate analysis credits for a wallet based on WQS.

        Args:
            wallet_address: Wallet to allocate credits for
            wqs_score: Current WQS score
            base_credits: Base credit amount to allocate

        Returns:
            AllocationResult with allocated credits and reason
        """
        with self._lock:
            level = self.get_conviction_level(wqs_score)
            multiplier = self.calculate_credit_multiplier(wqs_score)

            # Apply performance boost if wallet has good history
            perf_boost = 1.0
            if wallet_address in self._wallet_performance:
                perf = self._wallet_performance[wallet_address]
                if perf.get('trades', 0) >= self._config.PERFORMANCE_LOOKBACK_TRADES:
                    win_rate = perf.get('win_rate', 0)
                    if win_rate > 0.6:  # 60%+ win rate
                        perf_boost = self._config.PERFORMANCE_BOOST_MULTIPLIER

            allocated = int(base_credits * multiplier * perf_boost)

            # Check against allocation budget
            allocated = min(allocated, self._allocations.get(level, 0))

            # Track consumption
            if allocated > 0:
                self._consumed[level] += allocated

            reason = self._get_allocation_reason(level, multiplier, perf_boost)

            result = AllocationResult(
                wallet_address=wallet_address,
                wqs_score=wqs_score,
                conviction_level=level,
                credits_allocated=allocated,
                multiplier_used=multiplier * perf_boost,
                reason=reason,
            )

            logger.debug(
                f"Allocated {allocated} credits to {wallet_address[:8]}... "
                f"(WQS: {wqs_score:.1f}, Level: {level.value}, "
                f"Multiplier: {multiplier * perf_boost:.2f}x)"
            )

            return result

    def _get_allocation_reason(
        self, level: ConvictionLevel, multiplier: float, perf_boost: float
    ) -> str:
        """Get human-readable allocation reason."""
        if perf_boost > 1.0:
            return f"High conviction ({level.value}) with performance boost ({perf_boost:.1f}x)"
        return f"Conviction level: {level.value} ({multiplier:.1f}x multiplier)"

    def get_emerging_wallet_budget(self) -> int:
        """Get remaining budget for emerging wallets (WQS 30-50)."""
        with self._lock:
            allocated = self._allocations.get(ConvictionLevel.EMERGING, 0)
            consumed = self._consumed.get(ConvictionLevel.EMERGING, 0)
            remaining = max(0, allocated - consumed)

            # Ensure minimum allocation
            return max(remaining, self._config.MIN_EMERGING_ALLOCATION)

    def get_high_conviction_budget(self) -> int:
        """Get remaining budget for high-conviction wallets (WQS 70+)."""
        with self._lock:
            high_allocated = self._allocations.get(ConvictionLevel.HIGH, 0)
            high_consumed = self._consumed.get(ConvictionLevel.HIGH, 0)
            very_high_allocated = self._allocations.get(ConvictionLevel.VERY_HIGH, 0)
            very_high_consumed = self._consumed.get(ConvictionLevel.VERY_HIGH, 0)

            remaining = max(0, (high_allocated - high_consumed) +
                           (very_high_allocated - very_high_consumed))

            return remaining

    def rebalance_to_high_conviction(self) -> RebalanceResult:
        """
        Rebalance allocations to focus on high-conviction wallets.

        Moves credits from low-performing categories to high-conviction categories.
        """
        with self._lock:
            now = time.time()
            if now - self._last_rebalance < self._config.REBALANCE_INTERVAL_SECONDS:
                return RebalanceResult(
                    previous_allocations={},
                    new_allocations={},
                    credits_moved=0,
                    reason="Rebalance interval not reached",
                )

            previous = dict(self._allocations)

            # Calculate remaining credits for each level
            remaining = {
                level: max(0, allocated - self._consumed[level])
                for level, allocated in self._allocations.items()
            }

            # Check if any level is significantly under budget
            # and move those credits to high-conviction levels
            credits_to_move = 0
            for level in [ConvictionLevel.EMERGING, ConvictionLevel.LOW]:
                if remaining[level] > (self._allocations[level] * (1 - self._config.DEVIATION_THRESHOLD)):
                    excess = int(remaining[level] * 0.5)  # Move 50% of excess
                    credits_to_move += excess

            if credits_to_move > 0:
                # Distribute to high-conviction levels
                high_split = credits_to_move // 2
                very_high_split = credits_to_move - high_split

                self._allocations[ConvictionLevel.HIGH] += high_split
                self._allocations[ConvictionLevel.VERY_HIGH] += very_high_split

                self._last_rebalance = now

                result = RebalanceResult(
                    previous_allocations=previous,
                    new_allocations=self._allocations,
                    credits_moved=credits_to_move,
                    reason=f"Rebalanced {credits_to_move} credits to high-conviction wallets",
                )

                logger.info(f"Rebalanced: {credits_to_move} credits moved to high conviction")
                self._save_state()
                return result

            return RebalanceResult(
                previous_allocations=previous,
                new_allocations=self._allocations,
                credits_moved=0,
                reason="No significant deviation found",
            )

    def record_wallet_performance(
        self, wallet_address: str, wqs: float, win: bool, pnl: float
    ) -> None:
        """
        Record performance for a wallet to inform future allocations.

        Args:
            wallet_address: Wallet address
            wqs: Current WQS score
            win: Whether the trade was a win
            pnl: Profit/loss amount
        """
        with self._lock:
            if wallet_address not in self._wallet_performance:
                self._wallet_performance[wallet_address] = {
                    'trades': 0,
                    'wins': 0,
                    'total_pnl': 0.0,
                    'wqs': wqs,
                }

            perf = self._wallet_performance[wallet_address]
            perf['trades'] += 1
            perf['wins'] += 1 if win else 0
            perf['total_pnl'] += pnl
            perf['wqs'] = wqs
            perf['win_rate'] = perf['wins'] / perf['trades']
            perf['avg_pnl'] = perf['total_pnl'] / perf['trades']

            logger.debug(
                f"Recorded performance for {wallet_address[:8]}...: "
                f"win_rate={perf['win_rate']:.2f}, avg_pnl=${perf['avg_pnl']:.2f}"
            )

    def get_wallet_performance(self, wallet_address: str) -> Optional[Dict[str, Any]]:
        """Get performance tracking for a wallet."""
        with self._lock:
            return self._wallet_performance.get(wallet_address)

    def get_allocation_summary(self) -> Dict[str, Any]:
        """Get summary of current allocations and consumption."""
        with self._lock:
            total_allocated = sum(self._allocations.values())
            total_consumed = sum(self._consumed.values())

            return {
                'total_credits': self._total_credits,
                'total_allocated': total_allocated,
                'total_consumed': total_consumed,
                'total_remaining': total_allocated - total_consumed,
                'allocations_by_level': {
                    level.value: {
                        'allocated': self._allocations[level],
                        'consumed': self._consumed[level],
                        'remaining': max(0, self._allocations[level] - self._consumed[level]),
                        'percentage': (self._allocations[level] / max(1, total_allocated)) * 100,
                    }
                    for level in ConvictionLevel
                },
                'high_conviction_budget': self.get_high_conviction_budget(),
                'emerging_wallet_budget': self.get_emerging_wallet_budget(),
                'wallets_tracked': len(self._wallet_performance),
            }

    def get_allocation_efficiency(self) -> Dict[str, float]:
        """
        Calculate allocation efficiency metrics.

        Returns:
            Dict with efficiency metrics by conviction level
        """
        with self._lock:
            efficiency = {}

            for level in ConvictionLevel:
                allocated = self._allocations[level]
                consumed = self._consumed[level]

                if allocated == 0:
                    efficiency[level.value] = 0.0
                else:
                    # Efficiency = ratio of consumption to allocation
                    # Values > 1 indicate under-allocation
                    efficiency[level.value] = consumed / allocated if allocated > 0 else 0

            return efficiency

    def _load_state(self) -> None:
        """Load state from disk."""
        state_file = Path(self._config.STATE_FILE)
        if not state_file.exists():
            return

        try:
            with open(state_file, 'r') as f:
                data = json.load(f)

            # Restore consumption data
            for level_name, consumed in data.get('consumed', {}).items():
                try:
                    level = ConvictionLevel(level_name)
                    self._consumed[level] = consumed
                except ValueError:
                    continue

            # Restore wallet performance
            self._wallet_performance = data.get('wallet_performance', {})

            logger.info(f"Loaded state from {state_file}")

        except Exception as e:
            logger.warning(f"Failed to load state: {e}")

    def _save_state(self) -> None:
        """Save state to disk."""
        try:
            data = {
                'consumed': {level.value: consumed for level, consumed in self._consumed.items()},
                'wallet_performance': self._wallet_performance,
                'last_save': time.time(),
            }

            state_file = Path(self._config.STATE_FILE)
            with open(state_file, 'w') as f:
                json.dump(data, f, indent=2)

        except Exception as e:
            logger.warning(f"Failed to save state: {e}")

    def reset_consumption(self) -> None:
        """Reset consumption tracking (call at start of new period)."""
        with self._lock:
            for level in ConvictionLevel:
                self._consumed[level] = 0

            logger.info("Reset consumption tracking")
