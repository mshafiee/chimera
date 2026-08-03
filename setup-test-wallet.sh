#!/bin/bash
# Setup test wallet from mnemonic seed phrase
# Usage: ./setup-test-wallet.sh
# The mnemonic is read from the MNEMONIC environment variable (never committed
# to source); if unset, derive-wallet.py prompts interactively.

set -e

echo "=== Setting Up Test Wallet ==="
echo ""

# Derive wallet address
echo "Deriving wallet address from mnemonic..."
if [ -n "${MNEMONIC:-}" ]; then
    WALLET_ADDRESS=$(MNEMONIC="$MNEMONIC" python3 derive-wallet.py 2>&1 | grep "Wallet Address:" | cut -d' ' -f3)
else
    WALLET_ADDRESS=$(python3 derive-wallet.py 2>&1 | grep "Wallet Address:" | cut -d' ' -f3)
fi

if [ -z "$WALLET_ADDRESS" ]; then
    echo "Error: Failed to derive wallet address"
    exit 1
fi

# Validate Solana address format (base58, 32-44 chars)
if [[ ! "$WALLET_ADDRESS" =~ ^[1-9A-HJ-NP-Za-km-z]{32,44}$ ]]; then
    echo "Error: Derived value is not a valid Solana address: $WALLET_ADDRESS"
    exit 1
fi

echo "Wallet Address: $WALLET_ADDRESS"
echo ""

# Add to admin_wallets
echo "Adding wallet to admin_wallets..."
./add-admin-wallet.sh "$WALLET_ADDRESS" admin

echo ""

# Add to wallets table as CANDIDATE (will be analyzed by Scout).
# The address is passed via environment so no SQL interpretation of input occurs.
echo "Adding wallet to wallets table..."
WALLET_ADDRESS="$WALLET_ADDRESS" python3 << 'EOF'
import os
import sqlite3

conn = sqlite3.connect('data/chimera.db')
cursor = conn.cursor()

# Insert the wallet without wiping existing analysis data on re-run
cursor.execute("""
    INSERT INTO wallets (
        address, status, wqs_score, trade_count_30d, avg_trade_size_sol,
        notes, created_at, updated_at
    ) VALUES (
        ?, 'CANDIDATE', NULL, 0, 0.0,
        'Test wallet - auto-added from mnemonic', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
    )
    ON CONFLICT(address) DO NOTHING
""", (os.environ['WALLET_ADDRESS'],))

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
