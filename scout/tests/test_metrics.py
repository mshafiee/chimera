"""Tests for analysis.metrics and analysis.diagnostic."""

from datetime import datetime, timedelta
from decimal import Decimal

from hypothesis import given, settings
from hypothesis import strategies as st

from analysis.metrics import cohort_metrics
from analysis.diagnostic import render_funnel


def _dec_lists(pnls, amounts, base_time):
    pnl = [Decimal(str(p)) for p in pnls]
    amt = [Decimal(str(a)) for a in amounts]
    ts = [base_time + timedelta(minutes=i) for i in range(len(pnl))]
    return pnl, amt, ts


@given(
    pnls=st.lists(
        st.floats(min_value=-1.0, max_value=2.0, allow_nan=False, allow_infinity=False),
        min_size=1,
        max_size=25,
    )
)
@settings(max_examples=50)
def test_win_rate_in_unit_interval(pnls):
    pnl, amt, ts = _dec_lists(pnls, ["0.1"] * len(pnls), datetime(2026, 8, 1))
    m = cohort_metrics(pnl, amt, ts, Decimal("10"), 30)
    assert 0.0 <= m.win_rate <= 1.0
    assert m.trade_count == len(pnls)
    assert m.win_rate == pytest_win_rate(pnls)


def test_monotonic_increase_has_zero_drawdown():
    pnl, amt, ts = _dec_lists(["0.1", "0.2", "0.3"], ["1", "1", "1"], datetime(2026, 8, 1))
    m = cohort_metrics(pnl, amt, ts, Decimal("10"), 30)
    assert m.max_drawdown_pct == 0.0


def test_drawdown_is_positive_after_a_dip():
    pnl, amt, ts = _dec_lists(["1.0", "-0.5"], ["1", "1"], datetime(2026, 8, 1))
    m = cohort_metrics(pnl, amt, ts, Decimal("10"), 30)
    assert m.max_drawdown_pct > 0.0


def test_total_pnl_is_sum():
    pnl, amt, ts = _dec_lists(["0.5", "-0.2", "0.3"], ["1", "1", "1"], datetime(2026, 8, 1))
    m = cohort_metrics(pnl, amt, ts, Decimal("10"), 30)
    assert m.total_pnl_sol == Decimal("0.6")


def test_monthly_return_scales_with_window():
    pnl, amt, ts = _dec_lists(["1.0"], ["1"], datetime(2026, 8, 1))
    m30 = cohort_metrics(pnl, amt, ts, Decimal("10"), 30)
    m60 = cohort_metrics(pnl, amt, ts, Decimal("10"), 60)
    # Half the window → double the 30-day-equivalent return.
    assert abs(m30.monthly_return_pct - 2 * m60.monthly_return_pct) < 1e-9


def test_ci_brackets_mean_for_large_sample():
    # 60 wins of +0.2 and 40 losses of -0.05 on 1.0 SOL each: mean = +10%,
    # with real variance so the 95% CI meaningfully brackets 10%.
    pnl = [Decimal("0.2")] * 60 + [Decimal("-0.05")] * 40
    amt = [Decimal("1")] * 100
    ts = [datetime(2026, 8, 1) + timedelta(minutes=i) for i in range(100)]
    m = cohort_metrics(pnl, amt, ts, Decimal("10"), 30)
    assert m.ci_lower_pct < 10.0 < m.ci_upper_pct
    assert m.ci_lower_pct > 0.0  # statistically significant edge at 95%


def test_render_funnel_formats_dominant_gate_first():
    rows = [
        {"gate": "SINGLE_WALLET_UNPROVEN", "signal_count": 500, "winners": 210,
         "losers": 290, "avg_pnl_pct": Decimal("-1.2"), "total_pnl_sol": Decimal("-40.3")},
        {"gate": "ADMITTED", "signal_count": 12, "winners": 5,
         "losers": 7, "avg_pnl_pct": Decimal("0.4"), "total_pnl_sol": Decimal("1.1")},
    ]
    out = render_funnel(rows)
    assert "SINGLE_WALLET_UNPROVEN" in out
    assert out.index("SINGLE_WALLET_UNPROVEN") < out.index("ADMITTED")


def pytest_win_rate(pnls):
    n = len(pnls)
    return sum(1 for p in pnls if p > 0) / n if n else 0.0
