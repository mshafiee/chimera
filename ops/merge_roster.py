#!/usr/bin/env python3
"""
Simple script to merge roster_new.db into chimera.db
Can be run in scout container: docker compose exec scout python /app/ops/merge_roster.py
"""

import sqlite3
import sys
import os
from pathlib import Path

def merge_roster(roster_path: str, db_path: str) -> bool:
    """Merge wallets from roster_new.db into chimera.db"""
    
    roster_path = Path(roster_path)
    db_path = Path(db_path)
    
    if not roster_path.exists():
        print(f"ERROR: Roster file not found: {roster_path}")
        return False
    
    if not db_path.exists():
        print(f"ERROR: Database file not found: {db_path}")
        return False
    
    print(f"=== Chimera Roster Merge ===")
    print(f"Roster: {roster_path}")
    print(f"Database: {db_path}")
    print()
    
    try:
        # Connect to main database
        main_conn = sqlite3.connect(str(db_path))
        main_cursor = main_conn.cursor()
        
        # Attach roster database
        main_cursor.execute(f"ATTACH DATABASE '{roster_path}' AS new_roster")
        
        # Check integrity
        integrity_result = main_cursor.execute("PRAGMA new_roster.integrity_check").fetchone()
        if integrity_result and integrity_result[0] != "ok":
            print(f"WARNING: Integrity check failed: {integrity_result[0]}")
            main_cursor.execute("DETACH DATABASE new_roster")
            main_conn.close()
            return False
        
        # Count wallets in roster
        roster_count = main_cursor.execute("SELECT COUNT(*) FROM new_roster.wallets").fetchone()[0]
        print(f"Wallets in roster: {roster_count}")
        
        if roster_count == 0:
            print("WARNING: Roster contains 0 wallets")
            main_cursor.execute("DETACH DATABASE new_roster")
            main_conn.close()
            return False
        
        # Count before
        before_count = main_cursor.execute("SELECT COUNT(*) FROM wallets").fetchone()[0]
        print(f"Wallets before merge: {before_count}")
        
        # Start transaction
        main_cursor.execute("BEGIN TRANSACTION")
        
        try:
            # Delete existing wallets
            main_cursor.execute("DELETE FROM wallets")
            
            # Insert from new roster
            main_cursor.execute("""
                INSERT INTO wallets (
                    address, status, wqs_score, roi_7d, roi_30d,
                    trade_count_30d, win_rate, max_drawdown_30d,
                    avg_trade_size_sol, last_trade_at, promoted_at,
                    ttl_expires_at, notes, created_at, updated_at
                )
                SELECT 
                    address, status, wqs_score, roi_7d, roi_30d,
                    trade_count_30d, win_rate, max_drawdown_30d,
                    avg_trade_size_sol, last_trade_at, promoted_at,
                    ttl_expires_at, notes, created_at, CURRENT_TIMESTAMP
                FROM new_roster.wallets
            """)
            
            # Commit
            main_cursor.execute("COMMIT")
            
            # Count after
            after_count = main_cursor.execute("SELECT COUNT(*) FROM wallets").fetchone()[0]
            print(f"Wallets after merge: {after_count}")
            
            # Detach
            main_cursor.execute("DETACH DATABASE new_roster")
            main_conn.close()
            
            print("✓ Merge completed successfully!")
            return True
            
        except Exception as e:
            main_cursor.execute("ROLLBACK")
            main_cursor.execute("DETACH DATABASE new_roster")
            main_conn.close()
            raise e
            
    except Exception as e:
        print(f"ERROR: Merge failed: {e}")
        return False

if __name__ == "__main__":
    # Default paths
    roster_path = os.getenv("ROSTER_PATH", "/app/data/roster_new.db")
    db_path = os.getenv("DB_PATH", "/app/data/chimera.db")
    
    # Allow override via command line
    if len(sys.argv) > 1:
        roster_path = sys.argv[1]
    if len(sys.argv) > 2:
        db_path = sys.argv[2]
    
    success = merge_roster(roster_path, db_path)
    sys.exit(0 if success else 1)
