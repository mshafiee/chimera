"""
Scout Optimization Integration Module

This module integrates all optimization systems into a unified interface:
- Helius credit tracking and optimization
- Advanced multi-level caching
- ML-based profitability prediction
- Helius Developer Plan optimization
- Production monitoring and alerting

Usage:
    from scout.core.scout_optimizer import ScoutOptimizer

    optimizer = ScoutOptimizer()
    optimizer.initialize()

    # Use optimized operations
    prediction = optimizer.predict_profitability(wallet_metrics)
    cached_data = optimizer.get_cached_data(key)
    can_proceed = optimizer.can_make_request(cost, category)
"""
import os
import logging
from typing import Dict, List, Optional, Tuple, Any
from datetime import datetime

from .helius_credit_tracker import (
    get_credit_tracker,
    HeliusCreditTracker,
    can_analyze_wallet
)
from .advanced_cache import (
    get_cache,
    AdvancedCache,
    CacheCategory,
    get_wallet_metrics,
    set_wallet_metrics
)
from .profitability_predictor import (
    get_profitability_predictor,
    ProfitabilityPredictor,
    ProfitabilityPrediction
)
from .helius_optimizer import (
    get_helius_optimizer,
    HeliusOptimizer
)
from .production_monitor import (
    get_production_monitor,
    ProductionMonitor,
    AlertSeverity
)

logger = logging.getLogger(__name__)


class ScoutOptimizer:
    """
    Main optimization integration point.

    Features:
    - Unified interface for all optimization systems
    - Easy integration with existing Scout code
    - Automatic optimization decisions
    - Production monitoring
    - Growth goal optimization
    """

    def __init__(self):
        """Initialize the Scout optimizer."""
        # Optimization components
        self._credit_tracker: Optional[HeliusCreditTracker] = None
        self._cache: Optional[AdvancedCache] = None
        self._profitability_predictor: Optional[ProfitabilityPredictor] = None
        self._helius_optimizer: Optional[HeliusOptimizer] = None
        self._production_monitor: Optional[ProductionMonitor] = None

        # Configuration
        self._initialized = False
        self._growth_optimized = os.getenv("SCOUT_GROWTH_OPTIMIZED", "true").lower() == "true"
        self._current_capital = float(os.getenv("SCOUT_CURRENT_CAPITAL", "200.0"))
        self._target_capital = float(os.getenv("SCOUT_TARGET_CAPITAL", "1000.0"))

        logger.info("Scout Optimizer created")

    def initialize(self) -> bool:
        """
        Initialize all optimization systems.

        Returns:
            True if successful
        """
        try:
            logger.info("Initializing Scout optimization systems...")

            # Initialize credit tracking
            self._credit_tracker = get_credit_tracker()
            logger.info("✓ Credit tracking initialized")

            # Initialize caching
            self._cache = get_cache()
            logger.info("✓ Advanced caching initialized")

            # Initialize profitability predictor
            self._profitability_predictor = get_profitability_predictor()
            logger.info("✓ Profitability prediction initialized")

            # Initialize Helius optimizer
            self._helius_optimizer = get_helius_optimizer()
            logger.info("✓ Helius optimizer initialized")

            # Initialize production monitoring
            self._production_monitor = get_production_monitor()
            logger.info("✓ Production monitoring initialized")

            self._initialized = True

            logger.info("All optimization systems initialized successfully")
            return True

        except Exception as e:
            logger.error(f"Failed to initialize optimization systems: {e}")
            return False

    # ========== Caching Operations ==========

    def get_cached_data(self, prefix: str, identifier: str, *args,
                       category: str = "wallet_metrics", default: Any = None) -> Optional[Any]:
        """Get cached data with automatic category handling."""
        if not self._cache:
            return default

        # Map string category to CacheCategory
        try:
            cache_category = CacheCategory[category.upper()]
        except (KeyError, AttributeError):
            cache_category = CacheCategory.WALLET_METRICS

        return self._cache.get(prefix, identifier, *args,
                              category=cache_category, default=default)

    def set_cached_data(self, prefix: str, identifier: str, value: Any,
                       *args, category: str = "wallet_metrics"):
        """Set cached data with automatic category handling."""
        if not self._cache:
            return

        try:
            cache_category = CacheCategory[category.upper()]
        except (KeyError, AttributeError):
            cache_category = CacheCategory.WALLET_METRICS

        self._cache.set(prefix, identifier, value, *args, category=cache_category)

    def invalidate_cache(self, prefix: str, identifier: str, *args):
        """Invalidate cache entry."""
        if self._cache:
            self._cache.invalidate(prefix, identifier, *args)

    # ========== Wallet Operations ==========

    def can_analyze_wallet(self, wallet_address: str, wallet_wqs: Optional[float] = None) -> Tuple[bool, str]:
        """Check if wallet can be analyzed given current budget."""
        if not self._credit_tracker:
            return True, "No credit tracker"

        return can_analyze_wallet(wallet_wqs)

    def can_validate_backtest(self) -> Tuple[bool, str]:
        """Check if backtest validation can be performed given current budget."""
        if not self._credit_tracker:
            return True, "No credit tracker"

        # Backtest validation is expensive (approx. 5000 credits)
        # Check if we have enough budget for validation
        from .helius_credit_tracker import RequestPriority
        can_proceed, reason = self._credit_tracker.can_make_request(
            cost=5000,  # Estimated cost for backtest validation
            category="validation",  # Validation category
            priority=RequestPriority.MEDIUM,
            expected_value=0.7  # Backtests have high value for wallet validation
        )
        return can_proceed, reason

    def cache_wallet_metrics(self, address: str, metrics: Dict[str, Any]):
        """Cache wallet metrics."""
        set_wallet_metrics(address, metrics)

    def get_cached_wallet_metrics(self, address: str) -> Optional[Dict[str, Any]]:
        """Get cached wallet metrics."""
        return get_wallet_metrics(address)

    # ========== Profitability Prediction ==========

    def predict_profitability(self, wallet_metrics: Dict[str, Any]) -> ProfitabilityPrediction:
        """Predict wallet profitability."""
        if not self._profitability_predictor:
            # Return default prediction if not initialized
            from .profitability_predictor import ProfitabilityPrediction, ProfitabilityClass
            return ProfitabilityPrediction(
                expected_return_pct=0.0,
                confidence=0.0,
                risk_score=0.5,
                profitability_class=ProfitabilityClass.LOW_PROFIT,
                feature_importance={},
                prediction_timestamp=datetime.now().timestamp()
            )

        return self._profitability_predictor.predict_wallet_profitability(wallet_metrics)

    def rank_wallets_by_profitability(self, wallets_metrics: List[Dict[str, Any]],
                                     max_wallets: int = 50) -> List[Tuple[str, ProfitabilityPrediction]]:
        """Rank wallets by predicted profitability."""
        if not self._profitability_predictor:
            return []

        return self._profitability_predictor.rank_wallets_by_profitability(wallets_metrics, max_wallets)

    def get_investment_allocation(self, predictions: List[Tuple[str, ProfitabilityPrediction]]) -> Dict[str, float]:
        """Calculate optimal investment allocation."""
        if not self._profitability_predictor:
            return {}

        return self._profitability_predictor.get_investment_allocation(predictions, self._current_capital)

    # ========== Helius Optimization ==========

    def can_make_request(self, cost: int, category: str = "analysis",
                        priority: str = "medium", expected_value: float = 0.5) -> Tuple[bool, str]:
        """Check if request can be made given current constraints."""
        if not self._credit_tracker:
            return True, "No credit tracker"

        from .helius_credit_tracker import RequestPriority
        try:
            req_priority = RequestPriority[priority.upper()]
        except (KeyError, AttributeError):
            req_priority = RequestPriority.MEDIUM

        return self._credit_tracker.can_make_request(cost, category, req_priority, expected_value)

    def optimize_wallet_count(self, target_count: int) -> int:
        """Optimize wallet analysis count for current budget."""
        if not self._helius_optimizer:
            return target_count

        return self._helius_optimizer.optimize_wallet_analysis(target_count)

    def optimize_discovery_depth(self, current_depth_hours: int) -> int:
        """Optimize discovery depth for budget efficiency."""
        if not self._helius_optimizer:
            return current_depth_hours

        return self._helius_optimizer.optimize_discovery_depth(current_depth_hours)

    # ========== Production Monitoring ==========

    def check_production_health(self) -> Dict[str, Any]:
        """Get production health status."""
        if not self._production_monitor:
            return {"status": "unknown", "message": "Monitor not initialized"}

        return self._production_monitor.get_health_status()

    def is_production_ready(self) -> Tuple[bool, List[str]]:
        """Validate production readiness."""
        if not self._production_monitor:
            return True, ["Production monitoring not available"]

        return self._production_monitor.validate_production_readiness()

    def create_alert(self, severity: str, title: str, message: str,
                    source: str = "scout", details: Dict[str, Any] = None):
        """Create an alert."""
        if not self._production_monitor:
            return

        try:
            alert_severity = AlertSeverity[severity.upper()]
        except (KeyError, AttributeError):
            alert_severity = AlertSeverity.INFO

        self._production_monitor.create_alert(alert_severity, title, message, source, details)

    # ========== Status and Reporting ==========

    def print_optimization_report(self):
        """Print comprehensive optimization report."""
        print("\n" + "="*80)
        print("SCOUT OPTIMIZATION REPORT")
        print("="*80)

        print(f"\nInitialization Status: {'READY' if self._initialized else 'NOT INITIALIZED'}")
        print(f"Growth Optimized: {self._growth_optimized}")
        print(f"Current Capital: ${self._current_capital:.2f}")
        print(f"Target Capital: ${self._target_capital:.2f}")
        print(f"Growth Progress: {(self._current_capital/self._target_capital)*100:.1f}%")

        # Credit tracker status
        if self._credit_tracker:
            print("\n--- Credit Tracking ---")
            self._credit_tracker.print_status_report()

        # Cache status
        if self._cache:
            print("\n--- Cache Status ---")
            self._cache.print_stats()

        # Production monitoring status
        if self._production_monitor:
            print("\n--- Production Monitoring ---")
            self._production_monitor.print_status_report()

        # Helius optimizer status
        if self._helius_optimizer:
            print("\n--- Helius Optimization ---")
            self._helius_optimizer.print_status_report()

        print("="*80 + "\n")

    def get_optimization_suggestions(self) -> List[str]:
        """Get optimization suggestions from all systems."""
        suggestions = []

        if self._credit_tracker:
            suggestions.extend(self._credit_tracker.get_optimization_suggestions())

        if self._helius_optimizer:
            suggestions.extend(self._helius_optimizer.get_optimization_suggestions())

        if self._production_monitor:
            # Check production readiness
            is_ready, issues = self.is_production_ready()
            if not is_ready:
                suggestions.append("Production readiness issues:")
                suggestions.extend([f"  - {issue}" for issue in issues])

        return suggestions

    # ========== Lifecycle Management ==========

    def start_monitoring(self):
        """Start production monitoring."""
        if self._production_monitor:
            self._production_monitor.start_monitoring()

    def stop_monitoring(self):
        """Stop all optimization systems."""
        if self._production_monitor:
            self._production_monitor.stop_monitoring()

        logger.info("Scout Optimizer stopped")

    def shutdown(self):
        """Shutdown all optimization systems."""
        logger.info("Shutting down Scout optimization systems...")

        self.stop_monitoring()

        # Shutdown individual components
        if self._credit_tracker:
            self._credit_tracker.shutdown()

        if self._cache:
            self._cache.shutdown()

        if self._production_monitor:
            self._production_monitor.shutdown()

        logger.info("Scout Optimizer shut down complete")


# Global singleton instance
_optimizer: Optional[ScoutOptimizer] = None


def get_scout_optimizer() -> ScoutOptimizer:
    """Get the global Scout optimizer singleton."""
    global _optimizer

    if _optimizer is None:
        _optimizer = ScoutOptimizer()

    return _optimizer


def initialize_scout_optimizer() -> bool:
    """Initialize the global Scout optimizer."""
    optimizer = get_scout_optimizer()
    return optimizer.initialize()


if __name__ == "__main__":
    # Test the optimizer
    optimizer = get_scout_optimizer()

    if optimizer.initialize():
        print("Scout Optimizer initialized successfully")

        # Print status report
        optimizer.print_optimization_report()

        # Get suggestions
        suggestions = optimizer.get_optimization_suggestions()
        if suggestions:
            print("\nOptimization Suggestions:")
            for i, suggestion in enumerate(suggestions, 1):
                print(f"  {i}. {suggestion}")

        # Cleanup
        optimizer.shutdown()
    else:
        print("Failed to initialize Scout Optimizer")