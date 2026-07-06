"""Tests for db module - PostgreSQL-first database abstraction."""

import pytest
import tempfile
import os
from pathlib import Path
from unittest.mock import Mock, patch, MagicMock

# Import the module to test
from scout.core.db import (
    _get_backend,
    _is_sqlite,
    _is_postgres,
    _translate_pg_to_sqlite,
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
    """Test database backend detection functions."""

    @patch.dict(os.environ, {"CHIMERA_DB_MODE": "postgres"})
    def test_get_backend_postgres(self):
        """Test getting PostgreSQL backend."""
        assert _get_backend() == "postgres"

    @patch.dict(os.environ, {"CHIMERA_DB_MODE": "sqlite"})
    def test_get_backend_sqlite(self):
        """Test getting SQLite backend."""
        assert _get_backend() == "sqlite"

    @patch.dict(os.environ, {"SCOUT_DB_BACKEND": "postgresql"}, clear=True)
    def test_get_backend_deprecated_alias(self):
        """Test getting backend using deprecated SCOUT_DB_BACKEND."""
        assert _get_backend() == "postgresql"

    @patch.dict(os.environ, {}, clear=True)
    def test_get_backend_default(self):
        """Test getting backend defaults to SQLite."""
        assert _get_backend() == "sqlite"

    def test_is_sqlite_true(self):
        """Test _is_sqlite returns True for SQLite."""
        with patch("scout.core.db._get_backend", return_value="sqlite"):
            assert _is_sqlite() is True

    def test_is_sqlite_false(self):
        """Test _is_sqlite returns False for PostgreSQL."""
        with patch("scout.core.db._get_backend", return_value="postgres"):
            assert _is_sqlite() is False

    def test_is_postgres_true(self):
        """Test _is_postgres returns True for PostgreSQL."""
        with patch("scout.core.db._get_backend", return_value="postgres"):
            assert _is_postgres() is True

    def test_is_postgres_false(self):
        """Test _is_postgres returns False for SQLite."""
        with patch("scout.core.db._get_backend", return_value="sqlite"):
            assert _is_postgres() is False


class TestQueryTranslation:
    """Test PostgreSQL to SQLite query translation."""

    def test_translate_placeholders(self):
        """Test %s placeholder translation to ?."""
        pg_query = "SELECT * FROM wallets WHERE status = %s"
        sqlite_query = _translate_pg_to_sqlite(pg_query)
        assert "?" in sqlite_query
        assert "%s" not in sqlite_query

    def test_remove_pragma_statements(self):
        """Test PRAGMA statement removal."""
        pg_query = "PRAGMA synchronous=NORMAL; SELECT * FROM wallets"
        sqlite_query = _translate_pg_to_sqlite(pg_query)
        assert "PRAGMA" not in sqlite_query
        assert "SELECT * FROM wallets" in sqlite_query

    def test_keep_returning_clause(self):
        """Test RETURNING clause is preserved."""
        pg_query = "INSERT INTO wallets (address) VALUES (%s) RETURNING id"
        sqlite_query = _translate_pg_to_sqlite(pg_query)
        assert "RETURNING" in sqlite_query

    def test_forbid_literal_percent(self):
        """Test that literal % raises error."""
        pg_query = "SELECT * FROM wallets WHERE name LIKE '%test%'"
        with pytest.raises(ValueError, match="Literal '%' characters"):
            _translate_pg_to_sqlite(pg_query)

    def test_on_conflict_translation(self):
        """Test ON CONFLICT is preserved."""
        pg_query = "INSERT INTO wallets (address) VALUES (%s) ON CONFLICT DO NOTHING"
        sqlite_query = _translate_pg_to_sqlite(pg_query)
        assert "ON CONFLICT" in sqlite_query


class TestConnection:
    """Test database connection management."""

    @patch.dict(os.environ, {"CHIMERA_DB_MODE": "sqlite"})
    @patch("scout.core.db.sqlite3.connect")
    def test_sqlite_connection_success(self, mock_connect):
        """Test successful SQLite connection."""
        mock_conn = MagicMock()
        mock_conn.row_factory = sqlite3.Row
        mock_connect.return_value = mock_conn
        
        conn = get_connection()
        assert conn is not None
        mock_connect.assert_called_once()

    @patch.dict(os.environ, {"CHIMERA_DB_MODE": "sqlite"})
    @patch("scout.core.db.sqlite3.connect")
    def test_sqlite_connection_path_creation(self, mock_connect):
        """Test SQLite creates directory if needed."""
        mock_conn = MagicMock()
        mock_conn.row_factory = sqlite3.Row
        mock_connect.return_value = mock_conn
        
        test_path = "/tmp/test/chimera.db"
        conn = get_connection(test_path)
        
        # Should have created parent directory
        mock_connect.assert_called_once_with(test_path, timeout=10.0)

    @patch.dict(os.environ, {"CHIMERA_DB_MODE": "postgres", "DATABASE_URL": "postgresql://test:pass@localhost/test"})
    @patch("scout.core.db.psycopg_pool.ConnectionPool")
    def test_postgres_connection_success(self, mock_pool_class):
        """Test successful PostgreSQL connection."""
        # Reset module-level pool
        import scout.core.db as db_module
        db_module._postgres_pool = None
        
        mock_pool = MagicMock()
        mock_conn = MagicMock()
        mock_pool.getconn.return_value = mock_conn
        mock_pool_class.return_value = mock_pool
        
        conn = get_connection()
        assert conn is not None
        mock_pool_class.assert_called_once()

    @patch.dict(os.environ, {"CHIMERA_DB_MODE": "postgres"}, clear=True)
    def test_postgres_connection_no_url(self):
        """Test PostgreSQL connection fails without DATABASE_URL."""
        import scout.core.db as db_module
        db_module._postgres_pool = None
        
        with pytest.raises(ValueError, match="DATABASE_URL environment variable"):
            get_connection()


class TestExecuteQuery:
    """Test query execution."""

    @patch("scout.core.db._is_sqlite", return_value=False)
    @patch("scout.core.db._translate_pg_to_sqlite", side_effect=lambda x: x)
    def test_execute_query_postgres(self, mock_translate, mock_is_sqlite):
        """Test query execution in PostgreSQL mode."""
        mock_conn = MagicMock()
        mock_cursor = MagicMock()
        
        execute_query(mock_conn, "SELECT * FROM wallets WHERE status = %s", ('ACTIVE',))
        
        mock_cursor.execute.assert_called_once_with(
            "SELECT * FROM wallets WHERE status = %s", ('ACTIVE',)
        )

    @patch("scout.core.db._is_sqlite", return_value=True)
    @patch("scout.core.db._translate_pg_to_sqlite", return_value="SELECT * FROM wallets WHERE status = ?")
    def test_execute_query_sqlite(self, mock_translate, mock_is_sqlite):
        """Test query execution in SQLite mode (with translation)."""
        mock_conn = MagicMock()
        mock_cursor = MagicMock()
        
        execute_query(mock_conn, "SELECT * FROM wallets WHERE status = %s", ('ACTIVE',))
        
        mock_cursor.execute.assert_called_once_with(
            "SELECT * FROM wallets WHERE status = ?", ('ACTIVE',)
        )
        mock_translate.assert_called_once()


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

    @patch("scout.core.db.execute_query")
    @patch("scout.core.db._is_postgres", return_value=True)
    def test_execute_update_postgres(self, mock_is_postgres, mock_execute_query):
        """Test execute update in PostgreSQL mode."""
        mock_cursor = MagicMock()
        mock_cursor.rowcount = 5
        mock_execute_query.return_value = mock_cursor
        
        result = execute_update("UPDATE wallets SET status = %s", ("CANDIDATE",))
        assert result == 5

    @patch("scout.core.db.execute_query")
    @patch("scout.core.db._is_postgres", return_value=False)
    def test_execute_update_sqlite(self, mock_is_postgres, mock_execute_query):
        """Test execute update in SQLite mode."""
        mock_cursor = MagicMock()
        mock_cursor.rowcount = 3
        mock_execute_query.return_value = mock_cursor
        
        result = execute_update("UPDATE wallets SET status = %s", ("CANDIDATE",))
        assert result == 3


class TestConvenienceFunctions:
    """Test convenience functions for common patterns."""

    @patch("scout.core.db.execute_query")
    @patch("scout.core.db.fetch_rows")
    @patch("scout.core.db.Connection")
    def test_execute_and_fetchall(self, mock_connection_class, mock_fetch_rows, mock_execute_query):
        """Test execute query and fetch all results."""
        mock_conn = MagicMock()
        mock_connection_class.return_value.__enter__.return_value = mock_conn
        mock_connection_class.return_value.__exit__.return_value = None
        
        mock_cursor = MagicMock()
        mock_execute_query.return_value = mock_cursor
        mock_fetch_rows.return_value = [{"address": "wallet1"}]
        
        rows = execute_and_fetchall("SELECT * FROM wallets WHERE status = %s", ("ACTIVE",))
        assert len(rows) == 1
        assert rows[0]["address"] == "wallet1"

    @patch("scout.core.db.execute_query")
    @patch("scout.core.db.fetch_one")
    @patch("scout.core.db.Connection")
    def test_execute_and_fetchone(self, mock_connection_class, mock_fetch_one, mock_execute_query):
        """Test execute query and fetch one result."""
        mock_conn = MagicMock()
        mock_connection_class.return_value.__enter__.return_value = mock_conn
        mock_connection_class.return_value.__exit__.return_value = None
        
        mock_cursor = MagicMock()
        mock_execute_query.return_value = mock_cursor
        mock_fetch_one.return_value = {"address": "wallet1"}
        
        row = execute_and_fetchone("SELECT * FROM wallets WHERE address = %s", ("wallet1",))
        assert row is not None
        assert row["address"] == "wallet1"


class TestConnectionContextManager:
    """Test Connection context manager."""

    @patch("scout.core.db.get_connection")
    @patch("scout.core.db.commit")
    @patch("scout.core.db.close")
    def test_connection_success(self, mock_close, mock_commit, mock_get_connection):
        """Test successful connection context (commit)."""
        mock_conn = MagicMock()
        mock_get_connection.return_value = mock_conn
        
        with Connection():
            pass
        
        mock_commit.assert_called_once_with(mock_conn)
        mock_close.assert_called_once_with(mock_conn)

    @patch("scout.core.db.get_connection")
    @patch("scout.core.db.rollback")
    @patch("scout.core.db.close")
    def test_connection_failure(self, mock_close, mock_rollback, mock_get_connection):
        """Test failed connection context (rollback)."""
        mock_conn = MagicMock()
        mock_get_connection.return_value = mock_conn
        
        with pytest.raises(ValueError):
            with Connection():
                raise ValueError("Test error")
        
        mock_rollback.assert_called_once_with(mock_conn)
        mock_close.assert_called_once_with(mock_conn)