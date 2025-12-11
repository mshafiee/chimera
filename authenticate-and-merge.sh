#!/bin/bash
# Complete authentication and roster merge script
# Usage: ./authenticate-and-merge.sh <wallet-address> <keypair-path>

set -e

API_URL="http://localhost:8080"

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[✓]${NC} $1"
}

log_error() {
    echo -e "${RED}[✗]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[!]${NC} $1"
}

# Check arguments
if [ $# -lt 2 ]; then
    log_error "Usage: $0 <wallet-address> <keypair-path>"
    echo ""
    echo "Example:"
    echo "  $0 7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU ~/.config/solana/id.json"
    exit 1
fi

WALLET_ADDRESS="$1"
KEYPAIR_PATH="$2"

# Validate keypair exists
if [ ! -f "$KEYPAIR_PATH" ]; then
    log_error "Keypair file not found: $KEYPAIR_PATH"
    exit 1
fi

# Check if Solana CLI is available
if ! command -v solana &> /dev/null; then
    log_error "Solana CLI not found. Install from: https://docs.solana.com/cli/install-solana-cli-tools"
    exit 1
fi

log_info "Wallet: $WALLET_ADDRESS"
log_info "Keypair: $KEYPAIR_PATH"
echo ""

# Step 1: Create authentication message
log_info "Creating authentication message..."
TIMESTAMP=$(date +%s)
MESSAGE="Chimera Dashboard Authentication
Wallet: $WALLET_ADDRESS
Timestamp: $TIMESTAMP"

log_success "Message created"
echo ""

# Step 2: Sign message
log_info "Signing message with Solana CLI..."
log_warning "You may be prompted to confirm the signature"

# Sign the message
SIGNATURE_RAW=$(echo -e "$MESSAGE" | solana message sign --keypair "$KEYPAIR_PATH" 2>&1)

if [ $? -ne 0 ]; then
    log_error "Failed to sign message"
    echo "$SIGNATURE_RAW"
    exit 1
fi

# Encode signature to base64
SIGNATURE_B64=$(echo -e "$MESSAGE" | solana message sign --keypair "$KEYPAIR_PATH" | base64)

if [ -z "$SIGNATURE_B64" ]; then
    log_error "Failed to generate signature"
    exit 1
fi

log_success "Message signed and encoded"
echo ""

# Step 3: Authenticate
log_info "Authenticating with Chimera API..."

AUTH_RESPONSE=$(curl -s -X POST "$API_URL/api/v1/auth/wallet" \
  -H "Content-Type: application/json" \
  -d "{
    \"wallet_address\": \"$WALLET_ADDRESS\",
    \"message\": \"$MESSAGE\",
    \"signature\": \"$SIGNATURE_B64\"
  }")

# Check for errors
if echo "$AUTH_RESPONSE" | grep -q "error\|Error\|rejected"; then
    log_error "Authentication failed!"
    echo "$AUTH_RESPONSE" | python3 -m json.tool 2>/dev/null || echo "$AUTH_RESPONSE"
    exit 1
fi

# Extract token
TOKEN=$(echo "$AUTH_RESPONSE" | python3 -c "import sys, json; print(json.load(sys.stdin).get('token', ''))" 2>/dev/null)

if [ -z "$TOKEN" ] || [ "$TOKEN" = "null" ]; then
    log_error "Failed to extract token from response"
    echo "$AUTH_RESPONSE" | python3 -m json.tool 2>/dev/null || echo "$AUTH_RESPONSE"
    exit 1
fi

ROLE=$(echo "$AUTH_RESPONSE" | python3 -c "import sys, json; print(json.load(sys.stdin).get('role', ''))" 2>/dev/null)

log_success "Authenticated successfully!"
log_info "Role: $ROLE"
log_info "Token: ${TOKEN:0:50}..."
echo ""

# Step 4: Merge roster
log_info "Merging roster..."

MERGE_RESPONSE=$(curl -s -X POST "$API_URL/api/v1/roster/merge" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{}')

# Check merge result
if echo "$MERGE_RESPONSE" | grep -q "success.*true"; then
    WALLETS_MERGED=$(echo "$MERGE_RESPONSE" | python3 -c "import sys, json; print(json.load(sys.stdin).get('wallets_merged', 0))" 2>/dev/null)
    log_success "Roster merged successfully!"
    log_info "Wallets merged: $WALLETS_MERGED"
    echo ""
    echo "$MERGE_RESPONSE" | python3 -m json.tool 2>/dev/null || echo "$MERGE_RESPONSE"
else
    log_error "Roster merge failed!"
    echo "$MERGE_RESPONSE" | python3 -m json.tool 2>/dev/null || echo "$MERGE_RESPONSE"
    exit 1
fi

echo ""
log_success "All done! Wallets are now available in the system."


