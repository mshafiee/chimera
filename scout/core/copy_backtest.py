"""
Repeatable copy-engine backtest over shadow history (Phase 1).

Reads `shadow_positions` + `shadow_exits` + `trades` from PostgreSQL and emits
cost-adjusted, per-entry-gate and per-exit-strategy metric tables. This is the
measurement contract: no filter/exit parameter change ships without a
before/after diff of these metrics.

Cost model
----------
The shadow `pnl_sol` is raw price-based PnL. To make decisions on net
profitability we adjust it to mirror `trades.net_pnl_sol`: costs are estimated
from the observed `total_cost_sol / amount_sol` ratio in `trades` and applied
pro-rata to each shadow position's notional.

Financial precision: money stays in `Decimal`; float is used only for the
statistical summaries (mean/median/stddev/CI), mirroring how the operator's
profitability gate computes CIs. Statistical summaries are display metrics, not
financial amounts.
"""

from __future__ import annotations

import math
from dataclasses import dataclass
from decimal import Decimal
from typing import Dict, List, Optional

from .db import execute_and_fetchall, execute_and_fetchone
from .decimal_utils import float_to_decimal


# 95% confidence z-score (two-sided).
_Z = 1.96


@dataclass
class MetricRow:
    group: str
    n: int
    mean: float
    median: float
    p25: float
    p75: float
    stdev: float
    win_rate: float
    sum_pnl: Decimal
    ci_margin: float  # 95% half-width of the mean


def _stats(values: List[float]) -> Dict[str, float]:
    n = len(values)
    if n == 0:
        return {
            "n": 0, "mean": 0.0, "median": 0.0, "p25": 0.0, "p75": 0.0,
            "stdev": 0.0, "win_rate": 0.0, "ci_margin": 0.0,
        }
    mean = sum(values) / n
    s = sorted(values)

    def pct(p: float) -> float:
        k = (len(s) - 1) * p
        lo = math.floor(k)
        hi = math.ceil(k)
        if lo == hi:
            return s[lo]
        return s[lo] + (s[hi] - s[lo]) * (k - lo)

    var = (sum((v - mean) ** 2 for v in values) / (n - 1)) if n > 1 else 0.0
    stdev = math.sqrt(var)
    wins = sum(1 for v in values if v > 0)
    margin = _Z * stdev / math.sqrt(n) if n > 0 else 0.0
    return {
        "n": n, "mean": mean, "median": pct(0.5), "p25": pct(0.25),
        "p75": pct(0.75), "stdev": stdev, "win_rate": wins / n, "ci_margin": margin,
    }


def _metric_row(group: str, values: List[float], sum_pnl: Decimal) -> MetricRow:
    st = _stats(values)
    return MetricRow(
        group=group, n=st["n"], mean=st["mean"], median=st["median"],
        p25=st["p25"], p75=st["p75"], stdev=st["stdev"], win_rate=st["win_rate"],
        sum_pnl=sum_pnl, ci_margin=st["ci_margin"],
    )


def observed_cost_per_sol(days: int = 90) -> Decimal:
    """Cost per SOL of notional from closed trades (net-of-gross gap)."""
    row = execute_and_fetchone(
        "SELECT COALESCE(SUM(total_cost_sol),0) AS cost, "
        "       COALESCE(SUM(amount_sol),0) AS amt "
        "FROM trades WHERE status='CLOSED' AND created_at > NOW() - (%s || ' days')::interval",
        (str(days),),
    )
    amt = float_to_decimal(row["amt"]) if row else Decimal("0")
    cost = float_to_decimal(row["cost"]) if row else Decimal("0")
    if amt <= 0:
        return Decimal("0")
    return (cost / amt).quantize(Decimal("0.000001"))


class CopyBacktest:
    """Loads shadow history once and produces cost-adjusted metric tables."""

    def __init__(self, cost_per_sol: Optional[Decimal] = None, db_path: Optional[str] = None):
        self.db_path = db_path
        self.cost_per_sol = observed_cost_per_sol() if cost_per_sol is None else cost_per_sol

    # ── data ────────────────────────────────────────────────────────────────
    def _load_exits(self) -> List[dict]:
        return execute_and_fetchall(
            "SELECT e.exit_strategy, e.pnl_sol, e.pnl_pct, e.hold_duration_secs, "
            "       p.main_admitted, p.main_rejection_code, p.strategy, "
            "       p.entry_amount_sol, p.opened_at "
            "FROM shadow_exits e "
            "LEFT JOIN shadow_positions p ON p.shadow_id = e.shadow_id",
            db_path=self.db_path,
        )

    def _adjusted(self, row: dict) -> Decimal:
        raw = float_to_decimal(row["pnl_sol"])
        notional = float_to_decimal(row["entry_amount_sol"] or 1.0)
        return raw - (notional * self.cost_per_sol)

    # ── reports ─────────────────────────────────────────────────────────────
    def per_exit_strategy(self) -> List[MetricRow]:
        rows = self._load_exits()
        groups: Dict[str, List[float]] = {}
        sums: Dict[str, Decimal] = {}
        for r in rows:
            strat = r["exit_strategy"] or "unknown"
            groups.setdefault(strat, []).append(float(self._adjusted(r)))
            sums[strat] = sums.get(strat, Decimal("0")) + self._adjusted(r)
        return [_metric_row(g, groups[g], sums[g]) for g in sorted(groups)]

    def per_gate(self, exit_strategy: str = "mirror_main") -> List[MetricRow]:
        rows = [r for r in self._load_exits() if (r["exit_strategy"] or "") == exit_strategy]
        groups: Dict[str, List[float]] = {}
        sums: Dict[str, Decimal] = {}
        for r in rows:
            gate = "ADMITTED" if r["main_admitted"] else (r["main_rejection_code"] or "UNCLASSIFIED")
            pnl = self._adjusted(r)
            groups.setdefault(gate, []).append(float(pnl))
            sums[gate] = sums.get(gate, Decimal("0")) + pnl
        return [_metric_row(g, groups[g], sums[g]) for g in sorted(groups)]

    def by_strategy(self, exit_strategy: str = "mirror_main") -> List[MetricRow]:
        rows = [r for r in self._load_exits() if (r["exit_strategy"] or "") == exit_strategy]
        groups: Dict[str, List[float]] = {}
        sums: Dict[str, Decimal] = {}
        for r in rows:
            key = (r["strategy"] or "SHIELD").upper()
            pnl = self._adjusted(r)
            groups.setdefault(key, []).append(float(pnl))
            sums[key] = sums.get(key, Decimal("0")) + pnl
        return [_metric_row(g, groups[g], sums[g]) for g in sorted(groups)]

    # ── realize-vs-price gap (Phase 2 seed) ─────────────────────────────────
    def realize_vs_price_gap(self) -> dict:
        """Predicted (shadow mirror_main) vs realized (closed copy trades) win rate."""
        predicted = execute_and_fetchone(
            "SELECT COUNT(*) AS n, COUNT(*) FILTER (WHERE pnl_sol > 0) AS wins "
            "FROM shadow_exits WHERE exit_strategy='mirror_main'"
        )
        realized = execute_and_fetchone(
            "SELECT COUNT(*) AS n, COUNT(*) FILTER (WHERE net_pnl_sol > 0) AS wins "
            "FROM trades WHERE status='CLOSED' AND pnl_data_valid=TRUE"
        )
        p_n = predicted["n"] or 0
        r_n = realized["n"] or 0
        return {
            "predicted_win_rate": (predicted["wins"] / p_n) if p_n else None,
            "predicted_n": p_n,
            "realized_win_rate": (realized["wins"] / r_n) if r_n else None,
            "realized_n": r_n,
        }


ROWS_FORMAT = ["group", "n", "mean", "median", "p25", "p75", "stdev", "win_rate", "sum_pnl", "ci_margin"]


def format_report(title: str, rows: List[MetricRow]) -> str:
    lines = [f"== {title} ==", "group | n | mean | median | p25 | p75 | sd | win% | sum_pnl | 95%CI"]
    for r in rows:
        lines.append(
            f"{r.group} | {r.n} | {r.mean:.4f} | {r.median:.4f} | {r.p25:.4f} | {r.p75:.4f} "
            f"| {r.stdev:.4f} | {r.win_rate*100:.1f}% | {r.sum_pnl:.2f} | ±{r.ci_margin:.4f}"
        )
    return "\n".join(lines)
