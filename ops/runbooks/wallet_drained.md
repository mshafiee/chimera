# Runbook: Wallet Drained

## Overview

**Trigger:** `delta(sol_balance[1h]) < -5 SOL` (Prometheus alert) or manual observation

**Severity:** CRITICAL - SECURITY INCIDENT

**SLA:** Immediate response required

**On-Call:** @security-team, @platform-team

---

## ⚠️ CRITICAL: This May Be a Security Breach

If funds are being drained unexpectedly, assume the worst and act fast.
Every minute of delay could mean more funds lost.

---

## 1. IMMEDIATE: Kill Switch (30 seconds)

### Stop All Trading NOW
```bash
# SSH to server immediately
ssh chimera@your-server

# STOP THE SERVICE
sudo systemctl stop chimera

# Verify it's stopped
pgrep -f chimera_operator && sudo pkill -9 -f chimera_operator
```

### Disable Any External Access
```bash
# Block incoming webhook traffic (if using firewall)
sudo ufw deny 8080/tcp

# Or if using iptables
sudo iptables -A INPUT -p tcp --dport 8080 -j DROP
```

---

## 2. Verify the Drain (2 minutes)

### Check On-Chain Balance
```bash
# Using Solana CLI
solana balance YOUR_TRADING_WALLET_ADDRESS

# Or use Solscan
# https://solscan.io/account/YOUR_TRADING_WALLET_ADDRESS
```

### Compare with Expected Balance
```bash
# Query last known balance from database
sqlite3 /opt/chimera/data/chimera.db "
SELECT new_value, changed_at 
FROM config_audit 
WHERE key = 'wallet_balance' 
ORDER BY changed_at DESC 
LIMIT 5;"
```

### Identify If False Positive
Legitimate drains can occur from:
- Large position exits (check trades table)
- RPC fees accumulated
- Rent payments

```bash
# Check recent trades
sqlite3 /opt/chimera/data/chimera.db "
SELECT trade_uuid, strategy, side, amount_sol, pnl_sol, status, created_at
FROM trades 
ORDER BY created_at DESC 
LIMIT 20;"
```

---

## 3. Contain: Transfer Remaining Funds (5 minutes)

### If This Is a Real Security Incident

**DO NOT SKIP THIS STEP**

Transfer all remaining funds to a cold wallet immediately.

```bash
# Get current balance
BALANCE=$(solana balance YOUR_TRADING_WALLET --output json | jq -r '.lamports')

# Leave small amount for rent
TRANSFER_AMOUNT=$((BALANCE - 5000000))  # Leave 0.005 SOL for rent

# Transfer to cold wallet (requires access to private key)
solana transfer COLD_WALLET_ADDRESS ${TRANSFER_AMOUNT}lamports \
  --from YOUR_TRADING_WALLET_KEYPAIR.json \
  --allow-unfunded-recipient
```

### Alternative: Use Phantom/Solflare
1. Import trading wallet to Phantom (if you have the seed phrase)
2. Send all SOL to cold wallet address
3. Send all tokens to cold wallet address

---

## 4. Audit: Find the Rogue Transaction (10 minutes)

### Query Recent Transactions
```bash
# Get recent transactions from Solscan API
curl -s "https://api.solscan.io/account/transactions?account=YOUR_WALLET&limit=20" | jq .

# Or use Solana CLI
solana transaction-history YOUR_TRADING_WALLET --limit 20
```

### Check Database for Unauthorized Trades
```bash
# Find trades in the last hour
sqlite3 /opt/chimera/data/chimera.db "
SELECT 
    trade_uuid,
    wallet_address,
    token_address,
    strategy,
    side,
    amount_sol,
    tx_signature,
    status,
    created_at
FROM trades 
WHERE created_at > datetime('now', '-1 hour')
ORDER BY created_at DESC;"
```

### Check Dead Letter Queue
```bash
# Look for suspicious rejected signals
sqlite3 /opt/chimera/data/chimera.db "
SELECT payload, reason, source_ip, received_at
FROM dead_letter_queue
WHERE received_at > datetime('now', '-1 hour')
ORDER BY received_at DESC;"
```

### Check Config Audit for Unauthorized Changes
```bash
sqlite3 /opt/chimera/data/chimera.db "
SELECT * FROM config_audit 
WHERE changed_at > datetime('now', '-24 hours')
ORDER BY changed_at DESC;"
```

---

## 5. Investigate Root Cause

### Possible Attack Vectors

#### 5.1 Compromised Webhook Secret
**Signs:** Unauthorized trades appearing in database

```bash
# Check for unusual webhook activity
grep -i "hmac" /var/log/chimera/operator.log | tail -100

# Check source IPs of recent requests
grep "webhook" /var/log/chimera/operator.log | grep -oP '\d+\.\d+\.\d+\.\d+' | sort | uniq -c
```

#### 5.2 Compromised Private Key
**Signs:** On-chain transactions not in database

```bash
# Compare on-chain txs vs database txs
# Any on-chain tx NOT in our database is suspicious
```

#### 5.3 Malicious Wallet in Roster
**Signs:** Trades copying a wallet that's draining us

```bash
# Check which wallets triggered recent trades
sqlite3 /opt/chimera/data/chimera.db "
SELECT wallet_address, COUNT(*) as trades, SUM(pnl_sol) as total_pnl
FROM trades
WHERE created_at > datetime('now', '-24 hours')
GROUP BY wallet_address
ORDER BY total_pnl ASC;"
```

#### 5.4 Token Honeypot
**Signs:** Bought token but couldn't sell

```bash
# Check for tokens with failed sells
sqlite3 /opt/chimera/data/chimera.db "
SELECT token_address, token_symbol, COUNT(*) as attempts
FROM trades
WHERE side = 'SELL' AND status = 'FAILED'
AND created_at > datetime('now', '-24 hours')
GROUP BY token_address;"
```

---

## 6. Rotate Credentials (Required)

### Generate New Keypair
```bash
# Generate new trading wallet
solana-keygen new --outfile new_trading_wallet.json

# Get new address
solana-keygen pubkey new_trading_wallet.json
```

### Rotate Webhook Secret
```bash
# Generate new secret
NEW_SECRET=$(openssl rand -hex 32)

# Update configuration
echo "CHIMERA_SECURITY__WEBHOOK_SECRET=$NEW_SECRET" >> /opt/chimera/config/.env

# Notify signal provider of new secret
echo "New webhook secret: $NEW_SECRET"
```

### Update Vault
```bash
# If using encrypted vault, re-encrypt with new keys
# Follow your organization's key rotation procedure
```

---

## 7. Recovery Steps

### After Incident is Contained

1. **Verify new wallet is funded**
   ```bash
   solana airdrop 0.1 NEW_WALLET_ADDRESS  # devnet only
   # Or transfer from treasury
   ```

2. **Update configuration with new wallet**
   ```bash
   # Update .env with new wallet keypair path
   nano /opt/chimera/config/.env
   ```

3. **Clear and restart with fresh state**
   ```bash
   # Backup current database
   cp /opt/chimera/data/chimera.db /opt/chimera/data/chimera.db.incident_backup
   
   # Close all open positions in database (they're abandoned now)
   sqlite3 /opt/chimera/data/chimera.db "
   UPDATE positions SET state = 'CLOSED', 
       closed_at = datetime('now'),
       notes = 'Force closed due to security incident'
   WHERE state IN ('ACTIVE', 'EXITING');"
   ```

4. **Restart service with monitoring**
   ```bash
   # Re-enable firewall
   sudo ufw allow 8080/tcp
   
   # Start service
   sudo systemctl start chimera
   
   # Watch closely
   journalctl -u chimera -f
   ```

---

## 8. Communication Template

### Internal Notification
```
🚨 SECURITY INCIDENT: Wallet Drain Detected

Time: [UTC timestamp]
Wallet: [address]
Amount Lost: [X] SOL (~$[Y] USD)
Status: [Contained | Investigating | Resolved]

Actions Taken:
- [ ] Trading halted
- [ ] Remaining funds transferred to cold storage
- [ ] Credentials rotated
- [ ] Root cause identified

Root Cause: [if known]
Next Steps: [list]

Incident Commander: [name]
```

### If Required: External Notification
- Contact legal team before any external communication
- Document everything for potential law enforcement

---

## 9. Post-Incident Checklist

- [ ] All trading halted
- [ ] Remaining funds secured in cold wallet
- [ ] Trading wallet keypair rotated
- [ ] Webhook secret rotated
- [ ] RPC API keys rotated (if compromised)
- [ ] Root cause identified and documented
- [ ] Config audit log updated with incident record
- [ ] Incident report created
- [ ] Security review scheduled
- [ ] Team debrief completed

---

## 10. Prevention Measures

### Implement After Recovery

1. **Balance Monitoring**
   - Set up Prometheus alert for balance drops
   - Daily balance reconciliation

2. **Rate Limiting**
   - Limit max trade size per hour
   - Limit total exposure

3. **Multi-Sig for Large Transfers**
   - Require 2-of-3 for transfers > 10 SOL

4. **IP Allowlisting**
   - Only allow webhook requests from known IPs

5. **Audit Logging**
   - Log all trades with full context
   - Immutable audit trail

---

## Emergency Contacts

| Role | Contact |
|------|---------|
| Security Team Lead | @security-lead |
| Platform Team Lead | @platform-lead |
| Legal | legal@company.com |
| Cold Wallet Holder | @treasury |
