# Runbook: Reconciliation Discrepancies

## Overview

**Trigger:** Reconciliation script detects discrepancies between DB and on-chain state

**Severity:** MEDIUM to HIGH (depends on discrepancy type)

**SLA:** Investigate within 1 hour, resolve within 24 hours

**On-Call:** @platform-team, @compliance-team

---

## 1. Understand the Discrepancy (5 minutes)

### Check Reconciliation Log
```bash
# View recent reconciliation entries
sqlite3 /opt/chimera/data/chimera.db "
SELECT 
    id,
    trade_uuid,
    expected_state,
    actual_on_chain,
    discrepancy,
    on_chain_tx_signature,
    on_chain_amount_sol,
    expected_amount_sol,
    resolved_at,
    created_at
FROM reconciliation_log
WHERE resolved_at IS NULL
ORDER BY created_at DESC
LIMIT 20;"
```

### Discrepancy Types

#### SIGNATURE_MISMATCH
- **Meaning:** Transaction signature in DB doesn't match on-chain
- **Severity:** HIGH
- **Action:** Verify transaction manually

#### MISSING_TRANSACTION
- **Meaning:** Position in DB but no on-chain transaction found
- **Severity:** HIGH
- **Action:** Check if transaction was never submitted or failed silently

#### AMOUNT_MISMATCH
- **Meaning:** Amount in DB differs from on-chain amount
- **Severity:** MEDIUM (if within epsilon) to HIGH (if significant)
- **Action:** Verify if within dust tolerance (0.01%)

#### STATE_MISMATCH
- **Meaning:** Position state in DB doesn't match on-chain state
- **Severity:** MEDIUM
- **Action:** Check if position was closed externally

---

## 2. Investigate Specific Discrepancy (10 minutes)

### Get Full Position Details
```bash
# Replace TRADE_UUID with actual trade UUID from reconciliation log
TRADE_UUID="your-trade-uuid-here"

# Get position details
sqlite3 /opt/chimera/data/chimera.db "
SELECT 
    p.*,
    t.status as trade_status,
    t.tx_signature as trade_tx_signature
FROM positions p
JOIN trades t ON p.trade_uuid = t.trade_uuid
WHERE p.trade_uuid = '${TRADE_UUID}';"
```

### Check On-Chain Transaction
```bash
# If transaction signature exists, verify on-chain
TX_SIG="your-transaction-signature-here"

# Using Solana CLI
solana confirm ${TX_SIG} --output json | jq .

# Using Solscan API
curl -s "https://api.solscan.io/transaction?tx=${TX_SIG}" | jq .

# Check transaction status
curl -X POST "${RPC_URL}" \
  -H "Content-Type: application/json" \
  -d "{
    \"jsonrpc\": \"2.0\",
    \"id\": 1,
    \"method\": \"getTransaction\",
    \"params\": [\"${TX_SIG}\", {\"encoding\": \"json\", \"maxSupportedTransactionVersion\": 0}]
  }" | jq .
```

### Check Trade History
```bash
# Get all trades for this position
sqlite3 /opt/chimera/data/chimera.db "
SELECT 
    trade_uuid,
    side,
    amount_sol,
    tx_signature,
    status,
    error_message,
    created_at
FROM trades
WHERE wallet_address = (
    SELECT wallet_address FROM positions WHERE trade_uuid = '${TRADE_UUID}'
)
AND token_address = (
    SELECT token_address FROM positions WHERE trade_uuid = '${TRADE_UUID}'
)
ORDER BY created_at;"
```

---

## 3. Resolution Procedures by Type

### 3.1 SIGNATURE_MISMATCH

**Possible Causes:**
- Transaction was retried with different signature
- Database was updated incorrectly
- Transaction was replaced

**Resolution:**
```bash
# 1. Find the correct on-chain transaction
# Check recent transactions for the wallet
WALLET_ADDR="wallet-address-here"
solana transaction-history ${WALLET_ADDR} --limit 50

# 2. Update database with correct signature
sqlite3 /opt/chimera/data/chimera.db "
UPDATE positions
SET entry_tx_signature = 'CORRECT_SIGNATURE_HERE'
WHERE trade_uuid = '${TRADE_UUID}';"

# 3. Mark as resolved
sqlite3 /opt/chimera/data/chimera.db "
UPDATE reconciliation_log
SET resolved_at = datetime('now'),
    resolved_by = 'MANUAL',
    notes = 'Signature corrected: was retry transaction'
WHERE trade_uuid = '${TRADE_UUID}'
AND resolved_at IS NULL;"
```

### 3.2 MISSING_TRANSACTION

**Possible Causes:**
- Transaction was never submitted (RPC failure)
- Transaction failed silently
- Transaction was in a different wallet

**Resolution:**
```bash
# 1. Check if transaction was ever submitted
grep "${TRADE_UUID}" /var/log/chimera/operator.log | grep -i "execute\|submit\|signature"

# 2. Check dead letter queue
sqlite3 /opt/chimera/data/chimera.db "
SELECT * FROM dead_letter_queue
WHERE trade_uuid = '${TRADE_UUID}';"

# 3. If transaction was never submitted, mark position as FAILED
sqlite3 /opt/chimera/data/chimera.db "
UPDATE positions
SET state = 'CLOSED',
    exit_price = entry_price,
    realized_pnl_sol = 0.0,
    realized_pnl_usd = 0.0,
    closed_at = datetime('now')
WHERE trade_uuid = '${TRADE_UUID}';

UPDATE trades
SET status = 'FAILED',
    error_message = 'Transaction never submitted - reconciliation discrepancy'
WHERE trade_uuid = '${TRADE_UUID}';"

# 4. Mark as resolved
sqlite3 /opt/chimera/data/chimera.db "
UPDATE reconciliation_log
SET resolved_at = datetime('now'),
    resolved_by = 'AUTO',
    notes = 'Transaction never submitted, position marked as failed'
WHERE trade_uuid = '${TRADE_UUID}'
AND resolved_at IS NULL;"
```

### 3.3 AMOUNT_MISMATCH

**Possible Causes:**
- Slippage during execution
- Fee deductions
- Rounding differences
- Dust amounts

**Resolution:**
```bash
# 1. Calculate difference
EXPECTED_AMOUNT="amount-from-db"
ON_CHAIN_AMOUNT="amount-from-on-chain"
DIFF=$(echo "${EXPECTED_AMOUNT} - ${ON_CHAIN_AMOUNT}" | bc)
EPSILON="0.0001"  # 0.01% tolerance

# 2. Check if within epsilon (dust tolerance)
if (( $(echo "${DIFF#-} < ${EPSILON}" | bc -l) )); then
    # Within tolerance - update DB to match on-chain
    sqlite3 /opt/chimera/data/chimera.db "
    UPDATE positions
    SET entry_amount_sol = ${ON_CHAIN_AMOUNT}
    WHERE trade_uuid = '${TRADE_UUID}';
    
    UPDATE reconciliation_log
    SET resolved_at = datetime('now'),
        resolved_by = 'AUTO',
        notes = 'Amount mismatch within epsilon, updated to on-chain value'
    WHERE trade_uuid = '${TRADE_UUID}'
    AND resolved_at IS NULL;"
else
    # Significant mismatch - investigate further
    echo "WARNING: Significant amount mismatch detected: ${DIFF} SOL"
    # Escalate to compliance team
fi
```

### 3.4 STATE_MISMATCH

**Possible Causes:**
- Position was closed externally (manual intervention)
- Exit transaction succeeded but wasn't recorded
- State update failed

**Resolution:**
```bash
# 1. Check if exit transaction exists on-chain
EXIT_TX=$(sqlite3 /opt/chimera/data/chimera.db "
SELECT exit_tx_signature FROM positions WHERE trade_uuid = '${TRADE_UUID}';")

if [ -n "${EXIT_TX}" ]; then
    # Verify exit transaction on-chain
    solana confirm ${EXIT_TX} --output json | jq .
    
    # If confirmed, update state
    sqlite3 /opt/chimera/data/chimera.db "
    UPDATE positions
    SET state = 'CLOSED',
        closed_at = datetime('now')
    WHERE trade_uuid = '${TRADE_UUID}';"
else
    # Check if position was closed externally
    # Query on-chain for token balance
    # If balance is 0, position was closed
    # Update DB accordingly
fi

# Mark as resolved
sqlite3 /opt/chimera/data/chimera.db "
UPDATE reconciliation_log
SET resolved_at = datetime('now'),
    resolved_by = 'AUTO',
    notes = 'State mismatch resolved'
WHERE trade_uuid = '${TRADE_UUID}'
AND resolved_at IS NULL;"
```

---

## 4. Auto-Resolution (Epsilon Tolerance)

The reconciliation script automatically resolves minor discrepancies:

### Dust Amount Tolerance
- **Epsilon:** 0.0001 SOL (0.01%)
- **Auto-resolve:** Amount differences within epsilon
- **Action:** Update DB to match on-chain value

### Verification
```bash
# Check auto-resolved discrepancies
sqlite3 /opt/chimera/data/chimera.db "
SELECT 
    trade_uuid,
    discrepancy,
    resolved_by,
    resolved_at,
    notes
FROM reconciliation_log
WHERE resolved_by = 'AUTO'
AND resolved_at > datetime('now', '-24 hours')
ORDER BY resolved_at DESC;"
```

---

## 5. Manual Investigation Workflow

### Step 1: Gather Evidence
```bash
# Create investigation report
INVESTIGATION_FILE="/tmp/reconciliation_investigation_$(date +%Y%m%d_%H%M%S).txt"

{
    echo "=== Reconciliation Investigation ==="
    echo "Trade UUID: ${TRADE_UUID}"
    echo "Discrepancy Type: [TYPE]"
    echo "Timestamp: $(date -u)"
    echo ""
    echo "=== Database State ==="
    sqlite3 /opt/chimera/data/chimera.db "
    SELECT * FROM positions WHERE trade_uuid = '${TRADE_UUID}';" | column -t
    echo ""
    echo "=== On-Chain State ==="
    # Add on-chain query results
    echo ""
    echo "=== Trade History ==="
    sqlite3 /opt/chimera/data/chimera.db "
    SELECT * FROM trades WHERE trade_uuid = '${TRADE_UUID}';" | column -t
} > "${INVESTIGATION_FILE}"

cat "${INVESTIGATION_FILE}"
```

### Step 2: Compare States
```bash
# Create comparison table
sqlite3 /opt/chimera/data/chimera.db "
SELECT 
    'DB' as source,
    entry_tx_signature as tx_signature,
    entry_amount_sol as amount_sol,
    state
FROM positions
WHERE trade_uuid = '${TRADE_UUID}'
UNION ALL
SELECT 
    'ON_CHAIN' as source,
    on_chain_tx_signature as tx_signature,
    on_chain_amount_sol as amount_sol,
    actual_on_chain as state
FROM reconciliation_log
WHERE trade_uuid = '${TRADE_UUID}'
AND resolved_at IS NULL;"
```

### Step 3: Determine Root Cause
- Review logs around transaction time
- Check for RPC errors
- Verify wallet permissions
- Check for manual interventions

---

## 6. Escalation Criteria

### Escalate to Compliance Team
- Amount mismatch > 1 SOL
- Multiple discrepancies in 24 hours
- Unresolved discrepancies > 7 days
- Suspected fraud or manipulation

### Escalate to Platform Team
- System bug causing discrepancies
- Database corruption
- Reconciliation script failures

---

## 7. Prevention Measures

### Daily Reconciliation
- Runs automatically at 4 AM via cron
- Checks all ACTIVE and EXITING positions
- Logs all discrepancies

### Monitoring
```bash
# Set up alert for unresolved discrepancies
sqlite3 /opt/chimera/data/chimera.db "
SELECT COUNT(*) as unresolved
FROM reconciliation_log
WHERE resolved_at IS NULL
AND created_at > datetime('now', '-24 hours');"

# Alert if > 5 unresolved discrepancies
```

### Best Practices
- Regular database backups
- Transaction signature verification on submission
- Amount validation before position creation
- State machine enforcement

---

## 8. Communication Template

### Internal Notification
```
📊 RECONCILIATION DISCREPANCY DETECTED

Time: [UTC timestamp]
Trade UUID: [uuid]
Discrepancy Type: [SIGNATURE_MISMATCH / MISSING_TRANSACTION / etc.]
Severity: [MEDIUM / HIGH]

Database State:
- Signature: [sig]
- Amount: [X] SOL
- State: [ACTIVE / CLOSED]

On-Chain State:
- Signature: [sig]
- Amount: [Y] SOL
- Status: [FOUND / MISSING]

Investigation:
- [ ] Root cause identified
- [ ] Resolution plan determined
- [ ] Database updated (if needed)
- [ ] Discrepancy marked as resolved

ETA for Resolution: [if known]
```

---

## 9. Post-Resolution Verification

### Verify Resolution
```bash
# Re-run reconciliation for this trade
# (Manual check - reconciliation script runs daily)

# Verify position state is correct
sqlite3 /opt/chimera/data/chimera.db "
SELECT 
    p.trade_uuid,
    p.state,
    p.entry_tx_signature,
    p.entry_amount_sol,
    r.resolved_at,
    r.resolved_by
FROM positions p
LEFT JOIN reconciliation_log r ON p.trade_uuid = r.trade_uuid
WHERE p.trade_uuid = '${TRADE_UUID}';"
```

### Update Audit Log
```bash
# Resolution should be logged in reconciliation_log
# Verify entry exists
sqlite3 /opt/chimera/data/chimera.db "
SELECT * FROM reconciliation_log
WHERE trade_uuid = '${TRADE_UUID}'
ORDER BY created_at DESC
LIMIT 1;"
```

---

## 10. Prevention Checklist

- [ ] Daily reconciliation script running (cron)
- [ ] Epsilon tolerance configured (0.01%)
- [ ] Monitoring alerts for unresolved discrepancies
- [ ] Transaction signature verification on submission
- [ ] Amount validation before position creation
- [ ] Regular database integrity checks
- [ ] Team trained on reconciliation process

---

## Emergency Contacts

| Role | Contact |
|------|---------|
| Platform Team Lead | @platform-lead |
| Compliance Team | @compliance-team |
| Database Admin | @dba-team |
