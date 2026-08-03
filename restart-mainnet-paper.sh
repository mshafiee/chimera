#!/bin/bash
# Restart Chimera in mainnet paper trading mode with correct environment

set -euo pipefail

echo "🔄 Restarting Chimera Operator in MAINNET PAPER TRADING mode..."

# Stop services
COMPOSE_PROFILE=mainnet-paper docker compose down operator

# Start with mainnet environment
if ! COMPOSE_PROFILE=mainnet-paper docker compose up -d operator; then
    echo "❌ Failed to start operator - the service may be left stopped" >&2
    exit 1
fi

# Wait for the operator to become healthy (up to 60s)
echo "⏳ Waiting for operator to become healthy..."
for i in $(seq 1 60); do
  if curl -sf --max-time 5 http://localhost:8080/api/v1/health >/dev/null 2>&1; then
    echo "Operator is ready."
    break
  fi
  [ "$i" -eq 60 ] && { echo "❌ Operator did not become healthy in time" >&2; exit 1; }
  sleep 1
done

# Check health
echo ""
echo "📊 System Status:"
curl -sf --max-time 10 http://localhost:8080/api/v1/health | jq '{status, rpc: .rpc.status, trading: .circuit_breaker.trading_allowed}' || { echo "❌ Operator is unhealthy" >&2; exit 1; }

# Check RPC URL
echo ""
echo "🌐 Network Configuration:"
COMPOSE_PROFILE=mainnet-paper docker compose exec -T operator printenv | grep -E "SOLANA_NETWORK|PRIMARY_URL" | head -2 || echo "Could not read network configuration"

# Check polling
echo ""
echo "🔍 RPC Polling Status:"
if COMPOSE_PROFILE=mainnet-paper docker compose logs operator 2>&1 | grep -q "RPC polling task started"; then
    COMPOSE_PROFILE=mainnet-paper docker compose logs operator 2>&1 | grep "RPC polling task started" | tail -1
else
    echo "⚠ RPC polling task not seen in logs yet (may still be starting)"
fi

echo ""
echo "✅ Operator restarted successfully!"
echo ""
echo "📝 Monitor logs: COMPOSE_PROFILE=mainnet-paper docker compose logs operator -f"
echo "📊 Check trades: curl http://localhost:8080/api/v1/trades | jq"
