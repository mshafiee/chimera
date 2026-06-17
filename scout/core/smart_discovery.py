"""
Smart Discovery Prioritization for Credit-Cost-Aware Wallet Discovery

This module implements credit-cost-aware wallet discovery that maximizes
wallets found per credit spent under Helius Developer Plan constraints.

Strategy:
- Calculate efficiency score for each discovery strategy
- Rank strategies by (wallets_found * avg_wqs) / credits_consumed
- Boost high-WQS discoveries with bonus multiplier
- Adaptively select optimal strategy based on remaining credits

Features:
- Strategy efficiency scoring and ranking
- Credit-cost-aware discovery selection
- Adaptive strategy selection based on budget
- Efficiency tracking and optimization
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
from collections import defaultdict

logger = logging.getLogger(__name__)


class DiscoveryStrategy(Enum):
    """Wallet discovery strategies ranked by cost and quality."""
    DEX_AGGREGATOR_TRADES = "dex_aggregator_trades"  # High WQS, medium cost
    LARGE_DEX_TRADES = "large_dex_trades"            # High WQS, medium-high cost
    TOKEN_HOLDERS = "token_holders"                  # Medium WQS, low cost
    WHALE_ALERTS = "whale_alerts"                    # Variable WQS, low cost
    LARGE_TRANSFERS = "large_transfers"               # Low WQS, very low cost
    PROGRAM_INTERACTIONS = "program_interactions"     # Variable WQS, variable cost
    MENTIONED_TOKENS = "mentioned_tokens"             # Medium WQS, low cost


@dataclass
class StrategyScore:
    """Efficiency score for a discovery strategy."""
    strategy: DiscoveryStrategy
    efficiency_score: float  # (wallets * avg_wqs) / credits
    wallets_per_credit: float
    avg_wqs_found: float
    credits_per_operation: int
    total_wallets_found: int
    total_credits_consumed: int
    last_updated: float = field(default_factory=time.time)
    sample_size: int = 0


@dataclass
class DiscoveryResult:
    """Result of a discovery operation."""
    strategy: DiscoveryStrategy
    wallets_found: List[str]
    wqs_scores: Dict[str, float]
    credits_consumed: int
    time_taken_seconds: float
    efficiency: float
    timestamp: float = field(default_factory=time.time)


@dataclass
class DiscoveryConfig:
    """Configuration for smart discovery prioritization."""

    # Credit costs per operation (estimated)
    CREDIT_COSTS: Dict[DiscoveryStrategy, int] = field(default_factory=lambda: {
        DiscoveryStrategy.DEX_AGGREGATOR_TRADES: 50,
        DiscoveryStrategy.LARGE_DEX_TRADES: 75,
        DiscoveryStrategy.TOKEN_HOLDERS: 20,
        DiscoveryStrategy.WHALE_ALERTS: 15,
        DiscoveryStrategy.LARGE_TRANSFERS: 10,
        DiscoveryStrategy.PROGRAM_INTERACTIONS: 30,
        DiscoveryStrategy.MENTIONED_TOKENS: 25,
    })

    # Expected WQS distributions for each strategy
    EXPECTED_WQS: Dict[DiscoveryStrategy, float] = field(default_factory=lambda: {
        DiscoveryStrategy.DEX_AGGREGATOR_TRADES: 65.0,
        DiscoveryStrategy.LARGE_DEX_TRADES: 70.0,
        DiscoveryStrategy.TOKEN_HOLDERS: 45.0,
        DiscoveryStrategy.WHALE_ALERTS: 55.0,
        DiscoveryStrategy.LARGE_TRANSFERS: 35.0,
        DiscoveryStrategy.PROGRAM_INTERACTIONS: 50.0,
        DiscoveryStrategy.MENTIONED_TOKENS: 48.0,
    })

    # Expected wallets found per operation
    EXPECTED_WALLETS: Dict[DiscoveryStrategy, int] = field(default_factory=lambda: {
        DiscoveryStrategy.DEX_AGGREGATOR_TRADES: 15,
        DiscoveryStrategy.LARGE_DEX_TRADES: 8,
        DiscoveryStrategy.TOKEN_HOLDERS: 50,
        DiscoveryStrategy.WHALE_ALERTS: 5,
        DiscoveryStrategy.LARGE_TRANSFERS: 100,
        DiscoveryStrategy.PROGRAM_INTERACTIONS: 20,
        DiscoveryStrategy.MENTIONED_TOKENS: 30,
    })

    # High-conviction bonus for strategies that find WQS 70+ wallets
    HIGH_CONVIICTION_BONUS: float = 1.5  # 50% efficiency boost
    HIGH_CONVIICTION_THRESHOLD: float = 60.0  # WQS threshold for bonus

    # Strategy selection thresholds
    MIN_CREDITS_FOR_ANY_STRATEGY: int = 100
    MIN_CREDITS_FOR_HIGH_COST: int = 500
    BUDGET_LOW_THRESHOLD: float = 0.20  # Below 20% = low budget mode

    # Adaptive settings
    ADAPTIVE_SELECTION: bool = True
    REEVALUATION_INTERVAL_SECONDS: int = 1800  # 30 minutes

    # State persistence
    STATE_FILE: str = "smart_discovery_state.json"


class SmartDiscoveryPrioritizer:
    """
    Smart discovery prioritizer for credit-cost-aware wallet discovery.

    Strategy:
    - Rank strategies by efficiency: (wallets * avg_wqs) / credits
    - Boost high-WQS discoveries with 1.5x bonus
    - Adaptively select based on remaining budget
    - Track and optimize over time

    Features:
    - Strategy efficiency scoring
    - Credit-cost-aware selection
    - Adaptive budget-based selection
    - Performance tracking
    """

    def __init__(self, config: Optional[DiscoveryConfig] = None):
        """Initialize the smart discovery prioritizer."""
        self._config = config or DiscoveryConfig()
        self._lock = threading.Lock()

        # Strategy performance tracking
        self._strategy_scores: Dict[DiscoveryStrategy, StrategyScore] = {}

        # Initialize with expected values
        self._initialize_strategy_scores()

        # Discovery history for learning
        self._discovery_history: List[DiscoveryResult] = []

        # Current budget state
        self._remaining_credits = 0

        # Last reevaluation time
        self._last_reevaluation = time.time()

        # Load state if available
        self._load_state()

        logger.info("SmartDiscoveryPrioritizer initialized")

    def _initialize_strategy_scores(self) -> None:
        """Initialize strategy scores with expected values."""
        for strategy in DiscoveryStrategy:
            credits = self._config.CREDIT_COSTS[strategy]
            wallets = self._config.EXPECTED_WALLETS[strategy]
            avg_wqs = self._config.EXPECTED_WQS[strategy]

            # Calculate initial efficiency
            efficiency = (wallets * avg_wqs) / max(1, credits)
            wpc = wallets / max(1, credits)

            self._strategy_scores[strategy] = StrategyScore(
                strategy=strategy,
                efficiency_score=efficiency,
                wallets_per_credit=wpc,
                avg_wqs_found=avg_wqs,
                credits_per_operation=credits,
                total_wallets_found=0,
                total_credits_consumed=0,
                sample_size=0,
            )

        logger.info("Initialized strategy scores with expected values")

    def set_remaining_credits(self, credits: int) -> None:
        """Set remaining credits for budget-aware selection."""
        with self._lock:
            self._remaining_credits = credits

    def rank_discovery_strategies(self) -> List[StrategyScore]:
        """
        Rank discovery strategies by efficiency score.

        Returns:
            List of strategies sorted by efficiency (highest first)
        """
        with self._lock:
            # Check if reevaluation is needed
            if self._config.ADAPTIVE_SELECTION:
                now = time.time()
                if now - self._last_reevaluation > self._config.REEVALUATION_INTERVAL_SECONDS:
                    self._reevaluate_strategies()
                    self._last_reevaluation = now

            # Sort by efficiency score
            ranked = sorted(
                self._strategy_scores.values(),
                key=lambda s: s.efficiency_score,
                reverse=True
            )

            return ranked

    def _reevaluate_strategies(self) -> None:
        """Reevaluate strategy scores based on historical performance."""
        if not self._discovery_history:
            return

        # Group history by strategy
        history_by_strategy: Dict[DiscoveryStrategy, List[DiscoveryResult]] = defaultdict(list)
        for result in self._discovery_history:
            history_by_strategy[result.strategy].append(result)

        # Update scores based on actual performance
        for strategy, history in history_by_strategy.items():
            if not history or len(history) < 3:
                continue  # Not enough data

            total_wallets = sum(len(r.wallets_found) for r in history)
            total_credits = sum(r.credits_consumed for r in history)

            # Calculate average WQS
            all_wqs = []
            for r in history:
                all_wqs.extend(r.wqs_scores.values())

            avg_wqs = sum(all_wqs) / max(1, len(all_wqs))

            # Update score
            self._strategy_scores[strategy].total_wallets_found = total_wallets
            self._strategy_scores[strategy].total_credits_consumed = total_credits
            self._strategy_scores[strategy].avg_wqs_found = avg_wqs
            self._strategy_scores[strategy].wallets_per_credit = total_wallets / max(1, total_credits)
            self._strategy_scores[strategy].efficiency_score = (total_wallets * avg_wqs) / max(1, total_credits)
            self._strategy_scores[strategy].sample_size = len(history)
            self._strategy_scores[strategy].last_updated = time.time()

            logger.debug(
                f"Reevaluated {strategy.value}: efficiency={self._strategy_scores[strategy].efficiency_score:.2f}"
            )

    def calculate_wallets_per_credit(self, strategy: DiscoveryStrategy) -> float:
        """Calculate wallets found per credit for a strategy."""
        with self._lock:
            score = self._strategy_scores.get(strategy)
            if not score:
                return 0.0
            return score.wallets_per_credit

    def select_optimal_strategy(self, remaining_credits: Optional[int] = None) -> DiscoveryStrategy:
        """
        Select the optimal discovery strategy based on budget.

        Args:
            remaining_credits: Available credits (uses internal state if not provided)

        Returns:
            Best strategy for current budget
        """
        with self._lock:
            budget = remaining_credits if remaining_credits is not None else self._remaining_credits

            # Get ranked strategies
            ranked = self.rank_discovery_strategies()

            # Filter by budget
            affordable = []
            for score in ranked:
                if budget >= score.credits_per_operation:
                    affordable.append(score)

            if not affordable:
                # Budget too low for any strategy, return cheapest
                cheapest = min(ranked, key=lambda s: s.credits_per_operation)
                return cheapest.strategy

            # In low budget mode, prefer higher wallets-per-credit
            budget_ratio = budget / self._config.CREDIT_COSTS[DiscoveryStrategy.DEX_AGGREGATOR_TRADES]
            if budget_ratio < self._config.BUDGET_LOW_THRESHOLD:
                # Low budget: maximize wallets per credit
                best = max(affordable, key=lambda s: s.wallets_per_credit)
                logger.debug(f"Low budget mode: selected {best.strategy.value} for max wpc")
                return best.strategy
            else:
                # Normal budget: maximize efficiency
                best = max(affordable, key=lambda s: s.efficiency_score)
                logger.debug(f"Normal mode: selected {best.strategy.value} for max efficiency")
                return best.strategy

    def adaptive_discovery(
        self, budget_remaining: int, max_strategies: int = 3
    ) -> List[DiscoveryStrategy]:
        """
        Select multiple strategies for adaptive discovery.

        Returns a mix of high-efficiency and high-volume strategies
        based on remaining budget.

        Args:
            budget_remaining: Available credits
            max_strategies: Maximum number of strategies to return

        Returns:
            List of strategies to execute
        """
        with self._lock:
            self._remaining_credits = budget_remaining

            ranked = self.rank_discovery_strategies()

            # Select top strategies that fit in budget
            selected = []
            total_credits = 0

            for score in ranked:
                if len(selected) >= max_strategies:
                    break

                if total_credits + score.credits_per_operation <= budget_remaining:
                    selected.append(score.strategy)
                    total_credits += score.credits_per_operation

            if not selected and ranked:
                # At least return the cheapest
                selected.append(min(ranked, key=lambda s: s.credits_per_operation).strategy)

            logger.debug(f"Adaptive discovery selected: {[s.value for s in selected]}")
            return selected

    def record_discovery_result(self, result: DiscoveryResult) -> None:
        """
        Record a discovery result for learning.

        Args:
            result: Discovery result to record
        """
        with self._lock:
            self._discovery_history.append(result)

            # Trim history to last 100 results
            if len(self._discovery_history) > 100:
                self._discovery_history = self._discovery_history[-100:]

            # Trigger reevaluation if enough data
            strategy_history = [r for r in self._discovery_history if r.strategy == result.strategy]
            if len(strategy_history) >= 5:
                self._reevaluate_strategies()

            logger.debug(
                f"Recorded discovery result: {result.strategy.value}, "
                f"{len(result.wallets_found)} wallets, {result.credits_consumed} credits"
            )

    def get_strategy_summary(self) -> Dict[str, Any]:
        """Get summary of all strategy scores."""
        with self._lock:
            summary = {}
            for strategy, score in self._strategy_scores.items():
                summary[strategy.value] = {
                    'efficiency_score': score.efficiency_score,
                    'wallets_per_credit': score.wallets_per_credit,
                    'avg_wqs_found': score.avg_wqs_found,
                    'credits_per_operation': score.credits_per_operation,
                    'total_wallets_found': score.total_wallets_found,
                    'total_credits_consumed': score.total_credits_consumed,
                    'sample_size': score.sample_size,
                }
            return summary

    def get_optimization_suggestions(self) -> List[str]:
        """Get optimization suggestions based on strategy performance."""
        with self._lock:
            suggestions = []

            ranked = self.rank_discovery_strategies()

            if not ranked:
                return ["No strategy data available for optimization"]

            # Check for underperforming strategies
            best = ranked[0]
            worst = ranked[-1]

            if worst.efficiency_score > 0 and best.efficiency_score > 0:
                ratio = best.efficiency_score / worst.efficiency_score
                if ratio > 3.0:
                    suggestions.append(
                        f"Consider reducing {worst.strategy.value} - "
                        f"it's {ratio:.1f}x less efficient than {best.strategy.value}"
                    )

            # Check for insufficient data
            for score in ranked:
                if score.sample_size < 5:
                    suggestions.append(
                        f"Insufficient data for {score.strategy.value} "
                        f"(only {score.sample_size} samples)"
                    )

            # Budget-based suggestions
            if self._remaining_credits > 0:
                best_strategy = self.select_optimal_strategy()
                cost = self._strategy_scores[best_strategy].credits_per_operation

                if self._remaining_credits < cost:
                    suggestions.append(
                        f"Insufficient credits for any strategy "
                        f"(need {cost}, have {self._remaining_credits})"
                    )

            return suggestions

    def _load_state(self) -> None:
        """Load state from disk."""
        state_file = Path(self._config.STATE_FILE)
        if not state_file.exists():
            return

        try:
            with open(state_file, 'r') as f:
                data = json.load(f)

            # Restore strategy scores
            for strat_name, score_data in data.get('strategy_scores', {}).items():
                try:
                    strategy = DiscoveryStrategy(strat_name)
                    self._strategy_scores[strategy] = StrategyScore(
                        strategy=strategy,
                        efficiency_score=score_data.get('efficiency_score', 0),
                        wallets_per_credit=score_data.get('wallets_per_credit', 0),
                        avg_wqs_found=score_data.get('avg_wqs_found', 0),
                        credits_per_operation=score_data.get('credits_per_operation', 0),
                        total_wallets_found=score_data.get('total_wallets_found', 0),
                        total_credits_consumed=score_data.get('total_credits_consumed', 0),
                        sample_size=score_data.get('sample_size', 0),
                        last_updated=score_data.get('last_updated', time.time()),
                    )
                except ValueError:
                    continue

            logger.info(f"Loaded state from {state_file}")

        except Exception as e:
            logger.warning(f"Failed to load state: {e}")

    def _save_state(self) -> None:
        """Save state to disk."""
        try:
            data = {
                'strategy_scores': {
                    strat.value: {
                        'efficiency_score': score.efficiency_score,
                        'wallets_per_credit': score.wallets_per_credit,
                        'avg_wqs_found': score.avg_wqs_found,
                        'credits_per_operation': score.credits_per_operation,
                        'total_wallets_found': score.total_wallets_found,
                        'total_credits_consumed': score.total_credits_consumed,
                        'sample_size': score.sample_size,
                        'last_updated': score.last_updated,
                    }
                    for strat, score in self._strategy_scores.items()
                },
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
