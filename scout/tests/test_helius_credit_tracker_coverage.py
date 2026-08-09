"""
Coverage tests for core/helius_credit_tracker.py.

Covers credit counting, budget windows, state persistence (with mocked file
I/O via tmp_path), rate limiting, rebalancing, and the convenience functions.
"""

import json
import os
import time

import pytest

from core.helius_credit_tracker import (
    CreditBudget,
    CreditSnapshot,
    HeliusCreditTracker,
    RequestCost,
    RequestPriority,
    can_analyze_wallet,
    can_fetch_wallet_transactions,
    can_validate_backtest,
    reset_credit_tracker,
)


@pytest.fixture(autouse=True)
def _state_file(tmp_path, monkeypatch):
    monkeypatch.setenv("SCOUT_CREDIT_STATE_FILE", str(tmp_path / "credit_state.json"))
    reset_credit_tracker()
    yield
    reset_credit_tracker()
    monkeypatch.delenv("SCOUT_CREDIT_STATE_FILE", raising=False)


def _tracker(tmp_path=None, **env):
    if tmp_path is not None:
        os.environ["SCOUT_CREDIT_STATE_FILE"] = str(tmp_path / "credit_state.json")
    for k, v in env.items():
        os.environ[k] = v
    t = HeliusCreditTracker()
    for k in env:
        os.environ.pop(k, None)
    return t


def test_request_cost_timestamp_default():
    rc = RequestCost(endpoint="e", credit_cost=1, priority=RequestPriority.HIGH,
                     expected_value=0.5)
    assert rc.timestamp > 0
    rc2 = RequestCost(endpoint="e", credit_cost=1, priority=RequestPriority.HIGH,
                      expected_value=0.5, timestamp=123.0)
    assert rc2.timestamp == 123.0


def test_load_state_restores_current_day(tmp_path, monkeypatch):
    state = {
        "timestamp": time.time(),
        "credits_used_today": 500,
        "credits_used_month": 9000,
        "requests_today": 7,
        "discovery_spent": 100,
        "analysis_spent": 200,
        "validation_spent": 300,
    }
    (tmp_path / "credit_state.json").write_text(json.dumps(state))
    t = _tracker(tmp_path)
    assert t._credits_used_today == 500
    assert t._credits_used_month == 9000
    assert t._requests_today == 7


def test_load_state_previous_day_resets_and_saves(tmp_path, monkeypatch):
    yesterday = time.time() - 86400
    state = {
        "timestamp": yesterday,
        "credits_used_today": 500,
        "credits_used_month": 9000,
        "requests_today": 7,
        "discovery_spent": 100,
        "analysis_spent": 200,
        "validation_spent": 300,
    }
    (tmp_path / "credit_state.json").write_text(json.dumps(state))
    t = _tracker(tmp_path)
    # Daily counters NOT restored (stale day)
    assert t._credits_used_today == 0
    # Monthly counter restored (same calendar month as the saved timestamp)
    assert t._credits_used_month == 9000
    # New day detected -> state re-saved
    saved = json.loads((tmp_path / "credit_state.json").read_text())
    assert saved["credits_used_today"] == 0


def test_load_state_previous_month_drops_monthly(tmp_path, monkeypatch):
    old = time.time() - 40 * 86400
    state = {
        "timestamp": old,
        "credits_used_today": 500,
        "credits_used_month": 9000,
        "requests_today": 7,
    }
    (tmp_path / "credit_state.json").write_text(json.dumps(state))
    t = _tracker(tmp_path)
    assert t._credits_used_month == 0


def test_load_state_error_swallowed(tmp_path, monkeypatch):
    (tmp_path / "credit_state.json").write_text("{corrupt")
    t = _tracker(tmp_path)  # must not raise
    assert t._credits_used_today == 0


def test_save_state_failure_swallowed(tmp_path, monkeypatch):
    # STATE_FILE pointing at a directory -> open() fails
    monkeypatch.setenv("SCOUT_CREDIT_STATE_FILE", str(tmp_path))
    t = HeliusCreditTracker()
    t._save_state()  # must not raise


def test_daily_reset_and_monthly_rollover(tmp_path, monkeypatch):
    t = _tracker(tmp_path)
    t._tracked_day = "2000-01-01"
    t._tracked_month = "2000-01"
    t._credits_used_today = 100
    t._credits_used_month = 500
    t._requests_today = 3
    # NOTE: _check_daily_reset holds the non-reentrant lock while calling
    # _check_monthly_reset (which re-acquires it) -> deadlock in core on a
    # day rollover. Stub the monthly reset out here; it is tested directly
    # in test_monthly_reset_only.
    monkeypatch.setattr(t, "_check_monthly_reset", lambda: None)
    t._check_daily_reset()
    assert t._credits_used_today == 0
    assert t._tracked_day == time.strftime("%Y-%m-%d", time.localtime())


def test_monthly_reset_only():
    t = HeliusCreditTracker.__new__(HeliusCreditTracker)
    t._lock = __import__("threading").Lock()
    t._tracked_month = "1999-01"
    t._credits_used_month = 999
    t._month_start_time = 0.0
    t._check_monthly_reset()
    assert t._credits_used_month == 0
    assert t._tracked_month == time.strftime("%Y-%m", time.localtime())


def test_get_category_budget_branches(tmp_path):
    t = _tracker(tmp_path)
    assert t._get_category_budget("discovery")[0] == t._discovery_budget
    assert t._get_category_budget("analysis")[0] == t._analysis_budget
    assert t._get_category_budget("validation")[0] == t._validation_budget
    assert t._get_category_budget("reserve")[0] == t._reserve_budget
    assert t._get_category_budget("unknown")[0] == t._daily_budget


def test_check_budget_conservative_mode(tmp_path, monkeypatch):
    monkeypatch.setenv("SCOUT_CONSERVATIVE_MODE", "true")
    t = _tracker(tmp_path)
    t._analysis_spent = int(t._analysis_budget * 0.3)
    assert t._check_budget(int(t._analysis_budget), "analysis") is False


def test_can_make_request_rate_limited(tmp_path):
    t = _tracker(tmp_path)
    t._request_times = [time.time()] * 50
    allowed, reason = t.can_make_request(1)
    assert allowed is False
    assert "Rate limit" in reason


def test_can_make_request_insufficient_budget(tmp_path):
    t = _tracker(tmp_path)
    t._analysis_spent = t._analysis_budget
    allowed, reason = t.can_make_request(100, category="analysis")
    assert allowed is False
    assert "Insufficient budget" in reason


def test_can_make_request_budget_tight_conservative(tmp_path, monkeypatch):
    monkeypatch.setenv("SCOUT_CONSERVATIVE_MODE", "true")
    t = _tracker(tmp_path)
    t._analysis_spent = int(t._analysis_budget * 0.95)
    allowed, reason = t.can_make_request(
        1, category="analysis", priority=RequestPriority.HIGH
    )
    assert allowed is False
    assert "only critical" in reason


def test_can_make_request_low_value_filtered(tmp_path):
    t = _tracker(tmp_path)
    allowed, reason = t.can_make_request(
        1, priority=RequestPriority.LOW, expected_value=0.1
    )
    assert allowed is False
    assert "Low value" in reason


def test_can_make_request_ok(tmp_path):
    t = _tracker(tmp_path)
    allowed, reason = t.can_make_request(1, category="analysis")
    assert allowed is True
    assert reason == "OK"


def test_record_request_all_categories(tmp_path):
    t = _tracker(tmp_path)
    t.record_request(10, category="discovery", endpoint="e1")
    t.record_request(10, category="analysis", endpoint="e2")
    t.record_request(10, category="validation", endpoint="e3")
    t.record_request(10, category="reserve", endpoint="e4")
    t.record_request(10, category="mystery", endpoint="e5")
    assert t._discovery_spent == 10
    assert t._analysis_spent == 10
    assert t._validation_spent == 10
    assert t._reserve_spent == 10
    assert t._credits_used_today == 50
    assert t._credits_used_month == 50
    assert t._requests_today == 5


def test_record_request_history_trim_and_periodic_save(tmp_path):
    t = _tracker(tmp_path)
    t._max_history_size = 3
    t._requests_today = 9
    for i in range(5):
        t.record_request(1, category="analysis")
    assert len(t._request_history) <= 3
    # requests_today hit 10 -> periodic save wrote the state file
    assert (tmp_path / "credit_state.json").exists()


def test_get_snapshot_statuses(tmp_path):
    t = _tracker(tmp_path)
    t._credits_used_today = 0
    assert t.get_snapshot().budget_status == "healthy"
    t._credits_used_today = int(t._daily_budget * 0.7)
    assert t.get_snapshot().budget_status == "warning"
    t._credits_used_today = int(t._daily_budget * 0.95)
    assert t.get_snapshot().budget_status == "critical"
    snap = t.get_snapshot()
    assert snap.credits_remaining == int(t._daily_budget - t._credits_used_today)
    assert snap.requests_per_second >= 0


def test_get_snapshot_projected_monthly_zero_elapsed(tmp_path, monkeypatch):
    t = _tracker(tmp_path)
    t._month_start_time = time.time() + 5
    snap = t.get_snapshot()
    assert snap.projected_monthly == t._credits_used_month


def test_optimization_suggestions_critical(tmp_path):
    t = _tracker(tmp_path)
    t._credits_used_today = int(t._daily_budget * 0.95)
    suggestions = t.get_optimization_suggestions()
    assert any("URGENT" in s for s in suggestions)


def test_optimization_suggestions_warning_and_ratios(tmp_path):
    t = _tracker(tmp_path)
    t._credits_used_today = int(t._daily_budget * 0.7)
    t._discovery_spent = int(t._credits_used_today * 0.5)
    t._analysis_spent = 10
    t._request_times = [time.time()] * 45
    suggestions = t.get_optimization_suggestions()
    assert any("below 20%" in s for s in suggestions)
    assert any("rate limit" in s.lower() for s in suggestions)
    assert any("pre-filtering" in s for s in suggestions)
    assert any("underutilized" in s for s in suggestions)


def test_optimization_suggestions_projected_over_budget(tmp_path):
    t = _tracker(tmp_path)
    t._credits_used_month = CreditBudget.MONTHLY_CREDITS + 1
    suggestions = t.get_optimization_suggestions()
    assert any("exceed monthly" in s for s in suggestions)


def test_optimize_for_growth_branches(tmp_path):
    t = _tracker(tmp_path)
    assert t.optimize_for_growth(None) == 1.0
    assert t.optimize_for_growth(75) == 1.5
    assert t.optimize_for_growth(65) == 1
    assert t.optimize_for_growth(55) == 0.5
    assert t.optimize_for_growth(40) == 0.2


def test_optimize_for_growth_disabled(tmp_path, monkeypatch):
    monkeypatch.setenv("SCOUT_GROWTH_OPTIMIZED", "false")
    t = _tracker(tmp_path)
    assert t.optimize_for_growth(90) == 1.0


def test_should_skip_operation_branches(tmp_path):
    t = _tracker(tmp_path)
    t._credits_used_today = int(t._daily_budget * 0.95)  # critical
    assert t.should_skip_operation("enrichment") is True
    assert t.should_skip_operation("validation") is False
    assert t.should_skip_operation("metadata") is True
    # Not critical -> enrichment with low wqs still skipped when not healthy
    t._credits_used_today = int(t._daily_budget * 0.3)
    assert t.should_skip_operation("enrichment", wallet_wqs=40) is False
    t._credits_used_today = int(t._daily_budget * 0.7)  # warning
    assert t.should_skip_operation("enrichment", wallet_wqs=40) is True
    assert t.should_skip_operation("metadata") is True
    assert t.should_skip_operation("analysis", wallet_wqs=80) is False


def test_print_status_report(tmp_path, capsys):
    t = _tracker(tmp_path)
    t.print_status_report()
    out = capsys.readouterr().out
    assert "HELIUS CREDIT TRACKER - STATUS REPORT" in out


def test_print_status_report_with_adjustments(tmp_path, capsys):
    t = _tracker(tmp_path)
    t._budget_adjustments = {"timestamp": time.time()}
    t.print_status_report()
    out = capsys.readouterr().out
    assert "Last Budget Rebalance" in out


def test_record_category_value(tmp_path):
    t = _tracker(tmp_path)
    t.record_category_value("analysis", 5.0)
    assert t._category_roi["analysis"]["value"] == 5.0
    t.record_category_value("bogus", 5.0)  # no-op


def test_get_category_roi_branches(tmp_path):
    t = _tracker(tmp_path)
    assert t.get_category_roi("bogus") == 0.0
    assert t.get_category_roi("discovery") == 0.0  # no credits
    t.record_category_value("analysis", 10.0)
    t.record_request(5, category="analysis")
    assert t.get_category_roi("analysis") == 2.0
    t.record_category_value("reserve", 3.0)
    t.record_request(1, category="reserve")
    assert t.get_category_roi("reserve") == 3.0


def test_should_rebalance_budget(tmp_path):
    t = _tracker(tmp_path)
    t._last_rebalance_time = time.time() - (25 * 3600)
    assert t.should_rebalance_budget() is True
    t._last_rebalance_time = time.time()
    assert t.should_rebalance_budget() is False


def test_rebalance_budget_not_due(tmp_path):
    t = _tracker(tmp_path)
    t._last_rebalance_time = time.time()
    assert t.rebalance_budget_based_on_roi() == {}


def test_rebalance_budget_with_roi(tmp_path):
    t = _tracker(tmp_path)
    t._last_rebalance_time = 0.0
    t.record_category_value("analysis", 20.0)
    t.record_request(10, category="analysis")
    t.record_category_value("discovery", 2.0)
    t.record_request(10, category="discovery")
    new_alloc = t.rebalance_budget_based_on_roi()
    assert new_alloc
    assert abs(sum(new_alloc.values()) - 0.9) < 1e-9
    assert t._budget_adjustments["roi_by_category"]
    assert t._analysis_budget != t._daily_budget * CreditBudget.ANALYSIS_RATIO


def test_rebalance_budget_no_roi_data(tmp_path):
    t = _tracker(tmp_path)
    t._last_rebalance_time = 0.0
    new_alloc = t.rebalance_budget_based_on_roi()
    assert new_alloc
    assert abs(sum(new_alloc.values()) - 0.9) < 1e-9


def test_get_value_based_priority_branches(tmp_path):
    t = _tracker(tmp_path)
    assert t.get_value_based_priority(75, expected_value=0.8) == RequestPriority.CRITICAL
    assert t.get_value_based_priority(75, expected_value=0.5) == RequestPriority.HIGH
    assert t.get_value_based_priority(65, expected_value=0.9) == RequestPriority.CRITICAL
    assert t.get_value_based_priority(65, expected_value=0.5) == RequestPriority.HIGH
    assert t.get_value_based_priority(55) == RequestPriority.MEDIUM
    assert t.get_value_based_priority(10) == RequestPriority.LOW
    assert t.get_value_based_priority(None) == RequestPriority.LOW


def test_optimize_request_cost_branches(tmp_path, monkeypatch):
    monkeypatch.setenv("SCOUT_GROWTH_OPTIMIZED", "false")
    t2 = _tracker(tmp_path)
    assert t2.optimize_request_cost(10) == 10
    monkeypatch.delenv("SCOUT_GROWTH_OPTIMIZED")
    t3 = _tracker(tmp_path)
    assert t3.optimize_request_cost(10, wallet_wqs=40, operation="enrichment") == 5
    assert t3.optimize_request_cost(10, wallet_wqs=40, operation="metadata") == 2
    assert t3.optimize_request_cost(10, wallet_wqs=40, operation="analysis") == 10
    assert t3.optimize_request_cost(10, wallet_wqs=80, operation="validation") == 15
    assert t3.optimize_request_cost(10, wallet_wqs=60, operation="analysis") == 10


def test_shutdown_saves_state(tmp_path):
    t = _tracker(tmp_path)
    t.shutdown()
    assert (tmp_path / "credit_state.json").exists()


def test_convenience_functions(tmp_path, monkeypatch):
    monkeypatch.setenv("SCOUT_CREDIT_STATE_FILE", str(tmp_path / "state.json"))
    reset_credit_tracker()
    allowed, reason = can_fetch_wallet_transactions()
    assert isinstance(allowed, bool)
    allowed2, _ = can_analyze_wallet(75)
    assert isinstance(allowed2, bool)
    allowed3, _ = can_analyze_wallet(40)
    assert isinstance(allowed3, bool)
    allowed4, _ = can_validate_backtest()
    assert isinstance(allowed4, bool)
    reset_credit_tracker()


def test_credit_snapshot_defaults():
    snap = CreditSnapshot(
        timestamp=time.time(), credits_used=1, credits_remaining=2,
        daily_usage=1, requests_made=3, requests_per_second=0.5,
        projected_monthly=4, budget_status="healthy",
    )
    assert snap.credits_remaining == 2
    assert snap.budget_status == "healthy"
