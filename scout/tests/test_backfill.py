"""
Tests for scout.core.correlation_backfill (C2 rewrite).

The backfill now computes ACTUAL copy PnL from Chimera's `trades` table
(status='CLOSED', pnl_data_valid=TRUE, side='SELL') instead of the circular
`wallets.realized_pnl_30d_sol`. write_correlation_record is insert-only
(ON CONFLICT DO NOTHING) and write_promotion_episode appends an immutable row.

These tests mock the db abstraction (Connection / execute_query /
execute_update) so they run without a live PostgreSQL instance.
"""

import os
import sys
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).parent.parent.parent))

from core import correlation_backfill as cb


class _FakeCursor:
    """Minimal cursor stub: stores execute query/params and yields fetch rows."""

    def __init__(self):
        self.last_query = None
        self.last_params = None
        self._rows = []

    def execute(self, query, params=None):
        self.last_query = query
        self.last_params = params

    def fetchall(self):
        return self._rows

    def fetchone(self):
        return self._rows[0] if self._rows else None


class _FakeConn:
    """Connection context manager that returns a single shared cursor."""

    def __init__(self):
        self.cursor = _FakeCursor()

    def __enter__(self):
        return self

    def __exit__(self, *args):
        return False


class TestBackfillCorrelationPnl(unittest.TestCase):
    def setUp(self):
        # Patch Connection so execute_query operates on a fake cursor.
        self.conn = _FakeConn()
        self._conn_patch = mock.patch.object(
            cb, "Connection", side_effect=lambda *a, **k: self.conn
        )
        self._conn_patch.start()

        # Track execute_update calls (write_correlation_record / write_promotion_episode).
        self.update_calls = []
        self._update_patch = mock.patch.object(
            cb,
            "execute_update",
            side_effect=lambda q, p=None, **k: self.update_calls.append((q, p)) or 0,
        )
        self._update_patch.start()

        # Track execute_query calls (the backfill SELECTs).
        self.query_calls = []
        self._query_patch = mock.patch.object(
            cb,
            "execute_query",
            side_effect=self._fake_execute_query,
        )
        self._query_patch.start()

    def tearDown(self):
        mock.patch.stopall()

    def _fake_execute_query(self, conn, query, params=None, cursor=None):
        self.query_calls.append((query, params))
        # Return the fake cursor; the test sets conn.cursor._rows before the
        # call that should populate them.
        return conn.cursor

    def test_backfill_computes_pnl_from_trades_not_wallets(self):
        """Backfill reads from trades, not wallets.realized_pnl_30d_sol."""
        call_count = {"n": 0}

        def fetchall():
            call_count["n"] += 1
            if call_count["n"] == 1:
                return [{"wallet_address": "wallet_abc"}]
            # Second call: aggregated copy PnL from trades
            return [{
                "wallet_address": "wallet_abc",
                "copy_pnl_7d": 0.1,
                "copy_pnl_30d": 0.5,
                "copy_pnl_all": 0.7,
                "count_7d": 2,
                "count_30d": 5,
                "count_all": 8,
            }]

        self.conn.cursor.fetchall = fetchall

        updated = cb.backfill_correlation_pnl("../data/chimera.db")

        self.assertEqual(updated, 1)
        # The trades-aggregation query must reference the trades table, not wallets.
        trades_queries = [q for q, _ in self.query_calls if "FROM trades" in q]
        self.assertTrue(trades_queries, "backfill must query the trades table")
        self.assertFalse(
            any("FROM wallets" in q for q, _ in self.query_calls),
            "backfill must not read from the wallets table (circular PnL)",
        )
        # The UPDATE must populate copy_trade_count_all AND carry the computed
        # PnL/count values from the trades aggregation.
        updates = [q for q, _ in self.query_calls if "UPDATE wqs_pnl_correlation" in q]
        self.assertTrue(updates)
        self.assertIn("copy_trade_count_all", updates[0])
        update_params = [p for q, p in self.query_calls if "UPDATE wqs_pnl_correlation" in q][-1]
        self.assertIn(0.5, update_params)  # copy_pnl_30d from the trades aggregation
        self.assertIn(5, update_params)    # count_30d from the trades aggregation

    def test_backfill_skips_wallets_with_no_closed_trades(self):
        """A flagged wallet with no closed copy-trades is skipped (kept NULL)."""
        call_count = {"n": 0}

        def fetchall():
            call_count["n"] += 1
            if call_count["n"] == 1:
                return [{"wallet_address": "wallet_none"}]
            return []  # no trades aggregation rows

        self.conn.cursor.fetchall = fetchall
        updated = cb.backfill_correlation_pnl("../data/chimera.db")
        self.assertEqual(updated, 0)
        # The skip path must not issue any write (UPDATE/INSERT)
        write_queries = [q for q, _ in self.update_calls if q.strip().upper().startswith(("UPDATE", "INSERT"))]
        self.assertEqual(write_queries, [], "skip path must not write")

    def test_backfill_no_flagged_records(self):
        """No correlation rows with NULL PnL → no work, returns 0."""
        self.conn.cursor.fetchall = lambda: []
        updated = cb.backfill_correlation_pnl("../data/chimera.db")
        self.assertEqual(updated, 0)
        write_queries = [q for q, _ in self.update_calls if q.strip().upper().startswith(("UPDATE", "INSERT"))]
        self.assertEqual(write_queries, [], "empty path must not write")


class TestWriteCorrelationRecord(unittest.TestCase):
    def setUp(self):
        self.update_calls = []
        self._patch = mock.patch.object(
            cb,
            "execute_update",
            side_effect=lambda q, p=None, **k: self.update_calls.append((q, p)) or 0,
        )
        self._patch.start()

    def tearDown(self):
        mock.patch.stopall()

    def test_is_insert_only_with_do_nothing(self):
        """Upsert must NOT overwrite — ON CONFLICT DO NOTHING."""
        cb.write_correlation_record("wallet_abc", 75.0, "{}", "SHIELD")
        self.assertEqual(len(self.update_calls), 1)
        query, _ = self.update_calls[0]
        self.assertIn("INSERT INTO wqs_pnl_correlation", query)
        self.assertIn("ON CONFLICT (wallet_address) DO NOTHING", query)
        self.assertNotIn("DO UPDATE", query)


class TestWritePromotionEpisode(unittest.TestCase):
    def setUp(self):
        self.update_calls = []
        self._patch = mock.patch.object(
            cb,
            "execute_update",
            side_effect=lambda q, p=None, **k: self.update_calls.append((q, p)) or 0,
        )
        self._patch.start()

    def tearDown(self):
        mock.patch.stopall()

    def test_inserts_promoted_episode(self):
        cb.write_promotion_episode("wallet_abc", 80.0, 0.9, "{}", decision="promoted")
        self.assertEqual(len(self.update_calls), 1)
        query, params = self.update_calls[0]
        self.assertIn("INSERT INTO promotion_episodes", query)
        self.assertEqual(params[0], "wallet_abc")
        self.assertEqual(params[1], 80.0)
        self.assertEqual(params[4], "promoted")

    def test_inserts_shadow_episode(self):
        cb.write_promotion_episode("wallet_abc", 73.0, 0.6, None, decision="shadow")
        query, params = self.update_calls[0]
        self.assertEqual(params[4], "shadow")
        self.assertIsNone(params[3])

    def test_code_revision_falls_back_to_unknown(self):
        with mock.patch.dict(os.environ, {}, clear=False):
            os.environ.pop("GIT_HASH", None)
            cb.write_promotion_episode("w", 70.0, 0.5, "{}")
            _, params = self.update_calls[0]
            self.assertEqual(params[6], "unknown")


if __name__ == "__main__":
    unittest.main()
