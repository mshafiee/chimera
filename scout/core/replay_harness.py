"""
Replay harness orchestration (Phase 2C/2D).

Ties the pieces together: persist reconstructed price paths into
`price_path_points`, build the JSON input the Rust `replay_exit` binary
consumes (entry + opened_at + strategy + points per position), and invoke it.

The heavy external step (Helius pagination) lives in `price_path`; this module
is thin glue. DB access uses `scout.core.db`.
"""

from __future__ import annotations

import json
import subprocess
from decimal import Decimal
from typing import Dict, List, Sequence, Tuple

from .db import execute_and_fetchall, execute_update
from .price_path import reconstruct_price_path


def persist_paths(paths: Dict[str, List[Tuple[int, Decimal]]]) -> int:
    """Insert reconstructed paths keyed by token into `price_path_points`."""
    n = 0
    for token, points in paths.items():
        for ts, price in points:
            try:
                execute_update(
                    "INSERT INTO price_path_points (token_address, ts_unix, payable_sol) "
                    "VALUES (%s, %s, %s) ON CONFLICT (token_address, ts_unix) DO NOTHING",
                    (token, ts, str(price)),
                )
                n += 1
            except Exception as e:  # noqa: BLE001 - keep going on per-row errors
                print(f"[replay-harness] persist failed for {token}@{ts}: {e}")
    return n


def load_stored_paths(token_address: str) -> List[Tuple[int, Decimal]]:
    rows = execute_and_fetchall(
        "SELECT ts_unix, payable_sol FROM price_path_points "
        "WHERE token_address = %s ORDER BY ts_unix ASC",
        (token_address,),
    )
    return [(int(r["ts_unix"]), Decimal(str(r["payable_sol"]))) for r in rows]


def load_stored_marks(trade_uuid: str) -> List[Tuple[int, Decimal]]:
    """Load a real position's recorded price-cache USD marks.

    Backs the real-source replay: `position_price_marks` (migration 0021) is
    the monitor's recorded per-tick mark, ordered by ts."""
    rows = execute_and_fetchall(
        "SELECT ts_unix, price_usd FROM position_price_marks "
        "WHERE trade_uuid = %s ORDER BY ts_unix ASC",
        (trade_uuid,),
    )
    return [(int(r["ts_unix"]), Decimal(str(r["price_usd"]))) for r in rows]


def replay_input_json(positions: Sequence[dict]) -> dict:
    """Wrap a list of position dicts into the Rust replay_exit Input shape.

    Each position dict: {entry_price, opened_at, strategy, size_sol, points},
    where points is a list of (ts_unix, payable_sol) tuples.
    """
    out = []
    for p in positions:
        out.append(
            {
                "entry_price": str(p["entry_price"]),
                "opened_at": int(p["opened_at"]),
                "strategy": p.get("strategy", "SHIELD"),
                "size_sol": str(p.get("size_sol", "1.0")),
                "points": [[ts, str(price)] for ts, price in p["points"]],
            }
        )
    return {"overrides": {}, "positions": out}


async def reconstruct_and_store(
    helius,
    tokens: Dict[str, Tuple[int, int]],
) -> Dict[str, List[Tuple[int, Decimal]]]:
    """Reconstruct price paths for {token: (time_from, time_to)} windows and
    persist them. Returns the reconstructed paths dict."""
    all_paths: Dict[str, List[Tuple[int, Decimal]]] = {}
    for token, (tf, tt) in tokens.items():
        try:
            pts = await reconstruct_price_path(helius, token, tf, tt)
        except Exception as e:  # noqa: BLE001
            print(f"[replay-harness] reconstruct failed for {token}: {e}")
            continue
        if pts:
            all_paths[token] = pts
    persist_paths(all_paths)
    return all_paths


def run_replay(replay_json: dict, binary: str) -> Dict:
    """Pipe JSON to the Rust replay_exit binary and parse its stdout."""
    proc = subprocess.run(
        [binary, "--input", "-"],
        input=json.dumps(replay_json),
        text=True,
        capture_output=True,
        timeout=300,
        check=False,
    )
    if proc.returncode != 0:
        raise RuntimeError(f"replay_exit failed rc={proc.returncode}: {proc.stderr[-500:]}")
    return json.loads(proc.stdout)
