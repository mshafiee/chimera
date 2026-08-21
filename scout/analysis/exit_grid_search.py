"""Exit-parameter grid-search over reconstructed shadow price paths (Phase 2D).

Loads shadow positions that have reconstructed price paths (price_path_points),
replays each candidate `ProfitManagementConfig` through the Rust `replay_exit`
binary (the SAME exit rules the live monitor uses — no production drift), and
ranks configs by cost-adjusted sum PnL. This is the data-driven lever the
golden baseline said was impossible without a price-mark series: now that
`price_path_points` exists (Birdeye OHLCV reconstruction), arbitrary
stop/recovery/trailing/time params are tunable against real price paths.

Population: defaults to NOT main_admitted (the same cohort the CLI `replay`
command uses, so results are directly comparable to its baseline). Use
--all / --admitted-only to change the book. Cost model: observed
cost-per-SOL from closed trades, applied pro-rata to each position notional
(mirrors CopyBacktest).

Usage:
  python -m analysis.exit_grid_search [--limit N] [--binary /app/replay_exit]
                                      [--population {shadow,all,admitted}]
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from decimal import Decimal
from typing import Dict, List, Tuple

from core.db import execute_and_fetchall
from core.replay_harness import load_stored_paths
from core.copy_backtest import observed_cost_per_sol


def load_positions(limit: int, population: str, entry_mode: str = "anchor") -> List[dict]:
    if population == "admitted":
        extra = "AND COALESCE(sp.main_admitted, FALSE)"
    elif population == "shadow":
        extra = "AND NOT COALESCE(sp.main_admitted, FALSE)"
    else:  # all
        extra = ""
    rows = execute_and_fetchall(
        "SELECT sp.token_address, "
        "       COALESCE(sp.strategy,'SHIELD') AS strategy, "
        "       EXTRACT(EPOCH FROM sp.opened_at)::bigint AS opened_at, "
        "       sp.entry_amount_sol, "
        "       COALESCE(sp.entry_price_usd, 0) AS entry_price_usd "
        "FROM shadow_positions sp "
        "JOIN (SELECT token_address FROM price_path_points GROUP BY token_address "
        "      HAVING COUNT(*) >= 2) pp USING (token_address) "
        f"WHERE sp.token_address IS NOT NULL {extra} "
        "LIMIT %s",
        (limit,),
    )
    positions = []
    for r in rows:
        opened = int(r["opened_at"])
        pts = [(ts, p) for ts, p in load_stored_paths(r["token_address"]) if ts >= opened]
        if len(pts) < 2:
            continue
        rec = r["entry_price_usd"]
        use_recorded = entry_mode == "recorded" and rec is not None and float(rec) > 0
        entry = Decimal(str(rec)) if use_recorded else pts[0][1]
        positions.append(
            {
                "entry_price": str(entry),
                "opened_at": opened,
                "strategy": r["strategy"],
                "size_sol": str(r["entry_amount_sol"] or "1.0"),
                "points": [[ts, str(p)] for ts, p in pts],
                "_notional": float(r["entry_amount_sol"] or 1.0),
            }
        )
    return positions


def run_config(positions: List[dict], overrides: dict, binary: str) -> List[dict]:
    inp = {
        "overrides": overrides,
        "positions": [{k: v for k, v in p.items() if k != "_notional"} for p in positions],
    }
    proc = subprocess.run(
        [binary, "--input", "-"],
        input=json.dumps(inp),
        text=True,
        capture_output=True,
        timeout=300,
    )
    if proc.returncode != 0:
        raise RuntimeError(f"replay_exit rc={proc.returncode}: {proc.stderr[-500:]}")
    return json.loads(proc.stdout).get("results", [])


def metrics(results: List[dict], positions: List[dict], cost_per_sol: float) -> dict:
    n = len(results)
    pnls = [float(r["pnl_sol"]) for r in results]
    wins = sum(1 for p in pnls if p > 0)
    sum_raw = sum(pnls)
    notional = sum(p["_notional"] for p in positions[:n])
    sum_cost = sum_raw - notional * cost_per_sol
    reasons: Dict[str, int] = {}
    for r in results:
        reasons[r["exit_reason"]] = reasons.get(r["exit_reason"], 0) + 1
    top_reasons = ", ".join(f"{k}:{v}" for k, v in sorted(reasons.items(), key=lambda x: -x[1])[:4])
    return {
        "n": n,
        "win_pct": (wins / n * 100) if n else 0.0,
        "sum_raw": sum_raw,
        "sum_cost": sum_cost,
        "mean": (sum_raw / n) if n else 0.0,
        "max_loss": min(pnls) if pnls else 0.0,
        "reasons": top_reasons,
    }


def build_grid() -> List[Tuple[str, dict]]:
    """Baseline + one-factor-at-a-time sweeps + focused combos.

    Only the params exposed by replay_exit's Overrides are tunable: stop
    distance, recovery gate (threshold/hard/max_secs), wick max-loss, trailing
    (activation/distance), and the losing/time exits. Per-target caps (25%)
    are intentionally fixed — they are not overridable and act as the
    take-profit rail."""
    grid: List[Tuple[str, dict]] = [("BASELINE", {})]
    for v in (-3.0, -8.0, -12.0):
        grid.append((f"stop{v}", {"max_stop_loss_distance": v}))
    for v in (-1.0, -1.5, -4.0):
        grid.append((f"rg_thr{v}", {"recovery_gate_threshold": v}))
    for v in (-3.0, -8.0, -12.0):
        grid.append((f"rg_hard{v}", {"recovery_gate_hard_threshold": v}))
    for v in (120, 600):
        grid.append((f"rg_max{v}", {"recovery_gate_max_secs": v}))
    for v in (10.0, 30.0):
        grid.append((f"trail_act{v}", {"trailing_stop_activation": v}))
    for v in (8.0, 20.0):
        grid.append((f"trail_dist{v}", {"trailing_stop_distance": v}))
    for v in (2, 8):
        grid.append((f"lte_sh{v}", {"losing_time_exit_hours_shield": v}))
    for v in (4, 12):
        grid.append((f"time{v}", {"time_exit_hours": v}))
    # Focused combos on the loss-control levers the sweeps should surface.
    grid.append(
        (
            "tight_A",
            {
                "max_stop_loss_distance": -3.0,
                "recovery_gate_threshold": -1.5,
                "recovery_gate_hard_threshold": -3.0,
                "recovery_gate_max_secs": 120,
            },
        )
    )
    grid.append(
        (
            "tight_B",
            {
                "max_stop_loss_distance": -5.0,
                "recovery_gate_threshold": -1.5,
                "recovery_gate_hard_threshold": -5.0,
                "recovery_gate_max_secs": 120,
                "trailing_stop_activation": 10.0,
            },
        )
    )
    return grid


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--limit", type=int, default=5000)
    ap.add_argument("--binary", default="/app/replay_exit")
    ap.add_argument("--population", default="shadow", choices=["shadow", "all", "admitted"])
    ap.add_argument(
        "--entry-mode",
        default="anchor",
        choices=["anchor", "recorded"],
        help="anchor: use first in-window reconstructed price (default, matches the "
        "replay CLI). recorded: use the position's true entry_price_usd when present "
        "(reconciles against how shadow_exits.mirror_main is generated).",
    )
    args = ap.parse_args()

    cost_per_sol = float(observed_cost_per_sol())
    positions = load_positions(args.limit, args.population, args.entry_mode)
    if not positions:
        print("no positions with reconstructed paths", file=sys.stderr)
        return 1
    print(
        f"population={args.population} entry_mode={args.entry_mode} "
        f"positions={len(positions)} cost_per_sol={cost_per_sol:.4f}\n",
        file=sys.stderr,
    )

    rows = []
    for name, overrides in build_grid():
        res = run_config(positions, overrides, args.binary)
        m = metrics(res, positions, cost_per_sol)
        rows.append((name, overrides, m))

    rows.sort(key=lambda x: x[2]["sum_cost"], reverse=True)
    print(f"{'config':<14} {'n':>5} {'win%':>6} {'sum_raw':>9} {'sum_cost':>10} {'mean':>7} {'max_loss':>9}  reasons")
    for name, ov, m in rows:
        print(
            f"{name:<14} {m['n']:>5} {m['win_pct']:>5.1f}% {m['sum_raw']:>9.1f} "
            f"{m['sum_cost']:>10.1f} {m['mean']:>7.3f} {m['max_loss']:>9.1f}  {m['reasons']}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
