# Running Scout with Wallet Discovery

## Quick Start

Scout is now configured to automatically discover wallets from on-chain data using Helius API.

### Run Scout Now

```bash
# Option 1: Run via Docker (recommended)
docker exec chimera-scout python3 /app/main.py --verbose

# Option 2: Run from host
cd scout
python3 main.py --verbose
```

### What Happens

1. **Discovery**: Scout queries Helius API for recent swap transactions
2. **Extraction**: Identifies wallet addresses from transactions
3. **Analysis**: Fetches transaction history for each wallet
4. **Scoring**: Calculates WQS scores and metrics
5. **Output**: Writes real wallets to `data/roster_new.db`

### Verify Results

After Scout runs, check the discovered wallets:

```bash
# View roster
python3 -c "
import sqlite3
conn = sqlite3.connect('data/roster_new.db')
cursor = conn.cursor()
cursor.execute('SELECT address, status, wqs_score, trade_count_30d FROM wallets ORDER BY wqs_score DESC LIMIT 10')
print('Top 10 Discovered Wallets:')
print('-' * 80)
for row in cursor.fetchall():
    print(f'{row[0][:20]}... | {row[1]:10} | WQS: {row[2]:5.1f} | Trades: {row[3]}')
conn.close()
"
```

### Verify Wallets Are Real

Check a wallet on Solana Explorer:
- Go to: https://explorer.solana.com/address/WALLET_ADDRESS
- Or: https://solscan.io/account/WALLET_ADDRESS

The wallets should exist and show real transaction history!

### Merge Roster

Once you've verified the wallets, merge them into the main database:

```bash
# Using authentication
./authenticate-and-merge.sh YOUR_WALLET_ADDRESS /path/to/keypair.json

# Or manually (if operator is stopped)
python3 ops/merge_roster.py data/roster_new.db data/chimera.db
```

## Configuration

The Helius API key is automatically detected from your environment:
- `HELIUS_API_KEY` environment variable
- Or extracted from `CHIMERA_RPC__PRIMARY_URL` if it contains `api-key=`

Your current setup already has this configured in `docker/env.mainnet-paper.local`.

## Expected Output

When Scout runs successfully, you should see:

```
[Analyzer] Discovering wallets from on-chain data...
[Helius] Discovering wallets from recent swaps (limit: 200)...
[Helius] Discovered 45 wallets with 3+ trades
[Analyzer] Discovered 45 candidate wallets
[Scout] Analyzing wallets...
  [ACTIVE] 7xKXtg2C... WQS: 78.2 | Backtest: PASSED
  [CANDIDATE] 9mNpQrAb... WQS: 67.6 | Backtest: SKIPPED
  ...
[Scout] Analysis complete:
  Total analyzed: 45
  ACTIVE: 12
  CANDIDATE: 18
  REJECTED: 15
[Scout] Successfully wrote 45 wallets
```

## Troubleshooting

### No Wallets Discovered

If Scout discovers 0 wallets:
1. Check Helius API key is configured
2. Verify API has access to Enhanced Transactions
3. Check network connectivity
4. Review logs for errors

### API Errors

If you see Helius API errors:
- Check API key is valid
- Verify rate limits (50 req/sec for Developer plan)
- Wait a few seconds and retry

### Import Errors

If you see import errors:
```bash
# Rebuild Scout container
./docker/docker-compose.sh build mainnet-paper scout
./docker/docker-compose.sh restart mainnet-paper scout
```

## Next Steps

1. ✅ Run Scout to discover wallets
2. ✅ Verify wallets exist on Solana Explorer
3. ✅ Merge roster into main database
4. ✅ Promote high-quality wallets to ACTIVE
5. ✅ Start copying trades from ACTIVE wallets

The system is now ready to discover and analyze **real Solana wallets**!




