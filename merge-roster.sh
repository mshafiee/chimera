#!/bin/bash
# Quick roster merge script for mainnet-paper
# Merges wallets from roster_new.db into chimera.db

set -e

echo "=== Merging Roster ==="
echo ""

# Use Python script from scout container
docker exec chimera-scout python3 -c "
import sqlite3

roster_path = '/app/data/roster_new.db'
db_path = '/app/data/chimera.db'

# Check roster
roster_conn = sqlite3.connect(roster_path)
roster_cursor = roster_conn.cursor()
roster_cursor.execute('SELECT COUNT(*) FROM wallets')
roster_count = roster_cursor.fetchone()[0]
print(f'Wallets in roster: {roster_count}')

if roster_count == 0:
    print('No wallets to merge')
    sys.exit(0)

# Merge
main_conn = sqlite3.connect(db_path)
main_cursor = main_conn.cursor()

# Attach roster
main_cursor.execute(f\"ATTACH DATABASE '{roster_path}' AS new_roster\")

# Delete existing wallets
main_cursor.execute('DELETE FROM wallets')
print('Cleared existing wallets')

# Insert from roster
main_cursor.execute('''
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
''')

main_cursor.execute('SELECT COUNT(*) FROM wallets')
merged_count = main_cursor.fetchone()[0]
print(f'Wallets merged: {merged_count}')

main_cursor.execute('DETACH DATABASE new_roster')
main_conn.commit()
main_conn.close()
roster_conn.close()

print('✓ Roster merge completed successfully')
"

echo ""
echo "Verifying wallets..."
curl -s http://localhost:8080/api/v1/wallets | python3 -m json.tool 2>/dev/null | head -20




