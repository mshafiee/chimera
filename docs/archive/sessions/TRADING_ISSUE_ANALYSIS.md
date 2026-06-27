# Trading Activity Issue - Root Cause Analysis

## Problem
No trading activity after hours of running the system.

## Root Cause
**Monitoring is not enabled and configured.** The system requires:
1. Helius API key configured
2. Helius webhook URL configured  
3. Monitoring enabled for ACTIVE wallets

## Current Status

### ✅ What's Working
- System is healthy
- Circuit breaker is ACTIVE (trading allowed)
- 2 ACTIVE wallets in roster
- Operator is running correctly

### ❌ What's Missing
- **Monitoring endpoint**: Not available (404)
- **Helius webhook URL**: Not configured
- **Helius API key**: Still has placeholder value
- **Wallet monitoring**: Not enabled for any wallets

## How Trading Works

The system uses **Helius webhooks** for automatic copy trading:

1. **Helius monitors wallets** on-chain for transactions
2. **Helius sends webhook** to your operator when wallet trades
3. **Operator receives webhook** at `/api/v1/monitoring/helius-webhook`
4. **Operator parses transaction** and extracts swap details
5. **Operator queues trade** to copy the wallet's trade
6. **Trade executes** (in paper mode, simulated)

## Solution

### Step 1: Configure Helius API Key

Edit `docker/env.mainnet-paper` or `docker/env.mainnet-paper.local`:

```bash
# Replace YOUR_HELIUS_API_KEY with your actual Helius API key
CHIMERA_RPC__PRIMARY_URL=https://mainnet.helius-rpc.com/?api-key=YOUR_ACTUAL_API_KEY
HELIUS_API_KEY=your_actual_helius_api_key
```

### Step 2: Configure Helius Webhook URL

Add to `docker/env.mainnet-paper.local`:

```bash
# This is where Helius will send transaction notifications
# Must be publicly accessible (use ngrok or similar for local testing)
CHIMERA_MONITORING__HELIUS_WEBHOOK_URL=http://your-public-url:8080/api/v1/monitoring/helius-webhook

# Or for local testing with ngrok:
# CHIMERA_MONITORING__HELIUS_WEBHOOK_URL=https://your-ngrok-url.ngrok.io/api/v1/monitoring/helius-webhook
```

### Step 3: Enable Monitoring for Wallets

After configuration, enable monitoring for ACTIVE wallets:

```bash
# Get ACTIVE wallet addresses
curl http://localhost:8080/api/v1/wallets | python3 -m json.tool | grep -A 5 "ACTIVE"

# Enable monitoring for each ACTIVE wallet
curl -X POST http://localhost:8080/api/v1/monitoring/wallets/{WALLET_ADDRESS}/enable \
  -H "Content-Type: application/json"
```

### Step 4: Restart Services

```bash
./docker/docker-compose.sh restart mainnet-paper
```

## Alternative: Manual Webhook Testing

If you want to test without Helius webhooks, you can send manual signals:

```bash
# Generate HMAC signature (requires webhook secret)
# Then send signal:
curl -X POST http://localhost:8080/api/v1/webhook \
  -H "Content-Type: application/json" \
  -H "X-Signature: <hmac_signature>" \
  -d '{
    "wallet_address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
    "token_address": "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263",
    "token_symbol": "BONK",
    "strategy": "SHIELD",
    "action": "BUY",
    "amount_sol": 0.1
  }'
```

## Verification

After configuration, verify:

1. **Check monitoring status**:
   ```bash
   curl http://localhost:8080/api/v1/monitoring/status
   ```

2. **Check wallet monitoring**:
   ```bash
   sqlite3 data/chimera.db "SELECT * FROM wallet_monitoring WHERE monitoring_enabled = 1;"
   ```

3. **Watch for webhooks**:
   ```bash
   docker logs chimera-operator -f | grep -i webhook
   ```

## Important Notes

1. **Webhook URL must be publicly accessible** - Helius needs to reach it
   - Use ngrok for local testing: `ngrok http 8080`
   - Or deploy to a server with public IP

2. **Helius API key is required** - Without it, webhooks cannot be registered

3. **Monitoring is per-wallet** - Each ACTIVE wallet must be enabled individually

4. **Paper trading mode** - Trades are simulated, but monitoring still requires real Helius webhooks

## Quick Fix Script

I can create a script to:
1. Check configuration
2. Enable monitoring for all ACTIVE wallets
3. Verify setup

Would you like me to create this?

