#!/bin/bash
# Simple script to trigger roster merge in devnet
# Usage: ./ops/merge-roster.sh

set -e

echo "=== Chimera Roster Merge Script ==="
echo ""

# Check if running in docker or locally
if [ -f "/app/data/roster_new.db" ]; then
    ROSTER_PATH="/app/data/roster_new.db"
    DB_PATH="/app/data/chimera.db"
    echo "Running inside Docker container"
elif [ -f "./data/roster_new.db" ]; then
    ROSTER_PATH="./data/roster_new.db"
    DB_PATH="./data/chimera.db"
    echo "Running locally"
else
    echo "ERROR: roster_new.db not found"
    echo "Expected locations:"
    echo "  - /app/data/roster_new.db (inside container)"
    echo "  - ./data/roster_new.db (local)"
    exit 1
fi

# Check if roster file exists
if [ ! -f "$ROSTER_PATH" ]; then
    echo "ERROR: Roster file not found at $ROSTER_PATH"
    exit 1
fi

echo "Roster file: $ROSTER_PATH"
echo "Database: $DB_PATH"
echo ""

# Try API endpoint first (if available)
if command -v curl &> /dev/null; then
    echo "Attempting to trigger merge via API..."
    RESPONSE=$(curl -s -X POST http://localhost:8080/api/v1/roster/merge \
        -H "Content-Type: application/json" \
        -d '{}' 2>&1)
    
    if echo "$RESPONSE" | grep -q "wallets_merged"; then
        echo "✓ Merge successful via API"
        echo "$RESPONSE" | jq '.' 2>/dev/null || echo "$RESPONSE"
        exit 0
    elif echo "$RESPONSE" | grep -q "authentication_failed"; then
        echo "⚠ API requires authentication, trying direct merge..."
    else
        echo "API response: $RESPONSE"
    fi
fi

# Direct merge using SQLite (if available)
if command -v sqlite3 &> /dev/null; then
    echo "Performing direct merge using SQLite..."
    
    # Check wallet count in roster
    ROSTER_COUNT=$(sqlite3 "$ROSTER_PATH" "SELECT COUNT(*) FROM wallets;" 2>/dev/null || echo "0")
    echo "Wallets in roster: $ROSTER_COUNT"
    
    if [ "$ROSTER_COUNT" -eq "0" ]; then
        echo "WARNING: Roster file contains 0 wallets"
        exit 1
    fi
    
    # Perform merge
    sqlite3 "$DB_PATH" <<EOF
ATTACH DATABASE '$ROSTER_PATH' AS new_roster;

-- Check integrity
SELECT 'Integrity check: ' || GROUP_CONCAT(result) FROM pragma_integrity_check('new_roster');

-- Count before
SELECT 'Wallets before merge: ' || COUNT(*) FROM wallets;

-- Delete existing wallets
DELETE FROM wallets;

-- Insert from new roster
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

-- Count after
SELECT 'Wallets after merge: ' || COUNT(*) FROM wallets;

DETACH DATABASE new_roster;
EOF
    
    if [ $? -eq 0 ]; then
        echo "✓ Direct merge completed successfully"
        FINAL_COUNT=$(sqlite3 "$DB_PATH" "SELECT COUNT(*) FROM wallets;" 2>/dev/null || echo "0")
        echo "Total wallets in database: $FINAL_COUNT"
        exit 0
    else
        echo "ERROR: Direct merge failed"
        exit 1
    fi
else
    echo "ERROR: Neither API nor sqlite3 available"
    echo "Please ensure:"
    echo "  1. Operator service is running and accessible"
    echo "  2. Or sqlite3 is installed for direct merge"
    exit 1
fi
