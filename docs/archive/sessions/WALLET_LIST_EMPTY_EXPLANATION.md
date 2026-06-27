# Why Wallet List is Empty

## Root Cause

The wallet list is empty because **the roster hasn't been merged into the main database yet**.

## How the System Works

### Two-Stage Process:

1. **Scout Service** (Python):
   - Analyzes wallets on-chain
   - Calculates WQS (Wallet Quality Score) and performance metrics
   - Writes analyzed wallets to `data/roster_new.db`

2. **Roster Merge** (Operator):
   - Merges `roster_new.db` → `chimera.db`
   - Makes wallets available via API
   - Required before wallets appear in `/api/v1/wallets`

## Current Status

- ✅ `roster_new.db` exists with **5 wallets**
- ❌ Roster hasn't been merged into `chimera.db`
- ❌ Wallet list API returns empty

## Solution: Merge the Roster

### Option 1: Via API (Requires Authentication)

The roster merge endpoint requires authentication in mainnet-paper mode:

```bash
# You need a JWT token from wallet authentication first
curl -X POST http://localhost:8080/api/v1/roster/merge \
  -H "Authorization: Bearer <your-jwt-token>" \
  -H "Content-Type: application/json" \
  -d '{}'
```

### Option 2: Direct Database Merge (When Operator is Idle)

Use the Python merge script when the database isn't locked:

```bash
docker exec chimera-scout python3 /app/ops/merge_roster.py \
  /app/data/roster_new.db \
  /app/data/chimera.db
```

### Option 3: Temporarily Enable Devnet Mode

Set `CHIMERA_ENV=devnet` in the environment file to allow unauthenticated roster merge (for testing only).

## Wallets in Roster

The `roster_new.db` contains 5 wallets:
- 1 ACTIVE wallet (WQS: 78.2)
- 3 CANDIDATE wallets (WQS: 43.25 - 71.78)
- 1 REJECTED wallet (WQS: 23.625)

## After Merging

Once the roster is merged:
1. Wallets will appear in `/api/v1/wallets`
2. You can view wallet details and metrics
3. You can promote CANDIDATE wallets to ACTIVE
4. The system will start copying trades from ACTIVE wallets

## Next Steps

1. **Merge the roster** using one of the methods above
2. **Verify wallets appear** in the API
3. **Promote wallets** to ACTIVE status if needed
4. **Monitor** the system for trade copying




