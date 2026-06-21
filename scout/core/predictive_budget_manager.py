"""
Predictive Credit Budget Manager for Helius Developer Plan

This module implements proactive credit allocation using historical pattern analysis
and predicted ROI to optimize credit usage under Developer Plan constraints (10M/month).

Features:
- Credit forecasting with 7-day moving average and trend extrapolation
- Predictive ROI allocation based on category performance
- Real-time credit tracking with alerts
- Optimization suggestions for credit efficiency

Developer Plan Constraints:
- 10M credits/month (~333K daily target)
- 50 requests/second rate limit
- 5 sendTransaction/second
"""

import time
import logging
from datetime import datetime
from typing import Dict, List, Optional, Tuple, Any
from dataclasses import dataclass, field
from enum import Enum
import threading
import json
from pathlib import Path
import statistics

logger = logging.getLogger(__name__)


class BudgetCategory(Enum):
    """Budget allocation categories for Helius credit usage."""
    DISCOVERY = "discovery"           # Wallet discovery operations
    ANALYSIS = "analysis"            # Wallet analysis and WQS calculation
    VALIDATION = "validation"        # Backtest validation
    ENRICHMENT = "enrichment"        # Metadata and enrichment
    MONITORING = "monitoring"        # Active position monitoring
    RESERVE = "reserve"              # Emergency reserve


class CreditAlertLevel(Enum):
    """Credit alert severity levels."""
    NORMAL = "normal"                # > 50% remaining
    WARNING = "warning"              # 20-50% remaining
    CRITICAL = "critical"            # 5-20% remaining
    DEPLETED = "depleted"            # < 5% remaining


@dataclass
class CreditSnapshot:
    """Real-time credit usage snapshot."""
    total_credits: int = 10_000_000
    credits_used: int = 0
    credits_remaining: int = 10_000_000
    day_of_month: int = 1
    days_remaining: int = 30
    daily_target: int = 333_333
    daily_used: int = 0
    alert_level: CreditAlertLevel = CreditAlertLevel.NORMAL
    timestamp: float = field(default_factory=time.time)

    def get_usage_percentage(self) -> float:
        """Get current usage percentage."""
        if self.total_credits == 0:
            return 0.0
        return (self.credits_used / self.total_credits) * 100

    def get_daily_usage_percentage(self) -> float:
        """Get today's usage percentage of daily target."""
        if self.daily_target == 0:
            return 0.0
        return (self.daily_used / self.daily_target) * 100

    def is_daily_budget_exceeded(self) -> bool:
        """Check if daily budget has been exceeded."""
        return self.daily_used > self.daily_target

    def get_projected_monthly_usage(self) -> int:
        """Get projected monthly usage based on current daily rate."""
        if self.day_of_month == 0:
            return 0
        avg_daily_rate = self.credits_used / self.day_of_month
        return int(avg_daily_rate * 30)


@dataclass
class CreditForecast:
    """Credit forecast for future time horizon."""
    horizon_hours: int
    projected_usage: int
    projected_remaining: int
    confidence: float  # 0-1
    trend: str  # "increasing", "stable", "decreasing"
    recommendations: List[str] = field(default_factory=list)
    timestamp: float = field(default_factory=time.time)


@dataclass
class CategoryPerformance:
    """Performance tracking for budget category."""
    category: BudgetCategory
    credits_consumed: int = 0
    value_generated: float = 0.0  # ROI value (profit, high-WQS wallets found, etc.)
    roi_score: float = 0.0  # value / credits
    operations_count: int = 0
    last_updated: float = field(default_factory=time.time)

    def calculate_roi(self) -> float:
        """Calculate ROI for this category."""
        if self.credits_consumed == 0:
            return 0.0
        return self.value_generated / self.credits_consumed


@dataclass
class OptimizationAction:
    """Suggested optimization action."""
    priority: str  # "high", "medium", "low"
    action: str
    expected_savings: int  # credits
    description: str
    category: Optional[BudgetCategory] = None


@dataclass
class BudgetManagerConfig:
    """Configuration for predictive budget manager."""

    # Developer Plan constraints
    MONTHLY_CREDITS: int = 10_000_000
    DAILY_TARGET_CREDITS: int = 333_333

    # Forecast settings
    FORECAST_HORIZON_HOURS: int = 24
    HISTORY_LOOKBACK_DAYS: int = 7
    MIN_CONFIDENCE_THRESHOLD: float = 0.6

    # Allocation ratios (will be dynamically adjusted)
    DEFAULT_ALLOCATION: Dict[BudgetCategory, float] = field(default_factory=lambda: {
        BudgetCategory.DISCOVERY: 0.30,      # 30%
        BudgetCategory.ANALYSIS: 0.40,       # 40%
        BudgetCategory.VALIDATION: 0.15,     # 15%
        BudgetCategory.ENRICHMENT: 0.05,     # 5%
        BudgetCategory.MONITORING: 0.05,    # 5%
        BudgetCategory.RESERVE: 0.05,        # 5%
    })

    # Minimum allocation per category (prevent starvation)
    MIN_ALLOCATION_RATIO: float = 0.02  # 2% minimum

    # Rebalancing settings
    REBALANCE_INTERVAL_SECONDS: int = 3600  # 1 hour
    MIN_REBALANCE_THRESHOLD: float = 0.05   # 5% deviation

    # Alert thresholds
    WARNING_THRESHOLD: float = 0.50   # 50% used
    CRITICAL_THRESHOLD: float = 0.80  # 80% used
    DEPLETED_THRESHOLD: float = 0.95  # 95% used

    # Persistence settings
    STATE_FILE: str = "predictive_budget_state.json"


class PredictiveBudgetManager:
    """
    Predictive credit budget manager for proactive allocation.

    Features:
    - 7-day moving average forecast with trend extrapolation
    - Dynamic budget allocation based on predicted ROI
    - Real-time credit tracking with alert levels
    - Optimization suggestions for efficiency
    """

    def __init__(self, config: Optional[BudgetManagerConfig] = None):
        """Initialize the predictive budget manager."""
        self._config = config or BudgetManagerConfig()
        self._lock = threading.Lock()

        # Current credit state
        self._snapshot = CreditSnapshot()

        # Budget allocations (dynamic)
        self._allocations: Dict[BudgetCategory, float] = dict(
            self._config.DEFAULT_ALLOCATION
        )

        # Category performance tracking
        self._performance: Dict[BudgetCategory, CategoryPerformance] = {
            category: CategoryPerformance(category=category)
            for category in BudgetCategory
        }

        # Historical data for forecasting
        self._usage_history: List[Tuple[float, int, Dict[BudgetCategory, int]]] = []
        self._max_history_samples = 30  # Keep 30 days of history

        # Last rebalance time
        self._last_rebalance = time.time()

        # Load state if available
        self._load_state()

        logger.info("PredictiveBudgetManager initialized")

    def get_realtime_snapshot(self) -> CreditSnapshot:
        """Get current credit snapshot."""
        with self._lock:
            # Update snapshot
            self._update_snapshot()
            return self._snapshot

    def _update_snapshot(self) -> None:
        """Update credit snapshot with current state."""
        now = time.time()
        current_date = datetime.fromtimestamp(now)
        day_of_month = current_date.day

        # Calculate total credits used from all categories
        total_used = sum(p.credits_consumed for p in self._performance.values())

        # Estimate daily used (simplified - in production, track per-day)
        daily_used = int(total_used / max(1, day_of_month))

        # Update snapshot
        self._snapshot.credits_used = total_used
        self._snapshot.credits_remaining = self._config.MONTHLY_CREDITS - total_used
        self._snapshot.day_of_month = day_of_month
        self._snapshot.days_remaining = 30 - day_of_month
        self._snapshot.daily_used = daily_used
        self._snapshot.timestamp = now

        # Update alert level
        usage_pct = self._snapshot.get_usage_percentage()
        if usage_pct >= self._config.DEPLETED_THRESHOLD * 100:
            self._snapshot.alert_level = CreditAlertLevel.DEPLETED
        elif usage_pct >= self._config.CRITICAL_THRESHOLD * 100:
            self._snapshot.alert_level = CreditAlertLevel.CRITICAL
        elif usage_pct >= self._config.WARNING_THRESHOLD * 100:
            self._snapshot.alert_level = CreditAlertLevel.WARNING
        else:
            self._snapshot.alert_level = CreditAlertLevel.NORMAL

    def forecast_credit_needs(self, horizon_hours: int = 24) -> CreditForecast:
        """
        Forecast credit needs for the given horizon.

        Uses 7-day moving average with trend extrapolation.
        """
        with self._lock:
            if len(self._usage_history) < 3:
                # Not enough history, use simple projection
                return self._simple_forecast(horizon_hours)

            # Calculate moving average
            recent_samples = self._usage_history[-7:]
            daily_usages = [usage for (_, usage, _) in recent_samples]

            avg_daily = statistics.mean(daily_usages)
            std_daily = statistics.stdev(daily_usages) if len(daily_usages) > 1 else 0

            # Detect trend
            if len(daily_usages) >= 3:
                recent_avg = statistics.mean(daily_usages[-3:])
                older_avg = statistics.mean(daily_usages[:-3]) if len(daily_usages) > 3 else avg_daily
                if recent_avg > older_avg * 1.1:
                    trend = "increasing"
                    projected = int(avg_daily * 1.2)  # 20% increase
                elif recent_avg < older_avg * 0.9:
                    trend = "decreasing"
                    projected = int(avg_daily * 0.8)  # 20% decrease
                else:
                    trend = "stable"
                    projected = int(avg_daily)
            else:
                trend = "stable"
                projected = int(avg_daily)

            # Project for horizon
            horizon_days = horizon_hours / 24
            projected_usage = int(projected * horizon_days)

            # Calculate confidence based on data consistency
            if std_daily == 0:
                confidence = 0.9
            else:
                cv = std_daily / avg_daily  # Coefficient of variation
                confidence = max(0.3, min(0.95, 1.0 - cv))

            # Generate recommendations
            recommendations = self._generate_forecast_recommendations(
                projected_usage, horizon_hours, trend
            )

            return CreditForecast(
                horizon_hours=horizon_hours,
                projected_usage=projected_usage,
                projected_remaining=max(0, self._snapshot.credits_remaining - projected_usage),
                confidence=confidence,
                trend=trend,
                recommendations=recommendations,
            )

    def _simple_forecast(self, horizon_hours: int) -> CreditForecast:
        """Simple forecast when insufficient history available."""
        current_usage = self._snapshot.credits_used
        day_of_month = max(1, self._snapshot.day_of_month)

        avg_daily = current_usage / day_of_month
        horizon_days = horizon_hours / 24
        projected = int(avg_daily * horizon_days)

        return CreditForecast(
            horizon_hours=horizon_hours,
            projected_usage=projected,
            projected_remaining=max(0, self._snapshot.credits_remaining - projected),
            confidence=0.5,  # Low confidence without history
            trend="stable",
            recommendations=["Insufficient history for accurate forecasting. Continue tracking."],
        )

    def _generate_forecast_recommendations(
        self, projected_usage: int, horizon_hours: int, trend: str
    ) -> List[str]:
        """Generate recommendations based on forecast."""
        recommendations = []

        if trend == "increasing":
            recommendations.append("Credit usage is trending upward. Consider reducing non-critical operations.")
        elif trend == "decreasing":
            recommendations.append("Credit usage is trending downward. Good efficiency.")

        projected_remaining = self._snapshot.credits_remaining - projected_usage
        if projected_remaining < 0:
            recommendations.append(f"WARNING: Projected to exceed monthly budget by {abs(projected_remaining):,} credits.")
            recommendations.append("Immediate action required: Reduce discovery operations or upgrade plan.")
        elif projected_remaining < self._config.MONTHLY_CREDITS * 0.1:
            recommendations.append("WARNING: Projected to have < 10% buffer remaining.")

        return recommendations

    def allocate_budget_category(
        self, category: BudgetCategory, predicted_roi: float
    ) -> int:
        """
        Allocate budget for a category based on predicted ROI.

        Higher predicted ROI = larger allocation.
        """
        with self._lock:
            # Update ROI estimate for category
            if category in self._performance:
                self._performance[category].roi_score = predicted_roi

            # Check if rebalancing is needed
            if self._should_rebalance():
                self._rebalance_allocations()

            # Calculate allocation based on current ratio and remaining credits
            ratio = self._allocations.get(category, 0.1)
            allocated = int(self._snapshot.credits_remaining * ratio)

            logger.debug(
                f"Allocated {allocated:,} credits ({ratio*100:.1f}%) to {category.value} "
                f"(predicted ROI: {predicted_roi:.2f})"
            )
            return allocated

    def record_category_usage(
        self, category: BudgetCategory, credits: int, value: float = 0.0
    ) -> None:
        """
        Record credit usage and value for a category.

        Args:
            category: Budget category
            credits: Credits consumed
            value: Value generated (profit, wallets found, etc.)
        """
        with self._lock:
            perf = self._performance[category]
            perf.credits_consumed += credits
            perf.value_generated += value
            perf.operations_count += 1
            perf.last_updated = time.time()

            logger.debug(
                f"Recorded {credits:,} credits for {category.value} "
                f"(total: {perf.credits_consumed:,}, value: {value:.2f})"
            )

    def _should_rebalance(self) -> bool:
        """Check if allocations should be rebalanced."""
        now = time.time()
        if now - self._last_rebalance < self._config.REBALANCE_INTERVAL_SECONDS:
            return False

        # Check if any category is significantly underperforming
        for category, perf in self._performance.items():
            if perf.operations_count < 10:  # Not enough data
                continue

            if perf.roi_score < 0.5:  # Low ROI threshold
                return True

        return False

    def _rebalance_allocations(self) -> None:
        """
        Rebalance allocations based on ROI performance.

        Categories with higher ROI get more allocation.
        """
        self._last_rebalance = time.time()

        # Calculate ROI scores for each category
        roi_scores = {}
        for category, perf in self._performance.items():
            if perf.operations_count >= 5:  # Minimum samples
                roi_scores[category] = perf.calculate_roi()
            else:
                # Use default allocation for categories with insufficient data
                roi_scores[category] = 1.0  # Neutral

        if not roi_scores:
            return

        # Normalize scores
        total_roi = sum(roi_scores.values())
        if total_roi == 0:
            return

        # Calculate new allocations with minimum floor
        new_allocations = {}
        min_allocation = self._config.MIN_ALLOCATION_RATIO
        remaining_ratio = 1.0 - (min_allocation * len(BudgetCategory))

        for category in BudgetCategory:
            base = min_allocation
            if category in roi_scores and total_roi > 0:
                roi_portion = (roi_scores[category] / total_roi) * remaining_ratio
                new_allocations[category] = base + roi_portion
            else:
                new_allocations[category] = base

        # Normalize to ensure sum = 1.0
        total = sum(new_allocations.values())
        if total > 0:
            self._allocations = {
                cat: (ratio / total) for cat, ratio in new_allocations.items()
            }

        logger.info(f"Rebalanced allocations: {self._allocations}")

    def suggest_credit_optimization(self) -> List[OptimizationAction]:
        """
        Generate optimization suggestions for credit efficiency.

        Analyzes current usage and suggests actionable improvements.
        """
        suggestions = []

        with self._lock:
            snapshot = self._get_realtime_snapshot_internal()

            # Check alert level
            if snapshot.alert_level == CreditAlertLevel.CRITICAL:
                suggestions.append(OptimizationAction(
                    priority="high",
                    action="reduce_discovery",
                    expected_savings=int(snapshot.daily_used * 0.3),
                    description="Reduce wallet discovery operations by 30% to preserve credits",
                    category=BudgetCategory.DISCOVERY,
                ))
                suggestions.append(OptimizationAction(
                    priority="high",
                    action="pause_enrichment",
                    expected_savings=int(snapshot.daily_used * 0.05),
                    description="Pause non-critical metadata enrichment",
                    category=BudgetCategory.ENRICHMENT,
                ))

            # Check for low-ROI categories
            for category, perf in self._performance.items():
                if perf.operations_count >= 10 and perf.roi_score < 0.3:
                    savings = int(perf.credits_consumed * 0.5)
                    suggestions.append(OptimizationAction(
                        priority="medium",
                        action=f"reduce_{category.value}",
                        expected_savings=savings,
                        description=f"Reduce {category.value} operations by 50% (low ROI: {perf.roi_score:.2f})",
                        category=category,
                    ))

            # Check for high-ROI categories to expand
            for category, perf in self._performance.items():
                if perf.operations_count >= 10 and perf.roi_score > 2.0:
                    suggestions.append(OptimizationAction(
                        priority="low",
                        action=f"expand_{category.value}",
                        expected_savings=0,  # Investment, not savings
                        description=f"Increase allocation to {category.value} (high ROI: {perf.roi_score:.2f})",
                        category=category,
                    ))

            # Check daily budget
            if snapshot.is_daily_budget_exceeded():
                suggestions.append(OptimizationAction(
                    priority="high",
                    action="throttle_rate",
                    expected_savings=int(snapshot.daily_used - snapshot.daily_target),
                    description=f"Throttle operations to meet daily target ({snapshot.daily_target:,} credits)",
                ))

        return suggestions

    def _get_realtime_snapshot_internal(self) -> CreditSnapshot:
        """Get snapshot without lock (internal use)."""
        self._update_snapshot()
        return self._snapshot

    def get_allocations(self) -> Dict[BudgetCategory, float]:
        """Get current allocation ratios."""
        with self._lock:
            return dict(self._allocations)

    def get_category_performance(self) -> Dict[BudgetCategory, CategoryPerformance]:
        """Get performance metrics for all categories."""
        with self._lock:
            return dict(self._performance)

    def record_daily_usage(self, daily_credits: int, category_breakdown: Dict[BudgetCategory, int]) -> None:
        """
        Record daily usage for forecasting.

        Call this at the end of each day with total credits used and breakdown.

        Args:
            daily_credits: Total credits used today
            category_breakdown: Credits used per category
        """
        with self._lock:
            timestamp = time.time()
            self._usage_history.append((timestamp, daily_credits, category_breakdown))

            # Trim to max samples
            if len(self._usage_history) > self._max_history_samples:
                self._usage_history = self._usage_history[-self._max_history_samples:]

            logger.info(
                f"Recorded daily usage: {daily_credits:,} credits "
                f"(day {len(self._usage_history)})"
            )

            # Save state
            self._save_state()

    def _load_state(self) -> None:
        """Load state from disk."""
        state_file = Path(self._config.STATE_FILE)
        if not state_file.exists():
            return

        try:
            with open(state_file, 'r') as f:
                data = json.load(f)

            # Restore performance data
            for cat_name, perf_data in data.get('performance', {}).items():
                try:
                    category = BudgetCategory(cat_name)
                    self._performance[category] = CategoryPerformance(
                        category=category,
                        credits_consumed=perf_data.get('credits_consumed', 0),
                        value_generated=perf_data.get('value_generated', 0.0),
                        operations_count=perf_data.get('operations_count', 0),
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
                'performance': {
                    cat.value: {
                        'credits_consumed': perf.credits_consumed,
                        'value_generated': perf.value_generated,
                        'operations_count': perf.operations_count,
                    }
                    for cat, perf in self._performance.items()
                },
                'last_save': time.time(),
            }

            state_file = Path(self._config.STATE_FILE)
            with open(state_file, 'w') as f:
                json.dump(data, f, indent=2)

        except Exception as e:
            logger.warning(f"Failed to save state: {e}")

    def get_daily_summary(self) -> Dict[str, Any]:
        """Get daily summary of credit usage and performance."""
        with self._lock:
            self._update_snapshot()

            return {
                'snapshot': {
                    'credits_used': self._snapshot.credits_used,
                    'credits_remaining': self._snapshot.credits_remaining,
                    'usage_percentage': self._snapshot.get_usage_percentage(),
                    'daily_used': self._snapshot.daily_used,
                    'daily_target': self._snapshot.daily_target,
                    'daily_percentage': self._snapshot.get_daily_usage_percentage(),
                    'alert_level': self._snapshot.alert_level.value,
                    'day_of_month': self._snapshot.day_of_month,
                    'days_remaining': self._snapshot.days_remaining,
                },
                'allocations': {
                    cat.value: f"{ratio * 100:.1f}%"
                    for cat, ratio in self._allocations.items()
                },
                'category_performance': {
                    cat.value: {
                        'credits_used': perf.credits_consumed,
                        'value_generated': perf.value_generated,
                        'roi': perf.calculate_roi(),
                        'operations': perf.operations_count,
                    }
                    for cat, perf in self._performance.items()
                },
                'forecast': {
                    '24h_projected': self.forecast_credit_needs(24).projected_usage,
                    '7d_projected': self.forecast_credit_needs(168).projected_usage,
                },
            }
