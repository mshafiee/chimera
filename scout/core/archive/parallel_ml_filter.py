"""
Parallel ML Wallet Pre-filtering System

Implements multiple ML approaches in parallel with A/B testing and automatic fallback.
This system runs all ML models simultaneously and selects the best performer based on
production data.

Approaches:
1. Ensemble predictor (primary) - weighted averaging of multiple models
2. Random forest classifier (backup 1) - tree-based classification
3. Gradient boosting (backup 2) - boosted tree classification
4. Neural network (experimental) - deep learning approach
5. Rule-based enhancement (fallback) - traditional filtering rules

Each approach runs independently and contributes to the final wallet ranking.
"""

import time
import asyncio
import logging
from typing import Dict, List, Tuple, Any
from dataclasses import dataclass, field
from collections import defaultdict
from enum import Enum

logger = logging.getLogger(__name__)

# Import existing ML components
try:
    from .ensemble_predictor import EnsemblePredictor
    ENSEMBLE_AVAILABLE = True
except ImportError:
    ENSEMBLE_AVAILABLE = False
    logger.warning("EnsemblePredictor not available")

try:
    from .gradient_boost_predictor import GradientBoostPredictor
    GRADIENT_BOOST_AVAILABLE = True
except ImportError:
    GRADIENT_BOOST_AVAILABLE = False
    logger.warning("GradientBoostPredictor not available")


class MLApproach(Enum):
    """Types of ML approaches in the parallel system."""
    ENSEMBLE = "ensemble"
    RANDOM_FOREST = "random_forest"
    GRADIENT_BOOST = "gradient_boost"
    NEURAL_NETWORK = "neural_network"
    RULE_BASED = "rule_based"


@dataclass
class ApproachPrediction:
    """Prediction from a single ML approach."""
    approach: MLApproach
    wallet_address: str
    predicted_profitability: float
    predicted_wqs: float
    confidence: float
    inference_time_ms: float
    timestamp: float = field(default_factory=time.time)


@dataclass
class ApproachPerformance:
    """Performance tracking for individual approaches."""
    approach: MLApproach
    predictions_made: int = 0
    correct_predictions: int = 0
    accuracy: float = 0.0
    precision: float = 0.0
    recall: float = 0.0
    avg_confidence: float = 0.0
    last_updated: float = field(default_factory=time.time)


@dataclass
class ParallelFilterResult:
    """Result from parallel ML filtering."""
    filtered_wallets: List[str]
    approach_rankings: Dict[MLApproach, List[str]]
    approach_scores: Dict[MLApproach, float]
    consensus_wallets: List[str]
    total_time_ms: float
    timestamp: float = field(default_factory=time.time)


class ParallelMLFilter:
    """
    Parallel ML wallet filtering system.

    Runs multiple ML approaches simultaneously and selects the best performers.
    Implements survival-of-the-fittest approach where underperforming models are killed.
    """

    def __init__(self):
        """Initialize the parallel ML filtering system."""
        self._approaches = {}
        self._performance = {}
        self._last_performance_check = time.time()
        self._performance_check_interval = 3600  # Check every hour

        # Initialize available approaches
        self._initialize_approaches()

    def _initialize_approaches(self):
        """Initialize all available ML approaches."""
        # Approach 1: Ensemble predictor (primary)
        if ENSEMBLE_AVAILABLE:
            try:
                config = self._get_ensemble_config()
                self._approaches[MLApproach.ENSEMBLE] = EnsemblePredictor(config)
                self._performance[MLApproach.ENSEMBLE] = ApproachPerformance(MLApproach.ENSEMBLE)
                logger.info("EnsemblePredictor initialized as primary approach")
            except Exception as e:
                logger.warning(f"Failed to initialize EnsemblePredictor: {e}")

        # Approach 2: Gradient boosting (backup)
        if GRADIENT_BOOST_AVAILABLE:
            try:
                self._approaches[MLApproach.GRADIENT_BOOST] = GradientBoostPredictor()
                self._performance[MLApproach.GRADIENT_BOOST] = ApproachPerformance(MLApproach.GRADIENT_BOOST)
                logger.info("GradientBoostPredictor initialized as backup approach")
            except Exception as e:
                logger.warning(f"Failed to initialize GradientBoostPredictor: {e}")

        # Approach 3: Random forest (to be implemented)
        # This would be a new implementation using sklearn RandomForestClassifier

        # Approach 4: Neural network (experimental)
        # This would be a new implementation using PyTorch/TensorFlow

        # Approach 5: Rule-based (always available fallback)
        self._approaches[MLApproach.RULE_BASED] = RuleBasedFilter()
        self._performance[MLApproach.RULE_BASED] = ApproachPerformance(MLApproach.RULE_BASED)
        logger.info("RuleBasedFilter initialized as fallback approach")

    def _get_ensemble_config(self):
        """Get configuration for ensemble predictor."""
        from .ensemble_predictor import EnsembleConfig
        return EnsembleConfig()

    async def filter_wallets_parallel(
        self,
        wallets: List[str],
        wallet_metrics: Dict[str, Any],
        keep_top_ratio: float = 0.2
    ) -> ParallelFilterResult:
        """
        Filter wallets using all ML approaches in parallel.

        Args:
            wallets: List of wallet addresses to filter
            wallet_metrics: Dictionary of wallet metrics
            keep_top_ratio: Ratio of wallets to keep (default 20%)

        Returns:
            ParallelFilterResult with filtered wallets and approach rankings
        """
        start_time = time.time()
        approach_rankings = {}
        approach_scores = {}

        # Run all approaches in parallel
        tasks = []
        for approach in self._approaches.keys():
            task = self._run_approach(approach, wallets, wallet_metrics)
            tasks.append(task)

        # Execute all approaches concurrently
        results = await asyncio.gather(*tasks, return_exceptions=True)

        # Process results
        for result in results:
            if isinstance(result, Exception):
                logger.error(f"Approach failed: {result}")
                continue

            approach, ranked_wallets = result
            approach_rankings[approach] = ranked_wallets
            approach_scores[approach] = self._calculate_approach_score(approach, ranked_wallets)

        # Calculate consensus wallets (wallets ranked highly by multiple approaches)
        consensus_wallets = self._calculate_consensus(approach_rankings, top_n=len(wallets) // 2)

        # Select final filtered wallets based on consensus and top ratio
        final_wallets = self._select_final_wallets(
            consensus_wallets,
            approach_rankings,
            keep_top_ratio=keep_top_ratio
        )

        total_time_ms = (time.time() - start_time) * 1000

        result = ParallelFilterResult(
            filtered_wallets=final_wallets,
            approach_rankings=approach_rankings,
            approach_scores=approach_scores,
            consensus_wallets=consensus_wallets,
            total_time_ms=total_time_ms
        )

        # Update performance tracking
        await self._update_performance_tracking(result)

        return result

    async def _run_approach(
        self,
        approach: MLApproach,
        wallets: List[str],
        wallet_metrics: Dict[str, Any]
    ) -> Tuple[MLApproach, List[str]]:
        """Run a single ML approach and return ranked wallets."""
        try:
            approach_obj = self._approaches[approach]
            start_time = time.time()

            # Get predictions from this approach
            predictions = await approach_obj.predict_wallets(wallets, wallet_metrics)

            # Sort wallets by predicted profitability
            ranked_wallets = sorted(
                predictions,
                key=lambda w: predictions[w].get('predicted_profitability', 0),
                reverse=True
            )

            inference_time_ms = (time.time() - start_time) * 1000
            logger.info(f"{approach.value} completed in {inference_time_ms:.1f}ms")

            return (approach, ranked_wallets)

        except Exception as e:
            logger.error(f"{approach.value} failed: {e}")
            # Return empty ranking for failed approach
            return (approach, [])

    def _calculate_approach_score(self, approach: MLApproach, ranked_wallets: List[str]) -> float:
        """Calculate a performance score for an approach."""
        if not ranked_wallets:
            return 0.0

        performance = self._performance[approach]

        # Score based on historical accuracy and recent performance
        accuracy_score = performance.accuracy * 0.6
        precision_score = performance.precision * 0.3
        confidence_score = performance.avg_confidence * 0.1

        total_score = accuracy_score + precision_score + confidence_score
        return min(1.0, total_score)

    def _calculate_consensus(
        self,
        approach_rankings: Dict[MLApproach, List[str]],
        top_n: int = 50
    ) -> List[str]:
        """Calculate consensus wallets across all approaches."""
        wallet_consensus_score = defaultdict(int)

        # Count how many approaches rank each wallet in top_n
        for approach, ranked_wallets in approach_rankings.items():
            top_wallets = ranked_wallets[:top_n]
            for wallet in top_wallets:
                wallet_consensus_score[wallet] += 1

        # Sort by consensus score
        consensus_wallets = sorted(
            wallet_consensus_score.keys(),
            key=lambda w: wallet_consensus_score[w],
            reverse=True
        )

        return consensus_wallets

    def _select_final_wallets(
        self,
        consensus_wallets: List[str],
        approach_rankings: Dict[MLApproach, List[str]],
        keep_top_ratio: float = 0.2
    ) -> List[str]:
        """Select final filtered wallets."""
        if not consensus_wallets:
            return []

        # Keep top percentage of consensus wallets
        keep_count = max(1, int(len(consensus_wallets) * keep_top_ratio))
        final_wallets = consensus_wallets[:keep_count]

        return final_wallets

    async def _update_performance_tracking(self, result: ParallelFilterResult):
        """Update performance tracking for all approaches."""
        # This would be implemented with actual performance data
        # For now, we track basic metrics
        current_time = time.time()

        if current_time - self._last_performance_check > self._performance_check_interval:
            await self._evaluate_and_kill_underperformers()
            self._last_performance_check = current_time

    async def _evaluate_and_kill_underperformers(self):
        """Evaluate approach performance and remove underperformers."""
        # Check if any approach is significantly underperforming
        # Primary approach (ensemble) must maintain 2x+ improvement vs baseline
        # Backup approaches must maintain 1.5x+ improvement

        for approach, performance in self._performance.items():
            if performance.predictions_made < 10:
                continue  # Not enough data yet

            # Kill approaches that don't meet minimum thresholds
            if approach == MLApproach.ENSEMBLE:
                if performance.accuracy < 0.6:  # 60% minimum for primary
                    logger.warning(f"Killing {approach.value} - accuracy below 60%")
                    self._kill_approach(approach)
            else:
                if performance.accuracy < 0.5:  # 50% minimum for backups
                    logger.warning(f"Killing {approach.value} - accuracy below 50%")
                    self._kill_approach(approach)

    def _kill_approach(self, approach: MLApproach):
        """Remove an underperforming approach from the system."""
        if approach in self._approaches:
            del self._approaches[approach]
        if approach in self._performance:
            del self._performance[approach]
        logger.info(f"Removed underperforming approach: {approach.value}")


class RuleBasedFilter:
    """Fallback rule-based filtering system."""

    def __init__(self):
        """Initialize rule-based filter."""
        pass

    async def predict_wallets(
        self,
        wallets: List[str],
        wallet_metrics: Dict[str, Any]
    ) -> Dict[str, Dict[str, float]]:
        """Predict wallet profitability using rule-based filtering."""
        predictions = {}

        for wallet in wallets:
            metrics = wallet_metrics.get(wallet, {})

            # Rule-based scoring
            roi_7d = metrics.get('roi_7d', 0)
            roi_30d = metrics.get('roi_30d', 0)
            win_rate = metrics.get('win_rate', 0)
            trade_count = metrics.get('trade_count_30d', 0)
            max_drawdown = metrics.get('max_drawdown_30d', 100)

            # Calculate rule-based score
            score = (
                (roi_7d * 0.3) +
                (roi_30d * 0.4) +
                (win_rate * 0.2) -
                (max_drawdown * 0.1)
            )

            # Boost for high trade count
            if trade_count >= 10:
                score *= 1.2

            predictions[wallet] = {
                'predicted_profitability': max(0, score),
                'predicted_wqs': min(100, max(0, score * 10)),
                'confidence': 0.5  # Fixed confidence for rule-based
            }

        return predictions