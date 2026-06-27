# ROOT CAUSE: No Trading Activity

## The Problem
Everything is configured, but there's no trading activity after hours.

## Root Cause Found

**Monitoring routes are NOT registered in the router!**

### The Issue

In `operator/src/main.rs`, the monitoring routes are created but:

1. **Line 442-460**: Monitoring routes try to use `engine_handle` which doesn't exist at that point
2. **MonitoringState::new() fails** because `engine_handle` is not in scope
3. **Falls back to empty Router** - monitoring endpoints return 404
4. **Result**: Monitoring cannot be enabled, no webhooks registered, no trading signals

### Evidence

```bash
# Monitoring endpoint returns 404
curl http://localhost:8080/api/v1/monitoring/status
# HTTP/1.1 404 Not Found

# Database shows no monitoring enabled
sqlite3 data/chimera.db "SELECT * FROM wallet_monitoring;"
# (empty - no records)
```

### Why This Prevents Trading

The system uses **Helius webhooks** for automatic copy trading:

1. Helius monitors wallets on-chain
2. When wallet trades, Helius sends webhook to operator
3. Operator receives at `/api/v1/monitoring/helius-webhook`
4. Operator parses and queues trade to copy

**BUT**:
- Monitoring routes don't exist (404)
- Cannot enable monitoring for wallets
- Cannot register Helius webhooks
- **No webhooks received = No trading signals = No trades**

## Fix Applied

I've updated the code to:
1. ✅ Create MonitoringState using `_engine_handle` (available earlier)
2. ✅ Register monitoring routes properly
3. ⚠️ Fix handler return types (in progress - compilation errors)

## Status

- ✅ Root cause identified
- ✅ Code updated to register routes
- ⚠️ Compilation errors need fixing
- ⏳ Needs rebuild

## Next Steps

1. **Fix remaining compilation errors** (return type mismatches in handlers)
2. **Rebuild operator**:
   ```bash
   cd operator
   cargo build --release
   ```

3. **Rebuild Docker**:
   ```bash
   ./docker/docker-compose.sh build mainnet-paper
   ```

4. **Restart**:
   ```bash
   ./docker/docker-compose.sh restart mainnet-paper operator
   ```

5. **Verify**:
   ```bash
   curl http://localhost:8080/api/v1/monitoring/status
   ```

6. **Enable monitoring**:
   ```bash
   curl -X POST http://localhost:8080/api/v1/monitoring/wallets/{ACTIVE_WALLET_ADDRESS}/enable
   ```

Once fixed and restarted, monitoring will work and trading will begin!

