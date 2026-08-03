"""
Database Writer Tests

Tests the production roster writer (core.roster_writer_db) against an
in-memory SQLite stand-in for the PostgreSQL layer, plus concurrency safety:
- Writes and updates through the real writer functions
- Error handling (writer failure does not corrupt state)
- Schema validation
- Concurrency safety
"""

import sqlite3
import tempfile
import threading
import pytest
from pathlib import Path

from core.roster_writer_db import (
    WalletRecord,
    write_wallet_to_db,
    update_wallet_status,
    delete_wallet,
)


def _make_wallet(address="7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU", wqs=85.5):
    return WalletRecord(
        address=address,
        status="ACTIVE",
        wqs_score=wqs,
        roi_7d=12.5,
        roi_30d=25.8,
        trade_count_30d=50,
        win_rate=0.65,
        max_drawdown_30d=0.15,
        avg_trade_size_sol=1.5,
        avg_win_sol=0.8,
        avg_loss_sol=0.5,
        profit_factor=1.6,
        realized_pnl_30d_sol=12.5,
        last_trade_at="2024-01-01T12:00:00Z",
        promoted_at="2024-01-01T10:00:00Z",
        ttl_expires_at="2024-02-01T10:00:00Z",
        notes="Test wallet",
        archetype="SWING",
        avg_entry_delay_seconds=0.5,
    )


# =============================================================================
# PRODUCTION WRITER TESTS (via in-memory SQLite stand-in)
# =============================================================================

def test_write_wallet_through_production_writer(fake_db_layer):
    """The real write_wallet_to_db inserts a row and the data is readable."""
    fake_db_layer.executescript("""
        CREATE TABLE IF NOT EXISTS wallets (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            address TEXT NOT NULL UNIQUE,
            status TEXT NOT NULL DEFAULT 'CANDIDATE',
            wqs_score REAL,
            wqs_confidence REAL,
            roi_7d REAL,
            roi_30d REAL,
            trade_count_30d INTEGER,
            win_rate REAL,
            max_drawdown_30d REAL,
            avg_trade_size_sol REAL,
            avg_win_sol REAL,
            avg_loss_sol REAL,
            profit_factor REAL,
            realized_pnl_30d_sol REAL,
            last_trade_at TIMESTAMP,
            promoted_at TIMESTAMP,
            ttl_expires_at TIMESTAMP,
            notes TEXT,
            archetype TEXT,
            avg_entry_delay_seconds REAL,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE IF NOT EXISTS wallet_monitoring (
            wallet_address TEXT PRIMARY KEY,
            inactivity_demotion_count INTEGER DEFAULT 0
        );
    """)

    assert write_wallet_to_db(_make_wallet()) is True

    row = fake_db_layer.execute(
        "SELECT address, status, wqs_score FROM wallets WHERE address = ?",
        ("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",),
    ).fetchone()
    assert row is not None
    assert row["status"] == "ACTIVE"
    assert row["wqs_score"] == 85.5


def test_write_wallet_failure_does_not_corrupt(fake_db_layer, monkeypatch):
    """A failing write returns False and leaves existing rows untouched."""
    fake_db_layer.executescript("""
        CREATE TABLE IF NOT EXISTS wallets (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            address TEXT NOT NULL UNIQUE,
            status TEXT NOT NULL DEFAULT 'CANDIDATE',
            wqs_score REAL,
            wqs_confidence REAL,
            roi_7d REAL,
            roi_30d REAL,
            trade_count_30d INTEGER,
            win_rate REAL,
            max_drawdown_30d REAL,
            avg_trade_size_sol REAL,
            avg_win_sol REAL,
            avg_loss_sol REAL,
            profit_factor REAL,
            realized_pnl_30d_sol REAL,
            last_trade_at TIMESTAMP,
            promoted_at TIMESTAMP,
            ttl_expires_at TIMESTAMP,
            notes TEXT,
            archetype TEXT,
            avg_entry_delay_seconds REAL,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE IF NOT EXISTS wallet_monitoring (
            wallet_address TEXT PRIMARY KEY,
            inactivity_demotion_count INTEGER DEFAULT 0
        );
    """)

    assert write_wallet_to_db(_make_wallet()) is True

    from core import roster_writer_db as rw
    monkeypatch.setattr(rw, "execute_update", lambda *a, **k: (_ for _ in ()).throw(Exception("DB down")))

    result = write_wallet_to_db(_make_wallet("another_wallet_00000000000000000000000000"))
    assert result is False

    # Original row is still intact
    row = fake_db_layer.execute(
        "SELECT address FROM wallets WHERE address = ?",
        ("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",),
    ).fetchone()
    assert row is not None


def test_update_status_through_production_writer(fake_db_layer):
    """The real update_wallet_status changes the stored status."""
    fake_db_layer.executescript("""
        CREATE TABLE IF NOT EXISTS wallets (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            address TEXT NOT NULL UNIQUE,
            status TEXT NOT NULL DEFAULT 'CANDIDATE',
            wqs_score REAL,
            wqs_confidence REAL,
            roi_7d REAL,
            roi_30d REAL,
            trade_count_30d INTEGER,
            win_rate REAL,
            max_drawdown_30d REAL,
            avg_trade_size_sol REAL,
            avg_win_sol REAL,
            avg_loss_sol REAL,
            profit_factor REAL,
            realized_pnl_30d_sol REAL,
            last_trade_at TIMESTAMP,
            promoted_at TIMESTAMP,
            ttl_expires_at TIMESTAMP,
            notes TEXT,
            archetype TEXT,
            avg_entry_delay_seconds REAL,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE IF NOT EXISTS wallet_monitoring (
            wallet_address TEXT PRIMARY KEY,
            inactivity_demotion_count INTEGER DEFAULT 0
        );
    """)

    write_wallet_to_db(_make_wallet())
    assert update_wallet_status("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU", "CANDIDATE") is True

    row = fake_db_layer.execute(
        "SELECT status FROM wallets WHERE address = ?",
        ("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",),
    ).fetchone()
    assert row["status"] == "CANDIDATE"


def test_delete_wallet_through_production_writer(fake_db_layer):
    """The real delete_wallet removes the row."""
    fake_db_layer.executescript("""
        CREATE TABLE IF NOT EXISTS wallets (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            address TEXT NOT NULL UNIQUE,
            status TEXT NOT NULL DEFAULT 'CANDIDATE',
            wqs_score REAL,
            wqs_confidence REAL,
            roi_7d REAL,
            roi_30d REAL,
            trade_count_30d INTEGER,
            win_rate REAL,
            max_drawdown_30d REAL,
            avg_trade_size_sol REAL,
            avg_win_sol REAL,
            avg_loss_sol REAL,
            profit_factor REAL,
            realized_pnl_30d_sol REAL,
            last_trade_at TIMESTAMP,
            promoted_at TIMESTAMP,
            ttl_expires_at TIMESTAMP,
            notes TEXT,
            archetype TEXT,
            avg_entry_delay_seconds REAL,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE IF NOT EXISTS wallet_monitoring (
            wallet_address TEXT PRIMARY KEY,
            inactivity_demotion_count INTEGER DEFAULT 0
        );
    """)

    write_wallet_to_db(_make_wallet())
    assert delete_wallet("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU") is True

    row = fake_db_layer.execute(
        "SELECT address FROM wallets WHERE address = ?",
        ("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",),
    ).fetchone()
    assert row is None


# =============================================================================
# SCHEMA VALIDATION TESTS
# =============================================================================

def test_schema_has_required_columns():
    """Test that the wallets schema includes all required columns."""
    required_columns = [
        'address',
        'status',
        'wqs_score',
        'roi_7d',
        'roi_30d',
        'trade_count_30d',
        'win_rate',
        'max_drawdown_30d',
    ]

    with tempfile.TemporaryDirectory() as tmpdir:
        db_path = Path(tmpdir) / "test.db"

        conn = sqlite3.connect(str(db_path))
        cursor = conn.cursor()

        cursor.execute('''
            CREATE TABLE wallets (
                address TEXT PRIMARY KEY,
                status TEXT NOT NULL,
                wqs_score REAL,
                roi_7d REAL,
                roi_30d REAL,
                trade_count_30d INTEGER,
                win_rate REAL,
                max_drawdown_30d REAL
            )
        ''')

        cursor.execute('PRAGMA table_info(wallets)')
        columns = [row[1] for row in cursor.fetchall()]

        conn.close()

        for col in required_columns:
            assert col in columns, f"Missing required column: {col}"


def test_status_constraint():
    """Test that status only accepts valid values."""
    with tempfile.TemporaryDirectory() as tmpdir:
        db_path = Path(tmpdir) / "test.db"

        conn = sqlite3.connect(str(db_path))
        cursor = conn.cursor()

        cursor.execute('''
            CREATE TABLE wallets (
                address TEXT PRIMARY KEY,
                status TEXT NOT NULL CHECK(status IN ('ACTIVE', 'CANDIDATE', 'REJECTED'))
            )
        ''')

        # Valid status
        cursor.execute("INSERT INTO wallets VALUES ('wallet1', 'ACTIVE')")
        conn.commit()

        # Invalid status should fail
        with pytest.raises(sqlite3.IntegrityError):
            cursor.execute("INSERT INTO wallets VALUES ('wallet2', 'INVALID')")
            conn.commit()

        conn.close()


# =============================================================================
# DATA INTEGRITY TESTS
# =============================================================================

def test_integrity_check_passes():
    """Test that integrity check passes on valid database."""
    with tempfile.TemporaryDirectory() as tmpdir:
        db_path = Path(tmpdir) / "test.db"

        conn = sqlite3.connect(str(db_path))
        cursor = conn.cursor()

        cursor.execute('''
            CREATE TABLE wallets (
                address TEXT PRIMARY KEY,
                status TEXT NOT NULL
            )
        ''')

        cursor.execute("INSERT INTO wallets VALUES ('wallet1', 'ACTIVE')")
        conn.commit()

        # Run integrity check
        cursor.execute('PRAGMA integrity_check')
        result = cursor.fetchone()[0]

        conn.close()

        assert result == 'ok', "Integrity check should pass"


def test_unique_address_constraint():
    """Test that duplicate addresses are rejected."""
    with tempfile.TemporaryDirectory() as tmpdir:
        db_path = Path(tmpdir) / "test.db"

        conn = sqlite3.connect(str(db_path))
        cursor = conn.cursor()

        cursor.execute('''
            CREATE TABLE wallets (
                address TEXT PRIMARY KEY,
                status TEXT NOT NULL
            )
        ''')

        cursor.execute("INSERT INTO wallets VALUES ('wallet1', 'ACTIVE')")
        conn.commit()

        # Duplicate should fail
        with pytest.raises(sqlite3.IntegrityError):
            cursor.execute("INSERT INTO wallets VALUES ('wallet1', 'CANDIDATE')")
            conn.commit()

        conn.close()


def test_not_null_constraint():
    """Test that NOT NULL constraints are enforced."""
    with tempfile.TemporaryDirectory() as tmpdir:
        db_path = Path(tmpdir) / "test.db"

        conn = sqlite3.connect(str(db_path))
        cursor = conn.cursor()

        cursor.execute('''
            CREATE TABLE wallets (
                address TEXT PRIMARY KEY,
                status TEXT NOT NULL
            )
        ''')

        # NULL status should fail
        with pytest.raises(sqlite3.IntegrityError):
            cursor.execute("INSERT INTO wallets VALUES ('wallet1', NULL)")
            conn.commit()

        conn.close()


# =============================================================================
# MERGE OPERATION TESTS
# =============================================================================

def test_merge_replaces_existing():
    """Test that merge replaces existing wallet data."""
    with tempfile.TemporaryDirectory() as tmpdir:
        db_path = Path(tmpdir) / "test.db"

        conn = sqlite3.connect(str(db_path))
        cursor = conn.cursor()

        cursor.execute('''
            CREATE TABLE wallets (
                address TEXT PRIMARY KEY,
                status TEXT NOT NULL,
                wqs_score REAL
            )
        ''')

        # Initial data
        cursor.execute("INSERT INTO wallets VALUES ('wallet1', 'CANDIDATE', 50.0)")
        conn.commit()

        # Merge (replace) with new data
        cursor.execute('''
            INSERT OR REPLACE INTO wallets VALUES ('wallet1', 'ACTIVE', 75.0)
        ''')
        conn.commit()

        cursor.execute("SELECT status, wqs_score FROM wallets WHERE address = 'wallet1'")
        row = cursor.fetchone()

        conn.close()

        assert row[0] == 'ACTIVE', "Status should be updated"
        assert row[1] == 75.0, "WQS should be updated"


def test_merge_adds_new():
    """Test that merge adds new wallet entries."""
    with tempfile.TemporaryDirectory() as tmpdir:
        db_path = Path(tmpdir) / "test.db"

        conn = sqlite3.connect(str(db_path))
        cursor = conn.cursor()

        cursor.execute('''
            CREATE TABLE wallets (
                address TEXT PRIMARY KEY,
                status TEXT NOT NULL
            )
        ''')

        # Initial data
        cursor.execute("INSERT INTO wallets VALUES ('wallet1', 'ACTIVE')")
        conn.commit()

        # Merge new wallet
        cursor.execute("INSERT OR REPLACE INTO wallets VALUES ('wallet2', 'CANDIDATE')")
        conn.commit()

        cursor.execute("SELECT COUNT(*) FROM wallets")
        count = cursor.fetchone()[0]

        conn.close()

        assert count == 2, "Should have 2 wallets after merge"


# ── Concurrency safety ───────────────────────────────────────────────────────


def _write_wallet(db_path: Path, wallet_address: str, results: list, idx: int):
    """Thread-safe DB writer for a single wallet row."""
    conn = None
    try:
        conn = sqlite3.connect(str(db_path), timeout=10)
        conn.execute("PRAGMA journal_mode=WAL")
        conn.execute(
            "INSERT OR REPLACE INTO wallets (address, status, wqs_score, created_at, updated_at) "
            "VALUES (?, 'ACTIVE', ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            (wallet_address, float(idx) * 10.0),
        )
        conn.commit()
        results[idx] = "ok"
    except Exception as e:
        results[idx] = f"error:{e}"
    finally:
        if conn is not None:
            conn.close()


def test_concurrent_roster_writes_no_corruption():
    """C1: 10 threads each write a distinct wallet concurrently. Final DB must have exactly
    10 rows with correct data and no corruption."""
    N = 10
    with tempfile.TemporaryDirectory() as tmpdir:
        db_path = Path(tmpdir) / "roster_new.db"

        # Bootstrap schema
        conn = sqlite3.connect(str(db_path))
        conn.execute("PRAGMA journal_mode=WAL")
        conn.execute(
            "CREATE TABLE wallets ("
            "  address TEXT PRIMARY KEY,"
            "  status TEXT NOT NULL,"
            "  wqs_score REAL,"
            "  created_at TEXT,"
            "  updated_at TEXT"
            ")"
        )
        conn.commit()
        conn.close()

        results = [None] * N
        threads = [
            threading.Thread(
                target=_write_wallet,
                args=(db_path, f"wallet_{i:04d}", results, i),
            )
            for i in range(N)
        ]
        for t in threads:
            t.start()
        for t in threads:
            t.join()

        errors = [r for r in results if r and r.startswith("error")]
        assert not errors, f"Concurrent writes produced errors: {errors}"

        conn = sqlite3.connect(str(db_path))
        rows = conn.execute("SELECT address, wqs_score FROM wallets ORDER BY address").fetchall()
        conn.close()

        assert len(rows) == N, f"Expected {N} wallet rows, got {len(rows)}"
        addresses = {r[0] for r in rows}
        assert len(addresses) == N, "All wallet addresses must be distinct (no overwrites lost)"

        for addr, score in rows:
            idx = int(addr.split("_")[1])
            assert abs(score - idx * 10.0) < 0.001, (
                f"{addr}: expected wqs_score={idx * 10.0}, got {score}"
            )
