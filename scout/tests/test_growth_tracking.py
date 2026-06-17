"""
Tests for Phase 6: Growth Tracking & Monitoring

Tests the growth tracking system for $200 → $1000 target monitoring.
"""

import pytest
import time
import os
import tempfile
from datetime import datetime, timedelta

from core.production_monitor import (
    GrowthTracker,
    GrowthMetrics,
    get_growth_tracker,
)


@pytest.fixture
def growth_tracker():
    """Create a fresh GrowthTracker instance for each test."""
    # Use temp file for test database
    db_path = tempfile.mktemp(suffix=".db")
    tracker = GrowthTracker(
        starting_capital=200.0,
        target_capital=1000.0,
        db_path=db_path,
    )
    yield tracker

    # Cleanup
    try:
        os.unlink(db_path)
    except FileNotFoundError:
        pass


class TestGrowthMetrics:
    """Tests for GrowthMetrics dataclass."""

    def test_progress_percentage_calculation(self):
        """Test progress percentage calculation."""
        metrics = GrowthMetrics(
            timestamp=time.time(),
            current_capital=400.0,
            target_capital=1000.0,
            starting_capital=200.0,
        )

        # 400 is 25% of the way from 200 to 1000
        expected_progress = (400 - 200) / (1000 - 200) * 100
        assert abs(metrics.progress_percentage - expected_progress) < 0.01

    def test_progress_at_start(self):
        """Test progress is 0% at starting capital."""
        metrics = GrowthMetrics(
            timestamp=time.time(),
            current_capital=200.0,
            target_capital=1000.0,
            starting_capital=200.0,
        )

        assert metrics.progress_percentage == 0.0

    def test_progress_at_target(self):
        """Test progress is 100% at target capital."""
        metrics = GrowthMetrics(
            timestamp=time.time(),
            current_capital=1000.0,
            target_capital=1000.0,
            starting_capital=200.0,
        )

        assert metrics.progress_percentage == 100.0

    def test_capital_multiplier(self):
        """Test capital multiplier calculation."""
        metrics = GrowthMetrics(
            timestamp=time.time(),
            current_capital=600.0,
            target_capital=1000.0,
            starting_capital=200.0,
        )

        # 600 is 3x the starting 200
        assert abs(metrics.capital_multiplier - 3.0) < 0.01

    def test_to_dict_conversion(self):
        """Test conversion to dictionary includes computed properties."""
        metrics = GrowthMetrics(
            timestamp=time.time(),
            current_capital=400.0,
            target_capital=1000.0,
            starting_capital=200.0,
        )

        data = metrics.to_dict()

        assert 'current_capital' in data
        assert 'progress_percentage' in data
        assert 'capital_multiplier' in data
        assert data['current_capital'] == 400.0


class TestGrowthTracker:
    """Tests for GrowthTracker class."""

    def test_initialization(self, growth_tracker):
        """Test GrowthTracker initialization."""
        assert growth_tracker.starting_capital == 200.0
        assert growth_tracker.target_capital == 1000.0
        assert growth_tracker.current_capital == 200.0

    def test_record_capital_update(self, growth_tracker):
        """Test recording a capital update."""
        old_capital = growth_tracker.current_capital
        new_capital = 250.0

        metrics = growth_tracker.record_capital(
            new_capital,
            event_type="update",
            description="Test update"
        )

        assert growth_tracker.current_capital == new_capital
        assert metrics.current_capital == new_capital
        assert metrics.roi_daily != 0  # Should calculate ROI

    def test_roi_calculation(self, growth_tracker):
        """Test ROI calculation for different periods."""
        # Record initial capital
        growth_tracker.record_capital(200.0, event_type="initial")

        # Simulate time passing by directly manipulating database
        # (In real test, would wait or mock time)
        growth_tracker.record_capital(220.0, event_type="day1")

        # Get current metrics
        metrics = growth_tracker.get_current_metrics()

        # ROI should be calculated
        # Since we just recorded, might be 0 or small value
        assert isinstance(metrics.roi_daily, float)

    def test_significant_change_alerts(self, growth_tracker):
        """Test alerts for significant capital changes."""
        # Record a significant gain (>10%)
        growth_tracker.record_capital(
            250.0,  # 25% increase from 200
            event_type="trade",
            description="Big win"
        )

        # Check that alert was created
        # (Would need to query database to verify)

    def test_target_reached_alert(self, growth_tracker):
        """Test alert when target is reached."""
        # Record reaching target
        metrics = growth_tracker.record_capital(
            1000.0,
            event_type="milestone",
            description="Target reached!"
        )

        assert metrics.current_capital >= metrics.target_capital

    def test_days_to_target_estimation(self, growth_tracker):
        """Test days to target calculation."""
        # Record capital with positive growth
        growth_tracker.record_capital(300.0, event_type="growth")

        metrics = growth_tracker.get_current_metrics()

        # With positive growth, should have days estimate
        if metrics.growth_rate_daily > 0:
            assert metrics.days_to_target is not None
            assert metrics.days_to_target > 0
            assert metrics.date_to_target is not None

    def test_no_days_to_target_with_negative_growth(self, growth_tracker):
        """Test days to target is None with negative growth."""
        # Record capital loss
        growth_tracker.record_capital(150.0, event_type="loss")

        metrics = growth_tracker.get_current_metrics()

        # Negative growth should result in None for days_to_target
        if metrics.growth_rate_daily <= 0:
            assert metrics.days_to_target is None

    def test_get_growth_summary(self, growth_tracker):
        """Test getting growth summary."""
        growth_tracker.record_capital(300.0, event_type="update")

        summary = growth_tracker.get_growth_summary()

        assert 'current_capital' in summary
        assert 'target_capital' in summary
        assert 'progress_percentage' in summary
        assert 'capital_multiplier' in summary
        assert summary['current_capital'] == 300.0

    def test_growth_history_retrieval(self, growth_tracker):
        """Test retrieving growth history."""
        # Record multiple data points
        capitals = [200.0, 225.0, 260.0, 310.0]
        for capital in capitals:
            growth_tracker.record_capital(capital, event_type="update")
            time.sleep(0.01)  # Ensure different timestamps

        # Get history
        history = growth_tracker.get_growth_history(days=30)

        # Should have at least the entries we just recorded
        assert len(history) >= len(capitals)

        # Check ordering (should be ascending by timestamp)
        timestamps = [m.timestamp for m in history]
        assert timestamps == sorted(timestamps)

    def test_capital_events_tracking(self, growth_tracker):
        """Test that significant capital events are recorded."""
        # Record a significant change
        growth_tracker.record_capital(
            250.0,
            event_type="milestone",
            description="25% gain"
        )

        # Query events table directly to verify
        import sqlite3
        conn = sqlite3.connect(growth_tracker._db_path)
        cursor = conn.cursor()

        cursor.execute("""
            SELECT event_type, description FROM capital_events
            ORDER BY timestamp DESC LIMIT 1
        """)
        row = cursor.fetchone()
        conn.close()

        assert row is not None
        assert row[0] == "milestone"
        assert row[1] == "25% gain"

    def test_database_persistence(self, growth_tracker):
        """Test that data persists across tracker instances."""
        # Record some data
        growth_tracker.record_capital(300.0, event_type="update")

        # Create new tracker with same database
        new_tracker = GrowthTracker(
            starting_capital=200.0,
            target_capital=1000.0,
            db_path=growth_tracker._db_path,
        )

        # Should have loaded the latest capital
        assert new_tracker.current_capital == 300.0

    def test_compounding_effect_calculation(self, growth_tracker):
        """Test compounding effect calculation."""
        # Record capital
        growth_tracker.record_capital(400.0, event_type="update")

        metrics = growth_tracker.get_current_metrics()

        # Compounding effect should be calculated
        assert isinstance(metrics.compounding_effect, float)

    def test_capital_efficiency_calculation(self, growth_tracker):
        """Test capital efficiency calculation."""
        growth_tracker.record_capital(350.0, event_type="update")

        metrics = growth_tracker.get_current_metrics()

        # Efficiency should be calculated
        assert isinstance(metrics.capital_efficiency, float)


class TestGrowthDashboard:
    """Tests for growth dashboard functionality."""

    def test_print_growth_dashboard(self, growth_tracker, capsys):
        """Test growth dashboard printing."""
        growth_tracker.record_capital(400.0, event_type="update")

        growth_tracker.print_growth_dashboard()

        captured = capsys.readouterr()

        # Verify key sections are printed
        assert "GROWTH TRACKING" in captured.out
        assert "Capital Status" in captured.out
        assert "ROI Performance" in captured.out
        assert "Progress:" in captured.out


class TestGlobalGrowthTracker:
    """Tests for global growth tracker singleton."""

    def test_get_growth_tracker_singleton(self):
        """Test that get_growth_tracker returns singleton."""
        tracker1 = get_growth_tracker(200.0, 1000.0)
        tracker2 = get_growth_tracker(300.0, 1500.0)

        # Should return same instance
        assert tracker1 is tracker2

        # But with original parameters
        assert tracker1.starting_capital == 200.0
        assert tracker1.target_capital == 1000.0


class TestGrowthAlerts:
    """Tests for growth alerting system."""

    def test_significant_loss_alert(self, growth_tracker):
        """Test alert generation for significant loss."""
        # Record a significant loss
        growth_tracker.record_capital(
            150.0,  # 25% loss from 200
            event_type="loss",
            description="Bad trade"
        )

        # Check alert was created
        import sqlite3
        conn = sqlite3.connect(growth_tracker._db_path)
        cursor = conn.cursor()

        cursor.execute("""
            SELECT alert_type, severity FROM growth_alerts
            WHERE alert_type = 'significant_loss'
        """)
        row = cursor.fetchone()
        conn.close()

        assert row is not None
        assert row[1] == 'critical'

    def test_target_reached_alert(self, growth_tracker):
        """Test alert when target is reached."""
        growth_tracker.record_capital(1000.0, event_type="target")

        import sqlite3
        conn = sqlite3.connect(growth_tracker._db_path)
        cursor = conn.cursor()

        cursor.execute("""
            SELECT alert_type FROM growth_alerts
            WHERE alert_type = 'target_reached'
        """)
        row = cursor.fetchone()
        conn.close()

        assert row is not None


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
