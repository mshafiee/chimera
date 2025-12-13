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
"""

import os
import sqlite3
import tempfile
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import List, Optional


@dataclass
class WalletRecord:
    """Wallet data for roster output."""
    address: str
    status: str  # 'ACTIVE', 'CANDIDATE', 'REJECTED'
    wqs_score: Optional[float] = None
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


class RosterWriter:
    """
    Atomic SQLite writer for Scout roster output.
    
    Usage:
        writer = RosterWriter('/path/to/roster_new.db')
        wallets = [WalletRecord(address='...', status='ACTIVE', ...)]
        writer.write_roster(wallets)
    """
    
    # Schema for the wallets table (must match Operator's schema)
    WALLETS_SCHEMA = """
    CREATE TABLE IF NOT EXISTS wallets (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        address TEXT NOT NULL UNIQUE,
        status TEXT NOT NULL DEFAULT 'CANDIDATE'
            CHECK(status IN ('ACTIVE', 'CANDIDATE', 'REJECTED')),
        wqs_score REAL,
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
        created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
        updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
    )
    """
    
    def __init__(self, output_path: str):
        """
        Initialize the roster writer.
        
        Args:
            output_path: Path to the final roster file (e.g., roster_new.db)
        """
        self.output_path = Path(output_path)
        self.temp_path = Path(f"{output_path}.tmp")
    
    def write_roster(self, wallets: List[WalletRecord]) -> bool:
        """
        Write wallet roster to SQLite atomically.
        
        Args:
            wallets: List of wallet records to write
            
        Returns:
            True if write was successful, False otherwise
            
        Raises:
            Exception: If write fails (temp file is cleaned up)
        """
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
        
        # Create new database
        conn = sqlite3.connect(str(self.temp_path))
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
            now = datetime.utcnow().isoformat()
            
            for wallet in wallets:
                cursor.execute(
                    """
                    INSERT OR REPLACE INTO wallets (
                        address, status, wqs_score, roi_7d, roi_30d,
                        trade_count_30d, win_rate, max_drawdown_30d,
                        avg_trade_size_sol, avg_win_sol, avg_loss_sol, profit_factor, realized_pnl_30d_sol,
                        last_trade_at, promoted_at,
                        ttl_expires_at, notes, archetype, avg_entry_delay_seconds, created_at, updated_at
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    (
                        wallet.address,
                        wallet.status,
                        wallet.wqs_score,
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
