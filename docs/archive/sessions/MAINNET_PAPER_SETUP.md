# Mainnet Paper Trading Setup Guide

## Quick Setup

### 1. Configure Helius API Key (Required)

Edit `docker/env.mainnet-paper.local` and replace `YOUR_HELIUS_API_KEY` with your actual key:

```bash
# Get your API key from: https://www.helius.dev/
# Then edit the file:
nano docker/env.mainnet-paper.local

# Replace these lines:
SOLANA_RPC_URL=https://mainnet.helius-rpc.com/?api-key=YOUR_HELIUS_API_KEY
CHIMERA_RPC__PRIMARY_URL=https://mainnet.helius-rpc.com/?api-key=YOUR_HELIUS_API_KEY
```

### 2. Generate Webhook Secret

```bash
# Generate a secure secret
openssl rand -hex 32

# Update docker/env.mainnet-paper.local:
# CHIMERA_SECURITY__WEBHOOK_SECRET=<generated-secret>
```

### 3. Initialize Database

```bash
./docker/docker-compose.sh init-db mainnet-paper
```

### 4. Start Services

```bash
./docker/docker-compose.sh start mainnet-paper
```

### 5. Verify Services

```bash
# Check health
curl http://localhost:8080/api/v1/health

# View logs
./docker/docker-compose.sh logs mainnet-paper -f
```

## Service URLs

- **Operator API**: http://localhost:8080
- **Web Dashboard**: http://localhost:3000
- **Grafana**: http://localhost:3002 (admin/change-me-secure-password)
- **Prometheus**: http://localhost:9090

## Important Notes

- **Paper Trading Mode**: All trades are simulated (no real funds at risk)
- **Real Mainnet Data**: Uses actual mainnet blockchain data
- **Helius API Key**: Required for RPC access (free tier available)
- **Webhook Secret**: Should be unique and secure

## Troubleshooting

### RPC Errors
If you see RPC connection errors, verify your Helius API key is correct in `docker/env.mainnet-paper.local`.

### Service Won't Start
Check logs: `./docker/docker-compose.sh logs mainnet-paper`

### Database Issues
Reinitialize: `./docker/docker-compose.sh init-db mainnet-paper`

## Next Steps

1. Monitor the operator logs for any issues
2. Test webhook endpoint with a signal
3. Check Grafana dashboard for metrics
4. Verify paper trading mode is active (trades should be simulated)
