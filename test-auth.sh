#!/bin/bash
# Authentication and Advanced Testing Script
# Tests wallet-based auth, circuit breaker, load testing, and signal quality

set -e

API_URL="http://localhost:8080"
WEBHOOK_SECRET=$(docker exec chimera-operator printenv CHIMERA_SECURITY__WEBHOOK_SECRET 2>/dev/null || echo "devnet-webhook-secret-change-me-in-production")

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[PASS]${NC} $1"
}

log_error() {
    echo -e "${RED}[FAIL]${NC} $1"
}

log_section() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo ""
}

# Generate HMAC signature
generate_signature() {
    local timestamp=$1
    local payload=$2
    echo -n "${timestamp}${payload}" | openssl dgst -sha256 -hmac "$WEBHOOK_SECRET" | cut -d' ' -f2
}

log_section "Authentication Setup Check"
log_info "Admin wallet configured in config.yaml:"
grep -A 2 "admin_wallets:" config/config.yaml | head -3
echo ""

log_section "Testing Wallet Authentication"
log_info "To authenticate, you need to:"
echo "1. Sign a message with your admin wallet"
echo "2. POST to /api/v1/auth/wallet with:"
echo "   {"
echo "     \"wallet_address\": \"YOUR_ADDRESS\","
echo "     \"message\": \"MESSAGE_TO_SIGN\","
echo "     \"signature\": \"SIGNATURE_BASE58\""
echo "   }"
echo ""
log_info "Example using Solana CLI (if you have the keypair):"
echo "solana-keygen new --outfile /tmp/test-keypair.json"
echo "# Then use the wallet address in config.yaml"
echo ""

log_section "Testing Circuit Breaker (Requires Auth)"
log_info "Circuit Breaker Reset Endpoint:"
echo "POST ${API_URL}/api/v1/config/circuit-breaker/reset"
echo "Requires: Authorization header with JWT token"
echo ""

log_info "Circuit Breaker Trip Endpoint:"
echo "POST ${API_URL}/api/v1/config/circuit-breaker/trip"
echo "Body: {\"reason\": \"Emergency kill switch\"}"
echo "Requires: Authorization header with JWT token"
echo ""

log_section "Load Testing - Multiple Webhooks"
log_info "Sending 10 webhook signals in parallel..."

TIMESTAMP=$(date +%s)
PAYLOAD='{"strategy":"SHIELD","token":"So11111111111111111111111111111111111111112","action":"BUY","amount_sol":0.1,"wallet_address":"7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU","consensus_count":5}'

SUCCESS=0
FAILED=0

for i in {1..10}; do
    TS=$((TIMESTAMP + i))
    SIG=$(generate_signature "$TS" "$PAYLOAD")
    
    RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "${API_URL}/api/v1/webhook" \
        -H "Content-Type: application/json" \
        -H "X-Signature: $SIG" \
        -H "X-Timestamp: $TS" \
        -d "$PAYLOAD" 2>&1)
    
    STATUS=$(echo "$RESPONSE" | tail -1)
    
    if [ "$STATUS" = "200" ] || [ "$STATUS" = "202" ]; then
        ((SUCCESS++))
        echo -n "."
    else
        ((FAILED++))
        echo -n "F"
    fi
done

echo ""
log_info "Load test results: $SUCCESS succeeded, $FAILED failed"
echo ""

log_section "Advanced Signal Quality Testing"
log_info "Testing different signal quality scenarios..."

# High quality signal (consensus)
TIMESTAMP=$(date +%s)
HIGH_QUALITY='{"strategy":"SHIELD","token":"So11111111111111111111111111111111111111112","action":"BUY","amount_sol":0.1,"wallet_address":"7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU","consensus_count":5,"signal_quality":0.95}'
SIG=$(generate_signature "$TIMESTAMP" "$HIGH_QUALITY")

log_info "Sending high-quality signal (consensus=5, quality=0.95)..."
RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "${API_URL}/api/v1/webhook" \
    -H "Content-Type: application/json" \
    -H "X-Signature: $SIG" \
    -H "X-Timestamp: $TIMESTAMP" \
    -d "$HIGH_QUALITY" 2>&1)

STATUS=$(echo "$RESPONSE" | tail -1)
BODY=$(echo "$RESPONSE" | sed '$d')

if [ "$STATUS" = "200" ] || [ "$STATUS" = "202" ]; then
    log_success "High-quality signal accepted"
else
    log_info "Response: $(echo "$BODY" | python3 -m json.tool 2>/dev/null | grep -E '"reason"|"status"' | head -2)"
fi

# Medium quality signal
TIMESTAMP=$((TIMESTAMP + 1))
MEDIUM_QUALITY='{"strategy":"SPEAR","token":"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v","action":"BUY","amount_sol":0.05,"wallet_address":"7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU","consensus_count":2,"signal_quality":0.65}'
SIG=$(generate_signature "$TIMESTAMP" "$MEDIUM_QUALITY")

log_info "Sending medium-quality signal (consensus=2, quality=0.65)..."
RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "${API_URL}/api/v1/webhook" \
    -H "Content-Type: application/json" \
    -H "X-Signature: $SIG" \
    -H "X-Timestamp: $TIMESTAMP" \
    -d "$MEDIUM_QUALITY" 2>&1)

STATUS=$(echo "$RESPONSE" | tail -1)
BODY=$(echo "$RESPONSE" | sed '$d')

if [ "$STATUS" = "200" ] || [ "$STATUS" = "202" ]; then
    log_success "Medium-quality signal accepted"
else
    log_info "Response: $(echo "$BODY" | python3 -m json.tool 2>/dev/null | grep -E '"reason"|"status"' | head -2)"
fi

echo ""

log_section "Current System Status"
HEALTH=$(curl -s "${API_URL}/api/v1/health" | python3 -m json.tool 2>/dev/null)
echo "$HEALTH" | grep -A 3 '"circuit_breaker"'
echo ""

log_section "Next Steps"
echo "1. Set up wallet authentication using Solana CLI or web3.js"
echo "2. Test circuit breaker trip/reset with authenticated requests"
echo "3. Monitor metrics in Grafana during load testing"
echo "4. Review webhook acceptance rates"
echo ""
