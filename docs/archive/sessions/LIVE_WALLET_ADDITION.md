# Live Wallet Addition Setup

## Overview

The Chimera system now supports **automatic wallet addition** when wallets are detected making trades. This enables live discovery and tracking of profitable wallets without manual intervention.

## How It Works

### Automatic Addition Flow

1. **Webhook Detection**: Helius webhook receives transaction data
2. **Wallet Extraction**: System extracts wallet address from transaction
3. **Auto-Add**: If wallet doesn't exist, it's automatically added as CANDIDATE
4. **Scout Analysis**: Scout analyzes the wallet and calculates WQS score
5. **Promotion**: High-quality wallets can be promoted to ACTIVE

### Implementation

The system automatically adds wallets in two scenarios:

1. **Helius Webhook**: When a wallet makes a swap transaction detected via webhook
2. **Manual Addition**: Via API or database insert

## Setup Test Wallet

### Step 1: Derive Wallet Address

Your test wallet mnemonic:
```
tower squirrel silly adult derive case behave crisp ketchup other topic tray
```

Derived wallet address: **`12BHwSkzXoR1M2SXkEc8L68xBZDFzArg5WJXNWKPLERA`**

### Step 2: Setup Wallet

Run the setup script:

```bash
./setup-test-wallet.sh
```

This will:
- ✅ Add wallet to `admin_wallets` (for authentication)
- ✅ Add wallet to `wallets` table as CANDIDATE
- ✅ Display wallet information

### Step 3: Analyze Wallet

Run Scout to analyze the wallet:

```bash
# Add wallet to Scout's analysis list
echo "12BHwSkzXoR1M2SXkEc8L68xBZDFzArg5WJXNWKPLERA" >> scout/config/wallets.txt

# Copy to container
docker cp scout/config/wallets.txt chimera-scout:/app/config/wallets.txt

# Run Scout
docker exec chimera-scout python3 /app/main.py --verbose --output /app/data/roster_new.db
```

### Step 4: Merge Roster

After Scout analyzes the wallet:

```bash
# Merge roster (requires authentication)
./authenticate-and-merge.sh YOUR_ADMIN_WALLET /path/to/keypair.json
```

## Live Addition Configuration

### Enable Auto-Add from Webhooks

The system is **already configured** to automatically add wallets when detected via Helius webhooks. No additional configuration needed!

When a webhook receives a transaction:
1. System extracts wallet address
2. Checks if wallet exists in database
3. If not found, automatically adds as CANDIDATE
4. Logs the addition for tracking

### Monitor Auto-Added Wallets

Check logs for auto-added wallets:

```bash
docker logs chimera-operator --tail 100 | grep "New wallet detected"
```

### View Auto-Added Wallets

```bash
# Check wallets with auto-add notes
python3 -c "
import sqlite3
conn = sqlite3.connect('data/chimera.db')
cursor = conn.cursor()
cursor.execute(\"SELECT address, status, notes FROM wallets WHERE notes LIKE '%Auto-added%' OR notes LIKE '%auto-added%'\")
print('Auto-Added Wallets:')
for row in cursor.fetchall():
    print(f'  {row[0][:20]}... | {row[1]:10} | {row[2]}')
conn.close()
"
```

## Workflow

### Complete Live Addition Workflow

```
┌─────────────────┐
│ Helius Webhook  │
│ (Transaction)   │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Extract Wallet  │
│   Address       │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Wallet Exists?  │
└────────┬────────┘
         │
    ┌────┴────┐
    │          │
   NO         YES
    │          │
    ▼          ▼
┌─────────┐  ┌──────────────┐
│ Auto-Add│  │ Check Status │
│ CANDIDATE│  │   (ACTIVE?)  │
└────┬────┘  └──────┬───────┘
     │               │
     └───────┬───────┘
             │
             ▼
      ┌──────────────┐
      │ Queue Signal │
      │  (if ACTIVE)  │
      └──────────────┘
```

## Configuration Options

### Auto-Add Settings

The auto-add feature is enabled by default. You can control it via:

1. **Webhook Processing**: Always enabled (no config needed)
2. **Initial Status**: New wallets are added as `CANDIDATE`
3. **Auto-Promotion**: Disabled (requires manual promotion or Scout analysis)

### Scout Analysis

After a wallet is auto-added:
- Scout will analyze it on the next run
- Calculates WQS score and metrics
- Updates wallet status based on WQS

## Manual Addition

You can also manually add wallets:

### Via API (with authentication)

```bash
# First, add wallet to database
python3 << EOF
import sqlite3
conn = sqlite3.connect('data/chimera.db')
conn.execute("""
    INSERT OR REPLACE INTO wallets (address, status, notes)
    VALUES (?, 'CANDIDATE', 'Manually added')
""", ("WALLET_ADDRESS",))
conn.commit()
conn.close()
EOF

# Then promote if needed (requires auth)
curl -X PUT http://localhost:8080/api/v1/wallets/WALLET_ADDRESS \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"status": "ACTIVE"}'
```

### Via Scout Wallet List

Add to `scout/config/wallets.txt`:

```bash
echo "WALLET_ADDRESS" >> scout/config/wallets.txt
docker cp scout/config/wallets.txt chimera-scout:/app/config/wallets.txt
docker exec chimera-scout python3 /app/main.py
```

## Monitoring Auto-Added Wallets

### Check Recent Additions

```bash
# View recently added wallets
python3 -c "
import sqlite3
from datetime import datetime, timedelta
conn = sqlite3.connect('data/chimera.db')
cursor = conn.cursor()
cursor.execute(\"\"\"
    SELECT address, status, created_at, notes
    FROM wallets
    WHERE created_at > datetime('now', '-1 day')
    ORDER BY created_at DESC
    LIMIT 10
\"\"\")
print('Recently Added Wallets:')
for row in cursor.fetchall():
    print(f'  {row[0][:20]}... | {row[1]:10} | {row[2]} | {row[3][:50]}')
conn.close()
"
```

### Enable Monitoring for Auto-Added Wallets

Once a wallet is promoted to ACTIVE, enable monitoring:

```bash
curl -X POST http://localhost:8080/api/v1/monitoring/wallets/WALLET_ADDRESS/enable \
  -H "Authorization: Bearer <token>"
```

## Test Wallet Information

- **Address**: `12BHwSkzXoR1M2SXkEc8L68xBZDFzArg5WJXNWKPLERA`
- **Mnemonic**: `tower squirrel silly adult derive case behave crisp ketchup other topic tray`
- **Status**: Will be added as CANDIDATE
- **Explorer**: https://explorer.solana.com/address/12BHwSkzXoR1M2SXkEc8L68xBZDFzArg5WJXNWKPLERA

## Next Steps

1. ✅ Run `./setup-test-wallet.sh` to add test wallet
2. ✅ Run Scout to analyze the wallet
3. ✅ Merge roster to update database
4. ✅ Promote wallet to ACTIVE if WQS is high
5. ✅ Enable monitoring for live trade copying

The system is now ready for **live wallet addition**!




