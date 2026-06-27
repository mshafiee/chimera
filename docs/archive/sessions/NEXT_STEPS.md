# Next Steps: Complete Monitoring Setup

## ✅ Current Status

**Monitoring routes are now working!**

- ✅ `/api/v1/monitoring/status` - Working (returns JSON)
- ✅ `/api/v1/monitoring/wallets/{address}/enable` - Available
- ✅ `/api/v1/monitoring/wallets/{address}/disable` - Available
- ✅ MonitoringState initialized successfully
- ⚠️ Enable endpoint returns 500 (webhook URL not configured)

## Issue: Helius Webhook URL Not Configured

The enable endpoint is failing with:
```
"Invalid webhook URL format"
```

**Root Cause**: `CHIMERA_MONITORING__HELIUS_WEBHOOK_URL` is not set in the environment.

## Solution

### 1. Configure Helius Webhook URL

The webhook URL must be a **publicly accessible URL** where Helius can send webhooks.

**For localhost/development**, you have options:

#### Option A: Use a tunnel service (recommended for testing)
```bash
# Using ngrok (install: brew install ngrok)
ngrok http 8080

# This gives you a public URL like: https://abc123.ngrok.io
# Use: https://abc123.ngrok.io/api/v1/monitoring/helius-webhook
```

#### Option B: Use your public server IP
If you have a public server:
```bash
# Format: http://YOUR_PUBLIC_IP:8080/api/v1/monitoring/helius-webhook
# Or with domain: https://yourdomain.com/api/v1/monitoring/helius-webhook
```

### 2. Add to Environment File

Edit `docker/env.mainnet-paper` (or create `docker/env.mainnet-paper.local`):

```bash
# Monitoring Configuration
CHIMERA_MONITORING__ENABLED=true
CHIMERA_MONITORING__HELIUS_API_KEY=your-helius-api-key
CHIMERA_MONITORING__HELIUS_WEBHOOK_URL=http://your-public-url:8080/api/v1/monitoring/helius-webhook
CHIMERA_MONITORING__WEBHOOK_PROCESSING_RATE_LIMIT=40
CHIMERA_MONITORING__RPC_POLL_RATE_LIMIT=40
```

### 3. Restart Operator

```bash
./docker/docker-compose.sh restart mainnet-paper operator
```

### 4. Enable Monitoring for Wallets

Once webhook URL is configured:

```bash
# Get ACTIVE wallets
curl -s http://localhost:8080/api/v1/wallets | python3 -c "
import sys, json
data = json.load(sys.stdin)
for w in data.get('wallets', []):
    if w.get('status') == 'ACTIVE':
        print(w['address'])
"

# Enable for each wallet
curl -X POST http://localhost:8080/api/v1/monitoring/wallets/{WALLET_ADDRESS}/enable
```

### 5. Verify Webhooks Registered

```bash
# Check database
sqlite3 data/chimera.db "SELECT wallet_address, monitoring_enabled, helius_webhook_id FROM wallet_monitoring;"

# Check monitoring status
curl http://localhost:8080/api/v1/monitoring/status | python3 -m json.tool
```

## Expected Result

Once configured:
- ✅ Wallets enabled for monitoring
- ✅ Helius webhooks registered
- ✅ Trading signals received when wallets trade
- ✅ Trades executed automatically

## Summary

**All code issues are fixed!** The only remaining step is configuring the Helius webhook URL in your environment file. Once that's set, the system will be fully operational for automatic copy trading.
