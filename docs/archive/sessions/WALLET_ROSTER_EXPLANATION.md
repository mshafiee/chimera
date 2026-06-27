# Wallet Roster System - Why Wallet List is Empty

## How the Wallet System Works

The Chimera system uses a **two-stage wallet management process**:

### Stage 1: Scout Analysis (Python)
- The **Scout** service analyzes wallets on-chain
- It calculates WQS (Wallet Quality Score) and performance metrics
- Writes analyzed wallets to `roster_new.db`

### Stage 2: Roster Merge (Operator)
- The **Operator** merges `roster_new.db` into the main `chimera.db`
- This makes wallets available via the API
- Wallets can then be promoted to ACTIVE status for trading

## Why Your Wallet List is Empty

The wallet list is empty because:
1. ✅ `roster_new.db` exists (Scout has analyzed wallets)
2. ❌ The roster hasn't been merged into the main database yet

## Solution: Merge the Roster

### Option 1: Via API (Recommended)
```bash
# Validate roster first
curl http://localhost:8080/api/v1/roster/validate

# Merge the roster
curl -X POST http://localhost:8080/api/v1/roster/merge \
  -H "Content-Type: application/json" \
  -d '{}'
```

### Option 2: Check Scout Status
The Scout service runs on a schedule (daily at 2 AM UTC by default). You can:
- Check Scout logs: `./docker/docker-compose.sh logs mainnet-paper -f scout`
- Manually trigger Scout if needed
- Wait for the next scheduled run

## After Merging

Once the roster is merged:
1. Wallets will appear in `/api/v1/wallets`
2. You can view wallet details and metrics
3. You can promote wallets to ACTIVE status for trading
4. The system will start copying trades from ACTIVE wallets

## Roster File Location

- **Scout Output**: `data/roster_new.db` (created by Scout)
- **Main Database**: `data/chimera.db` (used by Operator)
- **Merge Process**: Copies wallets from `roster_new.db` → `chimera.db`

## Manual Wallet Addition

If you want to add wallets manually (for testing), you can:
1. Use the roster merge API endpoint
2. Or directly insert into the database (not recommended)
3. Or wait for Scout to generate a new roster

## Next Steps

1. **Merge the existing roster**: Use the API endpoint above
2. **Check wallet list**: Verify wallets appear after merge
3. **Promote wallets**: Set wallets to ACTIVE status for trading
4. **Monitor Scout**: Ensure Scout is running and generating rosters




