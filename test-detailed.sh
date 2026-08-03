#!/bin/bash
# Detailed testing for specific features

set -e

API_URL="http://localhost:8080"
WEBHOOK_SECRET=${CHIMERA_SECURITY__WEBHOOK_SECRET:-}
if [ -z "$WEBHOOK_SECRET" ]; then
    WEBHOOK_SECRET=$(docker exec chimera-operator printenv CHIMERA_SECURITY__WEBHOOK_SECRET 2>/dev/null || true)
fi
if [ -z "$WEBHOOK_SECRET" ]; then
    echo "ERROR: CHIMERA_SECURITY__WEBHOOK_SECRET unavailable (docker exec failed)" >&2
    exit 1
fi

GRAFANA_USER="${GRAFANA_USER:-admin}"
GRAFANA_PASSWORD="${GRAFANA_PASSWORD:-}"

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
    -d "$PAYLOAD" 2>&1 || true)

STATUS=$(echo "$RESPONSE" | tail -1)
BODY=$(echo "$RESPONSE" | sed '$d')

echo "Status: $STATUS"
echo "Response: $BODY" | python3 -m json.tool 2>/dev/null || echo "$BODY"
[ "$STATUS" = "200" ] || [ "$STATUS" = "202" ] || { echo "Webhook test FAILED (expected 200/202, got '$STATUS')" >&2; exit 1; }
echo ""

echo "=== Testing Circuit Breaker Reset (requires auth) ==="
echo "Note: This endpoint requires authentication - unauthenticated requests should be rejected"
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST "${API_URL}/api/v1/config/circuit-breaker/reset" \
    -H "Content-Type: application/json" || true)
echo "Unauthenticated reset status: $HTTP_CODE"
[ "$HTTP_CODE" = "401" ] || [ "$HTTP_CODE" = "403" ] || { echo "Unexpected status: $HTTP_CODE" >&2; exit 1; }
echo ""

echo "=== Current Metrics Status ==="
echo ""
echo "Queue Depth:"
curl -s "http://localhost:9090/api/v1/query?query=chimera_queue_depth" | python3 -m json.tool 2>/dev/null | grep -A 2 '"value"' || echo "Queue depth metric unavailable"
echo ""
echo "RPC Health:"
curl -s "http://localhost:9090/api/v1/query?query=chimera_rpc_health" | python3 -m json.tool 2>/dev/null | grep -A 2 '"value"' || echo "RPC health metric unavailable"
echo ""
echo "Circuit Breaker State:"
curl -s "http://localhost:9090/api/v1/query?query=chimera_circuit_breaker_state" | python3 -m json.tool 2>/dev/null | grep -A 2 '"value"' || echo "Circuit breaker metric unavailable"
echo ""

echo "=== Grafana Dashboard Check ==="
DASHBOARD_UID=$(curl -s "http://localhost:3002/api/search?query=Chimera" -u "$GRAFANA_USER:$GRAFANA_PASSWORD" | python3 -c "import sys, json; data=json.load(sys.stdin); print(data[0]['uid'] if data else 'not found')" 2>/dev/null || echo "not found")
if [ "$DASHBOARD_UID" != "not found" ] && [ -n "$DASHBOARD_UID" ]; then
    echo "✓ Dashboard found with UID: $DASHBOARD_UID"
    echo "  Access at: http://localhost:3002/d/$DASHBOARD_UID/chimera-trading-platform"
else
    echo "✗ Dashboard not found"
fi
echo ""

echo "=== All Available Chimera Metrics ==="
curl -s "http://localhost:9090/api/v1/label/__name__/values" | python3 -c "import sys, json; data=json.load(sys.stdin); metrics=[m for m in data['data'] if 'chimera_' in m]; print('\n'.join(sorted(metrics)))" 2>/dev/null || echo "Metrics endpoint unavailable"
echo ""
