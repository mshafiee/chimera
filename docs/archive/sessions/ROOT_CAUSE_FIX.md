# Root Cause: Monitoring Routes Not Registered

## Problem Identified

The monitoring routes are **empty** in `operator/src/main.rs` line 446:

```rust
let monitoring_routes = Router::new();  // EMPTY!
```

Even though:
- ✅ Monitoring handlers exist (`get_monitoring_status`, `enable_wallet_monitoring`, etc.)
- ✅ MonitoringState can be created
- ✅ Configuration may be set

**The routes are never actually registered**, so:
- `/api/v1/monitoring/status` returns 404
- `/api/v1/monitoring/wallets/{address}/enable` returns 404
- Monitoring cannot be enabled
- No webhooks can be registered
- **No trading signals can be received**

## Fix Applied

I've updated `operator/src/main.rs` to:
1. Create MonitoringState
2. Register all monitoring routes
3. Make MonitoringStatus struct public

## Next Steps

1. **Rebuild the operator**:
   ```bash
   cd operator
   cargo build --release
   ```

2. **Rebuild Docker image**:
   ```bash
   ./docker/docker-compose.sh build mainnet-paper operator
   ```

3. **Restart services**:
   ```bash
   ./docker/docker-compose.sh restart mainnet-paper operator
   ```

4. **Verify monitoring routes are available**:
   ```bash
   curl http://localhost:8080/api/v1/monitoring/status
   ```

5. **Enable monitoring for ACTIVE wallets**:
   ```bash
   # Get ACTIVE wallet addresses
   curl http://localhost:8080/api/v1/wallets | python3 -m json.tool | grep -A 5 "ACTIVE"
   
   # Enable for each wallet
   curl -X POST http://localhost:8080/api/v1/monitoring/wallets/{WALLET_ADDRESS}/enable
   ```

## Expected Result

After rebuild and restart:
- Monitoring endpoints will be available
- Wallets can be enabled for monitoring
- Helius webhooks can be registered
- Trading signals will be received when wallets trade

