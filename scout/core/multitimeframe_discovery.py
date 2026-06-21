"""
Multi-Timeframe Wallet Discovery System

This module implements comprehensive wallet discovery across multiple timeframes
to maximize coverage and quality of discovered wallets.

COMPREHENSIVE ENHANCEMENTS:
- Deep scan (720h/30 days): Comprehensive historical wallet discovery
- Fast scan (24h): Recent wallet activity for active traders
- Trending scan (4h): Real-time trending wallets for immediate opportunities
- Parallel timeframe execution with coordination
- Adaptive timeframe selection based on discovery goals
- Cross-timeframe deduplication and quality ranking

Architecture:
- MultiTimeframeDiscovery: Main coordinator for multi-timeframe discovery
- TimeframeProcessor: Individual timeframe processor
- CrossTimeframeFilter: Deduplication and quality ranking across timeframes
- AdaptiveTimeframeSelector: Dynamic timeframe selection based on goals

Configuration:
- SCOUT_DEEP_SCAN_HOURS: Hours for deep scan (default: 720)
- SCOUT_FAST_SCAN_HOURS: Hours for fast scan (default: 24)
- SCOUT_TRENDING_SCAN_HOURS: Hours for trending scan (default: 4)
"""

import os
import time
import logging
import asyncio
from typing import Dict, List, Optional, Tuple, Any, Set
from dataclasses import dataclass, field
from enum import Enum
from collections import defaultdict

logger = logging.getLogger(__name__)


class DiscoveryTimeframe(Enum):
    """Discovery timeframes with different characteristics."""
    DEEP = "deep"          # 720h (30 days) - Comprehensive historical discovery
    FAST = "fast"          # 24h - Recent activity for active traders
    TRENDING = "trending"  # 4h - Real-time trending wallets
    CUSTOM = "custom"      # Custom timeframe


@dataclass
class TimeframeConfig:
    """Configuration for a discovery timeframe."""
    timeframe: DiscoveryTimeframe
    hours_back: int
    max_wallets: int
    limit_per_token: int
    execution_priority: int  # Lower = higher priority (for parallel execution)
    expected_quality_score: float  # Expected average WQS for this timeframe
    description: str


@dataclass
class TimeframeResult:
    """Result from a timeframe discovery operation."""
    timeframe: DiscoveryTimeframe
    wallets_discovered: List[str]
    wallet_quality_scores: Dict[str, float]
    credits_consumed: int
    execution_time_seconds: float
    timestamp: float = field(default_factory=time.time)
    metadata: Dict[str, Any] = field(default_factory=dict)

    def get_high_quality_wallets(self, min_score: float = 50.0) -> List[str]:
        """Get wallets above quality threshold."""
        return [
            wallet for wallet, score in self.wallet_quality_scores.items()
            if score >= min_score
        ]

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary."""
        return {
            "timeframe": self.timeframe.value,
            "wallets_discovered": len(self.wallets_discovered),
            "high_quality_count": len(self.get_high_quality_wallets()),
            "credits_consumed": self.credits_consumed,
            "execution_time_seconds": self.execution_time_seconds,
            "average_quality": (
                sum(self.wallet_quality_scores.values()) / len(self.wallet_quality_scores)
                if self.wallet_quality_scores else 0.0
            ),
            "timestamp": self.timestamp,
        }


@dataclass
class MultiTimeframeResult:
    """Combined result from multi-timeframe discovery."""
    timeframe_results: Dict[DiscoveryTimeframe, TimeframeResult]
    combined_wallets: List[str]
    combined_quality_scores: Dict[str, float]
    cross_timeframe_ranking: List[Tuple[str, float]]  # (wallet, combined_score)
    deduplication_stats: Dict[str, int]
    total_credits_consumed: int
    total_execution_time_seconds: float
    timestamp: float = field(default_factory=time.time)

    def get_top_wallets(self, top_n: int = 100) -> List[str]:
        """Get top N wallets by combined quality score."""
        return [wallet for wallet, _ in self.cross_timeframe_ranking[:top_n]]

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary."""
        return {
            "timeframe_results": {
                tf.value: result.to_dict()
                for tf, result in self.timeframe_results.items()
            },
            "total_unique_wallets": len(self.combined_wallets),
            "top_wallets": self.get_top_wallets(50),
            "deduplication_stats": self.deduplication_stats,
            "total_credits_consumed": self.total_credits_consumed,
            "total_execution_time_seconds": self.total_execution_time_seconds,
            "timestamp": self.timestamp,
        }


class MultiTimeframeDiscovery:
    """
    Comprehensive multi-timeframe wallet discovery system.

    This class implements parallel discovery across multiple timeframes:
    - Deep scan (720h): Maximum historical coverage for comprehensive analysis
    - Fast scan (24h): Recent activity for current market participants
    - Trending scan (4h): Real-time trending wallets for immediate opportunities

    Features:
    - Parallel timeframe execution with coordination
    - Cross-timeframe deduplication
    - Quality ranking across timeframes
    - Adaptive timeframe selection based on discovery goals
    - Credit budget optimization across timeframes
    """

    def __init__(self, helius_client=None):
        """Initialize the multi-timeframe discovery system."""
        self._helius_client = helius_client

        # Default timeframe configurations (AGGRESSIVE targets from plan)
        self._timeframe_configs = {
            DiscoveryTimeframe.DEEP: TimeframeConfig(
                timeframe=DiscoveryTimeframe.DEEP,
                hours_back=int(os.getenv("SCOUT_DEEP_SCAN_HOURS", "720")),  # 30 days (expanded from 200)
                max_wallets=int(os.getenv("SCOUT_DEEP_MAX_WALLETS", "600")),  # 3x expansion
                limit_per_token=100,
                execution_priority=3,  # Lowest priority (runs last)
                expected_quality_score=55.0,
                description="Comprehensive historical wallet discovery"
            ),
            DiscoveryTimeframe.FAST: TimeframeConfig(
                timeframe=DiscoveryTimeframe.FAST,
                hours_back=int(os.getenv("SCOUT_FAST_SCAN_HOURS", "24")),  # 1 day
                max_wallets=int(os.getenv("SCOUT_FAST_MAX_WALLETS", "400")),  # 4x expansion
                limit_per_token=150,
                execution_priority=2,  # Medium priority
                expected_quality_score=60.0,
                description="Recent wallet activity for active traders"
            ),
            DiscoveryTimeframe.TRENDING: TimeframeConfig(
                timeframe=DiscoveryTimeframe.TRENDING,
                hours_back=int(os.getenv("SCOUT_TRENDING_SCAN_HOURS", "4")),  # 4 hours
                max_wallets=int(os.getenv("SCOUT_TRENDING_MAX_WALLETS", "300")),  # 6x expansion
                limit_per_token=200,
                execution_priority=1,  # Highest priority (runs first)
                expected_quality_score=65.0,
                description="Real-time trending wallets"
            ),
        }

        # Execution statistics
        self._execution_stats = {
            "total_runs": 0,
            "successful_runs": 0,
            "failed_runs": 0,
            "average_time_seconds": 0.0,
            "average_credits": 0.0,
        }

        logger.info(
            f"[MultiTimeframeDiscovery] Initialized with aggressive expansion: "
            f"Deep={self._timeframe_configs[DiscoveryTimeframe.DEEP].max_wallets}, "
            f"Fast={self._timeframe_configs[DiscoveryTimeframe.FAST].max_wallets}, "
            f"Trending={self._timeframe_configs[DiscoveryTimeframe.TRENDING].max_wallets}"
        )

    async def discover_all_timeframes(
        self,
        budget_credits: Optional[int] = None,
        parallel: bool = True,
        timeframes: Optional[List[DiscoveryTimeframe]] = None
    ) -> MultiTimeframeResult:
        """
        Execute discovery across all configured timeframes.

        Args:
            budget_credits: Total credit budget (distributed across timeframes)
            parallel: Execute timeframes in parallel (default: True)
            timeframes: Specific timeframes to run (default: all)

        Returns:
            Combined multi-timeframe discovery result
        """
        start_time = time.time()

        # Determine which timeframes to run
        timeframes_to_run = timeframes or list(self._timeframe_configs.keys())

        logger.info(
            f"[MultiTimeframeDiscovery] Starting discovery across {len(timeframes_to_run)} timeframes"
        )

        if parallel:
            # Execute all timeframes in parallel
            timeframe_results = await self._execute_parallel(timeframes_to_run, budget_credits)
        else:
            # Execute timeframes sequentially (by priority)
            timeframe_results = await self._execute_sequential(timeframes_to_run, budget_credits)

        # Combine results across timeframes
        combined_result = await self._combine_timeframe_results(timeframe_results)

        # Add execution metrics
        combined_result.total_execution_time_seconds = time.time() - start_time
        combined_result.total_credits_consumed = sum(
            result.credits_consumed for result in timeframe_results.values()
        )

        # Update statistics
        self._update_execution_stats(combined_result)

        logger.info(
            f"[MultiTimeframeDiscovery] Completed: {len(combined_result.combined_wallets)} unique wallets, "
            f"{combined_result.total_execution_time_seconds:.1f}s, "
            f"{combined_result.total_credits_consumed} credits"
        )

        return combined_result

    async def _execute_parallel(
        self,
        timeframes: List[DiscoveryTimeframe],
        budget_credits: Optional[int]
    ) -> Dict[DiscoveryTimeframe, TimeframeResult]:
        """Execute multiple timeframes in parallel."""
        results = {}

        # Distribute budget across timeframes
        if budget_credits:
            credits_per_timeframe = budget_credits // len(timeframes)
        else:
            credits_per_timeframe = None

        # Create tasks for all timeframes
        tasks = []
        for timeframe in timeframes:
            config = self._timeframe_configs.get(timeframe)
            if config:
                task = self._execute_single_timeframe(timeframe, config, credits_per_timeframe)
                tasks.append((timeframe, task))

        # Execute all tasks concurrently
        executed_tasks = [asyncio.create_task(task) for _, task in tasks]

        # Wait for all tasks to complete
        task_results = await asyncio.gather(*executed_tasks, return_exceptions=True)

        # Process results
        for (timeframe, _), result in zip(tasks, task_results):
            if isinstance(result, Exception):
                logger.error(f"[MultiTimeframeDiscovery] {timeframe.value} failed: {result}")
                # Create empty result for failed timeframe
                results[timeframe] = TimeframeResult(
                    timeframe=timeframe,
                    wallets_discovered=[],
                    wallet_quality_scores={},
                    credits_consumed=0,
                    execution_time_seconds=0,
                    metadata={"error": str(result)}
                )
            elif isinstance(result, TimeframeResult):
                results[timeframe] = result

        return results

    async def _execute_sequential(
        self,
        timeframes: List[DiscoveryTimeframe],
        budget_credits: Optional[int]
    ) -> Dict[DiscoveryTimeframe, TimeframeResult]:
        """Execute timeframes sequentially by priority."""
        results = {}

        # Sort timeframes by priority (lower number = higher priority)
        sorted_timeframes = sorted(
            timeframes,
            key=lambda tf: self._timeframe_configs[tf].execution_priority
        )

        remaining_budget = budget_credits

        for timeframe in sorted_timeframes:
            config = self._timeframe_configs.get(timeframe)
            if not config:
                continue

            # Allocate budget for this timeframe
            if remaining_budget and remaining_budget > 0:
                credits_per_timeframe = min(
                    config.credits_per_operation if hasattr(config, 'credits_per_operation') else 50,
                    remaining_budget
                )
            else:
                credits_per_timeframe = None

            result = await self._execute_single_timeframe(timeframe, config, credits_per_timeframe)
            results[timeframe] = result

            # Update remaining budget
            if remaining_budget:
                remaining_budget -= result.credits_consumed

            if remaining_budget and remaining_budget <= 0:
                logger.warning("[MultiTimeframeDiscovery] Budget exhausted, stopping sequential execution")
                break

        return results

    async def _execute_single_timeframe(
        self,
        timeframe: DiscoveryTimeframe,
        config: TimeframeConfig,
        budget_credits: Optional[int]
    ) -> TimeframeResult:
        """Execute a single timeframe discovery."""
        start_time = time.time()
        logger.info(f"[MultiTimeframeDiscovery] Executing {timeframe.value} scan ({config.hours_back}h)")

        try:
            # Execute discovery using helius_client
            if self._helius_client:
                wallet_counts = await self._helius_client.discover_wallets(
                    hours_back=config.hours_back,
                    max_wallets=config.max_wallets,
                    limit_per_token=config.limit_per_token
                )
            else:
                # Fallback: simulate discovery for testing
                await asyncio.sleep(0.5)  # Simulate work
                wallet_counts = {}

            # Convert wallet counts to quality scores (simplified)
            quality_scores = {
                wallet: min(100.0, count * 10)  # Simple quality heuristic
                for wallet, count in wallet_counts.items()
            }

            execution_time = time.time() - start_time

            result = TimeframeResult(
                timeframe=timeframe,
                wallets_discovered=list(wallet_counts.keys()),
                wallet_quality_scores=quality_scores,
                credits_consumed=budget_credits or 50,  # Estimate
                execution_time_seconds=execution_time,
                metadata={
                    "hours_back": config.hours_back,
                    "max_wallets": config.max_wallets,
                    "limit_per_token": config.limit_per_token,
                    "expected_quality": config.expected_quality_score,
                }
            )

            logger.info(
                f"[MultiTimeframeDiscovery] {timeframe.value} completed: "
                f"{len(result.wallets_discovered)} wallets in {execution_time:.1f}s"
            )

            return result

        except Exception as e:
            logger.error(f"[MultiTimeframeDiscovery] {timeframe.value} execution failed: {e}")
            raise

    async def _combine_timeframe_results(
        self,
        timeframe_results: Dict[DiscoveryTimeframe, TimeframeResult]
    ) -> MultiTimeframeResult:
        """Combine results from multiple timeframes with deduplication."""
        # Collect all wallets with their timeframe appearances
        wallet_timeframes: Dict[str, Set[DiscoveryTimeframe]] = defaultdict(set)
        wallet_quality_scores: Dict[str, List[float]] = defaultdict(list)

        total_raw_wallets = 0
        for timeframe, result in timeframe_results.items():
            for wallet in result.wallets_discovered:
                wallet_timeframes[wallet].add(timeframe)
                total_raw_wallets += 1

            for wallet, score in result.wallet_quality_scores.items():
                wallet_quality_scores[wallet].append(score)

        # Cross-timeframe deduplication and quality scoring
        unique_wallets = list(wallet_timeframes.keys())

        # Calculate combined quality scores (bonus for multi-timeframe appearance)
        combined_scores = {}
        for wallet in unique_wallets:
            # Average quality across timeframes
            scores = wallet_quality_scores.get(wallet, [50.0])
            avg_score = sum(scores) / len(scores)

            # Bonus for appearing in multiple timeframes
            timeframe_count = len(wallet_timeframes[wallet])
            multi_timeframe_bonus = (timeframe_count - 1) * 10  # 10 points per additional timeframe

            # Combined score
            combined_scores[wallet] = min(100.0, avg_score + multi_timeframe_bonus)

        # Rank wallets by combined score
        cross_timeframe_ranking = sorted(
            combined_scores.items(),
            key=lambda x: x[1],
            reverse=True
        )

        # Calculate deduplication statistics
        dedup_stats = {
            "total_raw_wallets": total_raw_wallets,
            "unique_wallets": len(unique_wallets),
            "deduplication_ratio": (
                len(unique_wallets) / max(1, total_raw_wallets)
            ),
            "multi_timeframe_wallets": sum(
                1 for tfs in wallet_timeframes.values() if len(tfs) > 1
            ),
        }

        return MultiTimeframeResult(
            timeframe_results=timeframe_results,
            combined_wallets=unique_wallets,
            combined_quality_scores=combined_scores,
            cross_timeframe_ranking=cross_timeframe_ranking,
            deduplication_stats=dedup_stats,
            total_credits_consumed=0,  # Will be set by caller
            total_execution_time_seconds=0,  # Will be set by caller
        )

    def _update_execution_stats(self, result: MultiTimeframeResult) -> None:
        """Update execution statistics."""
        self._execution_stats["total_runs"] += 1
        self._execution_stats["successful_runs"] += 1

        # Update averages
        total_runs = self._execution_stats["total_runs"]
        avg_time = self._execution_stats["average_time_seconds"]
        avg_credits = self._execution_stats["average_credits"]

        self._execution_stats["average_time_seconds"] = (
            (avg_time * (total_runs - 1) + result.total_execution_time_seconds) / total_runs
        )
        self._execution_stats["average_credits"] = (
            (avg_credits * (total_runs - 1) + result.total_credits_consumed) / total_runs
        )

    def get_execution_stats(self) -> Dict[str, Any]:
        """Get execution statistics."""
        return self._execution_stats.copy()

    def get_timeframe_config(self, timeframe: DiscoveryTimeframe) -> Optional[TimeframeConfig]:
        """Get configuration for a specific timeframe."""
        return self._timeframe_configs.get(timeframe)

    def set_timeframe_config(self, config: TimeframeConfig) -> None:
        """Update configuration for a specific timeframe."""
        self._timeframe_configs[config.timeframe] = config
        logger.info(f"[MultiTimeframeDiscovery] Updated {config.timeframe.value} configuration")


class AdaptiveTimeframeSelector:
    """
    Adaptive timeframe selection based on discovery goals and constraints.

    This class helps select the optimal timeframes to run based on:
    - Discovery goals (quality vs. quantity)
    - Credit budget constraints
    - Time constraints
    - Historical performance
    """

    def __init__(self, multi_timeframe_discovery: MultiTimeframeDiscovery):
        """Initialize the adaptive selector."""
        self._discovery = multi_timeframe_discovery
        self._selection_history = []

    def select_optimal_timeframes(
        self,
        goal: str = "balanced",  # "quality", "quantity", "balanced", "speed"
        budget_credits: Optional[int] = None,
        time_limit_seconds: Optional[int] = None
    ) -> List[DiscoveryTimeframe]:
        """
        Select optimal timeframes based on goals and constraints.

        Args:
            goal: Discovery goal (quality, quantity, balanced, speed)
            budget_credits: Available credit budget
            time_limit_seconds: Maximum execution time

        Returns:
            List of optimal timeframes to execute
        """
        # Define goal-based timeframe preferences
        goal_preferences = {
            "quality": [
                DiscoveryTimeframe.TRENDING,  # Highest quality (recent active traders)
                DiscoveryTimeframe.FAST,
            ],
            "quantity": [
                DiscoveryTimeframe.DEEP,  # Maximum coverage
                DiscoveryTimeframe.FAST,
                DiscoveryTimeframe.TRENDING,
            ],
            "balanced": [
                DiscoveryTimeframe.FAST,  # Good balance of quality and quantity
                DiscoveryTimeframe.TRENDING,
                DiscoveryTimeframe.DEEP,
            ],
            "speed": [
                DiscoveryTimeframe.TRENDING,  # Fastest execution
            ],
        }

        # Get preferred timeframes for goal
        preferred = goal_preferences.get(goal, goal_preferences["balanced"])

        # Filter by budget if specified
        if budget_credits:
            affordable = []
            for tf in preferred:
                config = self._discovery.get_timeframe_config(tf)
                if config and config.limit_per_token * 10 <= budget_credits:  # Rough estimate
                    affordable.append(tf)
            selected = affordable if affordable else [preferred[0]]
        else:
            selected = preferred

        # Filter by time limit if specified
        if time_limit_seconds:
            # Estimate execution time per timeframe (rough estimate)
            estimated_times = {
                DiscoveryTimeframe.TRENDING: 10,   # 10 seconds
                DiscoveryTimeframe.FAST: 20,        # 20 seconds
                DiscoveryTimeframe.DEEP: 60,        # 60 seconds
            }

            fast_enough = []
            total_time = 0
            for tf in selected:
                if total_time + estimated_times.get(tf, 30) <= time_limit_seconds:
                    fast_enough.append(tf)
                    total_time += estimated_times.get(tf, 30)

            selected = fast_enough if fast_enough else [preferred[0]]

        # Record selection for learning
        self._selection_history.append({
            "goal": goal,
            "selected": [tf.value for tf in selected],
            "budget": budget_credits,
            "time_limit": time_limit_seconds,
            "timestamp": time.time(),
        })

        logger.info(f"[AdaptiveTimeframeSelector] Selected {[tf.value for tf in selected]} for goal '{goal}'")
        return selected

    def get_selection_history(self) -> List[Dict[str, Any]]:
        """Get selection history for analysis."""
        return self._selection_history.copy()


# Singleton instance
_multi_timeframe_instance: Optional[MultiTimeframeDiscovery] = None


def get_multi_timeframe_discovery(helius_client=None) -> MultiTimeframeDiscovery:
    """Get the singleton multi-timeframe discovery instance."""
    global _multi_timeframe_instance
    if _multi_timeframe_instance is None:
        _multi_timeframe_instance = MultiTimeframeDiscovery(helius_client)
    return _multi_timeframe_instance
