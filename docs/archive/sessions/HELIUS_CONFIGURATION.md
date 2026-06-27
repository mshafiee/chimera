# Helius API Configuration - Mainnet Paper Trading

## ✅ Configuration Complete

All Helius endpoints have been configured with your API key: `609cb910-17a5-4a76-9d1b-2ca9c42f759e`

## Configured Endpoints

### 1. HTTP RPC Endpoint (Primary)
```
https://mainnet.helius-rpc.com/?api-key=609cb910-17a5-4a76-9d1b-2ca9c42f759e
```
- **Usage**: Primary RPC endpoint for all Solana RPC calls
- **Status**: ✅ Configured in `CHIMERA_RPC__PRIMARY_URL`

### 2. WebSocket Endpoint (WSS)
```
wss://mainnet.helius-rpc.com/?api-key=609cb910-17a5-4a76-9d1b-2ca9c42f759e
```
- **Usage**: Real-time subscriptions and WebSocket connections
- **Status**: ✅ Documented (Solana RPC client handles WebSocket automatically)

### 3. Transaction API Endpoint
```
https://api-mainnet.helius-rpc.com/v0/transactions/?api-key=609cb910-17a5-4a76-9d1b-2ca9c42f759e
```
- **Usage**: Enhanced transaction queries via Helius Transaction API
- **Status**: ✅ Documented (used by monitoring system when available)

### 4. Address Transactions Endpoint
```
https://api-mainnet.helius-rpc.com/v0/addresses/{address}/transactions/?api-key=609cb910-17a5-4a76-9d1b-2ca9c42f759e
```
- **Usage**: Query transactions for specific wallet addresses
- **Status**: ✅ Documented (used by wallet monitoring)

## Configuration File

All endpoints are configured in: `docker/env.mainnet-paper.local`

## Service Status

After restarting the operator service, verify it's using the new configuration:

```bash
# Check health
curl http://localhost:8080/api/v1/health

# View logs
./docker/docker-compose.sh logs mainnet-paper -f operator
```

## How It Works

1. **HTTP RPC**: Used for all standard Solana RPC calls (getBalance, getTransaction, etc.)
2. **WebSocket**: Automatically used by Solana RPC client for subscriptions (when RPC URL supports it)
3. **Transaction API**: Used by monitoring system for enhanced transaction parsing
4. **Address Transactions**: Used for wallet transaction monitoring

## Verification

The operator should now be:
- ✅ Connected to Helius RPC
- ✅ Using your API key for all requests
- ✅ Ready for paper trading on mainnet

## Next Steps

1. Monitor logs to verify RPC connectivity
2. Test webhook endpoint with a signal
3. Check Grafana dashboard for metrics
4. Verify paper trading mode is active

## Troubleshooting

If you see RPC errors:
- Verify API key is correct
- Check Helius dashboard for usage/quota
- Ensure API key has mainnet access
- Check operator logs: `./docker/docker-compose.sh logs mainnet-paper -f operator`
