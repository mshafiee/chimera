"""CLI: python -m analysis.copy_backtest_cli {exit|gate|strategy|gap|skew|path|replay} [opts]

Runs the repeatable copy-engine backtest over shadow history (Phase 1) and the
price-path replay harness (Phase 2C/2D).

Commands:
  exit      metric table per exit_strategy (cost-adjusted)
  gate      metric table per entry gate under a given exit strategy
  strategy  metric table split by Shield/Spear
  gap       predicted (shadow mirror_main) vs realized (closed trades) win rate
  skew      realized live-vs-mark sell fill skew + defer-trigger bands
  mark      summarize recorded per-position price marks (position_price_marks)
  reconcile  per-trade shadow mirror_main vs realized price gap (Phase 2F)
  screen     post-cost per-wallet entry screen (Phase 2G — clear the cost floor)
  path      reconstruct+store on-chain price paths for up to --limit shadow tokens
  replay    run the Rust replay_exit binary over stored paths (--limit)

Options:
  --cost X       override cost-per-SOL (default: observed from trades)
  --exit STRAT   exit strategy for gate/strategy reports (default mirror_main)
  --limit N      cap positions/tokens for path|replay (default 100)
  --binary PATH  path to replay_exit binary (default from env REPLAY_EXIT_BIN)
  --since H      scope every report to trades opened in the last H hours
                 (e.g. --since 48 to backtest the last 48h of trades)
 """

import asyncio
import json
import os
import sys
from decimal import Decimal

from core.copy_backtest import CopyBacktest, format_report
from core.db import execute_and_fetchall
from core.replay_harness import load_stored_paths, replay_input_json, run_replay

_HELP = (
    "usage: python -m analysis.copy_backtest_cli "
    "{exit|gate|strategy|gap|skew|path|replay} [--cost X] [--exit STRAT] [--limit N] [--binary PATH]"
)


def _opt(args: list, key: str):
    """Consume `--key <val>` returning value or None."""
    for i in range(len(args) - 1):
        if args[i] == key:
            return args[i + 1]
    return None


def _cmd_build_replay_limit(limit: int) -> list:
    # Bounded set of shadow_position tokens that already have stored paths.
    rows = execute_and_fetchall(
        "SELECT sp.token_address, "
        "       COALESCE(sp.entry_price_usd,0) AS entry_price_usd, "
        "       COALESCE(sp.strategy,'SHIELD') AS strategy, "
        "       EXTRACT(EPOCH FROM sp.opened_at)::bigint AS opened_at, "
        "       sp.entry_amount_sol "
        "FROM shadow_positions sp "
        "JOIN (SELECT token_address FROM price_path_points GROUP BY token_address "
        "      HAVING COUNT(*) >= 2) pp USING (token_address) "
        "WHERE NOT COALESCE(sp.main_admitted, FALSE) "
        "LIMIT %s",
        (limit,),
    )

    positions = []
    for r in rows:
        # Replay only from the position's own open time; shadow positions often
        # lack a recorded USD entry price, so anchor entry to the reconstructed
        # path's first in-window price (the standard relative-replay approach).
        opened = int(r["opened_at"])
        pts = [(ts, p) for ts, p in load_stored_paths(r["token_address"]) if ts >= opened]
        if len(pts) < 2:
            continue
        entry = pts[0][1]
        positions.append(
            {
                "entry_price": entry,
                "opened_at": opened,
                "strategy": r["strategy"],
                "size_sol": r["entry_amount_sol"] or "1.0",
                "points": pts,
            }
        )
        if len(positions) >= limit:
            break
    return positions


def main(argv: list[str]) -> int:
    if not argv or argv[0] in {"-h", "--help"}:
        print(_HELP, file=sys.stderr)
        return 2

    cmd = argv[0]
    rest = argv[1:]
    cost_s = _opt(rest, "--cost")
    exit_strat = _opt(rest, "--exit") or "mirror_main"
    limit_s = _opt(rest, "--limit")
    binary = _opt(rest, "--binary") or os.getenv("REPLAY_EXIT_BIN")
    since_s = _opt(rest, "--since")

    limit = int(limit_s) if limit_s else 100
    cost = Decimal(cost_s) if cost_s else None
    since = int(since_s) if since_s else None

    bt = CopyBacktest(cost_per_sol=cost, since_hours=since)
    if cmd == "exit":
        print(format_report("per-exit-strategy (cost-adjusted)", bt.per_exit_strategy()))
    elif cmd == "gate":
        print(format_report(f"per-gate under {exit_strat} (cost-adjusted)", bt.per_gate(exit_strat)))
    elif cmd == "strategy":
        print(format_report(f"by-strategy under {exit_strat} (cost-adjusted)", bt.by_strategy(exit_strat)))
    elif cmd == "gap":
        print(bt.realize_vs_price_gap())
    elif cmd == "skew":
        print(bt.fill_skew_report())
    elif cmd == "mark":
        print(bt.mark_gap_report())
    elif cmd == "reconcile":
        print(bt.reconcile_shadow_realized())
    elif cmd == "screen":
        print(bt.cost_aware_screen())
    elif cmd == "path":
        return _cmd_path(limit)
    elif cmd == "replay":
        if not binary:
            print("replay requires --binary PATH (or REPLAY_EXIT_BIN)", file=sys.stderr)
            return 2
        return _cmd_replay(limit, binary)
    else:
        print(_HELP, file=sys.stderr)
        return 2
    return 0


def _cmd_path(limit: int) -> int:
    from core.price_path import birdeye_ohlcv, geckoterminal_ohlcv
    from core.replay_harness import persist_paths

    use_birdeye = bool(os.getenv("BIRDEYE_API_KEY"))
    # Birdeye keyed plan caps at ~60 rpm (1 req/sec); GeckoTerminal free tier
    # is paced to ~4/sec. The Birdeye client also enforces a 1.0s inter-request
    # delay, so a 1.0s inter-token sleep keeps us safely under the limit.
    inter_token_sleep = 1.0 if use_birdeye else 0.20

    tokens = execute_and_fetchall(
        "SELECT DISTINCT token_address, "
        "       EXTRACT(EPOCH FROM MIN(opened_at))::bigint AS tf, "
        "       EXTRACT(EPOCH FROM MAX(COALESCE(closed_at, NOW())))::bigint AS tt "
        "FROM shadow_positions WHERE token_address IS NOT NULL "
        "AND token_address NOT IN (SELECT token_address FROM price_path_points) "
        "GROUP BY token_address LIMIT %s",
        (limit,),
    )

    async def _reconstruct() -> dict:
        paths = {}
        for r in tokens:
            addr = r["token_address"]
            tf = int(r["tf"])
            tt = int(r["tt"])
            try:
                if use_birdeye:
                    pts = await birdeye_ohlcv(addr, tf, tt)
                else:
                    pts = await geckoterminal_ohlcv(addr, timeframe="hour")
            except Exception as e:  # noqa: BLE001
                print(f"ERROR: path {addr}: {e}")
                continue
            # keep only the position's window
            in_window = [(ts, p) for ts, p in pts if tf <= ts <= tt]
            if in_window:
                paths[addr] = in_window
            await asyncio.sleep(inter_token_sleep)
        return paths

    src = "birdeye" if use_birdeye else "geckoterminal"
    paths = asyncio.run(_reconstruct())
    n = persist_paths(paths)
    print(f"[path:{src}] reconstructed {len(paths)} tokens / {n} points persisted")
    return 0


def _cmd_replay(limit: int, binary: str) -> int:
    positions = _cmd_build_replay_limit(limit)
    if not positions:
        print("[replay] no positions with stored paths (run `path` first)", file=sys.stderr)
        return 1
    replay_json = replay_input_json(positions)
    try:
        out = run_replay(replay_json, binary)
    except Exception as e:  # noqa: BLE001
        print(f"[replay] failed: {e}", file=sys.stderr)
        return 1
    n = len(out.get("results", []))
    wins = sum(1 for r in out.get("results", []) if float(r["pnl_sol"]) > 0)
    total = sum(Decimal(r["pnl_sol"]) for r in out.get("results", []))
    print(f"[replay] {n} replayed, {wins} wins, total pnl {total:.2f} SOL")
    print(json.dumps(out, indent=2, default=str))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

