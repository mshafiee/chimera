"""Admission/trade-rate validation tracker (Phase 2I).

The data-backed position is that NO further admission relax is justified: the
live selection config already waives the WQS floor and token-age gate for
proven wallets (t-stat > 1.645 / shadow-proven / consensus-or-proven), and the
per-gate backtest shows every bucket still rejected is net-negative (e.g.
SHADOW_MIRROR_INSUFFICIENT -4.7 SOL; the unproven single-wallet class is the
negative-EV population by design).

So the only volume lever with evidence is the ROSTER (net-positive wallets
added to the copy set). This tool validates that change on data: it snapshots
the gate buckets, predicted-vs-realized gap, trade rate and roster activity so
a before/after comparison proves the roster change lifted rate WITHOUT
admitting net-negative buckets.

Usage:
  python -m analysis.admission_tracker --save before.json     # baseline now
  python -m analysis.admission_tracker --save after.json      # after the window
  python -m analysis.admission_tracker --diff before.json after.json
"""

from __future__ import annotations

import argparse
import json
import sys
from datetime import datetime, timezone

from core.db import execute_and_fetchall
from core.copy_backtest import CopyBacktest

WATCH_GATES = (
    "ADMITTED",
    "TOKEN_TOO_NEW",
    "WQS_TOO_LOW",
    "SHADOW_MIRROR_INSUFFICIENT",
    "SHADOW_MIRROR_NEGATIVE",
    "WALLET_MUTED",
    "NON_SPECULATIVE_TOKEN",
    "SIGNAL_QUALITY_TOO_LOW",
)


def build_snapshot() -> dict:
    bt = CopyBacktest()
    gate = {}
    for r in bt.per_gate("mirror_main"):
        if r.group in WATCH_GATES:
            gate[r.group] = {
                "n": r.n,
                "sum_pnl": float(r.sum_pnl),
                "win_pct": round(r.win_rate * 100, 1),
            }
    gap = bt.realize_vs_price_gap()
    trades = execute_and_fetchall(
        "SELECT date_trunc('day', created_at)::date d, COUNT(*) AS n "
        "FROM trades WHERE created_at > NOW() - interval '14 days' "
        "GROUP BY d ORDER BY d"
    )
    active = execute_and_fetchall(
        "SELECT COUNT(*) AS n FROM wallets WHERE status = 'ACTIVE'"
    )[0]["n"]
    signaling = execute_and_fetchall(
        "SELECT COUNT(DISTINCT sp.wallet_address) AS n "
        "FROM shadow_positions sp JOIN wallets w ON w.address = sp.wallet_address "
        "WHERE w.status = 'ACTIVE' AND sp.opened_at > NOW() - interval '7 days'"
    )[0]["n"]
    return {
        "taken_at": datetime.now(timezone.utc).isoformat(),
        "gate": gate,
        "gap": {
            "predicted_win_pct": round(gap.get("predicted_win_rate", 0) * 100, 1),
            "realized_win_pct": round(gap.get("realized_win_rate", 0) * 100, 1),
            "predicted_n": gap.get("predicted_n", 0),
            "realized_n": gap.get("realized_n", 0),
        },
        "trades_per_day_last_14d": [{"d": str(r["d"]), "n": r["n"]} for r in trades],
        "active_roster": int(active),
        "signaling_wallets_7d": int(signaling),
    }


def diff_snapshots(before: dict, after: dict) -> str:
    lines = [f"before={before['taken_at']}  after={after['taken_at']}"]
    lines.append("--- gate buckets (n / sum_pnl, cost-adjusted) ---")
    keys = sorted(set(before["gate"]) | set(after["gate"]))
    for k in keys:
        b = before["gate"].get(k, {"n": 0, "sum_pnl": 0.0})
        a = after["gate"].get(k, {"n": 0, "sum_pnl": 0.0})
        lines.append(
            f"  {k:<26} n {b['n']:>6}->{a['n']:<6} sum {b['sum_pnl']:>9.2f}->{a['sum_pnl']:>9.2f}"
        )
    lines.append("--- gap ---")
    for k in ("predicted_win_pct", "realized_win_pct", "predicted_n", "realized_n"):
        lines.append(
            f"  {k:<20} {before['gap'].get(k, 0)} -> {after['gap'].get(k, 0)}"
        )
    lines.append("--- trade rate / roster ---")
    bt_n = sum(r["n"] for r in before.get("trades_per_day_last_14d", []))
    at_n = sum(r["n"] for r in after.get("trades_per_day_last_14d", []))
    lines.append(f"  trades_total_14d        {bt_n} -> {at_n}")
    lines.append(
        f"  active_roster           {before.get('active_roster', 0)} -> {after.get('active_roster', 0)}"
    )
    lines.append(
        f"  signaling_wallets_7d    {before.get('signaling_wallets_7d', 0)} -> {after.get('signaling_wallets_7d', 0)}"
    )
    return "\n".join(lines)


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description="Admission/trade-rate validation tracker")
    ap.add_argument("--save", metavar="PATH", help="write a snapshot JSON to PATH")
    ap.add_argument("--diff", nargs=2, metavar=("BEFORE", "AFTER"), help="diff two snapshots")
    args = ap.parse_args(argv)

    if args.save:
        snap = build_snapshot()
        with open(args.save, "w") as fh:
            json.dump(snap, fh, indent=2)
        print(json.dumps(snap, indent=2))
        print(f"[admission_tracker] saved {args.save}", file=sys.stderr)
        return 0
    if args.diff:
        with open(args.diff[0]) as fh:
            before = json.load(fh)
        with open(args.diff[1]) as fh:
            after = json.load(fh)
        print(diff_snapshots(before, after))
        return 0
    print(build_snapshot())
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))