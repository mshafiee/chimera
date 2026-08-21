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
from typing import Dict, List, Optional, Tuple

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

    # ── realized fill-skew (the gap's cost component) ───────────────────────
    def fill_skew_report(self, days: int = 90, skew_bands=(0, 2, 5, 8, 12)) -> dict:
        """Measure the live-vs-mark fill skew on real closed sells.

        `slippage_cost_sol / amount_sol * 100` is a direct proxy for the
        `loss_pct_cache - loss_pct_live` gap that `should_defer_exit` keys on:
        how much worse the live sell fill was than the price-cache mark. We
        report its distribution and, for each candidate `skew_pct` band, the
        share of exits that would (a) trigger a defer and (b) the slippage lying
        beyond the band — i.e. what a smarter fill could recover IF the price
        recovers to the mark (the recovery outcome itself is NOT observable from
        the recorded single-exit data, so it is reported separately and not
        assumed into the numbers).
        """
        rows = execute_and_fetchall(
            "SELECT amount_sol, slippage_cost_sol, side "
            "FROM trades WHERE status='CLOSED' AND pnl_data_valid=TRUE "
            "  AND side='SELL' AND COALESCE(amount_sol,0) > 0 "
            "  AND created_at > NOW() - (%s || ' days')::interval",
            (str(days),),
            db_path=self.db_path,
        )
        gaps: List[float] = []
        for r in rows:
            amt = float_to_decimal(r["amount_sol"] or 0)
            slip = float_to_decimal(r["slippage_cost_sol"] or 0)
            if amt > 0:
                gaps.append(float(slip / amt * 100))
        if not gaps:
            return {"n": 0}
        st = _stats(gaps)
        s90 = sorted(gaps)[int(0.9 * (len(gaps) - 1))]
        bands = {}
        for band in skew_bands:
            n_trig = sum(1 for g in gaps if g > band)
            beyond = float(sum(Decimal(g) - Decimal(band) for g in gaps if g > band))
            bands[str(band)] = {
                "trigger_frac": n_trig / len(gaps),
                "slippage_beyond_band_pct_total": beyond,
            }
        return {
            "n": st["n"],
            "mean_gap_pct": st["mean"],
            "median_gap_pct": st["median"],
            "p25_gap_pct": st["p25"],
            "p75_gap_pct": st["p75"],
            "p90_gap_pct": s90,
            "max_gap_pct": max(gaps),
            "bands": bands,
        }

    # ── recorded price marks (Phase 2E) ─────────────────────────────────────
    # `position_price_marks` (migration 0021) is the operator's recorded
    # price-cache USD mark per open position per tick. Before it existed the
    # realize-vs-price gap was tunable only against n=4 real sells; these marks
    # are the forward data series that makes the gap measurable on recorded
    # marks rather than snapshots.
    def mark_gap_report(self, days: int = 90) -> dict:
        """Summarize the recorded price-mark series (per-position geometry).

        From the marks alone, and without assuming any post-close recovery
        (marks stop when a position closes), report:
          - coverage (positions with marks, total marks, marks/position, cadence);
          - per-position intra-window geometry: worst drawdown the monitor saw,
            final pct at the last recorded mark, and recovery from the dip within
            the held window — the raw signal for whether deferring a protective
            exit would have helped (dip-then-recover) vs hurt (dip that kept
            falling).
        """
        rows = execute_and_fetchall(
            "SELECT ppm.trade_uuid, ppm.ts_unix, ppm.price_usd "
            "FROM position_price_marks ppm "
            "WHERE ppm.ts_unix >= EXTRACT(EPOCH FROM NOW() - (%s || ' days')::interval) "
            "ORDER BY ppm.trade_uuid, ppm.ts_unix",
            (str(days),),
            db_path=self.db_path,
        )
        if not rows:
            return {"n_positions": 0, "marks": 0}

        by_pos: Dict[str, List[Tuple[int, float]]] = {}
        for r in rows:
            by_pos.setdefault(r["trade_uuid"], []).append(
                (int(r["ts_unix"]), float(r["price_usd"]))
            )

        totals: List[float] = []
        mins: List[float] = []
        recs: List[float] = []
        counts: List[int] = []
        deltas: List[float] = []
        for pts in by_pos.values():
            first = pts[0][1]
            if first <= 0:
                continue
            last = pts[-1][1]
            mn = min(p for _, p in pts)
            totals.append((last - first) / first * 100)
            mins.append((mn - first) / first * 100)
            recs.append((last - mn) / first * 100)
            counts.append(len(pts))
            for (t0, _), (t1, _) in zip(pts, pts[1:]):
                if t1 > t0:
                    deltas.append(float(t1 - t0))

        if not totals:
            return {"n_positions": 0, "marks": len(rows)}
        t = _stats(totals)
        m = _stats(mins)
        r = _stats(recs)
        return {
            "n_positions": len(totals),
            "marks": len(rows),
            "mean_marks_per_position": round(sum(counts) / len(counts), 1),
            "median_tick_cadence_secs": float(sorted(deltas)[len(deltas) // 2]) if deltas else 0.0,
            "final_pct": {"mean": round(t["mean"], 3), "median": round(t["median"], 3)},
            "worst_drawdown_pct": {"mean": round(m["mean"], 3), "median": round(m["median"], 3)},
            "recovery_from_dip_pct": {"mean": round(r["mean"], 3), "median": round(r["median"], 3)},
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
