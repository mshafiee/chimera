"""
Atomic SQLite writer for Scout roster output.

This module provides safe, atomic writes to SQLite databases to prevent
corruption and ensure data consistency when the Rust Operator merges
the roster.

Pattern:
1. Write to a temporary file (roster_new.db.tmp)
2. Verify integrity of the temporary file
3. Atomic rename to final path (roster_new.db)

This ensures that roster_new.db is always in a valid state, even if
the Scout crashes mid-write.

# Schema Consistency

**CRITICAL**: The `wallets` table schema MUST match the schema defined in
`database/schema/wallets.sql`. This file is the shared source of truth used by:
- Rust (Operator): Used by sqlx migrations and roster merge function
- Python (Scout): Used by this RosterWriter class

When updating the schema:
1. Update `database/schema/wallets.sql` (source of truth)
2. Update Rust migrations in `operator/migrations/`
3. Update this RosterWriter.WALLETS_SCHEMA to match (loaded from schema file)
4. Test merge operation to ensure compatibility

The schema is automatically loaded from the shared file to prevent drift.
"""

import fcntl
import logging
import os
import sqlite3
import time
from dataclasses import dataclass
from datetime import datetime

from .utils import utcnow

from pathlib import Path
from typing import List, Optional

logger = logging.getLogger(__name__)


@dataclass
class WalletRecord:
    """Wallet data for roster output."""
    address: str
    status: str  # 'ACTIVE', 'CANDIDATE', 'REJECTED'
    wqs_score: Optional[float] = None
    wqs_confidence: Optional[float] = None  # Sample confidence 0-1, unbundled from score
    roi_7d: Optional[float] = None
    roi_30d: Optional[float] = None
    trade_count_30d: Optional[int] = None
    win_rate: Optional[float] = None
    max_drawdown_30d: Optional[float] = None
    avg_trade_size_sol: Optional[float] = None
    avg_win_sol: Optional[float] = None
    avg_loss_sol: Optional[float] = None
    profit_factor: Optional[float] = None
    realized_pnl_30d_sol: Optional[float] = None
    last_trade_at: Optional[str] = None
    promoted_at: Optional[str] = None
    ttl_expires_at: Optional[str] = None
    notes: Optional[str] = None
    archetype: Optional[str] = None  # TraderArchetype as string (SNIPER, SWING, SCALPER, INSIDER, WHALE)
    avg_entry_delay_seconds: Optional[float] = None


def _load_wallet_schema() -> str:
    """
    Load wallet schema from shared source of truth file.
    
    The schema is defined in database/schema/wallets.sql and is used by both
    Rust (sqlx) and Python (RosterWriter) to ensure consistency.
    
    Supports both local development (schema at project root) and Docker
    (schema copied to /app/database/schema/wallets.sql).
    """
    # Try Docker path first (when running in container)
    docker_schema_path = Path("/app/database/schema/wallets.sql")
    if docker_schema_path.exists():
        schema_path = docker_schema_path
    else:
        # Find the shared schema file relative to this module (local development)
        # scout/core/db_writer.py -> database/schema/wallets.sql
        current_file = Path(__file__)
        scout_dir = current_file.parent.parent
        project_root = scout_dir.parent
        schema_path = project_root / "database" / "schema" / "wallets.sql"
    
    if not schema_path.exists():
        raise FileNotFoundError(
            f"Wallet schema file not found at any expected location.\n"
            f"Tried:\n"
            f"  - Docker path: /app/database/schema/wallets.sql\n"
            f"  - Local path: {schema_path.absolute()}\n"
            f"Please ensure the shared schema file exists in one of these locations."
        )
    
    with open(schema_path, 'r') as f:
        schema_content = f.read()
    
    # Extract just the CREATE TABLE statement (skip comments and indexes)
    # The schema file contains the full CREATE TABLE statement
    lines = []
    in_create_table = False
    for line in schema_content.split('\n'):
        stripped = line.strip()
        if stripped.startswith('CREATE TABLE'):
            in_create_table = True
        if in_create_table:
            lines.append(line)
            if stripped.endswith(');'):
                break
    
    return '\n'.join(lines)


class RosterWriter:
    """
    Atomic SQLite writer for Scout roster output.
    
    Usage:
        writer = RosterWriter('/path/to/roster_new.db')
        wallets = [WalletRecord(address='...', status='ACTIVE', ...)]
        writer.write_roster(wallets)
    
    The schema is automatically loaded from database/schema/wallets.sql to ensure
    consistency with the Rust Operator's expectations. See module docstring for
    schema consistency requirements.
    """
    
    # Schema for the wallets table (loaded from shared source of truth)
    # Schema source of truth: database/schema/wallets.sql
    # This MUST match the schema used by Rust's roster merge function
    WALLETS_SCHEMA = _load_wallet_schema()
    
    def __init__(self, output_path: str):
        """
        Initialize the roster writer.
        
        Args:
            output_path: Path to the final roster file (e.g., roster_new.db)
        """
        self.output_path = Path(output_path)
        self.temp_path = Path(f"{output_path}.tmp")

    def _cleanup_stale_lock(self, lock_path: Path, max_age_seconds: int = 3600) -> None:
        """
        Clean up stale lock files that are older than max_age_seconds.

        A stale lock file may exist if a previous Scout process crashed while
        holding the lock. This method checks if the lock file is old enough
        to be considered stale and removes it.

        Args:
            lock_path: Path to the lock file
            max_age_seconds: Maximum age in seconds before considering a lock stale (default: 1 hour)
        """
        if not lock_path.exists():
            return

        try:
            lock_age = time.time() - lock_path.stat().st_mtime
            if lock_age > max_age_seconds:
                logger.warning(f"Removing stale lock file: {lock_path} (age: {lock_age:.0f}s)")
                lock_path.unlink()
        except Exception as e:
            logger.warning(f"Failed to check/cleanup stale lock file {lock_path}: {e}")

    def write_roster(self, wallets: List[WalletRecord]) -> bool:
        """
        Write wallet roster to SQLite atomically.

        Acquires an exclusive file lock so that concurrent Scout processes
        cannot corrupt the roster by writing simultaneously.

        Args:
            wallets: List of wallet records to write

        Returns:
            True if write was successful, False otherwise

        Raises:
            RuntimeError: If another Scout process is already writing
            Exception: If write fails (temp file is cleaned up)
        """
        lock_path = self.output_path.with_suffix('.lock')

        # Clean up stale lock files before attempting to acquire the lock
        self._cleanup_stale_lock(lock_path)

        with open(lock_path, 'w') as lock_file:
            try:
                fcntl.flock(lock_file.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            except IOError:
                raise RuntimeError("Another Scout process is already writing the roster")

            try:
                # Step 1: Write to temporary file
                self._write_to_temp(wallets)

                # Step 2: Verify integrity
                if not self._verify_integrity():
                    raise ValueError("Integrity check failed on temporary file")

                # Step 3: Atomic rename
                self._atomic_rename()

                print(f"[RosterWriter] Successfully wrote {len(wallets)} wallets to {self.output_path}")
                return True

            except Exception as e:
                # Clean up temp file on failure
                self._cleanup_temp()
                print(f"[RosterWriter] ERROR: Failed to write roster: {e}")
                raise
    
    def _write_to_temp(self, wallets: List[WalletRecord]) -> None:
        """Write wallets to the temporary database file."""
        # Remove any existing temp file
        self._cleanup_temp()
        
        # Create new database with WAL mode for concurrent access
        conn = sqlite3.connect(str(self.temp_path), timeout=10.0)
        conn.execute("PRAGMA journal_mode=WAL;")  # Enable concurrent read/write
        conn.execute("PRAGMA synchronous=FULL;")  # Ensures durability on power loss
        cursor = conn.cursor()
        
        try:
            # Create schema
            cursor.execute(self.WALLETS_SCHEMA)
            
            # Create index
            cursor.execute(
                "CREATE INDEX IF NOT EXISTS idx_wallets_status ON wallets(status)"
            )
            cursor.execute(
                "CREATE INDEX IF NOT EXISTS idx_wallets_wqs ON wallets(wqs_score DESC)"
            )
            
            # Insert wallets
            now = utcnow().isoformat() + "Z"
            
            for wallet in wallets:
                cursor.execute(
                    """
                    INSERT OR REPLACE INTO wallets (
                        address, status, wqs_score, wqs_confidence, roi_7d, roi_30d,
                        trade_count_30d, win_rate, max_drawdown_30d,
                        avg_trade_size_sol, avg_win_sol, avg_loss_sol, profit_factor, realized_pnl_30d_sol,
                        last_trade_at, promoted_at,
                        ttl_expires_at, notes, archetype, avg_entry_delay_seconds, created_at, updated_at
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    (
                        wallet.address,
                        wallet.status,
                        wallet.wqs_score,
                        wallet.wqs_confidence,
                        wallet.roi_7d,
                        wallet.roi_30d,
                        wallet.trade_count_30d,
                        wallet.win_rate,
                        wallet.max_drawdown_30d,
                        wallet.avg_trade_size_sol,
                        wallet.avg_win_sol,
                        wallet.avg_loss_sol,
                        wallet.profit_factor,
                        wallet.realized_pnl_30d_sol,
                        wallet.last_trade_at,
                        wallet.promoted_at,
                        wallet.ttl_expires_at,
                        wallet.notes,
                        wallet.archetype,
                        wallet.avg_entry_delay_seconds,
                        now,
                        now,
                    )
                )
            
            conn.commit()

            # Checkpoint the WAL so the file is self-contained before rename.
            ckpt = conn.execute("PRAGMA wal_checkpoint(FULL)")
            row = ckpt.fetchone()
            if row and row[1] != row[2]:  # pages_written != pages_checkpointed
                logger.warning(
                    "WAL checkpoint incomplete: written=%d checkpointed=%d",
                    row[1],
                    row[2],
                )

        finally:
            conn.close()
    
    def _verify_integrity(self) -> bool:
        """Verify integrity of the temporary database file."""
        if not self.temp_path.exists():
            return False
        
        try:
            conn = sqlite3.connect(str(self.temp_path))
            cursor = conn.cursor()
            
            # Run integrity check
            cursor.execute("PRAGMA integrity_check")
            result = cursor.fetchone()
            
            conn.close()
            
            return result is not None and result[0] == "ok"
            
        except Exception as e:
            print(f"[RosterWriter] Integrity check error: {e}")
            return False
    
    def _atomic_rename(self) -> None:
        """
        Atomically rename temp file to final path.
        
        On POSIX systems, os.rename() is atomic if source and destination
        are on the same filesystem.
        """
        # Ensure parent directory exists
        self.output_path.parent.mkdir(parents=True, exist_ok=True)
        
        # Atomic rename (POSIX guarantee)
        os.rename(str(self.temp_path), str(self.output_path))
    
    def _cleanup_temp(self) -> None:
        """Remove temporary file if it exists."""
        try:
            if self.temp_path.exists():
                self.temp_path.unlink()
        except Exception as e:
            print(f"[RosterWriter] Warning: Failed to cleanup temp file: {e}")


def write_roster_atomic(wallets: List[WalletRecord], output_path: str) -> bool:
    """
    Convenience function to write roster atomically.
    
    Args:
        wallets: List of wallet records to write
        output_path: Path to the final roster file
        
    Returns:
        True if successful, False otherwise
    """
    writer = RosterWriter(output_path)
    return writer.write_roster(wallets)


# Example usage
if __name__ == "__main__":
    # Test with sample data
    test_wallets = [
        WalletRecord(
            address="7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
            status="ACTIVE",
            wqs_score=85.3,
            roi_30d=45.2,
            trade_count_30d=127,
            win_rate=0.72,
        ),
        WalletRecord(
            address="9mNpQrAbCdEfGhIjKlMnOpQrStUvWxYz1234567890",
            status="CANDIDATE",
            wqs_score=72.1,
            roi_30d=32.8,
            trade_count_30d=89,
            win_rate=0.65,
        ),
    ]
    
    try:
        write_roster_atomic(test_wallets, "test_roster_new.db")
        print("Test successful!")
        
        # Cleanup test file
        Path("test_roster_new.db").unlink(missing_ok=True)
        
    except Exception as e:
        print(f"Test failed: {e}")
