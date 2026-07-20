"""
Database backfill: bridges wallets table (Operator) with wqs_pnl_correlation (Scout).

The backfill reads realized PnL from the wallets table and writes it into
the correlation table so adaptive weights calibration can compute
WQS-to-PnL correlations.

Uses the db.py abstraction (Connection / execute_query / execute_update) to
run against the configured PostgreSQL backend.
"""

from datetime import timedelta

from .db import Connection, execute_query, execute_update
from .utils import utcnow


def backfill_correlation_pnl(db_path: str) -> int:
    """
    Backfill actual copy PnL from wallets table into wqs_pnl_correlation.

    The Operator writes realized_pnl_30d_sol to the wallets table but never
    writes actual_copy_pnl_* to wqs_pnl_correlation. This function bridges
    that gap: for any correlation record promoted >=7 days ago that still
    has NULL PnL, it reads the wallets table and updates the correlation row.

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
            for row in rows:
                addr = row["wallet_address"]
                w_cursor = execute_query(
                    conn,
                    """SELECT realized_pnl_30d_sol, trade_count_30d
                       FROM wallets WHERE address = %s""",
                    (addr,),
                )
                w = w_cursor.fetchone()
                if w is None:
                    continue
                realized_pnl = w["realized_pnl_30d_sol"]
                trade_count = w["trade_count_30d"]
                if realized_pnl is not None:
                    execute_query(
                        conn,
                        """UPDATE wqs_pnl_correlation
                           SET actual_copy_pnl_30d_sol = %s,
                               copy_trade_count_30d = %s,
                               last_updated_at = %s
                           WHERE wallet_address = %s""",
                        (realized_pnl, trade_count or 0, utcnow().isoformat(), addr),
                    )
                    updated += 1
            # Connection context manager commits on clean exit
        if updated:
            print(f"[Scout] Backfilled PnL for {updated} wallets")
    except Exception as e:
        print(f"[Scout] PnL backfill skipped: {e}")
    return updated


def write_correlation_record(
    wallet_address: str,
    wqs_score: float,
    components_json_str: str,
    strategy: str,
) -> None:
    """
    Upsert into wqs_pnl_correlation table in the MAIN database.

    Writes the fields the Scout owns: wallet_address, wqs_score_at_promotion,
    wqs_components_json, promoted_at, strategy, last_updated_at.
    Actual PnL fields (actual_copy_pnl_*) are backfilled later by backfill_correlation_pnl().
    """
    now = utcnow().isoformat()
    try:
        execute_update(
            """INSERT INTO wqs_pnl_correlation
               (wallet_address, wqs_score_at_promotion, wqs_components_json,
                promoted_at, strategy, last_updated_at)
               VALUES (%s, %s, %s, %s, %s, %s)
               ON CONFLICT (wallet_address) DO UPDATE SET
                   wqs_score_at_promotion = EXCLUDED.wqs_score_at_promotion,
                   wqs_components_json = EXCLUDED.wqs_components_json,
                   promoted_at = EXCLUDED.promoted_at,
                   strategy = EXCLUDED.strategy,
                   last_updated_at = EXCLUDED.last_updated_at""",
            (wallet_address, wqs_score, components_json_str, now, strategy, now),
        )
    except Exception as e:
        print(f"[Scout] Failed to write correlation record: {e}")
