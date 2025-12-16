#!/bin/bash
# Restart Chimera in mainnet paper trading mode with correct environment

set -e

echo "🔄 Restarting Chimera Operator in MAINNET PAPER TRADING mode..."

# Stop services
COMPOSE_PROFILE=mainnet-paper docker compose down operator

# Start with mainnet environment
COMPOSE_PROFILE=mainnet-paper docker compose up -d operator

echo "⏳ Waiting for operator to start..."
sleep 10

# Check health
echo ""
echo "📊 System Status:"
curl -s http://localhost:8080/api/v1/health | jq '{status, rpc: .rpc.status, trading: .circuit_breaker.trading_allowed}'

# Check RPC URL
echo ""
echo "🌐 Network Configuration:"
docker exec chimera-operator printenv | grep -E "SOLANA_NETWORK|PRIMARY_URL" | head -2

# Check polling
echo ""
echo "🔍 RPC Polling Status:"
docker logs chimera-operator 2>&1 | grep "RPC polling task started" | tail -1

echo ""
echo "✅ Operator restarted successfully!"
echo ""
echo "📝 Monitor logs: docker logs chimera-operator -f"
echo "📊 Check trades: curl http://localhost:8080/api/v1/trades | jq"


