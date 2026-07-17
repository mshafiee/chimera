"""
Database abstraction layer for Scout.

Supports both SQLite (development) and PostgreSQL (production) via
CHIMERA_DB_MODE environment variable (sqlite|postgres).

PostgreSQL-first queries are automatically translated to SQLite when running locally.
Uses psycopg3 with ConnectionPool for production and explicit transactions.

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
from typing import Union, Optional, Dict, Any, List, Tuple
from decimal import Decimal

logger = logging.getLogger(__name__)

# Module-level connection pool (lazy initialization)
_postgres_pool = None


def _get_backend() -> str:
    """Get the database backend from environment variable."""
    # CHIMERA_DB_MODE is the primary selector (shared with operator)
    # SCOUT_DB_BACKEND is a deprecated alias for backward compatibility
    mode = os.environ.get('CHIMERA_DB_MODE') or os.environ.get('SCOUT_DB_BACKEND', 'sqlite')
    return mode.lower()


def _is_sqlite() -> bool:
    """Check if using SQLite backend."""
    return _get_backend() in ('sqlite', 'sqlite3', '')


def _is_postgres() -> bool:
    """Check if using PostgreSQL backend."""
    return _get_backend() in ('postgres', 'postgresql')


def translate_ddl(sql: str) -> str:
    """Translate SQLite DDL to PostgreSQL-compatible syntax when on PostgreSQL.

    Handles common incompatibilities:
    - ``INTEGER PRIMARY KEY AUTOINCREMENT`` → ``SERIAL PRIMARY KEY``
    - ``strftime('%s', 'now')`` → ``EXTRACT(EPOCH FROM NOW())``
    """
    if not _is_postgres():
        return sql
    sql = sql.replace("INTEGER PRIMARY KEY AUTOINCREMENT", "SERIAL PRIMARY KEY")
    sql = sql.replace("strftime('%s', 'now')", "EXTRACT(EPOCH FROM NOW())")
    sql = sql.replace("BOOLEAN DEFAULT 0", "BOOLEAN DEFAULT false")
    sql = sql.replace("BOOLEAN DEFAULT 1", "BOOLEAN DEFAULT true")
    return sql


def _translate_pg_to_sqlite(query: str) -> str:
    """
    Translate PostgreSQL dialect queries to SQLite dialect.
    
    Supported translations:
    - %s → ? (placeholder style, literal-aware)
    - PRAGMA ... → removed (no-op on SQLite)
    - RETURNING → kept (requires SQLite ≥ 3.35)
    - ON CONFLICT ... DO NOTHING/UPDATE → kept (portable)
    - TRUE/FALSE → kept (portable)
    - CURRENT_TIMESTAMP → kept (portable)
    
    Args:
        query: PostgreSQL query string
        
    Returns:
        SQLite-compatible query string
        
    Raises:
        ValueError: If literal % is found in SQL (forbidden)
    """
    # Check for forbidden literal % (should be passed as params)
    # We need to be careful not to catch %% in string literals
    # Simple heuristic: if % is not followed by s and not part of %%
    if re.search(r'(?<!%)%(?![s%])', query):
        raise ValueError(
            "Literal '%' characters in SQL are forbidden. "
            "Use parameterized queries and pass wildcards as parameters."
        )
    
    # Translate placeholders %s → ?
    # Need to be careful about string literals that might contain %s
    # This is a simple translation; for complex queries, manual adjustment may be needed
    query = re.sub(r'%s', '?', query)
    
    # Remove PRAGMA statements (no-op on PostgreSQL, swallowed here)
    query = re.sub(r'PRAGMA\s+[^;]+;?\s*', '', query, flags=re.IGNORECASE)
    
    # RETURNING clause is kept as-is (requires SQLite ≥ 3.35)
    # We'll validate version at startup
    
    return query


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
    Get a database connection based on the backend configuration.

    Args:
        db_path: Path to SQLite database (required for SQLite, ignored for PostgreSQL)
        force_sqlite: If True, force SQLite connection regardless of backend

    Returns:
        Connection object (sqlite3.Connection or psycopg.Connection)

    Raises:
        ValueError: If DATABASE_URL is not set for PostgreSQL backend
        ImportError: If psycopg3 is not installed for PostgreSQL backend
    """
    backend = _get_backend()
    
    # Force SQLite for specific use cases (e.g., advanced_cache, pipeline_optimizer)
    if force_sqlite or backend == 'sqlite':
        import sqlite3
        
        # Default path for main database
        if db_path is None:
            db_path = os.environ.get('CHIMERA_DB_PATH', 'data/chimera.db')
        
        # Ensure parent directory exists
        if db_path and isinstance(db_path, str) and db_path != ':memory:':
            os.makedirs(os.path.dirname(db_path) or '.', exist_ok=True)
        
        conn = sqlite3.connect(db_path, timeout=10.0)
        
        # Set row factory for dict-like access
        conn.row_factory = sqlite3.Row
        
        # Enable WAL mode for better concurrency and set performance pragmas
        try:
            conn.execute("PRAGMA journal_mode=WAL;")
            conn.execute("PRAGMA busy_timeout=10000;")  # Queue writes up to 10s
            conn.execute("PRAGMA cache_size=-64000;")   # ~64MB read cache
        except sqlite3.OperationalError:
            # WAL mode may fail for :memory: or certain file systems
            pass
        
        # Check SQLite version for RETURNING support
        cursor = conn.cursor()
        version_str = cursor.execute("SELECT sqlite_version()").fetchone()[0]
        version_parts = version_str.split('.')
        major, minor = int(version_parts[0]), int(version_parts[1]) if len(version_parts) > 1 else 0
        
        if major < 3 or (major == 3 and minor < 35):
            logger.warning(
                f"SQLite version {version_str} < 3.35 detected. "
                "RETURNING clauses may not work. Upgrade to SQLite 3.35+ for full compatibility."
            )
        cursor.close()
        
        return conn
    
    elif backend in ('postgres', 'postgresql'):
        try:
            import psycopg
            from psycopg_pool import ConnectionPool
        except ImportError:
            raise ImportError(
                "psycopg3 is required for PostgreSQL support. "
                "Install it with: pip install 'psycopg[binary]' 'psycopg-pool'"
            )
        
        # Use module-level pool
        global _postgres_pool
        if _postgres_pool is None:
            database_url = os.environ.get('DATABASE_URL')
            if not database_url:
                raise ValueError(
                    "DATABASE_URL environment variable is required for PostgreSQL backend. "
                    "Example: postgresql://user:password@host:5432/database"
                )
            
            _postgres_pool = ConnectionPool(
                conninfo=database_url,
                min_size=2,
                max_size=10,
                open=False
            )
            _postgres_pool.open()
            logger.info("PostgreSQL connection pool initialized")
        
        # Get connection from pool
        conn = _postgres_pool.getconn()

        # Use dict row factory for compatibility with SQLite
        conn.row_factory = psycopg.rows.dict_row

        # Wrap so the connection is returned to the pool on close/__exit__
        return _PooledConnection(conn, _postgres_pool)
    
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
        query: SQL query string (PostgreSQL dialect, auto-translated for SQLite)
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
    
    # Translate PG to SQLite if needed
    if _is_sqlite():
        query = _translate_pg_to_sqlite(query)
    
    try:
        cursor.execute(query, params)
    except Exception as e:
        logger.error(f"Query error: {e}\nQuery: {query}\nParams: {params}")
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
    """Commit transaction (no-op for pooled PostgreSQL connections)."""
    if _is_sqlite():
        conn.commit()
    # PostgreSQL connections from pool use transaction context manager


def rollback(conn):
    """Rollback transaction (no-op for pooled PostgreSQL connections)."""
    if _is_sqlite():
        conn.rollback()
    # PostgreSQL connections from pool use transaction context manager


def close(conn):
    """Close connection (return to pool for PostgreSQL)."""
    if _is_postgres():
        global _postgres_pool
        if _postgres_pool:
            _postgres_pool.putconn(conn)
    else:
        conn.close()


class Connection:
    """
    Context manager for database connections with transaction support.

    Usage:
        with Connection() as conn:
            cursor = execute_query(conn, "SELECT * FROM wallets WHERE status = %s", ('ACTIVE',))
            rows = fetch_rows(cursor)
    """

    def __init__(self, db_path: Optional[str] = None, force_sqlite: bool = False):
        self.db_path = db_path
        self.force_sqlite = force_sqlite
        self.conn = None

    def __enter__(self):
        self.conn = get_connection(self.db_path, force_sqlite=self.force_sqlite)
        
        # Start transaction for PostgreSQL
        if _is_postgres():
            self.conn.__enter__()  # Enter transaction context
        
        return self.conn

    def __exit__(self, exc_type, exc_val, exc_tb):
        if _is_postgres():
            # PostgreSQL: use transaction context manager
            self.conn.__exit__(exc_type, exc_val, exc_tb)
        else:
            # SQLite: manual transaction handling
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
        query: SQL query string (PostgreSQL dialect)
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
        query: SQL query string (PostgreSQL dialect)
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
        query: SQL query string (PostgreSQL dialect)
        params: Query parameters
        db_path: Database path (for SQLite)

    Returns:
        Number of affected rows (if available)
    """
    with Connection(db_path) as conn:
        cursor = execute_query(conn, query, params)
        return cursor.rowcount if hasattr(cursor, 'rowcount') else -1


def execute_script(
    script: str,
    db_path: Optional[str] = None,
    force_sqlite: bool = False
) -> None:
    """
    Execute a multi-statement SQL script (for schema initialization).

    Args:
        script: SQL script with multiple statements (PostgreSQL dialect)
        db_path: Database path (for SQLite)
        force_sqlite: Force SQLite (for specific use cases)
    """
    if _is_postgres() and not force_sqlite:
        # PostgreSQL: execute each statement separately
        with Connection(db_path, force_sqlite=force_sqlite) as conn:
            # Split by semicolon and execute each statement
            statements = [s.strip() for s in script.split(';') if s.strip()]
            for statement in statements:
                if statement and not statement.startswith('--'):
                    execute_query(conn, statement)
    else:
        # SQLite: use executescript for multiple statements
        with Connection(db_path, force_sqlite=force_sqlite) as conn:
            # Translate PG to SQLite if needed
            if not force_sqlite:
                script = _translate_pg_to_sqlite(script)
            conn.executescript(script)


def close_pool():
    """Close the PostgreSQL connection pool (call at shutdown)."""
    global _postgres_pool
    if _postgres_pool:
        _postgres_pool.close()
        _postgres_pool = None
        logger.info("PostgreSQL connection pool closed")