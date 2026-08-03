"""
Pytest configuration and fixtures for Scout tests.
"""

import re
import sqlite3

import pytest
from datetime import datetime, timedelta, timezone

from core.wqs import WalletMetrics
from core.models import BacktestConfig


class _SqliteCursor:
    """SQLite cursor wrapper that translates psycopg %s placeholders to ?."""

    def __init__(self, cursor):
        self._cursor = cursor

    @staticmethod
    def _translate(sql):
        return re.sub(r"%s", "?", sql)

    def execute(self, sql, params=None):
        if params is None:
            return self._cursor.execute(self._translate(sql))
        return self._cursor.execute(self._translate(sql), params)

    def executemany(self, sql, seq_of_params):
        return self._cursor.executemany(self._translate(sql), seq_of_params)

    def fetchone(self):
        return self._cursor.fetchone()

    def fetchall(self):
        return self._cursor.fetchall()

    def __getattr__(self, name):
        return getattr(self._cursor, name)


class _SqliteConn:
    """SQLite connection stand-in for core.db.get_connection (PostgreSQL-only)."""

    def __init__(self, conn):
        self._conn = conn

    def cursor(self):
        return _SqliteCursor(self._conn.cursor())

    def execute(self, sql, params=None):
        cursor = _SqliteCursor(self._conn.cursor())
        if params is None:
            return cursor.execute(sql)
        return cursor.execute(sql, params)

    def executemany(self, sql, seq_of_params):
        cursor = _SqliteCursor(self._conn.cursor())
        return cursor.executemany(sql, seq_of_params)

    def commit(self):
        return self._conn.commit()

    def close(self):
        # Shared in-memory connection: producers call close() per operation,
        # so closing the underlying handle would break later operations.
        return None

    def transaction(self):
        """No-op transaction context manager (psycopg-compatible surface)."""

        class _Tx:
            def __enter__(self):
                return self

            def __exit__(self, *args):
                return False

        return _Tx()

    def __getattr__(self, name):
        return getattr(self._conn, name)


@pytest.fixture
def fake_db_layer(monkeypatch):
    """Patch the DB layer with an in-memory SQLite database.

    The production db layer is PostgreSQL-only (SQLite was decommissioned), so
    tests that exercise persistence need this stand-in: it translates %s
    placeholders to ? and returns dict-style rows. Patches both the shared
    `core.db.get_connection` and the modules that bind it at import time.
    """
    conn = sqlite3.connect(":memory:")
    conn.row_factory = sqlite3.Row

    def fake_get_connection(db_path=None, force_sqlite=False):
        return _SqliteConn(conn)

    def fake_execute_query(conn, query, params=None, cursor=None):
        cursor = cursor or conn.cursor()
        cursor.execute(query, params)
        return cursor

    class FakeConnection:
        """Context-manager stand-in for core.db.Connection."""

        def __init__(self, db_path=None, *args, **kwargs):
            pass

        def __enter__(self):
            return _SqliteConn(conn)

        def __exit__(self, *args):
            return False

    from core import db as core_db
    monkeypatch.setattr(core_db, "get_connection", fake_get_connection)
    monkeypatch.setattr(core_db, "Connection", FakeConnection)
    monkeypatch.setattr(core_db, "execute_query", fake_execute_query)

    def identity_translate(sql):
        """The stand-in IS SQLite, so DDL needs no SQLite->PG translation."""
        return sql

    # Some modules import via the `scout.core.*` namespace, which loads a
    # second copy of the same files (distinct module objects, distinct
    # get_connection bindings) — patch both namespaces so either import path
    # routes to the SQLite stand-in.
    for pkg_name in ("core", "scout.core"):
        try:
            pkg_db = __import__(f"{pkg_name}.db", fromlist=["get_connection"])
            monkeypatch.setattr(pkg_db, "get_connection", fake_get_connection)
            monkeypatch.setattr(pkg_db, "Connection", FakeConnection)
            monkeypatch.setattr(pkg_db, "execute_query", fake_execute_query)
        except Exception:
            pass

        for module_name in (
            "production_monitor",
            "prediction_logger",
            "prediction_matcher",
            "validation_metrics",
            "validation_reporter",
            "state_persistence",
            "correlation_reader",
        ):
            try:
                module = __import__(
                    f"{pkg_name}.{module_name}", fromlist=["get_connection"]
                )
                for attr in ("get_connection", "Connection", "execute_query"):
                    if hasattr(module, attr):
                        monkeypatch.setattr(module, attr, {
                            "get_connection": fake_get_connection,
                            "Connection": FakeConnection,
                            "execute_query": fake_execute_query,
                        }[attr])
                # DDL translation is for the PostgreSQL backend; the stand-in
                # is SQLite, so keep the schema SQL unmodified.
                if hasattr(module, "translate_ddl"):
                    monkeypatch.setattr(module, "translate_ddl", identity_translate)
            except Exception:
                pass

    return conn


@pytest.fixture
def sample_wallet_address():
    """Sample Solana wallet address for testing."""
    return "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"


def _ts(days_ago: int) -> str:
    """ISO timestamp relative to now (recency-window-safe)."""
    return (datetime.now(timezone.utc) - timedelta(days=days_ago)).isoformat()


@pytest.fixture
def high_quality_wallet_metrics():
    """Fixture for a high-quality wallet that should be ACTIVE."""
    return WalletMetrics(
        address="high_quality_wallet_7xKXtg2CW87d97TXJSDpbD5jBkheTqA83",
        roi_7d=15.0,
        roi_30d=45.0,
        trade_count_30d=127,
        win_rate=0.72,
        max_drawdown_30d=8.5,
        win_streak_consistency=0.65,
    )


@pytest.fixture
def medium_quality_wallet_metrics():
    """Fixture for a medium-quality wallet that should be CANDIDATE."""
    return WalletMetrics(
        address="medium_quality_wallet_9mNpQrAbCdEfGhIjKlMnOpQrStUvWxYz",
        roi_7d=5.0,
        roi_30d=15.0,
        trade_count_30d=30,
        win_rate=0.55,
        max_drawdown_30d=15.0,
        win_streak_consistency=0.40,
    )


@pytest.fixture
def low_quality_wallet_metrics():
    """Fixture for a low-quality wallet that should be REJECTED."""
    return WalletMetrics(
        address="low_quality_wallet_5kLmNoAbCdEfGhIjKlMnOpQrStUvWxYz0",
        roi_7d=-5.0,
        roi_30d=-10.0,
        trade_count_30d=5,
        win_rate=0.30,
        max_drawdown_30d=40.0,
        win_streak_consistency=0.10,
    )


@pytest.fixture
def pump_and_dump_wallet_metrics():
    """Fixture for a wallet with pump-and-dump characteristics."""
    return WalletMetrics(
        address="pump_dump_wallet_3uGcxoHV5FCKQGqA77S2HDMMtTjsAcvF3x",
        roi_7d=200.0,  # Massive recent spike
        roi_30d=50.0,  # 7d ROI > 2x 30d ROI
        trade_count_30d=25,
        win_rate=0.80,
        max_drawdown_30d=5.0,
        win_streak_consistency=0.70,
    )


@pytest.fixture
def low_trade_count_wallet_metrics():
    """Fixture for a wallet with insufficient trade history."""
    return WalletMetrics(
        address="low_trade_count_wallet_8xPqRsTuVwXyZaBcDeFgHiJkLmNoPqRs",
        roi_7d=20.0,
        roi_30d=40.0,
        trade_count_30d=10,  # < 20 trades
        win_rate=0.75,
        max_drawdown_30d=5.0,
        win_streak_consistency=0.70,
    )


@pytest.fixture
def default_backtest_config():
    """Default backtest configuration matching PDD."""
    return BacktestConfig(
        min_liquidity_shield_usd=10000.0,
        min_liquidity_spear_usd=5000.0,
        dex_fee_percent=0.003,
        max_slippage_percent=0.05,
        min_trades_required=5,
    )


@pytest.fixture
def sample_historical_trade():
    """Sample historical trade for backtest."""
    return {
        "timestamp": _ts(days_ago=7),
        "token_address": "BONK111111111111111111111111111111111111111",
        "side": "BUY",
        "amount_sol": 0.5,
        "price": 0.000012,
        "tx_signature": "signature123",
    }


@pytest.fixture
def sample_trades_list(sample_historical_trade):
    """Sample list of historical trades."""
    return [
        sample_historical_trade,
        {
            "timestamp": _ts(days_ago=6),
            "token_address": "BONK111111111111111111111111111111111111111",
            "side": "SELL",
            "amount_sol": 0.5,
            "price": 0.000015,
            "tx_signature": "signature456",
        },
        {
            "timestamp": _ts(days_ago=5),
            "token_address": "WIF1111111111111111111111111111111111111111",
            "side": "BUY",
            "amount_sol": 0.3,
            "price": 1.25,
            "tx_signature": "signature789",
        },
    ]

