# Chimera API Documentation

## Overview

The Chimera Operator provides a RESTful API for managing the high-frequency copy-trading system. All API endpoints are versioned under `/api/v1`.

**Base URL:** `https://your-domain.com/api/v1`

**API Version:** v1

---

## Authentication

The API supports three authentication methods:

### 1. HMAC Webhook Authentication
Used for webhook endpoints. Requires:
- `X-Signature`: HMAC-SHA256 signature of `timestamp + payload`
- `X-Timestamp`: Unix timestamp (must be within ±5 minutes)
- `Content-Type: application/json`

### 2. Bearer Token Authentication
Used for management endpoints. Requires:
- `Authorization: Bearer <api_key>`
- API keys are stored in `admin_wallets` table with associated roles

### 3. Wallet Signature Authentication
Used for wallet-based login. Requires:
- Solana wallet signature verification
- Returns JWT token for subsequent requests

**Roles:**
- `readonly`: Read-only access to positions, wallets, trades, metrics
- `operator`: Read-only access + wallet management (promote/demote)
- `admin`: Full access including configuration management

---

## Endpoints

### Health & Status

#### GET /health
Simple health check for load balancers.

**Authentication:** None

**Response:**
```json
{
  "status": "ok"
}
```

#### GET /api/v1/health
Detailed health check with system metrics.

**Authentication:** None

**Response:**
```json
{
  "status": "healthy" | "degraded" | "unhealthy",
  "uptime_seconds": 86400,
  "queue_depth": 5,
  "rpc_latency_ms": 45,
  "last_trade_at": "2025-01-15T10:30:00Z",
  "database": {
    "status": "healthy",
    "message": null
  },
  "rpc": {
    "status": "healthy",
    "message": null
  },
  "circuit_breaker": {
    "state": "CLOSED",
    "trading_allowed": true,
    "trip_reason": null,
    "cooldown_remaining_secs": null
  },
  "price_cache": {
    "total_entries": 150,
    "tracked_tokens": 25
  }
}
```

---

### Webhook Endpoints

#### POST /api/v1/webhook
Submit trading signals from external providers.

**Authentication:** HMAC signature

**Headers:**
- `X-Signature`: HMAC-SHA256(timestamp + payload, SECRET)
- `X-Timestamp`: Unix timestamp

**Request Body:**
```json
{
  "strategy": "SHIELD" | "SPEAR" | "EXIT",
  "token": "BONK",
  "token_address": "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263",
  "action": "BUY" | "SELL",
  "amount_sol": 0.5,
  "wallet_address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
  "trade_uuid": "optional-uuid-from-signal-provider"
}
```

**Response:**
```json
{
  "status": "accepted" | "rejected",
  "trade_uuid": "uuid-1234",
  "reason": "optional rejection reason"
}
```

**Status Codes:**
- `200 OK`: Signal accepted
- `400 Bad Request`: Invalid payload
- `401 Unauthorized`: HMAC signature invalid
- `429 Too Many Requests`: Rate limit exceeded
- `503 Service Unavailable`: Circuit breaker tripped

**Example (curl):**
```bash
TIMESTAMP=$(date +%s)
PAYLOAD='{"strategy":"SHIELD","token":"BONK","action":"BUY","amount_sol":0.5,"wallet_address":"7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"}'
SIGNATURE=$(echo -n "${TIMESTAMP}${PAYLOAD}" | openssl dgst -sha256 -hmac "${WEBHOOK_SECRET}" | cut -d' ' -f2)

curl -X POST https://api.chimera.dev/api/v1/webhook \
  -H "Content-Type: application/json" \
  -H "X-Signature: ${SIGNATURE}" \
  -H "X-Timestamp: ${TIMESTAMP}" \
  -d "${PAYLOAD}"
```

---

### Authentication Endpoints

#### POST /api/v1/auth/wallet
Authenticate using Solana wallet signature.

**Authentication:** None (public endpoint)

**Request Body:**
```json
{
  "address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
  "message": "Chimera authentication message",
  "signature": "base64-encoded-signature"
}
```

**Response:**
```json
{
  "token": "jwt-token-here",
  "user": {
    "identifier": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
    "role": "readonly" | "operator" | "admin"
  }
}
```

**Status Codes:**
- `200 OK`: Authentication successful
- `400 Bad Request`: Invalid signature or message
- `401 Unauthorized`: Signature verification failed
- `404 Not Found`: Wallet not found in admin_wallets table

---

### Positions Endpoints

#### GET /api/v1/positions
List all positions.

**Authentication:** Bearer token (readonly+)

**Query Parameters:**
- `state` (optional): Filter by state (`ACTIVE`, `EXITING`, `CLOSED`)

**Response:**
```json
{
  "positions": [
    {
      "id": 1,
      "trade_uuid": "uuid-1234",
      "wallet_address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
      "token_address": "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263",
      "token_symbol": "BONK",
      "strategy": "SHIELD",
      "entry_amount_sol": 0.5,
      "entry_price": 0.000012,
      "entry_tx_signature": "5j7s8K9m...",
      "current_price": 0.000015,
      "unrealized_pnl_sol": 0.125,
      "unrealized_pnl_percent": 25.0,
      "state": "ACTIVE",
      "exit_price": null,
      "exit_tx_signature": null,
      "realized_pnl_sol": null,
      "realized_pnl_usd": null,
      "opened_at": "2025-01-15T10:00:00Z",
      "last_updated": "2025-01-15T10:05:00Z",
      "closed_at": null
    }
  ],
  "total": 1
}
```

**Example:**
```bash
curl -X GET "https://api.chimera.dev/api/v1/positions?state=ACTIVE" \
  -H "Authorization: Bearer YOUR_API_KEY"
```

#### GET /api/v1/positions/:trade_uuid
Get a single position by trade UUID.

**Authentication:** Bearer token (readonly+)

**Path Parameters:**
- `trade_uuid`: Unique trade identifier

**Response:**
Same as position object in list response.

**Status Codes:**
- `200 OK`: Position found
- `404 Not Found`: Position not found

---

### Wallets Endpoints

#### GET /api/v1/wallets
List all tracked wallets.

**Authentication:** Bearer token (readonly+)

**Query Parameters:**
- `status` (optional): Filter by status (`ACTIVE`, `CANDIDATE`, `REJECTED`)

**Response:**
```json
{
  "wallets": [
    {
      "id": 1,
      "address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
      "status": "ACTIVE",
      "wqs_score": 85.3,
      "roi_7d": 12.5,
      "roi_30d": 45.2,
      "trade_count_30d": 127,
      "win_rate": 0.72,
      "max_drawdown_30d": 8.3,
      "avg_trade_size_sol": 0.5,
      "last_trade_at": "2025-01-15T09:30:00Z",
      "promoted_at": "2025-01-10T08:00:00Z",
      "ttl_expires_at": null,
      "notes": null,
      "created_at": "2025-01-01T00:00:00Z",
      "updated_at": "2025-01-15T09:30:00Z"
    }
  ],
  "total": 1
}
```

#### GET /api/v1/wallets/:address
Get a single wallet by address.

**Authentication:** Bearer token (readonly+)

**Path Parameters:**
- `address`: Solana wallet address

**Response:**
Same as wallet object in list response.

#### PUT /api/v1/wallets/:address
Update wallet status (promote/demote).

**Authentication:** Bearer token (operator+)

**Path Parameters:**
- `address`: Solana wallet address

**Request Body:**
```json
{
  "status": "ACTIVE" | "CANDIDATE" | "REJECTED",
  "reason": "Promoted due to strong performance",
  "ttl_hours": 24
}
```

**Response:**
```json
{
  "success": true,
  "wallet": {
    // Updated wallet object
  },
  "message": "Wallet updated successfully"
}
```

**Status Codes:**
- `200 OK`: Wallet updated
- `400 Bad Request`: Invalid status or TTL validation failed
- `404 Not Found`: Wallet not found
- `403 Forbidden`: Insufficient permissions

**TTL Feature:**
- `ttl_hours`: Optional time-to-live in hours
- Only valid when promoting to `ACTIVE` status
- Wallet auto-demotes to `CANDIDATE` after TTL expires
- If omitted, promotion is permanent

---

### Trades Endpoints

#### GET /api/v1/trades
List trades with filtering and pagination.

**Authentication:** Bearer token (readonly+)

**Query Parameters:**
- `from` (optional): Start date (ISO 8601 format)
- `to` (optional): End date (ISO 8601 format)
- `status` (optional): Filter by status
- `strategy` (optional): Filter by strategy (`SHIELD`, `SPEAR`, `EXIT`)
- `limit` (optional): Results per page (default: 100, max: 1000)
- `offset` (optional): Pagination offset (default: 0)

**Response:**
```json
{
  "trades": [
    {
      "id": 1,
      "trade_uuid": "uuid-1234",
      "wallet_address": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU",
      "token_address": "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263",
      "token_symbol": "BONK",
      "strategy": "SHIELD",
      "side": "BUY",
      "amount_sol": 0.5,
      "price_at_signal": 0.000012,
      "tx_signature": "5j7s8K9m...",
      "status": "CLOSED",
      "retry_count": 0,
      "error_message": null,
      "pnl_sol": 0.125,
      "pnl_usd": 12.50,
      "created_at": "2025-01-15T10:00:00Z",
      "updated_at": "2025-01-15T10:30:00Z"
    }
  ],
  "total": 150,
  "limit": 100,
  "offset": 0
}
```

**Example:**
```bash
curl -X GET "https://api.chimera.dev/api/v1/trades?status=CLOSED&strategy=SHIELD&limit=50" \
  -H "Authorization: Bearer YOUR_API_KEY"
```

#### GET /api/v1/trades/export
Export trades in various formats.

**Authentication:** Bearer token (readonly+)

**Query Parameters:**
- `from`, `to`, `status`, `strategy`: Same as list endpoint
- `format`: Export format (`csv`, `json`, `pdf`)

**Response:**
- CSV: `Content-Type: text/csv`
- JSON: `Content-Type: application/json`
- PDF: `Content-Type: application/pdf`

**Headers:**
- `Content-Disposition: attachment; filename="chimera_trades_2025-01-15.csv"`

**Example:**
```bash
curl -X GET "https://api.chimera.dev/api/v1/trades/export?format=pdf&status=CLOSED" \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -o trades.pdf
```

---

### Metrics Endpoints

#### GET /api/v1/metrics/performance
Get performance metrics (24H, 7D, 30D PnL).

**Authentication:** Bearer token (readonly+)

**Response:**
```json
{
  "pnl_24h": 127.50,
  "pnl_7d": 892.30,
  "pnl_30d": 2340.00,
  "pnl_24h_change_percent": null,
  "pnl_7d_change_percent": null,
  "pnl_30d_change_percent": null
}
```

#### GET /api/v1/metrics/strategy/:strategy
Get strategy-specific performance metrics.

**Authentication:** Bearer token (readonly+)

**Path Parameters:**
- `strategy`: Strategy name (`SHIELD` or `SPEAR`)

**Query Parameters:**
- `days` (optional): Time period in days (default: 30)

**Response:**
```json
{
  "strategy": "SHIELD",
  "win_rate": 72.5,
  "avg_return": 8.2,
  "trade_count": 150,
  "total_pnl": 1230.50
}
```

---

### Configuration Endpoints

#### GET /api/v1/config
Get current system configuration.

**Authentication:** Bearer token (admin)

**Response:**
```json
{
  "circuit_breakers": {
    "max_loss_24h": 1000.0,
    "max_consecutive_losses": 5,
    "max_drawdown_percent": 20.0,
    "cool_down_minutes": 30
  },
  "strategy_allocation": {
    "shield_percent": 70,
    "spear_percent": 30
  },
  "jito_tip_strategy": {
    "tip_floor": 0.001,
    "tip_ceiling": 0.01,
    "tip_percentile": 90,
    "tip_percent_max": 0.05
  },
  "rpc_status": {
    "primary": "helius",
    "active": "helius",
    "fallback_triggered": false
  }
}
```

#### PUT /api/v1/config
Update system configuration.

**Authentication:** Bearer token (admin)

**Request Body:**
```json
{
  "circuit_breakers": {
    "max_loss_24h": 1500.0,
    "max_consecutive_losses": 5,
    "max_drawdown_percent": 25.0,
    "cool_down_minutes": 30
  },
  "strategy_allocation": {
    "shield_percent": 75,
    "spear_percent": 25
  }
}
```

**Response:**
Updated configuration object.

**Status Codes:**
- `200 OK`: Configuration updated
- `400 Bad Request`: Invalid configuration values
- `403 Forbidden`: Admin role required

#### POST /api/v1/config/circuit-breaker/reset
Reset circuit breaker to allow trading to resume.

**Authentication:** Bearer token (admin)

**Response:**
```json
{
  "success": true,
  "message": "Circuit breaker reset successfully",
  "previous_state": "OPEN",
  "new_state": "CLOSED"
}
```

**Status Codes:**
- `200 OK`: Circuit breaker reset
- `400 Bad Request`: Circuit breaker not in cooldown
- `403 Forbidden`: Admin role required

---

### Incidents Endpoints

#### GET /api/v1/incidents/dead-letter
List dead letter queue items (failed operations).

**Authentication:** Bearer token (readonly+)

**Query Parameters:**
- `limit` (optional): Results per page (default: 50, max: 200)
- `offset` (optional): Pagination offset (default: 0)

**Response:**
```json
{
  "items": [
    {
      "id": 1,
      "trade_uuid": "uuid-1234",
      "payload": "{\"strategy\":\"SHIELD\",...}",
      "reason": "QUEUE_FULL" | "PARSE_ERROR" | "VALIDATION_FAILED" | "MAX_RETRIES",
      "error_details": "Queue depth exceeded maximum",
      "source_ip": "192.168.1.1",
      "retry_count": 3,
      "can_retry": false,
      "received_at": "2025-01-15T10:00:00Z",
      "processed_at": null
    }
  ],
  "total": 10
}
```

#### GET /api/v1/incidents/config-audit
List configuration change history.

**Authentication:** Bearer token (readonly+)

**Query Parameters:**
- `limit` (optional): Results per page (default: 50, max: 200)
- `offset` (optional): Pagination offset (default: 0)

**Response:**
```json
{
  "items": [
    {
      "id": 1,
      "key": "circuit_breakers.max_loss_24h",
      "old_value": "1000.0",
      "new_value": "1500.0",
      "changed_by": "ADMIN",
      "change_reason": "Increased threshold for higher volatility",
      "changed_at": "2025-01-15T10:00:00Z"
    }
  ],
  "total": 50
}
```

---

### WebSocket Endpoint

#### GET /api/v1/ws
Real-time updates via WebSocket.

**Authentication:** Bearer token (readonly+)

**Connection:**
```javascript
const ws = new WebSocket('wss://api.chimera.dev/api/v1/ws?token=YOUR_JWT_TOKEN');

ws.onmessage = (event) => {
  const message = JSON.parse(event.data);
  console.log(message);
};
```

**Message Types:**
```json
{
  "type": "position_update" | "trade_update" | "health_update" | "alert",
  "data": {
    // Type-specific data
  }
}
```

**Position Update:**
```json
{
  "type": "position_update",
  "data": {
    "trade_uuid": "uuid-1234",
    "state": "ACTIVE",
    "unrealized_pnl_percent": 25.0
  }
}
```

**Trade Update:**
```json
{
  "type": "trade_update",
  "data": {
    "trade_uuid": "uuid-1234",
    "status": "CLOSED",
    "pnl_sol": 0.125
  }
}
```

**Health Update:**
```json
{
  "type": "health_update",
  "data": {
    "status": "healthy",
    "queue_depth": 5
  }
}
```

**Alert:**
```json
{
  "type": "alert",
  "data": {
    "severity": "critical" | "warning" | "info",
    "component": "circuit_breaker",
    "message": "Circuit breaker tripped: Max loss exceeded"
  }
}
```

---

## Error Responses

All errors follow a consistent format:

```json
{
  "error": "Error type",
  "message": "Human-readable error message",
  "details": "Additional error details (optional)"
}
```

**Common Status Codes:**
- `400 Bad Request`: Invalid request parameters
- `401 Unauthorized`: Authentication failed
- `403 Forbidden`: Insufficient permissions
- `404 Not Found`: Resource not found
- `429 Too Many Requests`: Rate limit exceeded
- `500 Internal Server Error`: Server error
- `503 Service Unavailable`: Service unavailable (circuit breaker)

---

## Rate Limiting

- **Webhook endpoint:** 100 requests/second (load shedding after threshold)
- **API endpoints:** 1000 requests/minute per API key
- **WebSocket:** No rate limit (connection-based)

Rate limit headers:
- `X-RateLimit-Limit`: Maximum requests allowed
- `X-RateLimit-Remaining`: Remaining requests
- `X-RateLimit-Reset`: Time when limit resets

---

## Pagination

List endpoints support pagination via `limit` and `offset` query parameters.

**Example:**
```bash
# First page (items 0-49)
GET /api/v1/trades?limit=50&offset=0

# Second page (items 50-99)
GET /api/v1/trades?limit=50&offset=50
```

---

## Examples

### Complete Workflow: Promote Wallet and Monitor Trades

```bash
# 1. Authenticate
TOKEN=$(curl -X POST https://api.chimera.dev/api/v1/auth/wallet \
  -H "Content-Type: application/json" \
  -d '{
    "address": "YOUR_WALLET_ADDRESS",
    "message": "Chimera authentication message",
    "signature": "YOUR_SIGNATURE"
  }' | jq -r '.token')

# 2. Promote wallet
curl -X PUT "https://api.chimera.dev/api/v1/wallets/WALLET_ADDRESS" \
  -H "Authorization: Bearer ${TOKEN}" \
  -H "Content-Type: application/json" \
  -d '{
    "status": "ACTIVE",
    "reason": "Strong performance",
    "ttl_hours": 24
  }'

# 3. Monitor positions
curl -X GET "https://api.chimera.dev/api/v1/positions?state=ACTIVE" \
  -H "Authorization: Bearer ${TOKEN}"

# 4. Export trade history
curl -X GET "https://api.chimera.dev/api/v1/trades/export?format=csv&status=CLOSED" \
  -H "Authorization: Bearer ${TOKEN}" \
  -o trades.csv
```

---

## Postman Collection

A Postman collection is available at `docs/postman/chimera-api.json` with all endpoints pre-configured.

---

## Changelog

### v1.0.0 (2025-01-15)
- Initial API release
- All endpoints documented
- WebSocket support added
- Metrics endpoints added
