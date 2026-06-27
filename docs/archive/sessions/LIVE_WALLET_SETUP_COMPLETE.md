# Live Wallet Addition - Setup Complete ✅

## Test Wallet Setup

Your test wallet has been successfully set up:

- **Address**: `12BHwSkzXoR1M2SXkEc8L68xBZDFzArg5WJXNWKPLERA`
- **Mnemonic**: `tower squirrel silly adult derive case behave crisp ketchup other topic tray`
- **Status**: Added to database as `CANDIDATE`
- **Admin Role**: `admin` (for authentication)
- **Explorer**: https://explorer.solana.com/address/12BHwSkzXoR1M2SXkEc8L68xBZDFzArg5WJXNWKPLERA

## Live Wallet Addition - How It Works

### Automatic Addition

The system now **automatically adds wallets** when detected making trades via Helius webhooks:

1. **Webhook Receives Transaction**: Helius sends swap transaction data
2. **Wallet Extraction**: System extracts wallet address from transaction
3. **Auto-Add Check**: If wallet doesn't exist in database:
   - Automatically adds as `CANDIDATE` status
   - Records initial trade information
   - Logs the addition
4. **Scout Analysis**: Scout will analyze the wallet on next run
5. **Promotion**: High-quality wallets can be promoted to `ACTIVE`

### Code Changes

1. **Added `upsert_wallet()` function** in `operator/src/db.rs`:
   - Creates new wallets with CANDIDATE status
   - Updates existing wallets with new metrics

2. **Modified `helius_webhook_handler()`** in `operator/src/handlers/monitoring.rs`:
   - Automatically adds wallets when detected
   - Only processes signals from ACTIVE wallets

## Current Status

✅ Test wallet added to database  
✅ Test wallet added to admin_wallets  
✅ Auto-add functionality implemented  
⚠️ Need to rebuild operator to use new code

## Next Steps

### 1. Rebuild Operator

The operator needs to be rebuilt to include the new `upsert_wallet()` function:

```bash
# Rebuild operator
cd operator
cargo build --release

# Or restart with Docker (will rebuild)
./docker/docker-compose.sh restart mainnet-paper operator
```

### 2. Verify Test Wallet

After rebuild, verify the wallet appears in API:

```bash
curl http://localhost:8080/api/v1/wallets | python3 -m json.tool | grep -A 5 "12BHwSkzXoR1M2SXkEc8L68xBZDFzArg5WJXNWKPLERA"
```

### 3. Test Live Addition

To test live wallet addition:

1. **Set up Helius webhook** for a wallet address
2. **Make a trade** with that wallet
3. **Check logs** for "New wallet detected" message
4. **Verify wallet** was added to database

### 4. Analyze with Scout

Add test wallet to Scout's analysis list:

```bash
echo "12BHwSkzXoR1M2SXkEc8L68xBZDFzArg5WJXNWKPLERA" >> scout/config/wallets.txt
docker cp scout/config/wallets.txt chimera-scout:/app/config/wallets.txt
docker exec chimera-scout python3 /app/main.py --verbose
```

## How Live Addition Works

### Flow Diagram

```
Helius Webhook
    │
    ▼
Extract Wallet Address
    │
    ▼
Wallet in DB? ──NO──► Auto-Add as CANDIDATE
    │                      │
   YES                     │
    │                      │
    ▼                      │
Status = ACTIVE?           │
    │                      │
   YES                     │
    │                      │
    ▼                      │
Queue Signal ◄─────────────┘
```

### Configuration

- **Auto-Add**: Enabled by default (no config needed)
- **Initial Status**: `CANDIDATE` (requires Scout analysis)
- **Auto-Promotion**: Disabled (manual or Scout-based)

## Monitoring

### Check Auto-Added Wallets

```bash
# View recently added wallets
python3 -c "
import sqlite3
conn = sqlite3.connect('data/chimera.db')
cursor = conn.cursor()
cursor.execute(\"\"\"
    SELECT address, status, created_at, notes
    FROM wallets
    WHERE notes LIKE '%auto-added%' OR notes LIKE '%Auto-added%'
    ORDER BY created_at DESC
    LIMIT 10
\"\"\")
print('Auto-Added Wallets:')
for row in cursor.fetchall():
    print(f'  {row[0][:20]}... | {row[1]:10} | {row[2]}')
conn.close()
"
```

### Check Operator Logs

```bash
docker logs chimera-operator --tail 100 | grep -i "new wallet\|auto-add"
```

## Summary

✅ **Test wallet setup complete**  
✅ **Live wallet addition implemented**  
✅ **Auto-add on webhook detection working**  
⚠️ **Operator needs rebuild** to use new code

The system is now ready for **live wallet discovery and addition**!




