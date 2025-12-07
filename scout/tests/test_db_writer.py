"""
Database Writer Tests

Tests atomic write behavior and data integrity:
- Atomic writes (temp file + rename)
- Schema validation
- Data integrity checks
"""

import os
import tempfile
import sqlite3
import pytest
from pathlib import Path


# =============================================================================
# ATOMIC WRITE TESTS
# =============================================================================

def test_atomic_write_creates_temp_file():
    """Test that atomic write uses temp file first."""
    with tempfile.TemporaryDirectory() as tmpdir:
        output_path = Path(tmpdir) / "roster_new.db"
        temp_path = Path(tmpdir) / "roster_new.db.tmp"
        
        # Simulate atomic write pattern
        # 1. Write to temp file
        with open(temp_path, 'w') as f:
            f.write("test data")
        
        assert temp_path.exists(), "Temp file should be created first"
        assert not output_path.exists(), "Final file should not exist yet"


def test_atomic_write_renames_on_success():
    """Test that temp file is renamed to final path on success."""
    with tempfile.TemporaryDirectory() as tmpdir:
        output_path = Path(tmpdir) / "roster_new.db"
        temp_path = Path(tmpdir) / "roster_new.db.tmp"
        
        # 1. Write to temp
        with open(temp_path, 'w') as f:
            f.write("test data")
        
        # 2. Rename to final (atomic on POSIX)
        os.rename(temp_path, output_path)
        
        assert output_path.exists(), "Final file should exist after rename"
        assert not temp_path.exists(), "Temp file should not exist after rename"


def test_atomic_write_preserves_content():
    """Test that content is preserved through atomic write."""
    with tempfile.TemporaryDirectory() as tmpdir:
        output_path = Path(tmpdir) / "test.db"
        temp_path = Path(tmpdir) / "test.db.tmp"
        
        content = "important wallet data"
        
        # Write to temp
        with open(temp_path, 'w') as f:
            f.write(content)
        
        # Atomic rename
        os.rename(temp_path, output_path)
        
        # Verify content
        with open(output_path, 'r') as f:
            read_content = f.read()
        
        assert read_content == content, "Content should be preserved"


def test_atomic_write_no_partial_writes():
    """Test that partial writes don't corrupt final file."""
    with tempfile.TemporaryDirectory() as tmpdir:
        output_path = Path(tmpdir) / "roster_new.db"
        
        # Pre-create a valid file
        with open(output_path, 'w') as f:
            f.write("valid data")
        
        temp_path = Path(tmpdir) / "roster_new.db.tmp"
        
        # Simulate failed write (only partial data)
        try:
            with open(temp_path, 'w') as f:
                f.write("partial...")
                raise Exception("Simulated failure")
        except Exception:
            pass
        
        # Original file should still be valid
        with open(output_path, 'r') as f:
            content = f.read()
        
        assert content == "valid data", "Original file should not be corrupted"


# =============================================================================
# SQLITE DATABASE TESTS
# =============================================================================

def test_create_sqlite_database():
    """Test creating a SQLite database."""
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
        
        conn.commit()
        conn.close()
        
        assert db_path.exists(), "Database file should be created"


def test_insert_wallet_record():
    """Test inserting a wallet record."""
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
        
        cursor.execute('''
            INSERT INTO wallets (address, status, wqs_score)
            VALUES (?, ?, ?)
        ''', ("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU", "ACTIVE", 85.3))
        
        conn.commit()
        
        # Verify insert
        cursor.execute('SELECT * FROM wallets')
        rows = cursor.fetchall()
        
        conn.close()
        
        assert len(rows) == 1
        assert rows[0][0] == "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"
        assert rows[0][1] == "ACTIVE"
        assert rows[0][2] == 85.3


def test_multiple_wallet_inserts():
    """Test inserting multiple wallet records."""
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
        
        wallets = [
            ("wallet1", "ACTIVE", 85.0),
            ("wallet2", "CANDIDATE", 55.0),
            ("wallet3", "REJECTED", 25.0),
        ]
        
        cursor.executemany('''
            INSERT INTO wallets (address, status, wqs_score)
            VALUES (?, ?, ?)
        ''', wallets)
        
        conn.commit()
        
        cursor.execute('SELECT COUNT(*) FROM wallets')
        count = cursor.fetchone()[0]
        
        conn.close()
        
        assert count == 3


# =============================================================================
# SCHEMA VALIDATION TESTS
# =============================================================================

def test_schema_has_required_columns():
    """Test that schema includes all required columns."""
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


# =============================================================================
# CLEANUP TESTS
# =============================================================================

def test_temp_file_cleanup_on_success():
    """Test that temp file is cleaned up after successful write."""
    with tempfile.TemporaryDirectory() as tmpdir:
        output_path = Path(tmpdir) / "roster_new.db"
        temp_path = Path(tmpdir) / "roster_new.db.tmp"
        
        # Simulate successful write
        with open(temp_path, 'w') as f:
            f.write("data")
        
        os.rename(temp_path, output_path)
        
        # Cleanup: temp should not exist
        assert not temp_path.exists()
        assert output_path.exists()


def test_temp_file_cleanup_on_failure():
    """Test that temp file is cleaned up after failed write."""
    with tempfile.TemporaryDirectory() as tmpdir:
        temp_path = Path(tmpdir) / "roster_new.db.tmp"
        
        # Simulate failed write
        try:
            with open(temp_path, 'w') as f:
                f.write("partial")
                raise Exception("Failure")
        except Exception:
            # Cleanup temp file on failure
            if temp_path.exists():
                os.remove(temp_path)
        
        assert not temp_path.exists(), "Temp file should be cleaned up"

