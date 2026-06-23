"""
Database backfill: bridges wallets table (Operator) with wqs_pnl_correlation (Scout).

The backfill reads realized PnL from the wallets table and writes it into
the correlation table so adaptive weights calibration can compute
WQS-to-PnL correlations.
"""

import os
import sqlite3

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
    conn = None
    try:
        conn = sqlite3.connect(db_path, timeout=10.0)
        conn.row_factory = sqlite3.Row
        rows = conn.execute(
            """SELECT c.wallet_address, c.promoted_at
               FROM wqs_pnl_correlation c
               WHERE c.actual_copy_pnl_30d_sol IS NULL
                 AND c.promoted_at < datetime('now', '-7 days')"""
        ).fetchall()
        if not rows:
            return 0
        for row in rows:
            addr = row["wallet_address"]
            w = conn.execute(
                """SELECT realized_pnl_30d_sol, trade_count_30d
                   FROM wallets WHERE address = ?""",
                (addr,)
            ).fetchone()
            if w is None:
                continue
            realized_pnl = w["realized_pnl_30d_sol"]
            trade_count = w["trade_count_30d"]
            if realized_pnl is not None:
                conn.execute(
                    """UPDATE wqs_pnl_correlation
                       SET actual_copy_pnl_30d_sol = ?,
                           copy_trade_count_30d = ?,
                           last_updated_at = ?
                       WHERE wallet_address = ?""",
                    (realized_pnl, trade_count or 0, utcnow().isoformat(), addr),
                )
                updated += 1
        conn.commit()
        if updated:
            print(f"[Scout] Backfilled PnL for {updated} wallets")
    except sqlite3.OperationalError as e:
        print(f"[Scout] PnL backfill skipped: {e}")
    finally:
        if conn:
            conn.close()
    return updated


def write_correlation_record(
    wallet_address: str,
    wqs_score: float,
    components_json_str: str,
    strategy: str,
) -> None:
    """
    INSERT OR REPLACE into wqs_pnl_correlation table in the MAIN database.

    Writes the fields the Scout owns: wallet_address, wqs_score_at_promotion,
    wqs_components_json, promoted_at, strategy, last_updated_at.
    Actual PnL fields (actual_copy_pnl_*) are backfilled later by backfill_correlation_pnl().
    """
    db_path = os.getenv("CHIMERA_DB_PATH", "../data/chimera.db")
    conn = None
    try:
        conn = sqlite3.connect(db_path, timeout=10.0)
        conn.execute("PRAGMA journal_mode=WAL;")
        now = utcnow().isoformat()
        conn.execute(
            """INSERT OR REPLACE INTO wqs_pnl_correlation
               (wallet_address, wqs_score_at_promotion, wqs_components_json,
                promoted_at, strategy, last_updated_at)
               VALUES (?, ?, ?, ?, ?, ?)""",
            (wallet_address, wqs_score, components_json_str, now, strategy, now),
        )
        conn.commit()
    except sqlite3.OperationalError as e:
        print(f"[Scout] Failed to write correlation record: {e}")
    finally:
        if conn:
            conn.close()
