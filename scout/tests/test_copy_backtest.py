"""
Unit tests for core/copy_backtest.py (Phase 1 backtest harness).

Validates the cost-adjusted metrics and report grouping against a stubbed
database layer, so the harness logic is verified without needing the
production PostgreSQL instance.
"""

import core.copy_backtest as cb


def _exit_rows():
    # Two mirror_main exits for one position pair + a wallet_sell row.
    return [
        {
            "exit_strategy": "mirror_main",
            "pnl_sol": 0.10, "pnl_pct": 10.0, "hold_duration_secs": 3600,
            "main_admitted": True, "main_rejection_code": None,
            "strategy": "SHIELD", "entry_amount_sol": 1.0, "opened_at": None,
        },
        {
            "exit_strategy": "mirror_main",
            "pnl_sol": -0.05, "pnl_pct": -5.0, "hold_duration_secs": 3600,
            "main_admitted": False, "main_rejection_code": "TOXIC_WALLET",
            "strategy": "SPEAR", "entry_amount_sol": 1.0, "opened_at": None,
        },
        {
            "exit_strategy": "wallet_sell",
            "pnl_sol": -0.20, "pnl_pct": -20.0, "hold_duration_secs": 7200,
            "main_admitted": True, "main_rejection_code": None,
            "strategy": "SHIELD", "entry_amount_sol": 1.0, "opened_at": None,
        },
    ]


def _stub(monkeypatch, exits=None, cost_row=None):
    monkeypatch.setattr(
        cb, "execute_and_fetchall", lambda *a, **k: exits if exits is not None else _exit_rows()
    )
    monkeypatch.setattr(
        cb, "execute_and_fetchone",
        lambda *a, **k: cost_row if cost_row is not None
        else {"cost": 0.02, "amt": 1.0},
    )


def test_observed_cost_per_sol(monkeypatch):
    _stub(monkeypatch, cost_row={"cost": 0.05, "amt": 2.0})
    assert cb.observed_cost_per_sol() == cb.Decimal("0.025")


def test_cost_adjustment_applied(monkeypatch):
    # cost_per_sol = 0.02; each position notional 1.0 -> each pnl reduced by 0.02
    _stub(monkeypatch, cost_row={"cost": 0.02, "amt": 1.0})
    bt = cb.CopyBacktest()
    rows = bt.per_exit_strategy()
    by = {r.group: r for r in rows}
    mm = by["mirror_main"]
    # raw 0.10 and -0.05, each minus 0.02 -> 0.08 and -0.07
    assert mm.n == 2
    assert mm.sum_pnl == cb.Decimal("0.01")  # 0.08 + (-0.07)
    ws = by["wallet_sell"]
    assert ws.sum_pnl == cb.Decimal("-0.22")  # -0.20 - 0.02


def test_per_gate_groups_admitted_vs_rejection(monkeypatch):
    _stub(monkeypatch, cost_row={"cost": 0.0, "amt": 1.0})
    bt = cb.CopyBacktest()
    rows = bt.per_gate("mirror_main")
    by = {r.group: r for r in rows}
    assert "ADMITTED" in by
    assert "TOXIC_WALLET" in by
    assert by["ADMITTED"].n == 1
    assert by["TOXIC_WALLET"].n == 1


def test_stats_math(monkeypatch):
    _stub(monkeypatch, cost_row={"cost": 0.0, "amt": 1.0})
    bt = cb.CopyBacktest()
    rows = bt.per_exit_strategy()
    mm = next(r for r in rows if r.group == "mirror_main")
    # values 0.10, -0.05 -> mean 0.025, median 0.025, win_rate 0.5
    assert mm.mean == 0.025
    assert abs(mm.median - 0.025) < 1e-9
    assert mm.win_rate == 0.5


def test_fill_skew_report_distribution_and_bands(monkeypatch):
    # Two SELL closes: amount 1.0 with slippage 0.15 (gap 15%) and 0.02 (gap 2%).
    rows = [
        {"amount_sol": 1.0, "slippage_cost_sol": 0.15, "side": "SELL"},
        {"amount_sol": 1.0, "slippage_cost_sol": 0.02, "side": "SELL"},
    ]
    monkeypatch.setattr(cb, "execute_and_fetchone", lambda *a, **k: {"cost": 0.0, "amt": 1.0})
    monkeypatch.setattr(
        cb, "execute_and_fetchall",
        lambda *a, **k: rows if "FROM trades" in (a[0] if a else "") else [],
    )
    bt = cb.CopyBacktest()
    rep = bt.fill_skew_report(skew_bands=(2, 10))
    assert rep["n"] == 2
    assert rep["median_gap_pct"] == 8.5  # (2 + 15) / 2
    # band=2: both gaps (2>2 false, 15>2 true) -> 1 trigger
    assert rep["bands"]["2"]["trigger_frac"] == 0.5
    # band=10: only 15>10 -> 1 trigger
    assert rep["bands"]["10"]["trigger_frac"] == 0.5
