# Discovering Real Solana Wallets

## Current Status

The wallet discovery from raw Helius API transactions requires the Enhanced Transactions API which has specific requirements. For now, we have two approaches:

## Approach 1: Manual Wallet List (Recommended)

Create a file with real wallet addresses you want to analyze:

```bash
# Edit the wallet list
nano scout/config/wallets.txt

# Add real wallet addresses, one per line:
7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU
9mNpQrAbCdEfGhIjKlMnOpQrStUvWxYz1234567890
# ... add more addresses
```

Then run Scout:
```bash
docker exec chimera-scout python3 /app/main.py --verbose --output /app/data/roster_new.db
```

## Approach 2: Find Wallets Manually

### Using Solana Explorer

1. Go to https://explorer.solana.com/
2. Browse recent token transactions
3. Click on wallet addresses that made profitable trades
4. Copy the wallet address
5. Add to `scout/config/wallets.txt`

### Using Solscan

1. Go to https://solscan.io/
2. Browse token pages
3. Check "Top Traders" section
4. Copy wallet addresses
5. Add to `scout/config/wallets.txt`

### Using Birdeye or DexScreener

1. Browse trending tokens
2. Check "Top Traders" or "Recent Transactions"
3. Identify profitable wallets
4. Copy addresses to `scout/config/wallets.txt`

## Running Scout with Real Wallets

Once you have wallet addresses:

```bash
# 1. Copy wallet list to container
docker cp scout/config/wallets.txt chimera-scout:/app/config/wallets.txt

# 2. Run Scout
docker exec chimera-scout python3 /app/main.py --verbose --output /app/data/roster_new.db

# 3. Check results
python3 -c "
import sqlite3
conn = sqlite3.connect('data/roster_new.db')
cursor = conn.cursor()
cursor.execute('SELECT address, status, wqs_score, trade_count_30d FROM wallets ORDER BY wqs_score DESC')
print('Discovered Wallets:')
for row in cursor.fetchall():
    print(f'{row[0][:20]}... | {row[1]:10} | WQS: {row[2]:5.1f} | Trades: {row[3]}')
conn.close()
"
```

## Verify Wallets Are Real

Check each wallet on Solana Explorer:
- https://explorer.solana.com/address/WALLET_ADDRESS
- Verify it has real transaction history
- Check it's actively trading

## Next Steps

1. ✅ Add real wallet addresses to `scout/config/wallets.txt`
2. ✅ Run Scout to analyze them
3. ✅ Verify wallets exist on Solana Explorer
4. ✅ Merge roster into main database
5. ✅ Promote high-quality wallets to ACTIVE

## Future Enhancement

Full automatic discovery from Helius Enhanced Transactions API requires:
- Proper API endpoint configuration
- Transaction parsing for swap detection
- Wallet address extraction from transaction data
- This is a more complex feature that can be added later

For now, the manual wallet list approach is the most reliable way to analyze real Solana wallets.




