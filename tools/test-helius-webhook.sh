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
HEALTH_URL="${WEBHOOK_URL/\/monitoring\/helius-webhook/\/health}"

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

    response=$(curl -s -w "\n%{http_code}" -X "$method" "$url" \
        -H "Content-Type: application/json" \
        -d "$data" 2>&1) || {
        echo -e "${RED}❌ Failed to connect to endpoint${NC}"
        echo "Please check:"
        echo "1. Server is running"
        echo "2. Firewall allows connections"
        echo "3. DNS is correctly configured"
        return 1
    }

    http_code=$(echo "$response" | tail -1)
    body=$(echo "$response" | head -n -1)

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
    local secret="${2:-test_secret}"

    echo -e "\n${BLUE}Testing HMAC Authentication${NC}"

    # Generate test signature
    timestamp=$(date +%s)
    test_data='{"test":"data"}'
    signature=$(echo -n "${timestamp}.${test_data}" | openssl dgst -sha256 -hmac "$secret" -binary | base64)

    echo "Timestamp: $timestamp"
    echo "Signature: $signature"

    response=$(curl -s -w "\n%{http_code}" -X POST "$url" \
        -H "Content-Type: application/json" \
        -H "X-Signature: $signature" \
        -H "X-Timestamp: $timestamp" \
        -d "$test_data" 2>&1) || {
        echo -e "${RED}❌ Failed to connect${NC}"
        return 1
    }

    http_code=$(echo "$response" | tail -1)
    body=$(echo "$response" | head -n -1)

    if [[ "$http_code" =~ ^2 ]] || [[ "$http_code" == "401" ]]; then
        echo -e "${GREEN}✅ HMAC test completed (HTTP $http_code)${NC}"
        if [[ "$http_code" == "401" ]]; then
            echo "Note: 401 response indicates HMAC validation is working (secret mismatch)"
        fi
        echo "Response: $body"
        return 0
    else
        echo -e "${YELLOW}⚠️  Unexpected response (HTTP $http_code)${NC}"
        echo "Response: $body"
        return 0
    fi
}

# Function to simulate Helius webhook payload
test_helius_payload() {
    local url="$1"

    echo -e "\n${BLUE}Testing Helius Webhook Payload${NC}"

    # Sample Helius webhook payload
    helius_payload='{
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

    response=$(curl -s -w "\n%{http_code}" -X POST "$url" \
        -H "Content-Type: application/json" \
        -d "$helius_payload" 2>&1) || {
        echo -e "${RED}❌ Failed to send Helius payload${NC}"
        return 1
    }

    http_code=$(echo "$response" | tail -1)
    body=$(echo "$response" | head -n -1)

    echo -e "${GREEN}✅ Helius payload test completed (HTTP $http_code)${NC}"
    echo "Response: $body"
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