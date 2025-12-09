#!/bin/sh
# Simple roster merge script for Docker container
# Can be run inside the container: docker compose exec operator sh -c "cat > /tmp/merge.sh && sh /tmp/merge.sh" < ops/merge-roster-simple.sh

set -e

ROSTER_PATH="${ROSTER_PATH:-/app/data/roster_new.db}"
DB_PATH="${DB_PATH:-/app/data/chimera.db}"

echo "=== Chimera Roster Merge ==="
echo "Roster: $ROSTER_PATH"
echo "Database: $DB_PATH"
echo ""

# Try API first (no auth needed in devnet)
if command -v curl >/dev/null 2>&1; then
    echo "Attempting merge via API..."
    RESPONSE=$(curl -s -X POST http://localhost:8080/api/v1/roster/merge \
        -H "Content-Type: application/json" \
        -d '{}' 2>&1)
    
    if echo "$RESPONSE" | grep -q "wallets_merged"; then
        echo "✓ Success!"
        echo "$RESPONSE"
        exit 0
    fi
    echo "API response: $RESPONSE"
    echo ""
fi

# Fallback: Direct SQLite merge (requires sqlite3)
if command -v sqlite3 >/dev/null 2>&1 && [ -f "$ROSTER_PATH" ]; then
    echo "Performing direct SQLite merge..."
    
    sqlite3 "$DB_PATH" <<SQL
ATTACH DATABASE '$ROSTER_PATH' AS new_roster;
DELETE FROM wallets;
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
FROM new_roster.wallets;
SELECT 'Merged ' || COUNT(*) || ' wallets' FROM wallets;
DETACH DATABASE new_roster;
SQL
    
    echo "✓ Merge completed"
    exit 0
fi

echo "ERROR: Cannot perform merge"
echo "  - API endpoint requires authentication or is unavailable"
echo "  - sqlite3 not available for direct merge"
exit 1
