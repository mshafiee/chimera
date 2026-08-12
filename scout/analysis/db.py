"""Read-only Postgres access for the profitability analysis harness."""

import os


def connect():
    """Open a connection using DATABASE_URL or CHIMERA_DB_URL.

    All access is read-only (SELECT against the shadow_* tables/views).

    psycopg is imported lazily so the rest of the package (and its unit tests)
    can be imported without the DB driver installed.
    """
    import psycopg  # lazy: prod-only dependency

    url = os.environ.get("DATABASE_URL") or os.environ.get("CHIMERA_DB_URL")
    if not url:
        raise RuntimeError("Set DATABASE_URL or CHIMERA_DB_URL to the operator Postgres")
    return psycopg.connect(url)
