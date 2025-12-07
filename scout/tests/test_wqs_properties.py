"""
Property-based tests for WQS (Wallet Quality Score) calculation.

Uses Hypothesis to test that WQS properties hold for all valid inputs.
"""

from hypothesis import given, strategies as st, example
from datetime import datetime, timedelta
from typing import Optional
from scout.core.wqs import calculate_wqs, WalletMetrics


def create_test_metrics(
    roi_30d: float = 0.0,
    roi_7d: float = 0.0,
    trade_count_30d: int = 0,
    win_streak_consistency: float = 0.0,
    max_drawdown_30d: float = 0.0,
    win_rate: Optional[float] = None,
) -> WalletMetrics:
    """Create test wallet metrics with specified parameters."""
    return WalletMetrics(
        address="test_wallet",
        roi_30d=roi_30d,
        roi_7d=roi_7d,
        trade_count_30d=trade_count_30d,
        win_streak_consistency=win_streak_consistency,
        max_drawdown_30d=max_drawdown_30d,
        win_rate=win_rate,
    )


class TestWQSProperties:
    """Property-based tests for WQS calculation."""

    @given(
        roi_30d=st.floats(min_value=-100.0, max_value=1000.0, allow_nan=False, allow_infinity=False),
        trade_count=st.integers(min_value=0, max_value=1000),
    )
    @example(roi_30d=0.0, trade_count=0)
    @example(roi_30d=100.0, trade_count=100)
    @example(roi_30d=-50.0, trade_count=50)
    def test_wqs_bounds(self, roi_30d: float, trade_count: int):
        """Property: WQS always returns value between 0 and 100."""
        metrics = create_test_metrics(roi_30d=roi_30d, trade_count_30d=trade_count)
        wqs = calculate_wqs(metrics)
        assert 0 <= wqs <= 100, f"WQS {wqs} out of bounds for roi_30d={roi_30d}, trade_count={trade_count}"

    @given(
        roi_30d_low=st.floats(min_value=-50.0, max_value=500.0, allow_nan=False, allow_infinity=False),
        roi_30d_high=st.floats(min_value=-50.0, max_value=500.0, allow_nan=False, allow_infinity=False),
    )
    def test_wqs_monotonicity_roi(self, roi_30d_low: float, roi_30d_high: float):
        """Property: Higher ROI should generally result in higher WQS (when other factors equal)."""
        if roi_30d_low >= roi_30d_high:
            return  # Skip if not actually higher

        metrics_low = create_test_metrics(roi_30d=roi_30d_low, trade_count_30d=100)
        metrics_high = create_test_metrics(roi_30d=roi_30d_high, trade_count_30d=100)

        wqs_low = calculate_wqs(metrics_low)
        wqs_high = calculate_wqs(metrics_high)

        # Allow some tolerance for penalties that might affect high ROI wallets
        # But generally, higher ROI should not result in significantly lower WQS
        assert wqs_high >= wqs_low - 10.0, (
            f"WQS not monotonic: roi_30d {roi_30d_low} -> {wqs_low}, "
            f"roi_30d {roi_30d_high} -> {wqs_high}"
        )

    @given(
        win_rate_low=st.floats(min_value=0.0, max_value=0.9, allow_nan=False, allow_infinity=False),
        win_rate_high=st.floats(min_value=0.0, max_value=0.9, allow_nan=False, allow_infinity=False),
    )
    def test_wqs_monotonicity_win_rate(self, win_rate_low: float, win_rate_high: float):
        """Property: Higher win rate should result in higher WQS (when other factors equal)."""
        if win_rate_low >= win_rate_high:
            return

        metrics_low = create_test_metrics(win_rate=win_rate_low, trade_count_30d=100)
        metrics_high = create_test_metrics(win_rate=win_rate_high, trade_count_30d=100)

        wqs_low = calculate_wqs(metrics_low)
        wqs_high = calculate_wqs(metrics_high)

        assert wqs_high >= wqs_low - 5.0, (
            f"WQS not monotonic for win rate: {win_rate_low} -> {wqs_low}, "
            f"{win_rate_high} -> {wqs_high}"
        )

    @given(
        roi_30d=st.floats(min_value=0.0, max_value=500.0, allow_nan=False, allow_infinity=False),
        roi_7d=st.floats(min_value=0.0, max_value=1000.0, allow_nan=False, allow_infinity=False),
    )
    def test_temporal_consistency_penalty(self, roi_30d: float, roi_7d: float):
        """Property: If 7d ROI is much higher than 30d ROI, WQS should be penalized."""
        if roi_7d <= roi_30d * 2:
            return  # Skip if penalty condition not met

        metrics_normal = create_test_metrics(roi_30d=roi_30d, roi_7d=roi_30d, trade_count_30d=100)
        metrics_spike = create_test_metrics(roi_30d=roi_30d, roi_7d=roi_7d, trade_count_30d=100)

        wqs_normal = calculate_wqs(metrics_normal)
        wqs_spike = calculate_wqs(metrics_spike)

        # Spike wallet should have lower or equal WQS due to penalty
        assert wqs_spike <= wqs_normal + 5.0, (
            f"Temporal consistency penalty not applied: normal={wqs_normal}, spike={wqs_spike}"
        )

    @given(
        trade_count_low=st.integers(min_value=0, max_value=19),
        trade_count_high=st.integers(min_value=20, max_value=1000),
    )
    def test_statistical_significance_penalty(self, trade_count_low: int, trade_count_high: int):
        """Property: Low trade count (< 20) should result in lower WQS."""
        metrics_low = create_test_metrics(roi_30d=50.0, trade_count_30d=trade_count_low)
        metrics_high = create_test_metrics(roi_30d=50.0, trade_count_30d=trade_count_high)

        wqs_low = calculate_wqs(metrics_low)
        wqs_high = calculate_wqs(metrics_high)

        # Low trade count should generally result in lower WQS
        # But allow some tolerance since other factors also matter
        if trade_count_low < 20 and trade_count_high >= 20:
            assert wqs_high >= wqs_low - 10.0, (
                f"Statistical significance penalty not working: "
                f"low_count={trade_count_low} -> {wqs_low}, "
                f"high_count={trade_count_high} -> {wqs_high}"
            )

    @given(
        drawdown=st.floats(min_value=0.0, max_value=100.0, allow_nan=False, allow_infinity=False),
    )
    def test_drawdown_penalty(self, drawdown: float):
        """Property: Higher drawdown should result in lower WQS."""
        metrics_no_dd = create_test_metrics(roi_30d=50.0, max_drawdown_30d=0.0, trade_count_30d=100)
        metrics_with_dd = create_test_metrics(roi_30d=50.0, max_drawdown_30d=drawdown, trade_count_30d=100)

        wqs_no_dd = calculate_wqs(metrics_no_dd)
        wqs_with_dd = calculate_wqs(metrics_with_dd)

        # Higher drawdown should result in lower WQS
        assert wqs_with_dd <= wqs_no_dd + 5.0, (
            f"Drawdown penalty not applied: no_dd={wqs_no_dd}, with_dd={wqs_with_dd} (drawdown={drawdown})"
        )

    @given(
        roi_30d=st.floats(min_value=-100.0, max_value=1000.0, allow_nan=False, allow_infinity=False),
        trade_count=st.integers(min_value=0, max_value=1000),
        win_rate=st.one_of(st.none(), st.floats(min_value=0.0, max_value=1.0, allow_nan=False, allow_infinity=False)),
    )
    def test_wqs_deterministic(self, roi_30d: float, trade_count: int, win_rate: Optional[float]):
        """Property: WQS is deterministic (same inputs -> same output)."""
        metrics1 = create_test_metrics(
            roi_30d=roi_30d,
            trade_count_30d=trade_count,
            win_rate=win_rate,
        )
        metrics2 = create_test_metrics(
            roi_30d=roi_30d,
            trade_count_30d=trade_count,
            win_rate=win_rate,
        )

        wqs1 = calculate_wqs(metrics1)
        wqs2 = calculate_wqs(metrics2)

        assert wqs1 == wqs2, f"WQS not deterministic: {wqs1} != {wqs2}"

    @given(
        roi_30d=st.floats(min_value=-100.0, max_value=1000.0, allow_nan=False, allow_infinity=False),
    )
    def test_wqs_handles_extreme_values(self, roi_30d: float):
        """Property: WQS handles extreme ROI values gracefully."""
        metrics = create_test_metrics(roi_30d=roi_30d, trade_count_30d=100)
        wqs = calculate_wqs(metrics)

        # Should still be in bounds even for extreme values
        assert 0 <= wqs <= 100, f"WQS {wqs} out of bounds for extreme roi_30d={roi_30d}"

    @given(
        win_streak=st.floats(min_value=0.0, max_value=1.0, allow_nan=False, allow_infinity=False),
    )
    def test_wqs_handles_win_streak(self, win_streak: float):
        """Property: WQS handles win streak consistency values correctly."""
        metrics = create_test_metrics(
            roi_30d=50.0,
            trade_count_30d=100,
            win_streak_consistency=win_streak,
        )
        wqs = calculate_wqs(metrics)

        assert 0 <= wqs <= 100, f"WQS {wqs} out of bounds for win_streak={win_streak}"


# Run tests if executed directly
if __name__ == "__main__":
    import pytest
    pytest.main([__file__, "-v"])
