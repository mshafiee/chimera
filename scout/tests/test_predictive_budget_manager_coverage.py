"""
Coverage tests for core/predictive_budget_manager.py.

Covers snapshot math, monthly rollover, forecasting branches, rebalancing,
optimization suggestions, state persistence, and the daily summary.
"""

import json
import time

import pytest

from core.predictive_budget_manager import (
    BudgetCategory,
    BudgetManagerConfig,
    CategoryPerformance,
    CreditAlertLevel,
    CreditSnapshot,
    PredictiveBudgetManager,
)


@pytest.fixture
def manager(tmp_path):
    config = BudgetManagerConfig(STATE_FILE=str(tmp_path / "budget_state.json"))
    m = PredictiveBudgetManager(config)
    m._last_rebalance = time.time()  # avoid spurious rebalances
    return m


def test_credit_snapshot_zero_branches():
    snap = CreditSnapshot(total_credits=0, daily_target=0, day_of_month=0)
    assert snap.get_usage_percentage() == 0.0
    assert snap.get_daily_usage_percentage() == 0.0
    assert snap.get_projected_monthly_usage() == 0
    assert snap.is_daily_budget_exceeded() is False
    snap2 = CreditSnapshot(daily_used=400000, daily_target=333333)
    assert snap2.is_daily_budget_exceeded() is True


def test_credit_snapshot_projection():
    snap = CreditSnapshot(credits_used=300000, day_of_month=10)
    assert snap.get_projected_monthly_usage() == 900000


def test_category_performance_roi_zero_credits():
    perf = CategoryPerformance(category=BudgetCategory.ANALYSIS)
    assert perf.calculate_roi() == 0.0
    perf.credits_consumed = 100
    perf.value_generated = 50.0
    assert perf.calculate_roi() == 0.5


def test_monthly_rollover_resets_state(manager):
    manager._tracked_month = "2000-01"
    manager.record_category_usage(BudgetCategory.ANALYSIS, 1000)
    manager._daily_usage["2000-01-01"] = 1000
    snap = manager.get_realtime_snapshot()
    assert snap.credits_used == 0
    assert manager._daily_usage == {}
    assert manager._tracked_month != "2000-01"


def test_daily_ledger_pruned(manager):
    for i in range(40):
        manager._daily_usage[f"2000-01-{i+1:02d}"] = i
    manager.get_realtime_snapshot()
    assert len(manager._daily_usage) <= 35


def test_snapshot_alert_levels(manager):
    manager.record_category_usage(BudgetCategory.ANALYSIS, 9_600_000)
    assert manager.get_realtime_snapshot().alert_level == CreditAlertLevel.DEPLETED
    manager2 = PredictiveBudgetManager(BudgetManagerConfig(STATE_FILE="/tmp/pbm_2.json"))
    manager2.record_category_usage(BudgetCategory.ANALYSIS, 8_500_000)
    assert manager2.get_realtime_snapshot().alert_level == CreditAlertLevel.CRITICAL
    manager3 = PredictiveBudgetManager(BudgetManagerConfig(STATE_FILE="/tmp/pbm_3.json"))
    manager3.record_category_usage(BudgetCategory.ANALYSIS, 6_000_000)
    assert manager3.get_realtime_snapshot().alert_level == CreditAlertLevel.WARNING
    manager4 = PredictiveBudgetManager(BudgetManagerConfig(STATE_FILE="/tmp/pbm_4.json"))
    assert manager4.get_realtime_snapshot().alert_level == CreditAlertLevel.NORMAL


def test_simple_forecast(manager):
    forecast = manager.forecast_credit_needs(horizon_hours=24)
    assert forecast.trend == "stable"
    assert forecast.confidence == 0.5
    assert "Insufficient history" in forecast.recommendations[0]


def test_forecast_increasing_trend(manager):
    for usage in (1000, 1100, 1200, 2000):
        manager.record_daily_usage(usage, {})
    forecast = manager.forecast_credit_needs(horizon_hours=24)
    assert forecast.trend == "increasing"
    assert forecast.projected_usage > 1000
    assert any("upward" in r for r in forecast.recommendations)


def test_forecast_decreasing_trend(manager):
    for usage in (2000, 1800, 1200, 1000):
        manager.record_daily_usage(usage, {})
    forecast = manager.forecast_credit_needs(horizon_hours=48)
    assert forecast.trend == "decreasing"
    assert any("downward" in r for r in forecast.recommendations)


def test_forecast_stable_trend(manager):
    for usage in (1000, 1100, 1050, 1000):
        manager.record_daily_usage(usage, {})
    forecast = manager.forecast_credit_needs(horizon_hours=24)
    assert forecast.trend == "stable"


def test_forecast_exceed_budget_recommendations(manager):
    manager.record_category_usage(BudgetCategory.ANALYSIS, 9_900_000)
    manager.get_realtime_snapshot()  # refresh snapshot.credits_remaining
    for _ in range(3):
        manager.record_daily_usage(300000, {})
    forecast = manager.forecast_credit_needs(horizon_hours=720)
    assert any("exceed monthly budget" in r for r in forecast.recommendations)


def test_forecast_low_buffer_recommendation(manager):
    manager.record_category_usage(BudgetCategory.ANALYSIS, 9_300_000)
    manager.get_realtime_snapshot()  # refresh snapshot.credits_remaining
    for _ in range(3):
        manager.record_daily_usage(100000, {})
    forecast = manager.forecast_credit_needs(horizon_hours=24)
    assert any("10% buffer" in r for r in forecast.recommendations)


def test_allocate_budget_category(manager):
    allocated = manager.allocate_budget_category(BudgetCategory.ANALYSIS, 2.0)
    assert allocated > 0
    assert manager._performance[BudgetCategory.ANALYSIS].roi_score == 2.0


def test_allocate_triggers_rebalance(manager):
    manager._last_rebalance = 0.0
    for _ in range(10):
        manager.record_category_usage(BudgetCategory.DISCOVERY, 100, value=0.1)
    allocated = manager.allocate_budget_category(BudgetCategory.ANALYSIS, 0.1)
    assert allocated >= 0
    # Rebalanced: allocations changed away from the defaults
    allocs = manager.get_allocations()
    assert allocs[BudgetCategory.DISCOVERY] != BudgetManagerConfig().DEFAULT_ALLOCATION[BudgetCategory.DISCOVERY]


def test_record_category_usage_and_get_performance(manager):
    manager.record_category_usage(BudgetCategory.VALIDATION, 500, value=25.0)
    perf = manager.get_category_performance()[BudgetCategory.VALIDATION]
    assert perf.credits_consumed == 500
    assert perf.operations_count == 1
    assert perf.value_generated == 25.0


def test_should_rebalance_interval_not_reached(manager):
    assert manager._should_rebalance() is False


def test_should_rebalance_no_low_roi_returns_false(manager):
    manager._last_rebalance = 0.0
    perf = manager._performance[BudgetCategory.ENRICHMENT]
    perf.operations_count = 10
    perf.roi_score = 0.9  # above the 0.5 threshold
    assert manager._should_rebalance() is False


def test_rebalance_total_roi_zero_returns(manager):
    # All categories have >= 5 operations but zero value -> ROI 0 -> return
    for perf in manager._performance.values():
        perf.operations_count = 5
        perf.credits_consumed = 100
        perf.value_generated = 0.0
    manager._last_rebalance = 0.0
    manager._rebalance_allocations()
    # Allocations unchanged
    assert manager._allocations == dict(manager._config.DEFAULT_ALLOCATION)


def test_should_rebalance_low_roi_category(manager):
    manager._last_rebalance = 0.0
    perf = manager._performance[BudgetCategory.ENRICHMENT]
    perf.operations_count = 10
    perf.roi_score = 0.1
    assert manager._should_rebalance() is True


def test_rebalance_allocations_min_floor(manager):
    manager._last_rebalance = 0.0
    manager._rebalance_allocations()
    allocs = manager.get_allocations()
    assert abs(sum(allocs.values()) - 1.0) < 1e-9
    assert all(v >= BudgetManagerConfig().MIN_ALLOCATION_RATIO for v in allocs.values())


def test_suggest_credit_optimization_critical(manager):
    manager.record_category_usage(BudgetCategory.ANALYSIS, 8_500_000)
    suggestions = manager.suggest_credit_optimization()
    actions = {s.action for s in suggestions}
    assert "reduce_discovery" in actions
    assert "pause_enrichment" in actions


def test_suggest_credit_optimization_roi_and_budget(manager):
    perf = manager._performance[BudgetCategory.MONITORING]
    perf.operations_count = 12
    perf.roi_score = 0.1
    perf.credits_consumed = 10000
    perf2 = manager._performance[BudgetCategory.DISCOVERY]
    perf2.operations_count = 12
    perf2.roi_score = 3.0
    from datetime import datetime
    manager._daily_usage[datetime.now().strftime("%Y-%m-%d")] = 400000
    suggestions = manager.suggest_credit_optimization()
    actions = {s.action for s in suggestions}
    assert any(a.startswith("reduce_") for a in actions)
    assert any(a.startswith("expand_") for a in actions)
    assert "throttle_rate" in actions


def test_record_daily_usage_trims_history(manager):
    manager._max_history_samples = 3
    for i in range(6):
        manager.record_daily_usage(1000 + i, {})
    assert len(manager._usage_history) == 3


def test_load_state_no_file(manager):
    # Fresh manager with missing state file -> no-op
    assert manager._performance[BudgetCategory.ANALYSIS].credits_consumed == 0


def test_load_state_restores_and_skips_bad_category(tmp_path):
    state = {
        "performance": {
            "analysis": {"credits_consumed": 500, "value_generated": 10.0,
                         "operations_count": 3},
            "bogus_category": {"credits_consumed": 1, "value_generated": 1.0,
                               "operations_count": 1},
        },
        "daily_usage": {"2099-01-01": 100},
        "tracked_month": "2099-01",
    }
    (tmp_path / "budget_state.json").write_text(json.dumps(state))
    manager = PredictiveBudgetManager(BudgetManagerConfig(
        STATE_FILE=str(tmp_path / "budget_state.json"),
    ))
    perf = manager._performance[BudgetCategory.ANALYSIS]
    assert perf.credits_consumed == 500
    assert manager._daily_usage == {"2099-01-01": 100}
    assert manager._tracked_month == "2099-01"


def test_load_state_corrupt(tmp_path):
    (tmp_path / "budget_state.json").write_text("{corrupt!!")
    manager = PredictiveBudgetManager(BudgetManagerConfig(
        STATE_FILE=str(tmp_path / "budget_state.json"),
    ))
    assert manager._performance[BudgetCategory.ANALYSIS].credits_consumed == 0


def test_save_state_failure_swallowed(tmp_path):
    manager = PredictiveBudgetManager(BudgetManagerConfig(STATE_FILE=str(tmp_path)))
    manager._save_state()  # directory path -> OSError, must not raise


def test_get_daily_summary(manager):
    manager.record_category_usage(BudgetCategory.ANALYSIS, 1000, value=50.0)
    summary = manager.get_daily_summary()
    assert summary["snapshot"]["credits_used"] == 1000
    assert summary["snapshot"]["usage_percentage"] > 0
    assert summary["category_performance"]["analysis"]["credits_used"] == 1000
    assert "24h_projected" in summary["forecast"]
    assert "7d_projected" in summary["forecast"]
    assert "%" in summary["allocations"]["analysis"]


def test_get_allocations_copy(manager):
    allocs = manager.get_allocations()
    allocs[BudgetCategory.ANALYSIS] = 0.99
    assert manager.get_allocations()[BudgetCategory.ANALYSIS] != 0.99
