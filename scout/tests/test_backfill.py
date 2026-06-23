"""
Tests for _backfill_correlation_pnl in main.py.

Validates that the backfill function correctly bridges the gap between the
wallets table (populated by the Operator) and wqs_pnl_correlation (read by
the Scout for adaptive weight calibration).
"""

import os
import sqlite3
import sys
import tempfile
import unittest
from datetime import datetime, timedelta
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent.parent.parent))

from scout.main import _backfill_correlation_pnl


class TestBackfillCorrelationPnl(unittest.TestCase):

    def setUp(self):
        """Create a temporary SQLite database with both tables."""
        self.temp_fd, self.db_path = tempfile.mkstemp(suffix='.db')
        conn = sqlite3.connect(self.db_path)

        conn.execute("""
            CREATE TABLE wallets (
                address TEXT PRIMARY KEY,
                status TEXT DEFAULT 'CANDIDATE',
                realized_pnl_30d_sol REAL,
                trade_count_30d INTEGER
            )
        """)
        conn.execute("""
            CREATE TABLE wqs_pnl_correlation (
                wallet_address TEXT PRIMARY KEY,
                wqs_score_at_promotion REAL NOT NULL,
                actual_copy_pnl_7d_sol TEXT,
                actual_copy_pnl_30d_sol TEXT,
                actual_copy_pnl_all_sol TEXT,
                copy_trade_count_7d INTEGER DEFAULT 0,
                copy_trade_count_30d INTEGER DEFAULT 0,
                strategy TEXT DEFAULT 'SHIELD',
                wqs_components_json TEXT,
                promoted_at TEXT NOT NULL,
                last_updated_at TEXT NOT NULL
            )
        """)
        conn.commit()
        conn.close()

    def tearDown(self):
        os.close(self.temp_fd)
        os.unlink(self.db_path)

    def _insert_wallet(self, conn, address, realized_pnl=0.5, trade_count=10):
        conn.execute(
            "INSERT INTO wallets (address, realized_pnl_30d_sol, trade_count_30d) "
            "VALUES (?, ?, ?)",
            (address, realized_pnl, trade_count),
        )

    def _insert_correlation(self, conn, address, days_ago=8):
        timestamp = (datetime.utcnow() - timedelta(days=days_ago)).isoformat()
        conn.execute(
            "INSERT INTO wqs_pnl_correlation "
            "(wallet_address, wqs_score_at_promotion, promoted_at, last_updated_at, strategy) "
            "VALUES (?, ?, ?, ?, ?)",
            (address, 75.0, timestamp, timestamp, "SHIELD"),
        )

    def test_backfill_populates_null_pnl(self):
        """Happy path: wallet with PnL gets backfilled."""
        conn = sqlite3.connect(self.db_path)
        self._insert_wallet(conn, "wallet_abc", realized_pnl=0.5, trade_count=10)
        self._insert_correlation(conn, "wallet_abc", days_ago=8)
        conn.commit()
        conn.close()

        updated = _backfill_correlation_pnl(self.db_path)

        self.assertEqual(updated, 1)

        conn = sqlite3.connect(self.db_path)
        conn.row_factory = sqlite3.Row
        row = conn.execute(
            "SELECT actual_copy_pnl_30d_sol, copy_trade_count_30d "
            "FROM wqs_pnl_correlation WHERE wallet_address = ?",
            ("wallet_abc",),
        ).fetchone()
        conn.close()

        self.assertIsNotNone(row)
        self.assertAlmostEqual(float(row["actual_copy_pnl_30d_sol"]), 0.5)
        self.assertEqual(row["copy_trade_count_30d"], 10)

    def test_backfill_skips_recently_promoted(self):
        """Wallets promoted <7 days ago are not backfilled."""
        conn = sqlite3.connect(self.db_path)
        self._insert_wallet(conn, "wallet_xyz", realized_pnl=0.3, trade_count=5)
        self._insert_correlation(conn, "wallet_xyz", days_ago=2)
        conn.commit()
        conn.close()

        updated = _backfill_correlation_pnl(self.db_path)

        self.assertEqual(updated, 0)

    def test_backfill_skips_wallet_not_in_wallets_table(self):
        """Correlation record with no matching wallet address is skipped."""
        conn = sqlite3.connect(self.db_path)
        self._insert_correlation(conn, "orphan_wallet", days_ago=10)
        conn.commit()
        conn.close()

        updated = _backfill_correlation_pnl(self.db_path)

        self.assertEqual(updated, 0)

    def test_backfill_skips_null_pnl_in_wallets(self):
        """Wallet exists but realized_pnl_30d_sol is NULL — skip."""
        conn = sqlite3.connect(self.db_path)
        self._insert_wallet(conn, "wallet_null_pnl", realized_pnl=None, trade_count=0)
        self._insert_correlation(conn, "wallet_null_pnl", days_ago=8)
        conn.commit()
        conn.close()

        updated = _backfill_correlation_pnl(self.db_path)

        self.assertEqual(updated, 0)

    def test_backfill_handles_missing_db(self):
        """Non-existent database path returns 0 without crashing."""
        updated = _backfill_correlation_pnl("/nonexistent/path/chimera.db")
        self.assertEqual(updated, 0)


if __name__ == "__main__":
    unittest.main()
