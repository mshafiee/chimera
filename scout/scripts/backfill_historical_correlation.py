#!/usr/bin/env python3
"""
One-time backfill: populate wqs_pnl_correlation with historical wallet data.

Reads all ACTIVE/CANDIDATE wallets that have realized_pnl_30d_sol from the
wallets table (populated by the Operator) and inserts them as correlation
records so the adaptive weight calibrator and prediction matcher have
immediate data to work with — no need to wait 30 days for the feed to
accumulate organically.

Usage:
    python scripts/backfill_historical_correlation.py
    python scripts/backfill_historical_correlation.py --db-path /path/to/chimera.db
"""

import argparse
import os
import sqlite3
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(
        description="Backfill historical PnL into wqs_pnl_correlation"
    )
    parser.add_argument("--db-path", default=None, help="Path to chimera.db")
    args = parser.parse_args()

    db_path = args.db_path or os.getenv("CHIMERA_DB_PATH", "../data/chimera.db")
    if not Path(db_path).exists():
        print(f"Database not found: {db_path}")
        return

    conn = sqlite3.connect(db_path)
    conn.row_factory = sqlite3.Row

    # Ensure the correlation table exists
    conn.execute("""
        CREATE TABLE IF NOT EXISTS wqs_pnl_correlation (
            wallet_address TEXT PRIMARY KEY,
            wqs_score_at_promotion REAL NOT NULL,
            actual_copy_pnl_7d_sol TEXT,
            actual_copy_pnl_30d_sol TEXT,
            actual_copy_pnl_all_sol TEXT,
            copy_trade_count_7d INTEGER DEFAULT 0,
            copy_trade_count_30d INTEGER DEFAULT 0,
            copy_trade_count_all INTEGER DEFAULT 0,
            strategy TEXT NOT NULL DEFAULT 'SHIELD',
            wqs_components_json TEXT,
            promoted_at TEXT NOT NULL,
            last_updated_at TEXT NOT NULL
        )
    """)

    # Read wallets with PnL that haven't been backfilled yet
    rows = conn.execute("""
        SELECT w.address, w.wqs_score, w.realized_pnl_30d_sol,
               w.trade_count_30d, w.win_rate, w.profit_factor,
               w.promoted_at, w.archetype
        FROM wallets w
        LEFT JOIN wqs_pnl_correlation c ON w.address = c.wallet_address
        WHERE w.realized_pnl_30d_sol IS NOT NULL
          AND w.promoted_at IS NOT NULL
          AND (c.wallet_address IS NULL
               OR c.actual_copy_pnl_30d_sol IS NULL)
    """).fetchall()

    if not rows:
        print("No new wallets with PnL data to backfill.")
        return

    now = __import__('datetime').datetime.utcnow().isoformat()
    inserted = 0
    for r in rows:
        conn.execute("""
            INSERT OR REPLACE INTO wqs_pnl_correlation
            (wallet_address, wqs_score_at_promotion,
             actual_copy_pnl_30d_sol, copy_trade_count_30d,
             strategy, promoted_at, last_updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?)
        """, (
            r["address"], r["wqs_score"],
            r["realized_pnl_30d_sol"], r["trade_count_30d"] or 0,
            "SHIELD", r["promoted_at"], now,
        ))
        inserted += 1

    conn.commit()
    conn.close()

    print(f"Backfilled {inserted} historical correlation records from wallets table.")
    print(f"Total records now in wqs_pnl_correlation: ", end="")
    conn = sqlite3.connect(db_path)
    total = conn.execute("SELECT COUNT(*) FROM wqs_pnl_correlation").fetchone()[0]
    conn.close()
    print(total)


if __name__ == "__main__":
    main()
