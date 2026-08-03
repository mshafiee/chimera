"""Tests for db module - PostgreSQL-first database abstraction."""

import sys

import pytest
from unittest.mock import patch, MagicMock

# Import the module to test
from core.db import (
    _is_sqlite,
    _is_postgres,
    get_connection,
    execute_query,
    fetch_rows,
    fetch_one,
    execute_update,
    execute_and_fetchall,
    execute_and_fetchone,
    Connection,
)


class TestBackendDetection:
    """Test database backend detection functions (PostgreSQL-only)."""

    def test_is_postgres_true(self):
        """Test _is_postgres returns True (PostgreSQL is the only backend)."""
        assert _is_postgres() is True

    def test_is_sqlite_false(self):
        """Test _is_sqlite returns False (SQLite decommissioned)."""
        assert _is_sqlite() is False

    def test_get_connection_requires_url(self):
        """Test get_connection raises without DATABASE_URL."""
        with patch.dict("os.environ", {}, clear=True):
            with patch.dict(sys.modules, {"psycopg": MagicMock(), "psycopg_pool": MagicMock()}):
                import core.db as db_module
                db_module._postgres_pool = None
                with pytest.raises(ValueError, match="DATABASE_URL environment variable"):
                    get_connection()

    def test_get_connection_force_sqlite_raises(self):
        """Test force_sqlite=True is rejected."""
        with pytest.raises(ValueError, match="SQLite was decommissioned"):
            get_connection(force_sqlite=True)


class TestConnection:
    """Test database connection management."""

    def test_postgres_connection_success(self):
        """Test successful PostgreSQL connection."""
        with patch.dict("os.environ", {"DATABASE_URL": "postgresql://test:pass@localhost/test"}):
            fake_pool_module = MagicMock()
            fake_pool_module.ConnectionPool = MagicMock(return_value=MagicMock())
            fake_psycopg = MagicMock()
            with patch.dict(sys.modules, {"psycopg": fake_psycopg, "psycopg_pool": fake_pool_module}):
                import core.db as db_module
                db_module._postgres_pool = None

                conn = get_connection()
                assert conn is not None
                fake_pool_module.ConnectionPool.assert_called_once()


class TestExecuteQuery:
    """Test query execution."""

    def test_execute_query(self):
        """Test query execution passes query and params to the cursor."""
        mock_conn = MagicMock()

        execute_query(mock_conn, "SELECT * FROM wallets WHERE status = %s", ('ACTIVE',))

        mock_conn.cursor().execute.assert_called_once_with(
            "SELECT * FROM wallets WHERE status = %s", ('ACTIVE',)
        )

    def test_execute_query_reuses_cursor(self):
        """Test execute_query uses the provided cursor when given."""
        mock_conn = MagicMock()
        mock_cursor = MagicMock()

        execute_query(mock_conn, "SELECT 1", None, cursor=mock_cursor)

        mock_cursor.execute.assert_called_once_with("SELECT 1", ())
        mock_conn.cursor.assert_not_called()


class TestFetchRows:
    """Test row fetching."""

    def test_fetch_rows_as_dicts(self):
        """Test fetching rows as dictionaries."""
        mock_cursor = MagicMock()
        mock_cursor.fetchall.return_value = [
            {"address": "wallet1", "status": "ACTIVE"},
            {"address": "wallet2", "status": "ACTIVE"},
        ]

        rows = fetch_rows(mock_cursor, as_dict=True)
        assert len(rows) == 2
        assert rows[0]["address"] == "wallet1"

    def test_fetch_rows_as_tuples(self):
        """Test fetching rows as tuples."""
        mock_cursor = MagicMock()
        mock_cursor.fetchall.return_value = [
            {"address": "wallet1", "status": "ACTIVE"},
            {"address": "wallet2", "status": "ACTIVE"},
        ]

        rows = fetch_rows(mock_cursor, as_dict=False)
        assert len(rows) == 2
        assert isinstance(rows[0], tuple)
        assert rows[0][0] == "wallet1"


class TestFetchOne:
    """Test fetching single row."""

    def test_fetch_one_found(self):
        """Test fetching one existing row."""
        mock_cursor = MagicMock()
        mock_cursor.fetchone.return_value = {"address": "wallet1", "status": "ACTIVE"}

        row = fetch_one(mock_cursor)
        assert row is not None
        assert row["address"] == "wallet1"

    def test_fetch_one_not_found(self):
        """Test fetching when no rows exist."""
        mock_cursor = MagicMock()
        mock_cursor.fetchone.return_value = None

        row = fetch_one(mock_cursor)
        assert row is None


class TestExecuteUpdate:
    """Test UPDATE/INSERT/DELETE execution."""

    @patch("core.db.execute_query")
    @patch("core.db.Connection")
    def test_execute_update(self, mock_connection_class, mock_execute_query):
        """Test execute update returns the rowcount."""
        mock_conn = MagicMock()
        mock_cursor = MagicMock()
        mock_cursor.rowcount = 5
        mock_execute_query.return_value = mock_cursor
        mock_connection_class.return_value.__enter__.return_value = mock_conn

        result = execute_update("UPDATE wallets SET status = %s", ("CANDIDATE",))
        assert result == 5
        mock_execute_query.assert_called_once_with(
            mock_conn, "UPDATE wallets SET status = %s", ("CANDIDATE",)
        )


class TestConvenienceFunctions:
    """Test convenience functions for common patterns."""

    @patch("core.db.execute_query")
    @patch("core.db.fetch_rows")
    @patch("core.db.Connection")
    def test_execute_and_fetchall(self, mock_connection_class, mock_fetch_rows, mock_execute_query):
        """Test execute query and fetch all results."""
        mock_conn = MagicMock()
        mock_connection_class.return_value.__enter__.return_value = mock_conn

        mock_cursor = MagicMock()
        mock_execute_query.return_value = mock_cursor
        mock_fetch_rows.return_value = [{"address": "wallet1"}]

        rows = execute_and_fetchall("SELECT * FROM wallets WHERE status = %s", ("ACTIVE",))
        assert len(rows) == 1
        assert rows[0]["address"] == "wallet1"

    @patch("core.db.execute_query")
    @patch("core.db.fetch_one")
    @patch("core.db.Connection")
    def test_execute_and_fetchone(self, mock_connection_class, mock_fetch_one, mock_execute_query):
        """Test execute query and fetch one result."""
        mock_conn = MagicMock()
        mock_connection_class.return_value.__enter__.return_value = mock_conn

        mock_cursor = MagicMock()
        mock_execute_query.return_value = mock_cursor
        mock_fetch_one.return_value = {"address": "wallet1"}

        row = execute_and_fetchone("SELECT * FROM wallets WHERE address = %s", ("wallet1",))
        assert row is not None
        assert row["address"] == "wallet1"


class TestConnectionContextManager:
    """Test Connection context manager."""

    def test_connection_success(self):
        """Test successful connection context (transaction committed)."""
        mock_conn = MagicMock()
        mock_tx = MagicMock()

        with patch("core.db.get_connection", return_value=mock_conn):
            mock_conn.transaction.return_value = mock_tx
            with Connection():
                pass

        mock_tx.__enter__.assert_called_once()
        mock_tx.__exit__.assert_called_once()
        mock_conn.close.assert_called_once()

    def test_connection_failure(self):
        """Test failed connection context (transaction rolled back)."""
        mock_conn = MagicMock()
        mock_tx = MagicMock()

        with patch("core.db.get_connection", return_value=mock_conn):
            mock_conn.transaction.return_value = mock_tx
            with pytest.raises(ValueError):
                with Connection():
                    raise ValueError("Test error")

        mock_tx.__enter__.assert_called_once()
        # __exit__ receives the exception so the transaction is rolled back
        assert mock_tx.__exit__.call_args[0][0] is ValueError
        mock_conn.close.assert_called_once()
