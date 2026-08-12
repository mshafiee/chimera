"""Phase B: marginal gate analysis + Pareto frontier from shadow counterfactuals.

The shadow trader opens a position for EVERY BUY signal (admitted or rejected,
per migration 0015_shadow_trader.sql:5) and records its mirror_main PnL. That
gives us the counterfactual: "what would have happened if we admitted the
signals gate X rejected?" — without fabricating a backtest.

For each rejection gate we compute the marginal effect of admitting its signals
on top of the current ADMITTED baseline, then derive the non-dominated
(single-gate) Pareto frontier of (trades, win rate, monthly return, drawdown).
"""

from dataclasses import dataclass
from datetime import datetime
from decimal import Decimal
from typing import Iterable

from .metrics import CohortMetrics, cohort_metrics


@dataclass(frozen=True)
class SignalRow:
    admitted: bool
    main_rejection_code: str | None
    pnl_sol: Decimal
    entry_amount_sol: Decimal
    exited_at: datetime


@dataclass(frozen=True)
class GateDelta:
    """Marginal effect of admitting one gate's currently-rejected signals."""

    gate: str
    delta_trades: int
    delta_win_rate: float
    delta_monthly_return_pct: float
    net_pnl_sol_if_admitted: Decimal


@dataclass(frozen=True)
class FrontierPoint:
    name: str
    trades: int
    win_rate: float
    monthly_pct: float
    drawdown_pct: float


def fetch_signals(conn, exit_strategy: str = "mirror_main") -> list[SignalRow]:
    """One row per closed shadow exit under `exit_strategy`."""
    rows = conn.execute(
        """
        SELECT sp.main_admitted, sp.main_rejection_code,
               se.pnl_sol, sp.entry_amount_sol, se.exited_at
        FROM   shadow_positions sp
        JOIN   shadow_exits se ON se.shadow_id = sp.shadow_id
        WHERE  se.exit_strategy = %s
          AND  se.pnl_sol IS NOT NULL
          AND  sp.entry_amount_sol > 0
        """,
        (exit_strategy,),
    ).fetchall()
    return [SignalRow(bool(r[0]), r[1], Decimal(r[2]), Decimal(r[3]), r[4]) for r in rows]


def _metrics_for(
    signals: Iterable[SignalRow], capital_base_sol, window_days: int
) -> CohortMetrics:
    sigs = list(signals)
    return cohort_metrics(
        [s.pnl_sol for s in sigs],
        [s.entry_amount_sol for s in sigs],
        [s.exited_at for s in sigs],
        Decimal(str(capital_base_sol)),
        window_days,
    )


def marginal_deltas(
    signals: list[SignalRow],
    baseline: CohortMetrics,
    capital_base_sol,
    window_days: int,
) -> list[GateDelta]:
    """For each rejection gate, the delta of admitting its signals on top of baseline."""
    admitted = [s for s in signals if s.admitted]
    gates = {
        s.main_rejection_code
        for s in signals
        if (not s.admitted) and s.main_rejection_code
    }
    out: list[GateDelta] = []
    for gate in gates:
        added = [
            s for s in signals if (not s.admitted) and s.main_rejection_code == gate
        ]
        if not added:
            continue
        with_g = _metrics_for(admitted + added, capital_base_sol, window_days)
        net = sum((s.pnl_sol for s in added), Decimal("0"))
        out.append(
            GateDelta(
                gate=gate,
                delta_trades=with_g.trade_count - baseline.trade_count,
                delta_win_rate=with_g.win_rate - baseline.win_rate,
                delta_monthly_return_pct=with_g.monthly_return_pct
                - baseline.monthly_return_pct,
                net_pnl_sol_if_admitted=net,
            )
        )
    out.sort(key=lambda d: d.net_pnl_sol_if_admitted, reverse=True)
    return out


def pareto_frontier(points: list[FrontierPoint]) -> list[FrontierPoint]:
    """Non-dominated points: maximize win rate, monthly return, trades; minimize drawdown."""
    front: list[FrontierPoint] = []
    for p in points:
        dominated = False
        for q in points:
            if q is p:
                continue
            if (
                q.win_rate >= p.win_rate
                and q.monthly_pct >= p.monthly_pct
                and q.drawdown_pct <= p.drawdown_pct
                and q.trades >= p.trades
                and (
                    q.win_rate,
                    q.monthly_pct,
                    -q.drawdown_pct,
                    q.trades,
                )
                > (p.win_rate, p.monthly_pct, -p.drawdown_pct, p.trades)
            ):
                dominated = True
                break
        if not dominated:
            front.append(p)
    return front


def run_frontier(conn, capital_base_sol=10, window_days=30) -> str:
    """Print the baseline, per-gate marginal table, and the Pareto frontier."""
    signals = fetch_signals(conn)
    admitted = [s for s in signals if s.admitted]
    baseline = _metrics_for(admitted, capital_base_sol, window_days)
    deltas = marginal_deltas(signals, baseline, capital_base_sol, window_days)

    lines = [
        (
            f"Baseline (ADMITTED only): trades={baseline.trade_count} "
            f"win={baseline.win_rate:.1%} monthly={baseline.monthly_return_pct:.2f}% "
            f"maxDD={baseline.max_drawdown_pct:.1f}% "
            f"(per-trade 95% CI {baseline.ci_lower_pct:.2f}%..{baseline.ci_upper_pct:.2f}%)"
        ),
        "",
        "Marginal effect of ADMITTING each rejected gate (sorted by net PnL):",
        f"{'gate':<30} {'+trades':>7} {'dWin%':>8} {'dMo%':>9} {'netSol':>12}",
    ]
    for d in deltas:
        lines.append(
            f"{d.gate:<30} {d.delta_trades:>7} {d.delta_win_rate * 100:>7.1f}% "
            f"{d.delta_monthly_return_pct:>8.2f}% {d.net_pnl_sol_if_admitted:>12.4f}"
        )

    # Single-gate frontier points: baseline + each gate admitted alone.
    points: list[FrontierPoint] = [
        FrontierPoint(
            "baseline",
            baseline.trade_count,
            baseline.win_rate,
            baseline.monthly_return_pct,
            baseline.max_drawdown_pct,
        )
    ]
    for d in deltas:
        added = [
            s for s in signals if (not s.admitted) and s.main_rejection_code == d.gate
        ]
        m = _metrics_for(admitted + added, capital_base_sol, window_days)
        points.append(
            FrontierPoint(
                d.gate, m.trade_count, m.win_rate, m.monthly_return_pct, m.max_drawdown_pct
            )
        )
    front = pareto_frontier(points)
    lines += ["", "Pareto frontier (non-dominated single-gate moves):"]
    for p in sorted(front, key=lambda x: -x.monthly_pct):
        lines.append(
            f"  {p.name:<30} trades={p.trades} win={p.win_rate:.1%} "
            f"monthly={p.monthly_pct:.2f}% maxDD={p.drawdown_pct:.1f}%"
        )
    return "\n".join(lines)
