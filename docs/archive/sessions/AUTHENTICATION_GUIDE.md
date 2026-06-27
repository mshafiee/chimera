# Authentication Guide for Mainnet-Paper Mode

## Overview

The Chimera system uses **Solana wallet-based authentication** with JWT tokens. To access protected endpoints (like roster merge), you need to:

1. Sign a message with your Solana wallet
2. Authenticate via `/api/v1/auth/wallet` to get a JWT token
3. Use the token in the `Authorization: Bearer <token>` header

## Authentication Flow

```
┌─────────────┐
│   Wallet    │
│  (Sign Msg) │
└──────┬──────┘
       │
       ▼
┌─────────────────────┐
│ POST /auth/wallet   │
│ (wallet, msg, sig)  │
└──────┬──────────────┘
       │
       ▼
┌─────────────────────┐
│   JWT Token          │
└──────┬──────────────┘
       │
       ▼
┌─────────────────────┐
│ Protected Endpoints  │
│ Authorization: Bearer │
└──────────────────────┘
```

## Step 1: Configure Admin Wallet

First, ensure your wallet is configured as an admin in the system.

### Option A: Via config.yaml

Add your wallet to `config/config.yaml`:

```yaml
admin_wallets:
  - address: "YOUR_WALLET_ADDRESS_HERE"
    role: "admin"  # or "operator" for limited access
```

### Option B: Via Database (Direct)

If you need to add an admin wallet directly to the database:

```bash
# Connect to the database
docker exec chimera-operator sqlite3 /app/data/chimera.db <<EOF
INSERT OR REPLACE INTO admin_wallets (wallet_address, role, created_at)
VALUES ('YOUR_WALLET_ADDRESS_HERE', 'admin', CURRENT_TIMESTAMP);
EOF
```

**Note:** The `admin_wallets` table should exist from migrations. If not, you may need to run migrations first.

## Step 2: Create Authentication Message

The message must:
- Contain the text: `"Chimera Dashboard Authentication"`
- Include your wallet address
- Be signed with your wallet's private key

**Message Format:**
```
Chimera Dashboard Authentication
Wallet: YOUR_WALLET_ADDRESS
Timestamp: 1234567890
```

## Step 3: Sign the Message

### Method 1: Using Solana CLI

```bash
# 1. Set your keypair
solana config set --keypair /path/to/your-keypair.json

# 2. Create the message
WALLET_ADDRESS="YOUR_WALLET_ADDRESS"
TIMESTAMP=$(date +%s)
MESSAGE="Chimera Dashboard Authentication
Wallet: $WALLET_ADDRESS
Timestamp: $TIMESTAMP"

# 3. Sign the message (this will prompt for confirmation)
echo -e "$MESSAGE" | solana message sign

# 4. The signature will be output - encode it to base64
SIGNATURE_B64=$(echo -e "$MESSAGE" | solana message sign | base64)
```

### Method 2: Using Web3.js / TypeScript

```typescript
import { Keypair } from '@solana/web3.js';
import nacl from 'tweetnacl';
import bs58 from 'bs58';

// Load your keypair
const keypair = Keypair.fromSecretKey(bs58.decode('YOUR_SECRET_KEY'));

// Create message
const walletAddress = keypair.publicKey.toBase58();
const timestamp = Date.now();
const message = `Chimera Dashboard Authentication\nWallet: ${walletAddress}\nTimestamp: ${timestamp}`;

// Sign message
const messageBytes = new TextEncoder().encode(message);
const signature = nacl.sign.detached(messageBytes, keypair.secretKey);

// Encode to base64
const signatureBase64 = Buffer.from(signature).toString('base64');

console.log('Wallet:', walletAddress);
console.log('Message:', message);
console.log('Signature (base64):', signatureBase64);
```

### Method 3: Using Python (solana-py)

```python
from solana.keypair import Keypair
from solana.publickey import PublicKey
import base64
import nacl.signing

# Load your keypair
with open('path/to/keypair.json', 'r') as f:
    keypair_data = json.load(f)
    secret_key = bytes(keypair_data)
    keypair = Keypair.from_secret_key(secret_key)

# Create message
wallet_address = str(keypair.public_key)
timestamp = int(time.time())
message = f"Chimera Dashboard Authentication\nWallet: {wallet_address}\nTimestamp: {timestamp}"

# Sign message
message_bytes = message.encode('utf-8')
signature = keypair.sign(message_bytes)

# Encode to base64
signature_base64 = base64.b64encode(signature.signature).decode('utf-8')

print(f"Wallet: {wallet_address}")
print(f"Message: {message}")
print(f"Signature (base64): {signature_base64}")
```

## Step 4: Authenticate and Get JWT Token

POST to the authentication endpoint:

```bash
curl -X POST http://localhost:8080/api/v1/auth/wallet \
  -H "Content-Type: application/json" \
  -d '{
    "wallet_address": "YOUR_WALLET_ADDRESS",
    "message": "Chimera Dashboard Authentication\nWallet: YOUR_WALLET_ADDRESS\nTimestamp: 1234567890",
    "signature": "BASE64_ENCODED_SIGNATURE"
  }'
```

**Response:**
```json
{
  "token": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...",
  "role": "admin",
  "identifier": "YOUR_WALLET_ADDRESS"
}
```

Save the `token` value for the next step.

## Step 5: Use Token for Protected Endpoints

Now you can use the JWT token to access protected endpoints:

### Roster Merge

```bash
TOKEN="your-jwt-token-here"

curl -X POST http://localhost:8080/api/v1/roster/merge \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{}'
```

### Circuit Breaker Reset

```bash
curl -X POST http://localhost:8080/api/v1/config/circuit-breaker/reset \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{}'
```

### Update Wallet Status

```bash
curl -X PUT http://localhost:8080/api/v1/wallets/WALLET_ADDRESS \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "status": "ACTIVE",
    "ttl_hours": 24
  }'
```

## Complete Example Script

Here's a complete bash script to authenticate and merge roster:

```bash
#!/bin/bash

API_URL="http://localhost:8080"
WALLET_ADDRESS="YOUR_WALLET_ADDRESS"
KEYPAIR_PATH="/path/to/keypair.json"

# Step 1: Create message
TIMESTAMP=$(date +%s)
MESSAGE="Chimera Dashboard Authentication
Wallet: $WALLET_ADDRESS
Timestamp: $TIMESTAMP"

# Step 2: Sign message (requires Solana CLI)
SIGNATURE_B64=$(echo -e "$MESSAGE" | solana message sign --keypair "$KEYPAIR_PATH" | base64)

# Step 3: Authenticate
AUTH_RESPONSE=$(curl -s -X POST "$API_URL/api/v1/auth/wallet" \
  -H "Content-Type: application/json" \
  -d "{
    \"wallet_address\": \"$WALLET_ADDRESS\",
    \"message\": \"$MESSAGE\",
    \"signature\": \"$SIGNATURE_B64\"
  }")

# Extract token
TOKEN=$(echo "$AUTH_RESPONSE" | jq -r '.token')

if [ "$TOKEN" = "null" ] || [ -z "$TOKEN" ]; then
  echo "Authentication failed!"
  echo "$AUTH_RESPONSE"
  exit 1
fi

echo "✓ Authenticated successfully"
echo "Token: ${TOKEN:0:50}..."

# Step 4: Merge roster
MERGE_RESPONSE=$(curl -s -X POST "$API_URL/api/v1/roster/merge" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{}')

echo "Roster merge result:"
echo "$MERGE_RESPONSE" | jq '.'
```

## Roles and Permissions

- **admin**: Full access to all endpoints
  - Roster merge
  - Circuit breaker controls
  - Wallet management
  - Configuration changes

- **operator**: Limited access
  - Wallet management
  - Configuration viewing
  - No circuit breaker controls

- **readonly**: Read-only access
  - View wallets
  - View configuration
  - View metrics
  - No write operations

## Troubleshooting

### "Wallet not authorized for dashboard access"

Your wallet is not in the `admin_wallets` table or `wallets` table. Add it:

```bash
docker exec chimera-operator sqlite3 /app/data/chimera.db \
  "INSERT INTO admin_wallets (wallet_address, role) VALUES ('YOUR_ADDRESS', 'admin');"
```

### "Invalid signature verification"

- Ensure the message format is correct
- Verify the signature is base64 encoded
- Check that you're signing with the correct wallet

### "Invalid authentication message"

The message must contain `"Chimera Dashboard Authentication"` exactly.

### Token Expired

JWT tokens expire after 24 hours. Re-authenticate to get a new token.

## Security Notes

1. **Never share your private key** - Keep keypairs secure
2. **Use environment variables** for sensitive data in scripts
3. **Rotate admin wallets** periodically in production
4. **Monitor authentication logs** for suspicious activity
5. **Use HTTPS** in production (not applicable for localhost)

## Quick Reference

```bash
# Authenticate
curl -X POST http://localhost:8080/api/v1/auth/wallet \
  -H "Content-Type: application/json" \
  -d '{"wallet_address":"...","message":"...","signature":"..."}'

# Use token
curl -X POST http://localhost:8080/api/v1/roster/merge \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{}'
```




