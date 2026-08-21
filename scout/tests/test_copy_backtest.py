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


def test_mark_gap_report_geometry(monkeypatch):
    # t1 dips (-10%) then recovers (+5%) within the held window; t2 falls (-10%).
    rows = [
        {"trade_uuid": "t1", "ts_unix": 100, "price_usd": 1.0},
        {"trade_uuid": "t1", "ts_unix": 200, "price_usd": 0.9},
        {"trade_uuid": "t1", "ts_unix": 300, "price_usd": 1.05},
        {"trade_uuid": "t2", "ts_unix": 100, "price_usd": 2.0},
        {"trade_uuid": "t2", "ts_unix": 400, "price_usd": 2.0},
        {"trade_uuid": "t2", "ts_unix": 600, "price_usd": 1.8},
    ]
    monkeypatch.setattr(cb, "execute_and_fetchall", lambda *a, **k: rows)
    monkeypatch.setattr(cb, "execute_and_fetchone", lambda *a, **k: {"cost": 0.0, "amt": 1.0})
    bt = cb.CopyBacktest()
    rep = bt.mark_gap_report()
    assert rep["n_positions"] == 2
    assert rep["marks"] == 6
    assert rep["mean_marks_per_position"] == 3.0
    # deltas: t1 100,100 ; t2 300,200 -> sorted [100,100,200,300] -> left-median 200
    assert rep["median_tick_cadence_secs"] == 200.0
    # final_pct: t1 +5%, t2 -10% -> mean -2.5%
    assert abs(rep["final_pct"]["mean"] - (-2.5)) < 1e-6
    # worst_drawdown: both -10% -> mean -10%
    assert abs(rep["worst_drawdown_pct"]["mean"] - (-10.0)) < 1e-6
    # recovery: t1 (1.05-0.9)/1.0=15% ; t2 (1.8-1.8)/2.0=0% -> mean 7.5%
    assert abs(rep["recovery_from_dip_pct"]["mean"] - 7.5) < 1e-6


def test_mark_gap_report_empty(monkeypatch):
    monkeypatch.setattr(cb, "execute_and_fetchall", lambda *a, **k: [])
    monkeypatch.setattr(cb, "execute_and_fetchone", lambda *a, **k: {"cost": 0.0, "amt": 1.0})
    bt = cb.CopyBacktest()
    rep = bt.mark_gap_report()
    assert rep["n_positions"] == 0
    assert rep["marks"] == 0


def test_reconcile_shadow_realized_attributes_gap(monkeypatch):
    # P1: shadow predicts +5%, realized gross -20%, net -25% (cost c. 5%).
    # P2: shadow predicts +12%, realized gross +10%, net +7.5% (cost c. 2.5%).
    closed = [
        {
            "wallet_address": "w1", "token_address": "t1",
            "entry_price": 1.0, "exit_price": 0.8,
            "entry_amount_sol": 1.0, "realized_pnl_sol": -0.20,
            "realized_net_pnl_sol": -0.25, "opened_ts": 1000,
        },
        {
            "wallet_address": "w2", "token_address": "t2",
            "entry_price": 2.0, "exit_price": 2.2,
            "entry_amount_sol": 1.0, "realized_pnl_sol": 0.20,
            "realized_net_pnl_sol": 0.15, "opened_ts": 2000,
        },
    ]
    shadows = [
        {"wallet_address": "w1", "token_address": "t1", "opened_ts": 1010,
         "shadow_pnl_pct": 5.0},
        {"wallet_address": "w2", "token_address": "t2", "opened_ts": 2000,
         "shadow_pnl_pct": 12.0},
    ]
    monkeypatch.setattr(
        cb, "execute_and_fetchall",
        lambda q, *a, **k: closed if "FROM positions" in q else shadows,
    )
    monkeypatch.setattr(cb, "execute_and_fetchone", lambda *a, **k: {"cost": 0.0, "amt": 1.0})
    bt = cb.CopyBacktest()
    rep = bt.reconcile_shadow_realized()
    assert rep["n_positions"] == 2
    assert rep["n_matched"] == 2
    assert rep["win_rates_pct"]["shadow"] == 100.0
    assert rep["win_rates_pct"]["realized_gross"] == 50.0
    assert rep["win_rates_pct"]["realized_net"] == 50.0
    # gap_gross: P1 5-(-20)=25, P2 12-10=2 -> mean 13.5
    assert abs(rep["gap_gross_pct"]["mean"] - 13.5) < 1e-6
    # cost: P1 (gross - net): -0.20 -> -0.25 = 0.05 /1.0 = 5% ; P2 0.20-0.15=0.05/1.0=5% -> mean 5
    assert abs(rep["mean_cost_pct"] - 5.0) < 1e-6


def test_cost_aware_screen_verdicts(monkeypatch):
    # cost_per_sol = 0.02 (2% per 1 SOL notional).
    realized = [
        {"wallet_address": "w1", "n": 10, "notional": 10.0, "gross_sol": 1.0, "net_sol": 0.4},
        {"wallet_address": "w2", "n": 10, "notional": 10.0, "gross_sol": 0.5, "net_sol": 0.05},
        {"wallet_address": "w3", "n": 10, "notional": 10.0, "gross_sol": 0.0, "net_sol": -0.2},
    ]
    shadow = [
        {"wallet_address": "w4", "n": 10, "notional": 10.0, "gross_sol": 1.0},
        {"wallet_address": "w5", "n": 10, "notional": 10.0, "gross_sol": 0.1},
    ]
    monkeypatch.setattr(
        cb, "execute_and_fetchall",
        lambda q, *a, **k: realized if "FROM positions" in q else shadow,
    )
    monkeypatch.setattr(cb, "execute_and_fetchone", lambda *a, **k: {"cost": 0.02, "amt": 1.0})
    bt = cb.CopyBacktest()
    sc = bt.cost_aware_screen(min_positions=5)

    rv = {w["wallet"]: w for w in sc["realized_book"]}
    # realized net is already net-of-cost: w1 4% -> CLEAR, w2 0.5% -> MARGINAL, w3 -2% -> NEGATIVE
    assert rv["w1"]["net_pct"] == 4.0 and rv["w1"]["verdict"] == "CLEAR"
    assert rv["w1"]["gross_pct"] == 10.0
    assert rv["w2"]["verdict"] == "MARGINAL"
    assert rv["w3"]["verdict"] == "NEGATIVE"

    sv = {w["wallet"]: w for w in sc["shadow_history"]}
    # shadow cost-adjusted: w4 net=(1.0-10*0.02)/10*100=8% -> CLEAR; w5 net=(0.1-0.2)/10*100=-1% -> NEGATIVE
    assert sv["w4"]["verdict"] == "CLEAR"
    assert sv["w5"]["verdict"] == "NEGATIVE"
    assert sc["min_positions"] == 5
