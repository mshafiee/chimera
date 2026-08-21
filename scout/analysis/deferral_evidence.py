"""Deferral-evidence detector (Phase 2J) — find "dip-then-recover-then-cut".

The realize-vs-price gap is tunable only on recorded evidence. `position_price_marks`
(migration 0021) records the monitor's USD mark per open position per tick.
This tool classifies every CLOSED position with enough marks into:

  - dip_then_recover : drew down past the dip threshold, then recovered a
    material amount BEFORE close, yet exited below the post-dip recovery peak
    -> a deferral at the dip would have realized a better price. The
    deferral-recoverable loss = (post-dip peak pct - exit pct) * notional.
  - kept_falling     : drew down and never recovered before close (deferral
    would have hurt — the honest control group).
  - clean            : no meaningful dip (normal win/loss).
  - insufficient     : fewer than min_marks (can't classify reliably).

Aggregate: total deferral-recoverable SOL across the book — the ceiling a
deferral tuned on recorded dip-then-recover cases could recover. This feeds
the future `protective_stop_should_defer` tuning decision.

Usage:
  python -m analysis.deferral_evidence [--days 7] [--dip-threshold-pct 3]
                                       [--recovery-threshold-pp 5]
                                       [--min-marks 10] [--limit N]
"""

from __future__ import annotations

import argparse
import json
import sys
from typing import List, Tuple

from core.db import execute_and_fetchall


def classify_position(
    marks: List[Tuple[int, float]],
    entry_price: float,
    exit_price: float,
    notional: float,
    dip_threshold_pct: float = 3.0,
    recovery_threshold_pp: float = 5.0,
    min_marks: int = 10,
) -> dict:
    """Classify one position's recorded marks.

    `marks` = [(ts_unix, price_usd), ...] ordered ascending. Pure function —
    no DB access, so it is directly unit-testable."""
    n = len(marks)
    result = {
        "type": "insufficient",
        "deferral_candidate": False,
        "min_pct": 0.0, "peak_after_dip_pct": 0.0,
        "exit_pct": 0.0, "last_pct": 0.0,
        "recoverable_pct": 0.0, "recoverable_sol": 0.0,
    }
    if n < min_marks or entry_price <= 0 or exit_price <= 0:
        return result
    pcts = [(p / entry_price - 1) * 100 for _, p in marks]
    mn_idx = min(range(n), key=lambda i: pcts[i])
    min_pct = pcts[mn_idx]
    peak_after = max(pcts[mn_idx:])
    exit_pct = (exit_price / entry_price - 1) * 100
    dipped = min_pct <= -dip_threshold_pct
    recovered = (peak_after - min_pct) >= recovery_threshold_pp
    # A min at the very last mark means the dip was still in progress at close
    # — nothing recovered to defer toward, so it is not a candidate.
    still_falling_at_end = mn_idx >= n - 1
    candidate = (
        dipped and recovered and not still_falling_at_end and exit_pct < peak_after
    )
    recoverable_pct = max(0.0, peak_after - exit_pct) if candidate else 0.0
    result.update(
        {
            "type": "dip_then_recover" if (dipped and recovered) else (
                "kept_falling" if dipped else "clean"
            ),
            "deferral_candidate": candidate,
            "min_pct": round(min_pct, 3),
            "peak_after_dip_pct": round(peak_after, 3),
            "exit_pct": round(exit_pct, 3),
            "last_pct": round(pcts[-1], 3),
            "recoverable_pct": round(recoverable_pct, 3),
            "recoverable_sol": round(notional * recoverable_pct / 100, 4),
        }
    )
    return result


def build_report(
    days: int = 7,
    dip_threshold_pct: float = 3.0,
    recovery_threshold_pp: float = 5.0,
    min_marks: int = 10,
    limit: int | None = None,
) -> dict:
    """Load closed positions with recorded marks and classify each."""
    pos_sql = (
        "SELECT p.trade_uuid, p.token_symbol, p.entry_price, p.exit_price, "
        "       p.entry_amount_sol "
        "FROM positions p "
        "WHERE p.state = 'CLOSED' AND p.entry_price > 0 AND p.exit_price IS NOT NULL "
        "AND p.closed_at > NOW() - %s::interval"
    )
    params: List[object] = [f"{days} days"]
    if limit:
        pos_sql += " LIMIT %s"
        params.append(limit)
    positions = execute_and_fetchall(pos_sql, tuple(params))

    cases: List[dict] = []
    for p in positions:
        marks = execute_and_fetchall(
            "SELECT ts_unix, price_usd FROM position_price_marks "
            "WHERE trade_uuid = %s ORDER BY ts_unix ASC",
            (p["trade_uuid"],),
        )
        if not marks:
            continue
        pts = [(int(r["ts_unix"]), float(r["price_usd"])) for r in marks]
        c = classify_position(
            pts,
            float(p["entry_price"]),
            float(p["exit_price"]),
            float(p["entry_amount_sol"] or 1.0),
            dip_threshold_pct,
            recovery_threshold_pp,
            min_marks,
        )
        cases.append(
            {
                "trade_uuid": p["trade_uuid"],
                "token_symbol": str(p["token_symbol"])[:14],
                "marks": len(pts),
                **c,
            }
        )

    n = len(cases)
    cands = [c for c in cases if c["deferral_candidate"]]
    dip_rec = [c for c in cases if c["type"] == "dip_then_recover"]
    falling = [c for c in cases if c["type"] == "kept_falling"]
    total_recoverable_sol = sum(c["recoverable_sol"] for c in cands)
    cands_sorted = sorted(cands, key=lambda c: -c["recoverable_sol"])[:15]
    return {
        "positions_with_marks": n,
        "dip_then_recover": len(dip_rec),
        "kept_falling": len(falling),
        "deferral_candidates": len(cands),
        "total_deferral_recoverable_sol": round(total_recoverable_sol, 3),
        "mean_recoverable_sol": (
            round(total_recoverable_sol / len(cands), 4) if cands else 0.0
        ),
        "top_candidates": cands_sorted,
    }


def _fmt_report(rep: dict) -> str:
    lines = [
        f"positions_with_marks={rep['positions_with_marks']}  "
        f"dip_then_recover={rep['dip_then_recover']}  "
        f"kept_falling={rep['kept_falling']}"
    ]
    lines.append(
        f"deferral_candidates={rep['deferral_candidates']}  "
        f"total_deferral_recoverable_sol={rep['total_deferral_recoverable_sol']}  "
        f"mean_recoverable_sol={rep['mean_recoverable_sol']}"
    )
    for c in rep["top_candidates"]:
        lines.append(
            f"  {c['token_symbol']:<14} type={c['type']:<15} "
            f"min={c['min_pct']:>6.2f}% peak_after={c['peak_after_dip_pct']:>6.2f}% "
            f"exit={c['exit_pct']:>6.2f}%  recoverable={c['recoverable_sol']:>7.4f} SOL"
        )
    return "\n".join(lines)


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description="Deferral-evidence detector")
    ap.add_argument("--days", type=int, default=7)
    ap.add_argument("--dip-threshold-pct", type=float, default=3.0)
    ap.add_argument("--recovery-threshold-pp", type=float, default=5.0)
    ap.add_argument("--min-marks", type=int, default=10)
    ap.add_argument("--limit", type=int, default=None)
    ap.add_argument("--save", metavar="PATH", default=None, help="write full report JSON")
    args = ap.parse_args(argv)

    report = build_report(
        days=args.days,
        dip_threshold_pct=args.dip_threshold_pct,
        recovery_threshold_pp=args.recovery_threshold_pp,
        min_marks=args.min_marks,
        limit=args.limit,
    )
    print(_fmt_report(report))
    if args.save:
        with open(args.save, "w") as fh:
            json.dump(report, fh, indent=2)
        print(f"[deferral_evidence] saved {args.save}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))