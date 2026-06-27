# Authentication Quick Start

## Quick Setup (3 Steps)

### Step 1: Add Your Wallet as Admin

**Option A: Using the script (Recommended)**
```bash
./add-admin-wallet.sh YOUR_WALLET_ADDRESS admin
```

**Option B: Direct database insert**
```bash
python3 << EOF
import sqlite3
conn = sqlite3.connect('data/chimera.db')
conn.execute("""
    INSERT OR REPLACE INTO admin_wallets (wallet_address, role, added_by)
    VALUES ('YOUR_WALLET_ADDRESS', 'admin', 'MANUAL')
""")
conn.commit()
conn.close()
print("✓ Wallet added")
EOF
```

### Step 2: Authenticate and Merge Roster

Use the automated script:
```bash
./authenticate-and-merge.sh YOUR_WALLET_ADDRESS /path/to/your-keypair.json
```

Or manually:

1. **Sign a message** (using Solana CLI):
```bash
WALLET="YOUR_WALLET_ADDRESS"
TIMESTAMP=$(date +%s)
MESSAGE="Chimera Dashboard Authentication
Wallet: $WALLET
Timestamp: $TIMESTAMP"

SIG_B64=$(echo -e "$MESSAGE" | solana message sign --keypair /path/to/keypair.json | base64)
```

2. **Get JWT token**:
```bash
TOKEN=$(curl -s -X POST http://localhost:8080/api/v1/auth/wallet \
  -H "Content-Type: application/json" \
  -d "{
    \"wallet_address\": \"$WALLET\",
    \"message\": \"$MESSAGE\",
    \"signature\": \"$SIG_B64\"
  }" | python3 -c "import sys, json; print(json.load(sys.stdin)['token'])")
```

3. **Merge roster**:
```bash
curl -X POST http://localhost:8080/api/v1/roster/merge \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{}'
```

### Step 3: Verify

```bash
curl http://localhost:8080/api/v1/wallets | python3 -m json.tool
```

You should see your wallets listed!

## Prerequisites

1. **Solana CLI installed**: [Install Guide](https://docs.solana.com/cli/install-solana-cli-tools)
2. **Keypair file**: Your Solana wallet keypair (JSON format)
3. **Wallet address**: Your Solana wallet public key

## Common Issues

### "Wallet not authorized for dashboard access"
→ Your wallet isn't in the `admin_wallets` table. Run Step 1.

### "Invalid signature verification"
→ Check that:
- Message format is correct (must contain "Chimera Dashboard Authentication")
- Signature is base64 encoded
- You're using the correct keypair

### "Database locked"
→ The operator is using the database. Stop it temporarily:
```bash
./docker/docker-compose.sh stop mainnet-paper operator
# ... do your database operations ...
./docker/docker-compose.sh start mainnet-paper
```

## Full Documentation

See [AUTHENTICATION_GUIDE.md](./AUTHENTICATION_GUIDE.md) for complete details.




