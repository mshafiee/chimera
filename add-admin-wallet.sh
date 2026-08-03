#!/bin/bash
# Add an admin wallet to the database
# Usage: ./add-admin-wallet.sh <wallet-address> [role]

set -e

if [ $# -lt 1 ]; then
    echo "Usage: $0 <wallet-address> [role]"
    echo ""
    echo "Roles: admin, operator, readonly"
    echo "Default: admin"
    echo ""
    echo "Example:"
    echo "  $0 7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU admin"
    exit 1
fi

WALLET_ADDRESS="$1"
ROLE="${2:-admin}"

# Validate role
if [[ ! "$ROLE" =~ ^(admin|operator|readonly)$ ]]; then
    echo "Error: Role must be one of: admin, operator, readonly"
    exit 1
fi

# Validate wallet address format (Solana base58, 32-44 chars)
if [[ ! "$WALLET_ADDRESS" =~ ^[1-9A-HJ-NP-Za-km-z]{32,44}$ ]]; then
    echo "Error: Invalid wallet address: $WALLET_ADDRESS"
    exit 1
fi

# Check if database exists
if [ ! -f "data/chimera.db" ]; then
    echo "Error: Database not found at data/chimera.db"
    echo "Make sure the operator has been started at least once."
    exit 1
fi

echo "Adding admin wallet to database..."
echo "  Address: $WALLET_ADDRESS"
echo "  Role: $ROLE"
echo ""

# Use Python to insert (works in both host and container).
# The address is passed via environment (quoted heredoc) so no shell or
# Python interpretation of the input can occur.
if WALLET_ADDRESS="$WALLET_ADDRESS" ROLE="$ROLE" python3 << 'PYTHON_SCRIPT'
import sqlite3
import sys
import os

db_path = 'data/chimera.db'
wallet_address = os.environ['WALLET_ADDRESS']
role = os.environ['ROLE']

try:
    conn = sqlite3.connect(db_path)
    cursor = conn.cursor()
    
    # Check if table exists
    cursor.execute("SELECT name FROM sqlite_master WHERE type='table' AND name='admin_wallets'")
    if not cursor.fetchone():
        print("Creating admin_wallets table...")
        cursor.execute("""
            CREATE TABLE IF NOT EXISTS admin_wallets (
                wallet_address TEXT PRIMARY KEY,
                role TEXT NOT NULL DEFAULT 'readonly'
                    CHECK(role IN ('admin', 'operator', 'readonly')),
                added_by TEXT NOT NULL DEFAULT 'SCRIPT',
                notes TEXT,
                added_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            )
        """)
    
    # Upsert wallet, preserving existing metadata (added_at, notes)
    cursor.execute("""
        INSERT INTO admin_wallets (wallet_address, role, added_by, added_at)
        VALUES (?, ?, 'SCRIPT', CURRENT_TIMESTAMP)
        ON CONFLICT(wallet_address) DO UPDATE SET
            role = excluded.role,
            added_by = 'SCRIPT'
    """, (wallet_address, role))
    
    conn.commit()
    
    # Verify
    cursor.execute("SELECT wallet_address, role FROM admin_wallets WHERE wallet_address = ?", (wallet_address,))
    result = cursor.fetchone()
    
    if result:
        print(f"✓ Successfully added wallet: {result[0]} with role: {result[1]}")
    else:
        print("Error: Wallet was not added")
        sys.exit(1)
    
    conn.close()
    
except sqlite3.Error as e:
    print(f"Database error: {e}")
    sys.exit(1)
except Exception as e:
    print(f"Error: {e}")
    sys.exit(1)
PYTHON_SCRIPT
then
    echo ""
    echo "✓ Admin wallet added successfully!"
    echo ""
    echo "You can now authenticate with this wallet using:"
    echo "  ./authenticate-and-merge.sh $WALLET_ADDRESS /path/to/keypair.json"
else
    echo ""
    echo "✗ Failed to add admin wallet"
    exit 1
fi
