"""Tests for analysis.frontier: marginal deltas + Pareto dominance."""

from datetime import datetime, timedelta
from decimal import Decimal

from analysis.frontier import (
    FrontierPoint,
    SignalRow,
    marginal_deltas,
    pareto_frontier,
)
from analysis.metrics import cohort_metrics


def _sig(admitted, code, pnl, amt, t):
    return SignalRow(admitted, code, Decimal(pnl), Decimal(amt), t)


def test_admitting_a_winning_rejected_gate_improves_pnl():
    base = datetime(2026, 8, 1)
    signals = [
        _sig(True, None, "0.2", "1", base),                          # admitted winner
        _sig(False, "SINGLE_WALLET_UNPROVEN", "0.5", "1", base + timedelta(hours=1)),
    ]
    admitted = [s for s in signals if s.admitted]
    baseline = cohort_metrics(
        [s.pnl_sol for s in admitted],
        [s.entry_amount_sol for s in admitted],
        [s.exited_at for s in admitted],
        Decimal("10"),
        30,
    )
    deltas = marginal_deltas(signals, baseline, Decimal("10"), 30)
    g = next(d for d in deltas if d.gate == "SINGLE_WALLET_UNPROVEN")
    assert g.delta_trades == 1
    assert g.net_pnl_sol_if_admitted == Decimal("0.5")
    assert g.delta_monthly_return_pct > 0


def test_admitting_a_losing_gate_hurts_net_pnl_and_is_ranked_last():
    base = datetime(2026, 8, 1)
    signals = [
        _sig(True, None, "0.2", "1", base),
        _sig(False, "GOOD_GATE", "0.5", "1", base + timedelta(hours=1)),
        _sig(False, "BAD_GATE", "-0.8", "1", base + timedelta(hours=2)),
    ]
    admitted = [s for s in signals if s.admitted]
    baseline = cohort_metrics(
        [s.pnl_sol for s in admitted],
        [s.entry_amount_sol for s in admitted],
        [s.exited_at for s in admitted],
        Decimal("10"),
        30,
    )
    deltas = marginal_deltas(signals, baseline, Decimal("10"), 30)
    # Sorted by net_pnl desc: GOOD_GATE (+0.5) before BAD_GATE (-0.8).
    assert deltas[0].gate == "GOOD_GATE"
    assert deltas[-1].gate == "BAD_GATE"
    assert deltas[-1].net_pnl_sol_if_admitted == Decimal("-0.8")


def test_pareto_frontier_excludes_dominated_points():
    pts = [
        FrontierPoint("a", trades=10, win_rate=0.5, monthly_pct=2.0, drawdown_pct=10),
        FrontierPoint("b", trades=20, win_rate=0.5, monthly_pct=4.0, drawdown_pct=10),  # dominates a
        FrontierPoint("c", trades=20, win_rate=0.6, monthly_pct=4.0, drawdown_pct=5),   # dominates b too
    ]
    front = pareto_frontier(pts)
    assert {p.name for p in front} == {"c"}


def test_pareto_frontier_keeps_tradeoff_points():
    # Neither dominates: one has higher return + higher drawdown.
    pts = [
        FrontierPoint("safe", trades=10, win_rate=0.6, monthly_pct=2.0, drawdown_pct=5),
        FrontierPoint("aggressive", trades=10, win_rate=0.5, monthly_pct=5.0, drawdown_pct=15),
    ]
    front = pareto_frontier(pts)
    assert {p.name for p in front} == {"safe", "aggressive"}
