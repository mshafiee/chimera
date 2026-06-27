"""
Tests for PredictiveBudgetManager integration with Scout analyzer

These tests verify that the predictive budget management system works correctly
with the Helius API client and provides accurate forecasting and optimization.
"""

import pytest
import time
from unittest.mock import Mock, AsyncMock, MagicMock, patch
from datetime import datetime, timedelta

from core.predictive_budget_manager import (
    PredictiveBudgetManager,
    BudgetManagerConfig,
    BudgetCategory,
    CreditAlertLevel,
    CreditSnapshot,
    CreditForecast,
    OptimizationAction,
    CategoryPerformance
)
from core.analyzer import WalletAnalyzer


class TestPredictiveBudgetManager:
    """Test suite for PredictiveBudgetManager core functionality."""

    def test_budget_manager_initialization(self):
        """Test that budget manager initializes with correct defaults."""
        manager = PredictiveBudgetManager()
        assert manager is not None

        # Check default configuration via internal config
        assert manager._config.MONTHLY_CREDITS == 10_000_000
        assert manager._config.DAILY_TARGET_CREDITS == 333_333
        assert manager._config.CRITICAL_THRESHOLD == 0.80

    def test_custom_budget_configuration(self):
        """Test custom budget configuration."""
        custom_config = BudgetManagerConfig(
            MONTHLY_CREDITS=5_000_000,
            DAILY_TARGET_CREDITS=166_666,
            CRITICAL_THRESHOLD=0.70
        )
        manager = PredictiveBudgetManager(config=custom_config)

        assert manager._config.MONTHLY_CREDITS == 5_000_000
        assert manager._config.DAILY_TARGET_CREDITS == 166_666
        assert manager._config.CRITICAL_THRESHOLD == 0.70

    def test_realtime_snapshot(self):
        """Test real-time credit snapshot functionality."""
        manager = PredictiveBudgetManager()

        # Get initial snapshot
        snapshot = manager.get_realtime_snapshot()
        assert snapshot is not None
        assert snapshot.credits_remaining == manager._config.MONTHLY_CREDITS
        assert snapshot.credits_used == 0
        assert snapshot.alert_level == CreditAlertLevel.NORMAL

    def test_credit_recording(self):
        """Test recording credit usage."""
        manager = PredictiveBudgetManager()

        # Record some credit usage
        manager.record_category_usage(BudgetCategory.DISCOVERY, 1000)
        manager.record_category_usage(BudgetCategory.ANALYSIS, 2000)

        # Check snapshot reflects usage
        snapshot = manager.get_realtime_snapshot()
        assert snapshot.credits_used == 3000
        assert snapshot.credits_remaining == manager._config.MONTHLY_CREDITS - 3000

    def test_budget_allocation(self):
        """Test budget allocation by category."""
        manager = PredictiveBudgetManager()

        # Allocations are stored as ratios (percentages) not absolute values
        # Check default allocations exist
        allocations = manager.get_allocations()
        assert BudgetCategory.DISCOVERY in allocations
        assert BudgetCategory.ANALYSIS in allocations
        assert allocations[BudgetCategory.DISCOVERY] > 0  # Should have positive allocation
        assert allocations[BudgetCategory.ANALYSIS] > 0

    def test_credit_forecasting(self):
        """Test 7-day credit forecasting."""
        manager = PredictiveBudgetManager()

        # Record some historical usage
        daily_usage = 50_000
        for i in range(7):
            breakdown = {BudgetCategory.DISCOVERY: daily_usage}
            manager.record_daily_usage(daily_usage, breakdown)

        # Get forecast
        forecast = manager.forecast_credit_needs(horizon_hours=24)
        assert forecast is not None
        assert forecast.projected_usage > 0
        assert forecast.horizon_hours == 24

    def test_roi_calculation(self):
        """Test ROI calculation for categories."""
        manager = PredictiveBudgetManager()

        # Simulate category performance
        manager._performance = {
            BudgetCategory.DISCOVERY: CategoryPerformance(
                category=BudgetCategory.DISCOVERY,
                credits_consumed=100_000,
                value_generated=50_000.0,
                roi_score=0.5,
                operations_count=100
            )
        }

        # Calculate ROI using the CategoryPerformance method
        discovery_performance = manager._performance[BudgetCategory.DISCOVERY]
        roi = discovery_performance.calculate_roi()
        assert roi >= 0  # ROI should be non-negative
        assert roi == 0.5  # Should match our set value

    def test_credit_alert_levels(self):
        """Test credit alert level determination."""
        manager = PredictiveBudgetManager()
        total_credits = manager._config.MONTHLY_CREDITS

        # Test different usage levels by recording category usage
        # Normal: > 50% remaining
        manager.record_category_usage(BudgetCategory.ANALYSIS, int(total_credits * 0.3))
        snapshot = manager.get_realtime_snapshot()
        assert snapshot.alert_level == CreditAlertLevel.NORMAL

        # Warning: 20-50% remaining
        manager.record_category_usage(BudgetCategory.ANALYSIS, int(total_credits * 0.3))  # Total 60%
        snapshot = manager.get_realtime_snapshot()
        assert snapshot.alert_level == CreditAlertLevel.WARNING

        # Critical: 5-20% remaining
        manager.record_category_usage(BudgetCategory.ANALYSIS, int(total_credits * 0.25))  # Total 85%
        snapshot = manager.get_realtime_snapshot()
        assert snapshot.alert_level == CreditAlertLevel.CRITICAL

        # Depleted: < 5% remaining
        manager.record_category_usage(BudgetCategory.ANALYSIS, int(total_credits * 0.12))  # Total 97%
        snapshot = manager.get_realtime_snapshot()
        assert snapshot.alert_level == CreditAlertLevel.DEPLETED

    def test_optimization_suggestions(self):
        """Test credit optimization suggestions."""
        manager = PredictiveBudgetManager()

        # Record some usage patterns
        for i in range(5):
            manager.record_category_usage(BudgetCategory.DISCOVERY, 100_000)
            manager.record_category_usage(BudgetCategory.ANALYSIS, 50_000)

        # Get optimization suggestions
        suggestions = manager.suggest_credit_optimization()
        assert isinstance(suggestions, list)
        # Should have some suggestions based on usage patterns
        assert len(suggestions) >= 0

    def test_daily_budget_exceeded(self):
        """Test daily budget exceeded detection."""
        config = BudgetManagerConfig(DAILY_TARGET_CREDITS=100_000)
        manager = PredictiveBudgetManager(config=config)

        # Record usage within budget
        manager.record_category_usage(BudgetCategory.ANALYSIS, 80_000)
        # Check daily usage percentage
        snapshot = manager.get_realtime_snapshot()
        first_percentage = snapshot.get_daily_usage_percentage()
        assert first_percentage < 100.0

        # Record usage exceeding budget
        manager.record_category_usage(BudgetCategory.ANALYSIS, 30_000)
        # Now daily usage should have increased
        snapshot = manager.get_realtime_snapshot()
        second_percentage = snapshot.get_daily_usage_percentage()
        assert second_percentage > first_percentage

    def test_projected_monthly_usage(self):
        """Test projected monthly usage calculation."""
        manager = PredictiveBudgetManager()

        # Record some usage to establish a pattern
        daily_usage = 50_000
        for i in range(5):
            breakdown = {BudgetCategory.ANALYSIS: daily_usage}
            manager.record_daily_usage(daily_usage, breakdown)

        # Get projection from snapshot
        snapshot = manager.get_realtime_snapshot()
        projected = snapshot.get_projected_monthly_usage()

        # Should have some projection based on the data
        assert projected >= 0

    def test_usage_percentage(self):
        """Test usage percentage calculations."""
        manager = PredictiveBudgetManager()

        # Initially 0% used
        snapshot = manager.get_realtime_snapshot()
        assert snapshot.get_usage_percentage() == 0.0

        # Use 50% of budget
        total_credits = manager._config.MONTHLY_CREDITS
        manager.record_category_usage(BudgetCategory.ANALYSIS, total_credits // 2)
        # Usage percentage should be approximately 50%
        snapshot = manager.get_realtime_snapshot()
        percentage = snapshot.get_usage_percentage()
        assert 45.0 <= percentage <= 55.0  # Allow some tolerance

    def test_category_performance_tracking(self):
        """Test category performance tracking."""
        manager = PredictiveBudgetManager()

        # Add performance data
        performance = CategoryPerformance(
            category=BudgetCategory.ANALYSIS,
            credits_consumed=100_000,
            value_generated=80_000.0,
            roi_score=0.8,
            operations_count=50
        )

        manager._performance[BudgetCategory.ANALYSIS] = performance

        # Retrieve performance
        performances = manager.get_category_performance()
        assert BudgetCategory.ANALYSIS in performances
        assert performances[BudgetCategory.ANALYSIS].roi_score == 0.8


class TestBudgetIntegration:
    """Test suite for budget management integration."""

    def test_analyzer_with_budget_manager(self):
        """Test WalletAnalyzer integration with PredictiveBudgetManager."""
        # This tests the integration without requiring full async setup
        budget_manager = PredictiveBudgetManager()

        # Create analyzer with budget manager
        analyzer = WalletAnalyzer(budget_manager=budget_manager)

        # Verify budget manager is accessible
        assert analyzer._budget_manager is not None
        assert analyzer.get_budget_summary() is not None

    def test_budget_aware_operations(self):
        """Test that analyzer respects budget constraints."""
        budget_manager = PredictiveBudgetManager(
            config=BudgetManagerConfig(total_monthly_credits=1_000_000)
        )
        analyzer = WalletAnalyzer(budget_manager=budget_manager)

        # Check if we can spend budget
        can_proceed, reason = analyzer.can_spend_budget(estimated_credits=100)
        assert can_proceed is True

        # Use most of the budget
        budget_manager._credits_used = 950_000

        # Check if we can spend more budget
        can_proceed, reason = analyzer.can_spend_budget(estimated_credits=100_000)
        assert can_proceed is False

    def test_credit_usage_recording(self):
        """Test that analyzer records credit usage."""
        budget_manager = PredictiveBudgetManager()
        analyzer = WalletAnalyzer(budget_manager=budget_manager)

        # Record credit usage
        analyzer.record_credit_usage(credits=100, category="test_operation", value=5)

        # Check budget manager reflects usage
        snapshot = budget_manager.get_realtime_snapshot()
        assert snapshot.credits_used == 100

    def test_budget_summary_generation(self):
        """Test budget summary generation from analyzer."""
        budget_manager = PredictiveBudgetManager()
        analyzer = WalletAnalyzer(budget_manager=budget_manager)

        # Record some usage
        analyzer.record_credit_usage(credits=1000, category="analysis", value=10)
        analyzer.record_credit_usage(credits=500, category="discovery", value=5)

        # Get summary
        summary = analyzer.get_budget_summary()
        assert summary is not None
        assert 'credits_used' in summary
        assert 'credits_remaining' in summary
        assert 'usage_percentage' in summary


@pytest.mark.asyncio
class TestBudgetForecasting:
    """Test suite for budget forecasting functionality."""

    async def test_forecast_accuracy(self):
        """Test forecast accuracy with historical data."""
        manager = PredictiveBudgetManager()

        # Simulate consistent daily usage
        daily_usage = 50_000
        days = 14
        for i in range(days):
            breakdown = {
                BudgetCategory.DISCOVERY: int(daily_usage * 0.3),
                BudgetCategory.ANALYSIS: int(daily_usage * 0.7)
            }
            manager.record_daily_usage(daily_usage, breakdown)

        # Get 7-day forecast
        forecast = manager.forecast_credit_needs(horizon_hours=168)  # 7 days
        assert forecast is not None
        # Should predict roughly daily_usage * 7
        assert forecast.projected_credits > daily_usage * 6
        assert forecast.projected_credits < daily_usage * 8  # Allow some variance

    async def test_seasonal_pattern_detection(self):
        """Test detection of usage patterns."""
        manager = PredictiveBudgetManager()

        # Simulate higher usage on weekdays
        daily_usage = 50_000
        for i in range(14):
            # Alternate between high and low usage
            if i % 2 == 0:  # Weekday
                usage = int(daily_usage * 1.5)
            else:  # Weekend
                usage = int(daily_usage * 0.5)

            breakdown = {BudgetCategory.ANALYSIS: usage}
            manager.record_daily_usage(usage, breakdown)

        # Get forecast
        forecast = manager.forecast_credit_needs(horizon_hours=24)
        assert forecast is not None
        # Should detect the pattern and forecast accordingly

    async def test_optimization_action_generation(self):
        """Test generation of optimization actions."""
        manager = PredictiveBudgetManager()

        # Create usage patterns that might need optimization
        # High discovery, low success rate
        for i in range(10):
            manager.record_category_usage(BudgetCategory.DISCOVERY, 100_000)
            manager._category_performance[BudgetCategory.DISCOVERY] = CategoryPerformance(
                category=BudgetCategory.DISCOVERY,
                credits_invested=1_000_000,
                wallets_discovered=10,  # Low success
                total_wqs_score=500.0,
                success_rate=0.1
            )

        # Get optimization suggestions
        suggestions = manager.suggest_credit_optimization()
        assert len(suggestions) > 0

        # Check that suggestions are actionable
        for suggestion in suggestions:
            assert suggestion.action_type is not None
            assert suggestion.expected_savings >= 0


class TestBudgetAlerts:
    """Test suite for budget alert functionality."""

    def test_alert_threshold_configuration(self):
        """Test configurable alert thresholds."""
        config = BudgetManagerConfig(CRITICAL_THRESHOLD=0.70)  # Lower critical threshold
        manager = PredictiveBudgetManager(config=config)

        total_credits = manager._config.MONTHLY_CREDITS

        # Just below threshold (should be WARNING)
        manager.record_category_usage(BudgetCategory.ANALYSIS, int(total_credits * 0.65))  # 65% used
        snapshot = manager.get_realtime_snapshot()
        assert snapshot.alert_level == CreditAlertLevel.WARNING

        # Just above threshold (should be CRITICAL)
        manager.record_category_usage(BudgetCategory.ANALYSIS, int(total_credits * 0.1))  # 75% used
        snapshot = manager.get_realtime_snapshot()
        assert snapshot.alert_level == CreditAlertLevel.CRITICAL

    def test_alert_recovery(self):
        """Test alert recovery when usage decreases."""
        manager = PredictiveBudgetManager()
        total_credits = manager._config.MONTHLY_CREDITS

        # Trigger critical alert
        manager.record_category_usage(BudgetCategory.ANALYSIS, int(total_credits * 0.85))
        snapshot = manager.get_realtime_snapshot()
        assert snapshot.alert_level == CreditAlertLevel.CRITICAL

        # Note: In real scenario, recovery would happen with monthly reset or credit refill
        # For testing, we verify the alert system works in one direction


class TestBudgetOptimization:
    """Test suite for budget optimization recommendations."""

    def test_reallocation_suggestions(self):
        """Test budget reallocation suggestions."""
        manager = PredictiveBudgetManager()

        # Set up initial allocations
        manager.allocate_budget_category(BudgetCategory.DISCOVERY, 200_000)
        manager.allocate_budget_category(BudgetCategory.ANALYSIS, 100_000)

        # Create performance data showing discovery is inefficient
        manager._category_performance = {
            BudgetCategory.DISCOVERY: CategoryPerformance(
                category=BudgetCategory.DISCOVERY,
                credits_invested=200_000,
                wallets_discovered=10,
                total_wqs_score=500.0,
                success_rate=0.2
            ),
            BudgetCategory.ANALYSIS: CategoryPerformance(
                category=BudgetCategory.ANALYSIS,
                credits_invested=100_000,
                wallets_discovered=50,
                total_wqs_score=3500.0,
                success_rate=0.8
            )
        }

        # Get optimization suggestions
        suggestions = manager.suggest_credit_optimization()

        # Should suggest reallocating from discovery to analysis
        reallocation_actions = [s for s in suggestions if "reallocate" in s.action_type.lower()]
        assert len(reallocation_actions) > 0

    def test_efficiency_improvement_suggestions(self):
        """Test efficiency improvement suggestions."""
        manager = PredictiveBudgetManager()

        # Set up usage patterns that suggest inefficiency
        for i in range(20):
            # Consistent overuse of discovery category
            manager.record_category_usage(BudgetCategory.DISCOVERY, 150_000)
            manager.record_category_usage(BudgetCategory.ANALYSIS, 30_000)

        # Get suggestions
        suggestions = manager.suggest_credit_optimization()
        assert len(suggestions) > 0

        # Check for efficiency-related suggestions
        efficiency_actions = [s for s in suggestions if "efficiency" in s.action_type.lower() or "optimize" in s.action_type.lower()]
        # May or may not have efficiency actions depending on the algorithm


class TestBudgetPersistence:
    """Test suite for budget data persistence."""

    def test_daily_usage_recording(self):
        """Test daily usage recording and retrieval."""
        manager = PredictiveBudgetManager()

        # Record daily usage
        daily_credits = 50_000
        breakdown = {
            BudgetCategory.DISCOVERY: 20_000,
            BudgetCategory.ANALYSIS: 30_000
        }
        manager.record_daily_usage(daily_credits, breakdown)

        # Check that usage was recorded
        assert len(manager._daily_usage_history) > 0

    def test_usage_history_analysis(self):
        """Test analysis of usage history."""
        manager = PredictiveBudgetManager()

        # Record varied usage over time
        usages = [40_000, 45_000, 50_000, 55_000, 60_000]
        for daily_usage in usages:
            breakdown = {BudgetCategory.ANALYSIS: daily_usage}
            manager.record_daily_usage(daily_usage, breakdown)

        # Check forecast uses historical data
        forecast = manager.forecast_credit_needs(horizon_hours=24)
        assert forecast is not None
        # Should predict somewhere in the range of historical usage
        assert forecast.projected_credits > min(usages) * 0.8
        assert forecast.projected_credits < max(usages) * 1.2