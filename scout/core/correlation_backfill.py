"""
Database backfill: bridges closed Chimera trades with wqs_pnl_correlation
(Scout) and writes append-only promotion_episodes (C2).

The backfill computes ACTUAL copy PnL from the Operator's `trades` table
(not the source wallet's realized_pnl in `wallets`, which was circular — it
measured the source wallet's own skill, not Chimera's copy performance).

Uses the db.py abstraction (Connection / execute_query / execute_update) to
run against the configured PostgreSQL backend.
"""

import os
import traceback
from datetime import timedelta

from .db import Connection, execute_query, execute_update
from .utils import utcnow


def backfill_correlation_pnl(db_path: str) -> int:
    """
    Backfill actual copy PnL from Chimera `trades` into `wqs_pnl_correlation`.

    For any correlation record promoted >=7 days ago that still has NULL
    actual_copy_pnl_30d_sol, computes copy PnL from closed Chimera trades:

    - actual_copy_pnl_7d_sol  (last 7 days of closed SELLs)
    - actual_copy_pnl_30d_sol (last 30 days)
    - actual_copy_pnl_all_sol (all time)
    - copy_trade_count_7d, copy_trade_count_30d, copy_trade_count_all

    Returns the number of records updated.
    """
    updated = 0
    try:
        with Connection(db_path) as conn:
            cutoff = (utcnow() - timedelta(days=7)).isoformat()
            cursor = execute_query(
                conn,
                """SELECT c.wallet_address
                   FROM wqs_pnl_correlation c
                   WHERE c.actual_copy_pnl_30d_sol IS NULL
                     AND c.promoted_at < %s""",
                (cutoff,),
            )
            rows = cursor.fetchall()
            if not rows:
                return 0

            # Batch-compute copy PnL from Chimera trades for all flagged wallets.
            # psycopg3 (unlike psycopg2) does NOT expand a tuple in `IN %s` — it
            # binds it as a single parameter, producing invalid `IN $1`. Use the
            # array-any form with a Python LIST, which psycopg3 adapts to a
            # Postgres array (works for one or many addresses). Matches the
            # `= ANY($1)` pattern used in the Rust operators.
            addresses = [r["wallet_address"] for r in rows]

            pnl_cursor = execute_query(
                conn,
                """SELECT
                       t.wallet_address,
                       SUM(t.net_pnl_sol) FILTER (WHERE t.created_at >= NOW() - INTERVAL '7 days') AS copy_pnl_7d,
                       SUM(t.net_pnl_sol) FILTER (WHERE t.created_at >= NOW() - INTERVAL '30 days') AS copy_pnl_30d,
                       SUM(t.net_pnl_sol) AS copy_pnl_all,
                       COUNT(*) FILTER (WHERE t.created_at >= NOW() - INTERVAL '7 days') AS count_7d,
                       COUNT(*) FILTER (WHERE t.created_at >= NOW() - INTERVAL '30 days') AS count_30d,
                       COUNT(*) AS count_all
                   FROM trades t
                   WHERE t.status = 'CLOSED'
                     AND t.pnl_data_valid = TRUE
                     AND t.side = 'SELL'
                       AND t.wallet_address = ANY(%s)
                   GROUP BY t.wallet_address""",
                (addresses,),
            )
            pnl_rows = {r["wallet_address"]: r for r in pnl_cursor.fetchall()}

            for row in rows:
                addr = row["wallet_address"]
                pnl = pnl_rows.get(addr)
                if pnl is None:
                    # No closed copy-trades yet — skip; correlation keeps NULL.
                    continue
                if pnl["copy_pnl_30d"] is None and pnl["copy_pnl_all"] is None:
                    continue
                execute_query(
                    conn,
                    """UPDATE wqs_pnl_correlation
                       SET actual_copy_pnl_7d_sol = %s,
                           actual_copy_pnl_30d_sol = %s,
                           actual_copy_pnl_all_sol = %s,
                           copy_trade_count_7d = %s,
                           copy_trade_count_30d = %s,
                           copy_trade_count_all = %s,
                           last_updated_at = %s
                       WHERE wallet_address = %s""",
                    (
                        pnl["copy_pnl_7d"] if pnl["copy_pnl_7d"] is not None else 0,
                        pnl["copy_pnl_30d"] if pnl["copy_pnl_30d"] is not None else 0,
                        pnl["copy_pnl_all"] if pnl["copy_pnl_all"] is not None else 0,
                        pnl["count_7d"] or 0,
                        pnl["count_30d"] or 0,
                        pnl["count_all"] or 0,
                        utcnow().isoformat(),
                        addr,
                    ),
                )
                updated += 1
        if updated:
            print(f"[Scout] Backfilled PnL for {updated} wallets (from trades)")
    except Exception as e:
        # The Connection context manager wraps the batch in an explicit
        # transaction: nothing is committed on failure, so surface the error
        # instead of returning a misleading partial count.
        print(f"[Scout] PnL backfill failed: {e}")
        traceback.print_exc()
        raise
    return updated


def write_correlation_record(
    wallet_address: str,
    wqs_score: float,
    components_json_str: str,
    strategy: str,
) -> None:
    """
    Insert into wqs_pnl_correlation table in the MAIN database.

    Insert-only with `ON CONFLICT DO NOTHING`: if a row already exists for the
    wallet, the first promotion record is preserved. The table keeps the FIRST
    promotion snapshot; subsequent re-evaluations append to promotion_episodes
    instead of overwriting this row.
    """
    now = utcnow().isoformat()
    try:
        execute_update(
            """INSERT INTO wqs_pnl_correlation
               (wallet_address, wqs_score_at_promotion, wqs_components_json,
                promoted_at, strategy, last_updated_at)
               VALUES (%s, %s, %s, %s, %s, %s)
               ON CONFLICT (wallet_address) DO NOTHING""",
            (wallet_address, wqs_score, components_json_str, now, strategy, now),
        )
    except Exception as e:
        print(f"[Scout] Failed to write correlation record: {e}")
        traceback.print_exc()


def write_promotion_episode(
    wallet_address: str,
    wqs: float,
    wqs_confidence: float | None,
    components_json: str | None,
    decision: str = "promoted",
) -> None:
    """
    Append an immutable promotion episode (C2).

    Called from the promotion flow in main.py Step 3c. Every promotion or
    shadow decision inserts a row into promotion_episodes (never updates), so
    WQS-to-PnL feedback is honest and re-evaluation never erases the original
    promotion-time features.
    """
    policy_version = os.getenv("SCOUT_PROMOTION_POLICY_VERSION", "default")
    code_revision = os.getenv("GIT_HASH", "unknown")
    try:
        execute_update(
            """INSERT INTO promotion_episodes
               (wallet_address, promoted_at, wqs, wqs_confidence,
                components_json, decision, policy_version, code_revision)
               VALUES (%s, NOW(), %s, %s, %s, %s, %s, %s)""",
            (wallet_address, wqs, wqs_confidence, components_json, decision, policy_version, code_revision),
        )
    except Exception as e:
        print(f"[Scout] Failed to write promotion episode: {e}")
        traceback.print_exc()
