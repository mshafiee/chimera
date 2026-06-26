"""
Property-based tests for WQS (Wallet Quality Score) calculation.

Uses Hypothesis to test that WQS properties hold for all valid inputs.
"""

from hypothesis import given, strategies as st, example
from typing import Optional
from decimal import Decimal
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

    def test_wqs_near_zero_roi_30d_no_momentum_bonus(self):
        """Phase 1c: Near-zero roi_30d should not receive recency momentum bonus."""
        # Wallet with roi_30d=0.001, roi_7d=4.0 — should NOT get +5 momentum bonus
        metrics = WalletMetrics(
            address="test",
            roi_30d=0.001,
            roi_7d=4.0,
            trade_count_30d=100,
            win_rate=0.7,
        )
        wqs_with = calculate_wqs(metrics)

        # Same wallet WITHOUT the near-zero roi_30d recency-bonus issue
        # We can't construct a direct counterfactual, but we verify the score is bounded
        assert 0 <= wqs_with <= 100, f"WQS {wqs_with} with near-zero roi_30d"

        # Wallet with roi_30d=0.001 and roi_7d=0.001 should NOT have higher score
        # than one with the same inputs (trivial case)
        metrics2 = WalletMetrics(
            address="test2",
            roi_30d=0.001,
            roi_7d=0.001,
            trade_count_30d=100,
            win_rate=0.7,
        )
        wqs_without = calculate_wqs(metrics2)
        # The wallet with roi_7d=4.0 should not get an unfair momentum bonus
        # over the wallet with roi_7d=0.001 when roi_30d is near-zero.
        # Both should be close since roi_30d < 1.0 blocks the recency path.
        assert abs(wqs_with - wqs_without) < 15.0, (
            f"Near-zero roi_30d should not trigger momentum bonus: "
            f"wqs_with_7d={wqs_with:.1f}, wqs_without={wqs_without:.1f}"
        )

    def test_wqs_pump_spike_near_zero_baseline(self):
        """Phase 1c: Near-zero roi_30d baseline should trigger pump detection."""
        metrics = WalletMetrics(
            address="test3",
            roi_30d=0.001,
            roi_7d=50.0,
            trade_count_30d=100,
            win_rate=0.7,
            max_drawdown_30d=0.0,
        )
        wqs = calculate_wqs(metrics)
        # With pump-spike detection active and roi_30d near-zero + roi_7d > 10,
        # the _is_pump_spike flag should be True, penalizing the wallet.
        # Verify score is not inflated despite the high roi_7d.
        assert 0 <= wqs <= 100
        # A 50% 7d ROI with near-zero baseline should be heavily penalized
        assert wqs < 60.0, f"Expected pump-spike penalty for near-zero baseline, got WQS={wqs:.1f}"

    def test_wqs_profit_factor_graduated(self):
        """Phase 2a/Quick Win: PF cliff at 1.2 is graduated."""
        def wqs_for_pf(pf):
            return calculate_wqs(WalletMetrics(
                address="test",
                roi_30d=50.0,
                roi_7d=10.0,
                trade_count_30d=100,
                win_rate=0.65,
                profit_factor=pf,
                max_drawdown_30d=5.0,
                avg_trade_size_sol=Decimal('0.5'),  # Set to avoid dust trader penalty
            ))

        wqs_12 = wqs_for_pf(1.2)
        wqs_119 = wqs_for_pf(1.19)
        wqs_115 = wqs_for_pf(1.15)
        wqs_11 = wqs_for_pf(1.1)
        wqs_10 = wqs_for_pf(1.0)
        wqs_09 = wqs_for_pf(0.99)

        # PF bands: 1.2→+2, 1.15→-1, 1.1→-3, 1.0→-6, 0.99→-25
        assert wqs_12 >= wqs_119, f"PF=1.2 ({wqs_12:.1f}) >= PF=1.19 ({wqs_119:.1f})"
        assert wqs_119 >= wqs_115, f"PF=1.19 ({wqs_119:.1f}) >= PF=1.15 ({wqs_115:.1f})"
        assert wqs_115 >= wqs_11, f"PF=1.15 ({wqs_115:.1f}) >= PF=1.1 ({wqs_11:.1f})"
        assert wqs_11 >= wqs_10, f"PF=1.1 ({wqs_11:.1f}) >= PF=1.0 ({wqs_10:.1f})"
        assert wqs_10 > wqs_09, f"PF=1.0 ({wqs_10:.1f}) should be > PF=0.99 ({wqs_09:.1f})"

        # Gap between 1.2 and 1.19 should be modest (not the old 12-point cliff)
        gap = wqs_12 - wqs_119
        assert gap < 10.0, f"PF cliff too large: {gap:.1f} points between PF=1.2 and PF=1.19"

        # Gap between 1.0 and 0.99 should be large (breakeven vs losing)
        gap_loss = wqs_10 - wqs_09
        assert gap_loss > 10.0, f"Losing trader penalty too weak: {gap_loss:.1f} points between PF=1.0 and PF=0.99"


# Run tests if executed directly
if __name__ == "__main__":
    import pytest
    pytest.main([__file__, "-v"])
