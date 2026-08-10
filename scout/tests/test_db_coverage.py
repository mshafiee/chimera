"""Coverage completion tests for core/db.py (PostgreSQL-only abstraction)."""

import sys
from unittest.mock import MagicMock, patch

import pytest

from core import db as db_module
from core.db import (
    _PooledConnection,
    _split_sql_statements,
    close,
    close_pool,
    commit,
    execute_query,
    execute_script,
    fetch_one,
    fetch_rows,
    rollback,
    translate_ddl,
)


class TestTranslateDdl:
    def test_autoincrement_translated(self):
        sql = "id INTEGER PRIMARY KEY AUTOINCREMENT"
        assert translate_ddl(sql) == "id SERIAL PRIMARY KEY"

    def test_strftime_translated(self):
        sql = "timestamp REAL DEFAULT (strftime('%s', 'now'))"
        assert "EXTRACT(EPOCH FROM NOW())" in translate_ddl(sql)

    def test_boolean_defaults_translated(self):
        assert "BOOLEAN DEFAULT false" in translate_ddl("x BOOLEAN DEFAULT 0")
        assert "BOOLEAN DEFAULT true" in translate_ddl("x BOOLEAN DEFAULT 1")


class TestPooledConnection:
    def test_getattr_delegates(self):
        conn = MagicMock()
        pooled = _PooledConnection(conn, MagicMock())
        assert pooled.some_attr is conn.some_attr

    def test_setattr_delegates(self):
        conn = MagicMock()
        pooled = _PooledConnection(conn, MagicMock())
        pooled.row_factory = "dict_row"
        assert conn.row_factory == "dict_row"

    def test_enter_exit_returns_to_pool(self):
        conn = MagicMock()
        pool = MagicMock()
        pooled = _PooledConnection(conn, pool)
        result = pooled.__enter__()
        assert result is pooled
        conn.__enter__.assert_called_once()
        pooled.__exit__(None, None, None)
        conn.__exit__.assert_called_once()
        pool.putconn.assert_called_once_with(conn)

    def test_close_returns_to_pool(self):
        conn = MagicMock()
        pool = MagicMock()
        pooled = _PooledConnection(conn, pool)
        pooled.close()
        pool.putconn.assert_called_once_with(conn)


class TestGetConnectionExtras:
    def test_configure_and_check_callbacks(self):
        """Pool configure/check callbacks set row_factory and run health checks."""
        with patch.dict(
            sys.modules,
            {"psycopg": MagicMock(), "psycopg_pool": MagicMock()},
            clear=False,
        ):
            import core.db as cdb
            fake_psycopg = MagicMock()
            fake_pool_module = MagicMock()
            pool_instance = MagicMock()
            fake_pool_module.ConnectionPool.return_value = pool_instance
            with patch.dict("os.environ", {"DATABASE_URL": "postgresql://u:p@h/db"}):
                with patch.dict(
                    sys.modules,
                    {"psycopg": fake_psycopg, "psycopg_pool": fake_pool_module},
                ):
                    cdb._postgres_pool = None
                    cdb.get_connection()
                    kwargs = fake_pool_module.ConnectionPool.call_args.kwargs
                    configure = kwargs["configure"]
                    check = kwargs["check"]
                    mock_conn = MagicMock()
                    configure(mock_conn)
                    mock_conn.row_factory = fake_psycopg.rows.dict_row
                    assert mock_conn.row_factory is fake_psycopg.rows.dict_row
                    assert mock_conn.autocommit is True
                    check(mock_conn)
                    mock_conn.execute.assert_called_with("SELECT 1")
            cdb._postgres_pool = None


class TestExecuteQueryError:
    def test_execute_query_reraises_and_logs(self):
        mock_conn = MagicMock()
        mock_cursor = MagicMock()
        mock_cursor.execute.side_effect = RuntimeError("boom")
        with pytest.raises(RuntimeError, match="boom"):
            execute_query(mock_conn, "SELECT bad", cursor=mock_cursor)


class TestFetchRowsExtras:
    def test_fetch_rows_as_dict_from_tuples(self):
        mock_cursor = MagicMock()
        mock_cursor.fetchall.return_value = [[("address", "w1")], [("status", "ACTIVE")]]
        rows = fetch_rows(mock_cursor, as_dict=True)
        assert rows == [{"address": "w1"}, {"status": "ACTIVE"}]

    def test_fetch_rows_as_dict_from_dicts(self):
        mock_cursor = MagicMock()
        mock_cursor.fetchall.return_value = [{"address": "w1"}]
        assert fetch_rows(mock_cursor, as_dict=True) == [{"address": "w1"}]

    def test_fetch_rows_empty(self):
        mock_cursor = MagicMock()
        mock_cursor.fetchall.return_value = []
        assert fetch_rows(mock_cursor) == []

    def test_fetch_rows_non_dict_rows_kept(self):
        class Row:
            pass

        mock_cursor = MagicMock()
        row = Row()
        mock_cursor.fetchall.return_value = [row]
        rows = fetch_rows(mock_cursor, as_dict=False)
        assert rows == [row]


class TestFetchOneExtras:
    def test_fetch_one_dict_from_tuple(self):
        mock_cursor = MagicMock()
        mock_cursor.fetchone.return_value = [("address", "w1")]
        assert fetch_one(mock_cursor, as_dict=True) == {"address": "w1"}

    def test_fetch_one_tuple_from_dict(self):
        mock_cursor = MagicMock()
        mock_cursor.fetchone.return_value = {"address": "w1", "status": "ACTIVE"}
        assert fetch_one(mock_cursor, as_dict=False) == ("w1", "ACTIVE")

    def test_fetch_one_tuple_row(self):
        mock_cursor = MagicMock()
        mock_cursor.fetchone.return_value = ("a", 1)
        assert fetch_one(mock_cursor, as_dict=False) == ("a", 1)

    def test_fetch_one_dict_row_passthrough(self):
        mock_cursor = MagicMock()
        row = {"address": "w1"}
        mock_cursor.fetchone.return_value = row
        assert fetch_one(mock_cursor, as_dict=True) is row


class TestNoopHelpers:
    def test_commit_and_rollback_noop(self):
        conn = MagicMock()
        assert commit(conn) is None
        assert rollback(conn) is None


class TestClose:
    def test_close_without_pool(self):
        old_pool = db_module._postgres_pool
        db_module._postgres_pool = None
        try:
            close(MagicMock())
        finally:
            db_module._postgres_pool = old_pool

    def test_close_pooled_connection(self):
        pool = MagicMock()
        conn = MagicMock()
        wrapped = _PooledConnection(conn, pool)
        db_module._postgres_pool = pool
        try:
            close(wrapped)
            pool.putconn.assert_called_once_with(conn)
        finally:
            db_module._postgres_pool = None

    def test_close_raw_connection(self):
        pool = MagicMock()
        conn = MagicMock()
        db_module._postgres_pool = pool
        try:
            close(conn)
            pool.putconn.assert_called_once_with(conn)
        finally:
            db_module._postgres_pool = None


class TestSplitSqlStatements:
    def test_empty_script(self):
        assert _split_sql_statements("") == []

    def test_simple_statements(self):
        assert _split_sql_statements("SELECT 1; SELECT 2;") == ["SELECT 1", "SELECT 2"]

    def test_trailing_statement_without_semicolon(self):
        assert _split_sql_statements("SELECT 1") == ["SELECT 1"]

    def test_double_dash_comment(self):
        sql = "-- comment with ; inside\nSELECT 1; SELECT 2"
        assert _split_sql_statements(sql) == ["SELECT 1", "SELECT 2"]

    def test_comment_to_end_of_script(self):
        assert _split_sql_statements("SELECT 1 -- no terminator") == ["SELECT 1"]

    def test_block_comment(self):
        sql = "/* block ; comment */ SELECT 1; SELECT 2"
        assert _split_sql_statements(sql) == ["SELECT 1", "SELECT 2"]

    def test_unterminated_block_comment(self):
        assert _split_sql_statements("SELECT 1 /* oops") == ["SELECT 1"]

    def test_single_quote_with_semicolon_and_escaped_quote(self):
        sql = "INSERT INTO t VALUES ('a;b', 'it''s'); SELECT 2"
        stmts = _split_sql_statements(sql)
        assert stmts[0] == "INSERT INTO t VALUES ('a;b', 'it''s')"
        assert stmts[1] == "SELECT 2"

    def test_single_quote_unescaped_run(self):
        stmts = _split_sql_statements("SELECT 'a;b")
        assert stmts == ["SELECT 'a;b"]

    def test_double_quotes(self):
        sql = 'SELECT "col;name" FROM t; SELECT 2'
        stmts = _split_sql_statements(sql)
        assert stmts[0] == 'SELECT "col;name" FROM t'
        assert stmts[1] == "SELECT 2"

    def test_double_quote_escaped(self):
        assert _split_sql_statements('SELECT "a""b"') == ['SELECT "a""b"']

    def test_dollar_quoted_body(self):
        sql = "CREATE FUNCTION f() RETURNS void AS $fn$ BEGIN NULL; END; $fn$; SELECT 1"
        stmts = _split_sql_statements(sql)
        assert len(stmts) == 2
        assert "BEGIN NULL; END;" in stmts[0]

    def test_dollar_quote_unterminated(self):
        sql = "SELECT $tag$abc"
        assert _split_sql_statements(sql) == ["SELECT"]

    def test_comment_at_end_without_newline(self):
        stmts = _split_sql_statements("SELECT 1; -- trailing")
        assert stmts == ["SELECT 1"]


class TestExecuteScript:
    @patch("core.db.execute_query")
    @patch("core.db.Connection")
    def test_execute_script_runs_statements(self, mock_connection_cls, mock_execute_query):
        mock_conn = MagicMock()
        mock_connection_cls.return_value.__enter__.return_value = mock_conn
        execute_script("CREATE TABLE t (id INT); INSERT INTO t VALUES (1);")
        assert mock_execute_query.call_count == 2

    def test_execute_script_force_sqlite_raises(self):
        with pytest.raises(ValueError, match="SQLite was decommissioned"):
            execute_script("SELECT 1", force_sqlite=True)


class TestClosePool:
    def test_close_pool_with_pool(self):
        pool = MagicMock()
        db_module._postgres_pool = pool
        try:
            close_pool()
            pool.close.assert_called_once()
            assert db_module._postgres_pool is None
        finally:
            db_module._postgres_pool = None

    def test_close_pool_without_pool(self):
        old = db_module._postgres_pool
        db_module._postgres_pool = None
        try:
            close_pool()
        finally:
            db_module._postgres_pool = old


class TestPoolSelfHealing:
    """get_connection() must recover from an exhausted pool instead of
    leaving Scout unable to reach the DB for the rest of the process life.

    A leaked slot (get_connection() returned but never close()'d) permanently
    occupies a pool slot; after max_size leaks every getconn() times out.
    """

    def _fakes(self):
        # psycopg_pool isn't installed in the test env, so synthesize a module
        # whose PoolTimeout the in-function `except PoolTimeout` can catch.
        pool_timeout = type("PoolTimeout", (Exception,), {})
        fake_psycopg = MagicMock()
        fake_pool_module = MagicMock()
        fake_pool_module.PoolTimeout = pool_timeout
        return fake_psycopg, fake_pool_module, pool_timeout

    def test_reset_pool_closes_old_and_clears_global(self):
        pool = MagicMock()
        db_module._postgres_pool = pool
        try:
            db_module.reset_pool()
            assert db_module._postgres_pool is None
            pool.close.assert_called_once_with(timeout=2.0)
        finally:
            db_module._postgres_pool = None

    def test_reset_pool_noop_without_pool(self):
        db_module._postgres_pool = None
        try:
            db_module.reset_pool()  # must not raise
            assert db_module._postgres_pool is None
        finally:
            db_module._postgres_pool = None

    def test_get_connection_resets_and_retries_on_pool_timeout(self):
        fake_psycopg, fake_pool_module, pool_timeout = self._fakes()
        fake_conn = MagicMock()
        mock_pool = MagicMock()
        mock_pool.getconn.side_effect = [pool_timeout("exhausted"), fake_conn]
        db_module._postgres_pool = mock_pool
        try:
            with patch.dict(sys.modules, {"psycopg": fake_psycopg, "psycopg_pool": fake_pool_module}):
                with patch.object(db_module, "reset_pool") as mock_reset:
                    result = db_module.get_connection()
            assert mock_pool.getconn.call_count == 2
            mock_reset.assert_called_once()
            assert result is not None
        finally:
            db_module._postgres_pool = None

    def test_get_connection_reraises_when_retry_also_times_out(self):
        fake_psycopg, fake_pool_module, pool_timeout = self._fakes()
        mock_pool = MagicMock()
        mock_pool.getconn.side_effect = pool_timeout("still exhausted")
        db_module._postgres_pool = mock_pool
        try:
            with patch.dict(sys.modules, {"psycopg": fake_psycopg, "psycopg_pool": fake_pool_module}):
                with patch.object(db_module, "reset_pool"):
                    with pytest.raises(Exception):
                        db_module.get_connection()
            assert mock_pool.getconn.call_count == 2
        finally:
            db_module._postgres_pool = None


class TestPsycopgImportError:
    """Reloads the module — must run last so later tests use fresh bindings."""

    def test_import_error_without_psycopg(self):
        with patch.dict(sys.modules, {"psycopg": None, "psycopg_pool": None}):
            import importlib
            import core.db as cdb
            importlib.reload(cdb)
            try:
                with pytest.raises(ImportError, match="psycopg3 is required"):
                    cdb.get_connection()
            finally:
                importlib.reload(cdb)
