"""
Database abstraction layer for Scout.

Supports both SQLite (development) and PostgreSQL (production) via
SCOUT_DB_BACKEND environment variable.

Usage:
    from .db import get_connection, execute_query, fetch_rows

    conn = get_connection(db_path)
    cursor = execute_query(conn, "SELECT * FROM wallets WHERE status = ?", ('ACTIVE',))
    rows = fetch_rows(cursor)
    conn.close()
"""

import os
import logging
from typing import Union, Optional, Dict, Any, List, Tuple

logger = logging.getLogger(__name__)


def _get_backend() -> str:
    """Get the database backend from environment variable."""
    return os.environ.get('SCOUT_DB_BACKEND', 'sqlite').lower()


def _is_sqlite() -> bool:
    """Check if using SQLite backend."""
    return _get_backend() in ('sqlite', 'sqlite3', '')


def _is_postgres() -> bool:
    """Check if using PostgreSQL backend."""
    return _get_backend() in ('postgres', 'postgresql', 'psycopg2')


def get_connection(db_path: Optional[str] = None, force_sqlite: bool = False):
    """
    Get a database connection based on the backend configuration.

    Args:
        db_path: Path to SQLite database (required for SQLite, ignored for PostgreSQL)
        force_sqlite: If True, force SQLite connection regardless of SCOUT_DB_BACKEND

    Returns:
        Connection object (sqlite3.Connection or psycopg2.extensions.connection)

    Raises:
        ValueError: If DATABASE_URL is not set for PostgreSQL backend
        ImportError: If psycopg2 is not installed for PostgreSQL backend
    """
    backend = _get_backend()

    # Force SQLite for roster files (atomic writes)
    if force_sqlite or backend == 'sqlite':
        import sqlite3

        # Default path for main database
        if db_path is None:
            db_path = os.environ.get('CHIMERA_DB_PATH', 'data/chimera.db')

        # Ensure parent directory exists
        if db_path and isinstance(db_path, str) and db_path != ':memory:':
            import os
            os.makedirs(os.path.dirname(db_path) or '.', exist_ok=True)

        conn = sqlite3.connect(db_path, timeout=10.0)

        # Set row factory for dict-like access
        conn.row_factory = sqlite3.Row

        # Enable WAL mode for better concurrency
        try:
            conn.execute("PRAGMA journal_mode=WAL;")
        except sqlite3.OperationalError:
            # WAL mode may fail for :memory: or certain file systems
            pass

        return conn

    elif backend in ('postgres', 'postgresql'):
        try:
            import psycopg2
            import psycopg2.extras
        except ImportError:
            raise ImportError(
                "psycopg2-binary is required for PostgreSQL support. "
                "Install it with: pip install psycopg2-binary"
            )

        database_url = os.environ.get('DATABASE_URL')
        if not database_url:
            raise ValueError(
                "DATABASE_URL environment variable is required for PostgreSQL backend. "
                "Example: postgresql://user:password@host:5432/database"
            )

        conn = psycopg2.connect(database_url)

        # Set isolation level to autocommit for consistency with SQLite
        conn.set_isolation_level(psycopg2.extensions.ISOLATION_LEVEL_AUTOCOMMIT)

        return conn

    else:
        raise ValueError(
            f"Unknown database backend: {backend}. "
            f"Supported backends: 'sqlite', 'postgres'"
        )


def execute_query(
    conn,
    query: str,
    params: Optional[Tuple] = None,
    cursor: Optional[Any] = None
) -> Any:
    """
    Execute a query with parameters.

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
        # Get more specific error info
        if _is_postgres():
            logger.error(f"PostgreSQL query error: {e}")
        else:
            logger.error(f"SQLite query error: {e}")
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
        return rows

    # Convert to dicts
    if _is_postgres():
        # PostgreSQL cursor with RealDictCursor
        return [dict(row) for row in rows]
    else:
        # SQLite with Row factory
        if rows:
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
        if _is_postgres():
            return dict(row)
        else:
            return dict(row) if row else None

    return row


def commit(conn):
    """Commit transaction."""
    conn.commit()


def rollback(conn):
    """Rollback transaction."""
    conn.rollback()


def close(conn):
    """Close connection."""
    conn.close()


class Connection:
    """
    Context manager for database connections.

    Usage:
        with Connection(db_path) as conn:
            cursor = execute_query(conn, "SELECT * FROM wallets")
            rows = fetch_rows(cursor)
    """

    def __init__(self, db_path: Optional[str] = None, force_sqlite: bool = False):
        self.db_path = db_path
        self.force_sqlite = force_sqlite
        self.conn = None

    def __enter__(self):
        self.conn = get_connection(self.db_path, force_sqlite=self.force_sqlite)
        return self.conn

    def __exit__(self, exc_type, exc_val, exc_tb):
        if exc_type is None:
            commit(self.conn)
        else:
            rollback(self.conn)
        close(self.conn)


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
        query: SQL query string
        params: Query parameters
        db_path: Database path (for SQLite)
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
    Execute query and fetch one result.

    Args:
        query: SQL query string
        params: Query parameters
        db_path: Database path (for SQLite)
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
        query: SQL query string
        params: Query parameters
        db_path: Database path (for SQLite)

    Returns:
        Number of affected rows (if available)
    """
    with Connection(db_path) as conn:
        cursor = execute_query(conn, query, params)

        if _is_postgres():
            return cursor.rowcount
        else:
            return cursor.rowcount


def execute_script(
    script: str,
    db_path: Optional[str] = None,
    force_sqlite: bool = False
) -> None:
    """
    Execute a multi-statement SQL script (for schema initialization).

    Args:
        script: SQL script with multiple statements
        db_path: Database path (for SQLite)
        force_sqlite: Force SQLite (for roster files)
    """
    with Connection(db_path, force_sqlite=force_sqlite) as conn:
        if _is_postgres() and not force_sqlite:
            # PostgreSQL: execute script directly
            conn.cursor().execute(script)
        else:
            # SQLite: use executescript for multiple statements
            conn.executescript(script)
