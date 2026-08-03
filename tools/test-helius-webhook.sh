#!/bin/bash
# Helius Webhook Testing Script for Chimera Trading System
# Tests webhook endpoint connectivity and functionality

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
WEBHOOK_URL="${1:-https://chimera.example.com/api/v1/monitoring/helius-webhook}"
# The real webhook secret must be provided via the environment; a hardcoded
# default could never validate the actual shared-secret flow
HELIUS_WEBHOOK_SECRET="${HELIUS_WEBHOOK_SECRET:?HELIUS_WEBHOOK_SECRET must be set to the real webhook secret}"

HEALTH_URL="${WEBHOOK_URL/\/monitoring\/helius-webhook/\/health}"
if [[ "$HEALTH_URL" == "$WEBHOOK_URL" ]]; then
    HEALTH_URL="${WEBHOOK_URL%/}/health"
fi

echo "======================================================================"
echo "Chimera Trading System - Helius Webhook Testing"
echo "======================================================================"
echo "Webhook URL: $WEBHOOK_URL"
echo "Health Check URL: $HEALTH_URL"
echo "======================================================================"

# Function to test endpoint
test_endpoint() {
    local url="$1"
    local method="${2:-GET}"
    local data="$3"
    local description="$4"

    echo -e "\n${BLUE}Testing: $description${NC}"
    echo "URL: $url"
    echo "Method: $method"

    if [[ -n "$data" ]]; then
        echo "Data: $data"
    fi

    local curl_args=(-s -w "\n%{http_code}" --connect-timeout 5 --max-time 15 -X "$method" "$url")
    curl_args+=(-H "Content-Type: application/json")
    # Only send a body for POST/PUT (a GET with -d can be rejected)
    if [[ -n "$data" ]] && [[ "$method" == "POST" || "$method" == "PUT" ]]; then
        curl_args+=(-d "$data")
    fi

    local response
    response=$(curl "${curl_args[@]}" 2>&1) || {
        echo -e "${RED}❌ Failed to connect to endpoint${NC}"
        echo "Please check:"
        echo "1. Server is running"
        echo "2. Firewall allows connections"
        echo "3. DNS is correctly configured"
        return 1
    }

    local http_code
    http_code=$(echo "$response" | tail -1)
    local body
    body=$(echo "$response" | sed '$d')

    if [[ "$http_code" =~ ^2 ]]; then
        echo -e "${GREEN}✅ Success (HTTP $http_code)${NC}"
        echo "Response: $body"
        return 0
    else
        echo -e "${RED}❌ Failed (HTTP $http_code)${NC}"
        echo "Response: $body"
        return 1
    fi
}

# Function to test HMAC authentication
test_hmac() {
    local url="$1"
    local secret="$HELIUS_WEBHOOK_SECRET"

    echo -e "\n${BLUE}Testing HMAC Authentication${NC}"

    # Generate test signature (${timestamp}.${payload} — must match the
    # server's scheme)
    timestamp=$(date +%s)
    test_data='{"test":"data"}'
    signature=$(echo -n "${timestamp}.${test_data}" | openssl dgst -sha256 -hmac "$secret" -binary | base64)

    echo "Timestamp: $timestamp"
    echo "Signature: $signature"

    local response
    response=$(curl -s -w "\n%{http_code}" --connect-timeout 5 --max-time 15 -X POST "$url" \
        -H "Content-Type: application/json" \
        -H "X-Signature: $signature" \
        -H "X-Timestamp: $timestamp" \
        -d "$test_data" 2>&1) || {
        echo -e "${RED}❌ Failed to connect${NC}"
        return 1
    }

    local http_code
    http_code=$(echo "$response" | tail -1)
    local body
    body=$(echo "$response" | sed '$d')

    # Only a genuine 2xx proves the HMAC validation passed
    if [[ "$http_code" =~ ^2 ]]; then
        echo -e "${GREEN}✅ HMAC test passed (HTTP $http_code)${NC}"
        echo "Response: $body"
        return 0
    else
        echo -e "${RED}❌ HMAC test failed (HTTP $http_code)${NC}"
        echo "Response: $body"
        return 1
    fi
}

# Function to simulate Helius webhook payload
test_helius_payload() {
    local url="$1"

    echo -e "\n${BLUE}Testing Helius Webhook Payload${NC}"

    # Sample Helius webhook payload
    local helius_payload='{
        "accountData": [],
        "nativeTransfers": [],
        "signature": "test123",
        "slot": 12345,
        "timestamp": 1234567890,
        "type": "SWAP",
        "transaction": {
            "transactionData": {
                "message": {
                    "accountKeys": ["test1", "test2"],
                    "instructions": [
                        {
                            "programId": "whirLbMiicVpio4NvAXUYHADi3EJcLJV8tgouCUto",
                            "data": "base64data"
                        }
                    ]
                }
            }
        }
    }'

    local response
    response=$(curl -s -w "\n%{http_code}" --connect-timeout 5 --max-time 15 -X POST "$url" \
        -H "Content-Type: application/json" \
        -d "$helius_payload" 2>&1) || {
        echo -e "${RED}❌ Failed to send Helius payload${NC}"
        return 1
    }

    local http_code
    http_code=$(echo "$response" | tail -1)
    local body
    body=$(echo "$response" | sed '$d')

    if [[ "$http_code" =~ ^2 ]]; then
        echo -e "${GREEN}✅ Helius payload accepted (HTTP $http_code)${NC}"
        echo "Response: $body"
        return 0
    else
        echo -e "${RED}❌ Helius payload rejected (HTTP $http_code)${NC}"
        echo "Response: $body"
        return 1
    fi
}

# Main testing sequence
main() {
    echo -e "${BLUE}Starting webhook endpoint tests...${NC}\n"

    # Test 1: Health check
    test_endpoint "$HEALTH_URL" "GET" "" "Health Check Endpoint"

    # Test 2: Webhook endpoint (basic)
    test_endpoint "$WEBHOOK_URL" "POST" '{"test":"data"}' "Webhook Endpoint (Basic)"

    # Test 3: HMAC authentication
    test_hmac "$WEBHOOK_URL"

    # Test 4: Helius payload simulation
    test_helius_payload "$WEBHOOK_URL"

    # Summary
    echo -e "\n======================================================================"
    echo -e "${GREEN}✅ Webhook Testing Complete${NC}"
    echo "======================================================================"
    echo ""
    echo "Webhook endpoint is ready for Helius integration!"
    echo ""
    echo "Next steps:"
    echo "1. Register wallets with Helius: python tools/register_helius_webhooks.py"
    echo "2. Monitor webhook activity in operator logs"
    echo "3. Verify trading signals are generated"
    echo "======================================================================"
}

# Run main function
main "$@"
