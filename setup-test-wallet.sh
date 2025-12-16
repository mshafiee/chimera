#!/bin/bash
# Setup test wallet from mnemonic seed phrase
# Usage: ./setup-test-wallet.sh

set -e

MNEMONIC="tower squirrel silly adult derive case behave crisp ketchup other topic tray"

echo "=== Setting Up Test Wallet ==="
echo ""

# Derive wallet address
echo "Deriving wallet address from mnemonic..."
WALLET_ADDRESS=$(python3 derive-wallet.py 2>&1 | grep "Wallet Address:" | cut -d' ' -f3)

if [ -z "$WALLET_ADDRESS" ]; then
    echo "Error: Failed to derive wallet address"
    exit 1
fi

echo "Wallet Address: $WALLET_ADDRESS"
echo ""

# Add to admin_wallets
echo "Adding wallet to admin_wallets..."
./add-admin-wallet.sh "$WALLET_ADDRESS" admin

echo ""

# Add to wallets table as CANDIDATE (will be analyzed by Scout)
echo "Adding wallet to wallets table..."
python3 << EOF
import sqlite3
conn = sqlite3.connect('data/chimera.db')
cursor = conn.cursor()

# Insert or update wallet
cursor.execute("""
    INSERT OR REPLACE INTO wallets (
        address, status, wqs_score, trade_count_30d, avg_trade_size_sol,
        notes, created_at, updated_at
    ) VALUES (
        ?, 'CANDIDATE', NULL, 0, 0.0,
        'Test wallet - auto-added from mnemonic', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
    )
""", ("$WALLET_ADDRESS",))

conn.commit()
conn.close()
print("✓ Wallet added to wallets table")
EOF

echo ""
echo "=== Test Wallet Setup Complete ==="
echo ""
echo "Wallet Address: $WALLET_ADDRESS"
echo "Status: CANDIDATE (will be analyzed by Scout)"
echo "Admin Role: admin"
echo ""
echo "Next steps:"
echo "1. Run Scout to analyze this wallet: docker exec chimera-scout python3 /app/main.py"
echo "2. Promote to ACTIVE if WQS score is high enough"
echo "3. Enable monitoring: curl -X POST http://localhost:8080/api/v1/monitoring/wallets/$WALLET_ADDRESS/enable"
echo ""
echo "Verify on Solana Explorer:"
echo "  https://explorer.solana.com/address/$WALLET_ADDRESS"






