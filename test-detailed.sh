#!/bin/bash
# Detailed testing for specific features

set -e

API_URL="http://localhost:8080"
WEBHOOK_SECRET=$(docker exec chimera-operator printenv CHIMERA_SECURITY__WEBHOOK_SECRET 2>/dev/null || echo "devnet-webhook-secret-change-me-in-production")

echo "=== Testing Webhook with Better Signal Quality ==="
echo ""

# Create a webhook with higher signal quality (consensus signal)
TIMESTAMP=$(date +%s)
PAYLOAD='{"strategy":"SHIELD","token":"So11111111111111111111111111111111111111112","action":"BUY","amount_sol":0.1,"wallet_address":"7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU","consensus_count":3}'
SIGNATURE=$(echo -n "${TIMESTAMP}${PAYLOAD}" | openssl dgst -sha256 -hmac "$WEBHOOK_SECRET" | cut -d' ' -f2)

echo "Sending webhook with consensus signal..."
RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "${API_URL}/api/v1/webhook" \
    -H "Content-Type: application/json" \
    -H "X-Signature: $SIGNATURE" \
    -H "X-Timestamp: $TIMESTAMP" \
    -d "$PAYLOAD" 2>&1)

STATUS=$(echo "$RESPONSE" | tail -1)
BODY=$(echo "$RESPONSE" | sed '$d')

echo "Status: $STATUS"
echo "Response: $BODY" | python3 -m json.tool 2>/dev/null || echo "$BODY"
echo ""

echo "=== Testing Circuit Breaker Reset (requires auth) ==="
echo "Note: This requires authentication - checking endpoint exists..."
curl -s -X POST "${API_URL}/api/v1/config/circuit-breaker/reset" \
    -H "Content-Type: application/json" 2>&1 | head -3
echo ""

echo "=== Current Metrics Status ==="
echo ""
echo "Queue Depth:"
curl -s "http://localhost:9090/api/v1/query?query=chimera_queue_depth" | python3 -m json.tool 2>/dev/null | grep -A 2 '"value"'
echo ""
echo "RPC Health:"
curl -s "http://localhost:9090/api/v1/query?query=chimera_rpc_health" | python3 -m json.tool 2>/dev/null | grep -A 2 '"value"'
echo ""
echo "Circuit Breaker State:"
curl -s "http://localhost:9090/api/v1/query?query=chimera_circuit_breaker_state" | python3 -m json.tool 2>/dev/null | grep -A 2 '"value"'
echo ""

echo "=== Grafana Dashboard Check ==="
DASHBOARD_UID=$(curl -s "http://localhost:3002/api/search?query=Chimera" -u admin:admin | python3 -c "import sys, json; data=json.load(sys.stdin); print(data[0]['uid'] if data else 'not found')" 2>/dev/null)
if [ "$DASHBOARD_UID" != "not found" ] && [ -n "$DASHBOARD_UID" ]; then
    echo "✓ Dashboard found with UID: $DASHBOARD_UID"
    echo "  Access at: http://localhost:3002/d/$DASHBOARD_UID/chimera-trading-platform"
else
    echo "✗ Dashboard not found"
fi
echo ""

echo "=== All Available Chimera Metrics ==="
curl -s "http://localhost:9090/api/v1/label/__name__/values" | python3 -c "import sys, json; data=json.load(sys.stdin); metrics=[m for m in data['data'] if 'chimera_' in m]; print('\n'.join(sorted(metrics)))" 2>/dev/null
echo ""
