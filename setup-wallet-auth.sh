#!/bin/bash
# Wallet Authentication Setup Helper
# Helps configure and test wallet-based authentication

set -e

API_URL="http://localhost:8080"

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
    echo -e "${GREEN}[✓]${NC} $1"
}

log_error() {
    echo -e "${RED}[✗]${NC} $1"
}

log_section() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo ""
}

log_section "Wallet Authentication Setup"

log_info "Admin wallet from config.yaml:"
ADMIN_WALLET=$(grep -A 2 "admin_wallets:" config/config.yaml | grep "address" | head -1 | cut -d'"' -f4)
echo "  $ADMIN_WALLET"
echo ""

log_section "Authentication Flow"

echo "1. Sign a message with your Solana wallet"
echo "2. POST the signature to /api/v1/auth/wallet"
echo "3. Receive a JWT token"
echo "4. Use the token in Authorization header for protected endpoints"
echo ""

log_section "Message Format"

MESSAGE="Chimera Dashboard Authentication\nWallet: $ADMIN_WALLET\nTimestamp: $(date +%s)"
echo "Required message format:"
echo "  'Chimera Dashboard Authentication'"
echo "  Must include wallet address"
echo ""
echo "Example message:"
echo "  $MESSAGE"
echo ""

log_section "Using Solana CLI"

if command -v solana &> /dev/null; then
    log_success "Solana CLI is installed"
    echo ""
    echo "To authenticate with Solana CLI:"
    echo ""
    echo "  # 1. Set your keypair:"
    echo "     solana config set --keypair /path/to/your-keypair.json"
    echo ""
    echo "  # 2. Create authentication message:"
    echo "     MESSAGE=\"Chimera Dashboard Authentication\\nWallet: $ADMIN_WALLET\\nTimestamp: \$(date +%s)\""
    echo ""
    echo "  # 3. Sign the message:"
    echo "     SIGNATURE=\$(echo -e \"\$MESSAGE\" | solana message sign)"
    echo ""
    echo "  # 4. Encode signature to base64:"
    echo "     SIG_B64=\$(echo \"\$SIGNATURE\" | base64)"
    echo ""
    echo "  # 5. POST to auth endpoint:"
    echo "     curl -X POST $API_URL/api/v1/auth/wallet \\"
    echo "       -H 'Content-Type: application/json' \\"
    echo "       -d '{\"wallet_address\":\"$ADMIN_WALLET\",\"message\":\"\$MESSAGE\",\"signature\":\"\$SIG_B64\"}'"
    echo ""
else
    log_warning "Solana CLI not found"
    echo "Install from: https://docs.solana.com/cli/install-solana-cli-tools"
fi

log_section "Using Web3.js / TypeScript"

cat << 'EOF'
// Example TypeScript/JavaScript code for wallet authentication

import { Connection, Keypair } from '@solana/web3.js';
import nacl from 'tweetnacl';
import bs58 from 'bs58';

async function authenticateWallet(keypair: Keypair, walletAddress: string) {
  // Create authentication message
  const timestamp = Date.now();
  const message = `Chimera Dashboard Authentication\nWallet: ${walletAddress}\nTimestamp: ${timestamp}`;
  
  // Sign message
  const messageBytes = new TextEncoder().encode(message);
  const signature = nacl.sign.detached(messageBytes, keypair.secretKey);
  
  // Encode signature to base64
  const signatureBase64 = Buffer.from(signature).toString('base64');
  
  // POST to auth endpoint
  const response = await fetch('http://localhost:8080/api/v1/auth/wallet', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      wallet_address: walletAddress,
      message: message,
      signature: signatureBase64
    })
  });
  
  const data = await response.json();
  return data.token; // JWT token
}

// Use the token for authenticated requests
const token = await authenticateWallet(keypair, walletAddress);
const response = await fetch('http://localhost:8080/api/v1/config/circuit-breaker/reset', {
  method: 'POST',
  headers: {
    'Authorization': `Bearer ${token}`,
    'Content-Type': 'application/json'
  }
});
EOF

echo ""

log_section "Testing Authentication"

log_info "To test authentication, you need:"
echo "  1. A Solana wallet keypair"
echo "  2. The wallet address must be in config.yaml admin_wallets"
echo "  3. Sign the authentication message"
echo "  4. POST to /api/v1/auth/wallet"
echo ""

log_section "Protected Endpoints"

echo "These endpoints require authentication:"
echo "  - POST /api/v1/config/circuit-breaker/reset (admin)"
echo "  - POST /api/v1/config/circuit-breaker/trip (admin)"
echo "  - PUT /api/v1/wallets/:address (operator+)"
echo "  - POST /api/v1/config/* (operator+)"
echo ""

log_info "Use the JWT token in the Authorization header:"
echo "  Authorization: Bearer <token>"
echo ""
