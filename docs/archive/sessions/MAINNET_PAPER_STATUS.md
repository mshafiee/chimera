# Mainnet Paper Trading - Setup Status

## ✅ Setup Complete

The bot is now running in **Mainnet Paper Trading** mode!

### Current Status

- ✅ Services started successfully
- ✅ Database initialized
- ✅ Webhook secret generated
- ⚠️ **Helius API key needs to be configured** (required for RPC calls)

## ⚠️ Important: Configure Helius API Key

**Before the bot can make RPC calls, you need to add your Helius API key:**

1. Get your API key from: https://www.helius.dev/ (free tier available)

2. Edit `docker/env.mainnet-paper.local`:
   ```bash
   nano docker/env.mainnet-paper.local
   ```

3. Replace `YOUR_HELIUS_API_KEY` in these two lines:
   ```
   SOLANA_RPC_URL=https://mainnet.helius-rpc.com/?api-key=YOUR_HELIUS_API_KEY
   CHIMERA_RPC__PRIMARY_URL=https://mainnet.helius-rpc.com/?api-key=YOUR_HELIUS_API_KEY
   ```

4. Restart the operator service:
   ```bash
   ./docker/docker-compose.sh restart mainnet-paper operator
   ```

## Service URLs

- **Operator API**: http://localhost:8080
- **Web Dashboard**: http://localhost:3000  
- **Grafana**: http://localhost:3002 (admin/change-me-secure-password)
- **Prometheus**: http://localhost:9090

## Verify Services

```bash
# Check health
curl http://localhost:8080/api/v1/health

# View logs
./docker/docker-compose.sh logs mainnet-paper -f

# Check specific service
./docker/docker-compose.sh logs mainnet-paper -f operator
```

## Paper Trading Mode

- ✅ **Real Mainnet Data**: Uses actual mainnet blockchain data
- ✅ **Simulated Trades**: All trades are simulated (no real funds at risk)
- ✅ **Production-like**: Uses production thresholds and settings
- ✅ **Jito Enabled**: MEV protection enabled for mainnet

## Next Steps

1. **Add Helius API key** (required for RPC calls)
2. **Monitor logs** to verify RPC connectivity
3. **Test webhook endpoint** with a signal
4. **Check Grafana dashboard** for metrics
5. **Verify paper trading mode** is active

## Troubleshooting

### RPC Connection Errors
- Verify Helius API key is correct
- Check operator logs: `./docker/docker-compose.sh logs mainnet-paper -f operator`
- Ensure API key has mainnet access

### Service Not Starting
- Check logs: `./docker/docker-compose.sh logs mainnet-paper`
- Verify Docker has enough resources
- Check port conflicts (8080, 3000, 3002, 9090, 9093)

### Database Issues
- Reinitialize: `./docker/docker-compose.sh init-db mainnet-paper`
- Check data directory permissions

## Commands Reference

```bash
# Start services
./docker/docker-compose.sh start mainnet-paper

# Stop services
./docker/docker-compose.sh stop mainnet-paper

# View logs
./docker/docker-compose.sh logs mainnet-paper -f

# Restart a service
./docker/docker-compose.sh restart mainnet-paper operator

# Check status
docker ps | grep chimera
```
