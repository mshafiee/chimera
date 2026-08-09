"""Coverage completion tests for core/realtime_profit_tracker.py."""

import time

import pytest

from core.realtime_profit_tracker import (
    GrowthStage,
    OptimizationTrigger,
    ProfitSnapshot,
    RealtimeProfitTracker,
    TrackerConfig,
)


def make_tracker(capital=200.0):
    return RealtimeProfitTracker(TrackerConfig(STARTING_CAPITAL=capital))


class TestUpdateProfit:
    def test_duplicate_trade_id_ignored(self):
        tracker = make_tracker()
        tracker.update_profit("t1", 5.0)
        tracker.update_profit("t1", 100.0)
        assert tracker.get_current_profit() == pytest.approx(5.0)

    def test_category_roi_tracked(self):
        tracker = make_tracker()
        tracker.update_profit("t1", 5.0, category="discovery")
        tracker.update_profit("t2", -2.0, category="discovery")
        data = tracker.get_category_roi("discovery")
        assert data["profit"] == 3.0
        assert data["trades"] == 2
        assert data["avg_pnl"] == 1.5

    def test_wqs_band_tracked(self):
        tracker = make_tracker()
        tracker.update_profit("t1", 5.0, wqs=85.0)
        data = tracker.get_wqs_band_roi("very_high")
        assert data["profit"] == 5.0
        assert data["trades"] == 1

    def test_seen_trade_ids_bounded(self):
        tracker = make_tracker()
        tracker._max_seen_trade_ids = 3
        for i in range(6):
            tracker.update_profit(f"t{i}", 1.0)
        assert len(tracker._seen_trade_ids) == 3
        assert "t0" not in tracker._seen_trade_ids

    def test_peak_capital_updated(self):
        tracker = make_tracker()
        tracker.update_profit("t1", 50.0)
        assert tracker._peak_capital == 250.0

    def test_timestamp_provided(self):
        tracker = make_tracker()
        tracker.update_profit("t1", 5.0, timestamp=1000.0)
        assert tracker._profit_history[0].timestamp == 1000.0


class TestWqsBand:
    def test_bands(self):
        tracker = make_tracker()
        assert tracker._get_wqs_band(80) == "very_high"
        assert tracker._get_wqs_band(70) == "high"
        assert tracker._get_wqs_band(50) == "medium"
        assert tracker._get_wqs_band(30) == "emerging"
        assert tracker._get_wqs_band(10) == "low"


class TestGrowthStage:
    def test_stages(self):
        tracker = make_tracker()
        tracker._current_capital = 250.0
        assert tracker._get_growth_stage() == GrowthStage.EARLY
        tracker._current_capital = 400.0
        assert tracker._get_growth_stage() == GrowthStage.MID
        tracker._current_capital = 700.0
        assert tracker._get_growth_stage() == GrowthStage.GROWTH
        tracker._current_capital = 900.0
        assert tracker._get_growth_stage() == GrowthStage.FINAL


class TestGetters:
    def test_get_current_profit(self):
        tracker = make_tracker()
        tracker.update_profit("t1", 25.0)
        assert tracker.get_current_profit() == pytest.approx(25.0)

    def test_get_current_capital(self):
        tracker = make_tracker()
        tracker.update_profit("t1", 25.0)
        assert tracker.get_current_capital() == pytest.approx(225.0)


class TestProfitVelocity:
    def test_insufficient_samples(self):
        tracker = make_tracker()
        velocity = tracker.get_profit_velocity()
        assert velocity.hourly_rate == 0.0
        assert velocity.trend == "stable"

    def test_velocity_computed(self):
        tracker = make_tracker()
        now = time.time()
        for i in range(6):
            tracker.update_profit(f"t{i}", 1.0, timestamp=now - (5 - i) * 3600)
        velocity = tracker.get_profit_velocity()
        assert velocity.hourly_rate == pytest.approx(1.0, abs=0.001)
        assert velocity.daily_rate == pytest.approx(24.0, abs=0.05)

    def test_window_filters_old_snapshots(self):
        tracker = make_tracker()
        now = time.time()
        for i in range(5):
            tracker.update_profit(f"old{i}", 1.0, timestamp=now - 48 * 3600 - i)
        for i in range(5):
            tracker.update_profit(f"new{i}", 1.0, timestamp=now - (4 - i) * 3600)
        velocity = tracker.get_profit_velocity()
        assert velocity.hourly_rate > 0

    def test_zero_time_diff_guarded(self):
        tracker = make_tracker()
        now = time.time()
        for i in range(5):
            tracker.update_profit(f"t{i}", 1.0, timestamp=now)
        velocity = tracker.get_profit_velocity()
        assert velocity.hourly_rate == pytest.approx(4.0)

    def test_snapshots_outside_window(self):
        tracker = make_tracker()
        now = time.time()
        for i in range(5):
            tracker.update_profit(f"t{i}", 1.0, timestamp=now - 48 * 3600 - i)
        velocity = tracker.get_profit_velocity()
        assert velocity.hourly_rate == 0.0
        assert velocity.trend == "stable"

    def test_trend_increasing(self):
        tracker = make_tracker()
        now = time.time()
        for i in range(5):
            tracker.update_profit(f"t{i}", 1.0, timestamp=now - (4 - i) * 3600)
        tracker.get_profit_velocity()
        for i in range(5):
            tracker.update_profit(f"u{i}", 3.0, timestamp=now + (1 + i) * 3600)
        velocity = tracker.get_profit_velocity()
        assert velocity.trend == "increasing"

    def test_trend_decreasing(self):
        tracker = make_tracker()
        now = time.time()
        for i in range(5):
            tracker.update_profit(f"t{i}", 3.0, timestamp=now - (4 - i) * 3600)
        tracker.get_profit_velocity()
        for i in range(5):
            tracker.update_profit(f"u{i}", 0.0, timestamp=now + (1 + i) * 3600)
        velocity = tracker.get_profit_velocity()
        assert velocity.trend == "decreasing"

    def test_trend_stable(self):
        tracker = make_tracker()
        now = time.time()
        for i in range(5):
            tracker.update_profit(f"t{i}", 1.0, timestamp=now - (4 - i) * 3600)
        tracker.get_profit_velocity()
        for i in range(5):
            tracker.update_profit(f"u{i}", 1.0, timestamp=now + (1 + i) * 3600)
        velocity = tracker.get_profit_velocity()
        assert velocity.trend == "stable"


class TestETA:
    def test_target_reached(self):
        tracker = make_tracker()
        tracker.update_profit("t1", 1000.0)
        eta = tracker.get_eta_to_1000()
        assert eta.remaining == 0
        assert eta.confidence == 1.0

    def test_no_velocity_inf(self):
        tracker = make_tracker()
        eta = tracker.get_eta_to_1000()
        assert eta.days_remaining == float("inf")
        assert eta.confidence == 0.0

    def test_normal_eta(self):
        tracker = make_tracker()
        now = time.time()
        for i in range(5):
            tracker.update_profit(f"t{i}", 1.0, timestamp=now - (4 - i) * 3600)
        eta = tracker.get_eta_to_1000()
        assert eta.days_remaining > 0
        assert eta.hours_remaining > 0


class TestEtaConfidence:
    def test_insufficient_samples(self):
        tracker = make_tracker()
        assert tracker._calculate_eta_confidence() == 0.3

    def test_mean_zero(self):
        tracker = make_tracker()
        now = time.time()
        for i in range(10):
            tracker.update_profit(f"t{i}", 0.0, timestamp=now + i)
        assert tracker._calculate_eta_confidence() == 0.5

    def test_zero_variance_high_confidence(self):
        tracker = make_tracker()
        now = time.time()
        for i in range(12):
            tracker._profit_history.append(ProfitSnapshot(
                timestamp=now + i, capital=201.0, profit=1.0,
                profit_pct=0.5, growth_stage=GrowthStage.EARLY,
            ))
        assert tracker._calculate_eta_confidence() == 1.0

    def test_normal_confidence(self):
        tracker = make_tracker()
        now = time.time()
        for i in range(12):
            tracker.update_profit(f"t{i}", float(i % 3), timestamp=now + i)
        confidence = tracker._calculate_eta_confidence()
        assert 0.0 <= confidence <= 1.0


class TestOptimizationTriggers:
    def test_target_reached(self):
        tracker = make_tracker()
        tracker.update_profit("t1", 1000.0)
        actions = tracker.trigger_optimization_if_needed()
        assert len(actions) == 1
        assert actions[0].trigger == OptimizationTrigger.TARGET_REACHED

    def test_velocity_low(self):
        tracker = make_tracker()
        actions = tracker.trigger_optimization_if_needed()
        triggers = {a.trigger for a in actions}
        assert OptimizationTrigger.VELOCITY_LOW in triggers

    def test_velocity_high(self):
        tracker = make_tracker()
        now = time.time()
        for i in range(5):
            tracker.update_profit(f"t{i}", 5.0, timestamp=now - (4 - i) * 3600)
        actions = tracker.trigger_optimization_if_needed()
        triggers = {a.trigger for a in actions}
        assert OptimizationTrigger.VELOCITY_HIGH in triggers

    def test_win_rate_low(self):
        tracker = make_tracker()
        for i in range(10):
            tracker.update_profit(f"t{i}", -1.0)
        actions = tracker.trigger_optimization_if_needed()
        triggers = {a.trigger for a in actions}
        assert OptimizationTrigger.WIN_RATE_LOW in triggers

    def test_win_rate_high(self):
        tracker = make_tracker()
        now = time.time()
        for i in range(10):
            tracker.update_profit(f"t{i}", 2.0, timestamp=now - (9 - i) * 3600)
        actions = tracker.trigger_optimization_if_needed()
        triggers = {a.trigger for a in actions}
        assert OptimizationTrigger.WIN_RATE_HIGH in triggers

    def test_drawdown_exceeded(self):
        tracker = make_tracker()
        tracker.update_profit("t1", 100.0)
        tracker.update_profit("t2", -100.0)
        actions = tracker.trigger_optimization_if_needed()
        triggers = {a.trigger for a in actions}
        assert OptimizationTrigger.DRAWDOWN_EXCEEDED in triggers


class TestWinRateAndDrawdown:
    def test_win_rate_empty(self):
        tracker = make_tracker()
        assert tracker._calculate_win_rate() == 0.0

    def test_win_rate_computed(self):
        tracker = make_tracker()
        tracker.update_profit("t1", 1.0)
        tracker.update_profit("t2", -1.0)
        assert tracker._calculate_win_rate() == 0.5

    def test_drawdown_peak_zero(self):
        tracker = make_tracker()
        tracker._peak_capital = 0.0
        assert tracker._calculate_drawdown() == 0.0

    def test_drawdown_computed(self):
        tracker = make_tracker()
        tracker.update_profit("t1", 100.0)
        tracker.update_profit("t2", -80.0)
        assert tracker._calculate_drawdown() == pytest.approx(0.2666667)


class TestRoiGetters:
    def test_category_missing(self):
        tracker = make_tracker()
        data = tracker.get_category_roi("nope")
        assert data["profit"] == 0.0

    def test_wqs_band_missing(self):
        tracker = make_tracker()
        data = tracker.get_wqs_band_roi("nope")
        assert data["profit"] == 0.0

    def test_wqs_band_win_rate(self):
        tracker = make_tracker()
        tracker.update_profit("t1", 1.0, wqs=85.0)
        tracker.update_profit("t2", -1.0, wqs=82.0)
        data = tracker.get_wqs_band_roi("very_high")
        assert data["win_rate"] == 0.5

    def test_wqs_band_unknown_wqs_excluded(self):
        tracker = make_tracker()
        tracker.update_profit("t1", 1.0, wqs=None)
        data = tracker.get_wqs_band_roi("very_high")
        assert data["win_rate"] == 0.0


class TestSummary:
    def test_get_tracker_summary(self):
        tracker = make_tracker()
        tracker.update_profit("t1", 10.0, wqs=85.0, category="discovery")
        summary = tracker.get_tracker_summary()
        assert summary["capital"]["profit"] == pytest.approx(10.0)
        assert summary["capital"]["growth_stage"] == "early"
        assert summary["eta"]["target"] == 1000.0
        assert summary["performance"]["total_trades"] == 1
        assert "very_high" in summary["performance"]["wqs_band_roi"]

    def test_save_and_load_state_disabled(self):
        tracker = make_tracker()
        assert tracker.save_state() is None
        assert tracker._load_state() is None


class TestReset:
    def test_reset_to_capital(self):
        tracker = make_tracker()
        tracker.update_profit("t1", 50.0, wqs=85.0, category="discovery")
        tracker.reset_to_capital(500.0)
        assert tracker.get_current_capital() == 500.0
        assert tracker.get_current_profit() == 0.0
        assert len(tracker._profit_history) == 0
        assert tracker._last_velocity is None
        assert tracker.get_category_roi("discovery")["profit"] == 0.0
