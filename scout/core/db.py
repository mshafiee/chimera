"""
Database abstraction layer for Scout (PostgreSQL-only).

SQLite was decommissioned (2026-07). PostgreSQL is the only supported backend.
Uses psycopg3 with ConnectionPool and explicit transactions.

Usage:
    from .db import get_connection, execute_query, fetch_rows

    conn = get_connection()
    with conn.transaction():
        cursor = execute_query(conn, "SELECT * FROM wallets WHERE status = %s", ('ACTIVE',))
        rows = fetch_rows(cursor)
"""

import os
import logging
import re
import threading
from typing import Union, Optional, Dict, Any, List, Tuple

logger = logging.getLogger(__name__)

# Module-level connection pool (lazy initialization)
_postgres_pool = None
_pool_lock = threading.Lock()


def _is_postgres() -> bool:
    """PostgreSQL is the only supported backend; always True.

    Retained as a shim for legacy callers during the SQLite decommissioning
    sweep. New code must not branch on this."""
    return True


def _is_sqlite() -> bool:
    """SQLite was decommissioned; always False.

    Retained as a shim for legacy callers during the SQLite decommissioning
    sweep. New code must not branch on this."""
    return False


def translate_ddl(sql: str) -> str:
    """Translate legacy SQLite DDL to PostgreSQL-compatible syntax.

    Handles common incompatibilities:
    - ``INTEGER PRIMARY KEY AUTOINCREMENT`` → ``SERIAL PRIMARY KEY``
    - ``strftime('%s', 'now')`` → ``EXTRACT(EPOCH FROM NOW())``
    """
    sql = sql.replace("INTEGER PRIMARY KEY AUTOINCREMENT", "SERIAL PRIMARY KEY")
    sql = sql.replace("strftime('%s', 'now')", "EXTRACT(EPOCH FROM NOW())")
    sql = sql.replace("BOOLEAN DEFAULT 0", "BOOLEAN DEFAULT false")
    sql = sql.replace("BOOLEAN DEFAULT 1", "BOOLEAN DEFAULT true")
    return sql


class _PooledConnection:
    """Wrapper that returns the connection to the pool on close/__exit__.

    psycopg3's own __exit__ only commits/rolls back — it does NOT return the
    connection to the pool. Without this wrapper every get_connection() call
    permanently removes a slot from the pool, causing exhaustion after max_size
    calls.
    """

    def __init__(self, conn, pool):
        object.__setattr__(self, "_conn", conn)
        object.__setattr__(self, "_pool", pool)

    def __getattr__(self, name):
        return getattr(self._conn, name)

    def __setattr__(self, name, value):
        setattr(self._conn, name, value)

    def __enter__(self):
        self._conn.__enter__()
        return self

    def __exit__(self, *args):
        try:
            self._conn.__exit__(*args)
        finally:
            self._pool.putconn(self._conn)

    def close(self):
        self._pool.putconn(self._conn)


def get_connection(db_path: Optional[str] = None, force_sqlite: bool = False):
    """
    Get a PostgreSQL database connection from the shared pool.

    Args:
        db_path: Ignored (legacy SQLite parameter kept for call-site compat).
        force_sqlite: Legacy parameter — passing True raises, SQLite is
            decommissioned.

    Returns:
        Pooled psycopg connection wrapper.

    Raises:
        ValueError: If DATABASE_URL is not set, or force_sqlite=True.
        ImportError: If psycopg3 is not installed.
    """
    if force_sqlite:
        raise ValueError(
            "force_sqlite=True is no longer supported: SQLite was decommissioned. "
            "Use the shared PostgreSQL connection."
        )
    try:
        import psycopg
        from psycopg_pool import ConnectionPool
    except ImportError:
        raise ImportError(
            "psycopg3 is required for PostgreSQL support. "
            "Install it with: pip install 'psycopg[binary]' 'psycopg-pool'"
        )

    # Use module-level pool (double-checked locking: only one thread may
    # initialize the pool, otherwise leaked pools and connections occur)
    global _postgres_pool
    if _postgres_pool is None:
        with _pool_lock:
            if _postgres_pool is None:
                database_url = os.environ.get('DATABASE_URL')
                if not database_url:
                    raise ValueError(
                        "DATABASE_URL environment variable is required. "
                        "Example: postgresql://user:password@host:5432/database"
                    )

                # Pool sizing: scout fans out many concurrent wallet analyses
                # (asyncio semaphores), each of which briefly checks the DB for
                # cached metrics. The previous max_size=10 was exhausted under
                # load, producing "couldn't get a connection after 30.00 sec" and
                # silently dropping every wallet's DB lookup. max_size is now
                # configurable and defaults to 20.
                max_size = int(os.environ.get('SCOUT_DB_POOL_MAX_SIZE', '20'))
                min_size = min(2, max_size)
                # Fail fast (10s) instead of hanging the whole analysis batch
                # for 30s per wallet when the pool is momentarily saturated.
                timeout = float(os.environ.get('SCOUT_DB_POOL_TIMEOUT', '10'))

                def _configure(conn):
                    # Applied to every NEW connection the pool creates, so the
                    # row_factory survives recycling (setting it only after
                    # getconn() was lost whenever the pool handed out a
                    # previously-created connection that predated the setting).
                    conn.row_factory = psycopg.rows.dict_row
                    conn.autocommit = True

                def _check(conn):
                    # Health check: dead TCP connections (container restarts,
                    # idle TCP kills) are pruned before being handed out, so a
                    # stale connection never blocks a getconn() slot.
                    conn.execute("SELECT 1")

                _postgres_pool = ConnectionPool(
                    conninfo=database_url,
                    min_size=min_size,
                    max_size=max_size,
                    timeout=timeout,
                    max_lifetime=1800.0,   # recycle connections every 30 min
                    max_idle=300.0,        # close idle conns after 5 min
                    configure=_configure,
                    check=_check,
                    name="scout",
                    open=False,
                )
                _postgres_pool.open()
                logger.info(
                    "PostgreSQL connection pool initialized: min=%d max=%d timeout=%ss",
                    min_size, max_size, timeout,
                )

    # Get connection from pool
    conn = _postgres_pool.getconn()

    # Use dict row factory (belt-and-suspenders alongside the pool's
    # configure callback above).
    conn.row_factory = psycopg.rows.dict_row

    # Wrap so the connection is returned to the pool on close/__exit__
    return _PooledConnection(conn, _postgres_pool)


def execute_query(
    conn,
    query: str,
    params: Optional[Tuple] = None,
    cursor: Optional[Any] = None
) -> Any:
    """
    Execute a query with parameters (PostgreSQL dialect, %s placeholders).

    Args:
        conn: Database connection
        query: SQL query string
        params: Query parameters (tuple or dict)
        cursor: Optional cursor to reuse

    Returns:
        Cursor object
    """
    if cursor is None:
        cursor = conn.cursor()

    # Handle None params
    if params is None:
        params = ()

    try:
        cursor.execute(query, params)
    except Exception as e:
        logger.error(f"Query error: {e}\nQuery: {query}")
        logger.debug("Query params: %s", params)
        raise

    return cursor


def fetch_rows(cursor, as_dict: bool = True) -> List[Union[tuple, Dict[str, Any]]]:
    """
    Fetch all rows from a cursor.

    Args:
        cursor: Database cursor
        as_dict: If True, return rows as dictionaries (keyed by column name)
                 If False, return rows as tuples

    Returns:
        List of rows (dicts or tuples)
    """
    rows = cursor.fetchall()

    if not as_dict:
        # Convert dict rows to tuples if requested
        if rows and isinstance(rows[0], dict):
            return [tuple(row.values()) for row in rows]
        return rows

    # Return as dicts (already in dict format from row_factory)
    if rows:
        if isinstance(rows[0], dict):
            return rows
        else:
            return [dict(row) for row in rows]
    return []


def fetch_one(cursor, as_dict: bool = True) -> Optional[Union[tuple, Dict[str, Any]]]:
    """
    Fetch one row from a cursor.

    Args:
        cursor: Database cursor
        as_dict: If True, return row as dictionary

    Returns:
        Row (dict or tuple) or None
    """
    row = cursor.fetchone()

    if row is None:
        return None

    if as_dict:
        if isinstance(row, dict):
            return row
        else:
            return dict(row) if row else None

    return tuple(row.values()) if isinstance(row, dict) else row


def commit(conn):
    """Commit is a no-op for pooled PostgreSQL connections (autocommit /
    transaction context manager handles it)."""
    pass


def rollback(conn):
    """Rollback is a no-op for pooled PostgreSQL connections (transaction
    context manager handles it)."""
    pass


def close(conn):
    """Return connection to the pool."""
    global _postgres_pool
    if _postgres_pool:
        if isinstance(conn, _PooledConnection):
            # The pool only tracks raw psycopg connections; delegate to the
            # wrapper so the slot is matched and released correctly.
            conn.close()
        else:
            _postgres_pool.putconn(conn)


class Connection:
    """
    Context manager for database connections with transaction support.

    Uses an explicit psycopg3 ``transaction()`` block so the batch is atomic
    even though pooled connections are configured with autocommit=True.

    Usage:
        with Connection() as conn:
            cursor = execute_query(conn, "SELECT * FROM wallets WHERE status = %s", ('ACTIVE',))
            rows = fetch_rows(cursor)
    """

    def __init__(self, db_path: Optional[str] = None, force_sqlite: bool = False):
        self.db_path = db_path
        self.force_sqlite = force_sqlite
        self.conn = None
        self._tx = None

    def __enter__(self):
        self.conn = get_connection(self.db_path, force_sqlite=self.force_sqlite)
        self._tx = self.conn.transaction()
        self._tx.__enter__()
        return self.conn

    def __exit__(self, exc_type, exc_val, exc_tb):
        try:
            self._tx.__exit__(exc_type, exc_val, exc_tb)
        finally:
            self.conn.close()


# Convenience functions for common patterns

def execute_and_fetchall(
    query: str,
    params: Optional[Tuple] = None,
    db_path: Optional[str] = None,
    as_dict: bool = True
) -> List[Union[tuple, Dict[str, Any]]]:
    """
    Execute query and fetch all results in one call.

    Args:
        query: SQL query string (PostgreSQL dialect)
        params: Query parameters
        db_path: Ignored (legacy SQLite parameter)
        as_dict: Return rows as dictionaries

    Returns:
        List of rows
    """
    with Connection(db_path) as conn:
        cursor = execute_query(conn, query, params)
        return fetch_rows(cursor, as_dict=as_dict)


def execute_and_fetchone(
    query: str,
    params: Optional[Tuple] = None,
    db_path: Optional[str] = None,
    as_dict: bool = True
) -> Optional[Union[tuple, Dict[str, Any]]]:
    """
    Execute query and fetch one result in one call.

    Args:
        query: SQL query string (PostgreSQL dialect)
        params: Query parameters
        db_path: Ignored (legacy SQLite parameter)
        as_dict: Return row as dictionary

    Returns:
        Row or None
    """
    with Connection(db_path) as conn:
        cursor = execute_query(conn, query, params)
        return fetch_one(cursor, as_dict=as_dict)


def execute_update(
    query: str,
    params: Optional[Tuple] = None,
    db_path: Optional[str] = None
) -> int:
    """
    Execute an UPDATE/INSERT/DELETE query.

    Args:
        query: SQL query string (PostgreSQL dialect)
        params: Query parameters
        db_path: Ignored (legacy SQLite parameter)

    Returns:
        Number of affected rows (if available)
    """
    with Connection(db_path) as conn:
        cursor = execute_query(conn, query, params)
        return cursor.rowcount if hasattr(cursor, 'rowcount') else -1


def _split_sql_statements(script: str) -> List[str]:
    """Split a SQL script into individual statements.

    Handles single/double-quoted strings, dollar-quoted bodies, and
    ``--``/``/* */`` comments, so statement boundaries inside those are
    never mistaken for ``;`` separators.
    """
    statements: List[str] = []
    current: List[str] = []
    i = 0
    n = len(script)
    while i < n:
        ch = script[i]
        nxt = script[i + 1] if i + 1 < n else ''
        if ch == '-' and nxt == '-':
            j = script.find('\n', i)
            if j == -1:
                break
            i = j + 1
            continue
        if ch == '/' and nxt == '*':
            j = script.find('*/', i + 2)
            if j == -1:
                break
            i = j + 2
            continue
        if ch == "'":
            current.append(ch)
            i += 1
            while i < n:
                if script[i] == "'":
                    if i + 1 < n and script[i + 1] == "'":
                        current.append("''")
                        i += 2
                        continue
                    current.append("'")
                    i += 1
                    break
                current.append(script[i])
                i += 1
            continue
        if ch == '"':
            current.append(ch)
            i += 1
            while i < n:
                if script[i] == '"':
                    if i + 1 < n and script[i + 1] == '"':
                        current.append('""')
                        i += 2
                        continue
                    current.append('"')
                    i += 1
                    break
                current.append(script[i])
                i += 1
            continue
        if ch == '$':
            m = re.match(r'\$[A-Za-z_0-9]*\$', script[i:])
            if m:
                tag = m.group(0)
                end = script.find(tag, i + len(tag))
                if end == -1:
                    break
                current.append(script[i:end + len(tag)])
                i = end + len(tag)
                continue
        if ch == ';':
            stmt = ''.join(current).strip()
            if stmt:
                statements.append(stmt)
            current = []
            i += 1
            continue
        current.append(ch)
        i += 1
    stmt = ''.join(current).strip()
    if stmt:
        statements.append(stmt)
    return statements


def execute_script(
    script: str,
    db_path: Optional[str] = None,
    force_sqlite: bool = False
) -> None:
    """
    Execute a multi-statement SQL script (for schema initialization).

    Args:
        script: SQL script with multiple statements (PostgreSQL dialect)
        db_path: Ignored (legacy SQLite parameter)
        force_sqlite: Legacy parameter — passing True raises.
    """
    if force_sqlite:
        raise ValueError(
            "force_sqlite=True is no longer supported: SQLite was decommissioned."
        )
    # PostgreSQL: execute each statement separately, split with a
    # quote/comment-aware parser (plain ';' splitting breaks dollar-quoted
    # function bodies and string literals containing ';').
    with Connection(db_path) as conn:
        for statement in _split_sql_statements(script):
            if statement:
                execute_query(conn, statement)


def close_pool():
    """Close the PostgreSQL connection pool (call at shutdown)."""
    global _postgres_pool
    if _postgres_pool:
        _postgres_pool.close()
        _postgres_pool = None
        logger.info("PostgreSQL connection pool closed")
