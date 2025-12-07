# Runbook: RPC Fallback Triggered

## Overview

**Trigger:** `rpc_fallback_triggered == true` (Prometheus alert) or logs show "Switching to fallback RPC mode"

**Severity:** HIGH - Trading continues but with reduced performance

**SLA:** Investigate within 15 minutes, resolve within 2 hours

**On-Call:** @platform-team

---

## 1. Verify Fallback Status (2 minutes)

### Check Current RPC Mode
```bash
# Check API config endpoint
curl -s http://localhost:8080/api/v1/config | jq '.rpc_status'

# Expected output:
# {
#   "primary": "helius",
#   "active": "quicknode",  # <-- indicates fallback active
#   "fallback_triggered": true
# }
```

### Check Logs for Fallback Trigger
```bash
# Search for fallback activation
grep -i "fallback" /var/log/chimera/operator.log | tail -20

# Check config audit for RPC mode change
sqlite3 /opt/chimera/data/chimera.db "
SELECT key, old_value, new_value, changed_by, change_reason, changed_at
FROM config_audit
WHERE key = 'rpc_mode'
ORDER BY changed_at DESC
LIMIT 5;"
```

### Check Health Endpoint
```bash
# Verify system is still operational
curl -s http://localhost:8080/health | jq '.rpc'

# Should show:
# {
#   "status": "degraded",  # or "healthy" if fallback is working
#   "message": "Using fallback RPC provider"
# }
```

---

## 2. Understand Why Fallback Triggered (5 minutes)

### Check Failure Count
The system switches to fallback when:
- Consecutive RPC failures >= `max_consecutive_failures` (default: 3)
- Primary RPC (Helius) becomes unresponsive or returns errors

### Review Recent Errors
```bash
# Check for RPC errors in logs
grep -E "(RPC|connection|timeout|failed)" /var/log/chimera/operator.log | tail -50

# Check for specific error patterns
grep -i "executor error" /var/log/chimera/operator.log | tail -20
```

### Test Primary RPC Connectivity
```bash
# Test Helius RPC directly
curl -X POST "${HELIUS_RPC_URL}" \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "getHealth"
  }'

# Check response time
time curl -X POST "${HELIUS_RPC_URL}" \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "getSlot"
  }'
```

### Check RPC Rate Limits
```bash
# Check if we're hitting rate limits
grep -i "rate limit\|429\|too many" /var/log/chimera/operator.log | tail -20

# Check current rate limit configuration
grep -i "rate_limit" /opt/chimera/config/config.yaml
```

---

## 3. Verify Fallback is Working (2 minutes)

### Check Trading Status
```bash
# Verify trades are still being executed
sqlite3 /opt/chimera/data/chimera.db "
SELECT COUNT(*) as recent_trades
FROM trades
WHERE created_at > datetime('now', '-10 minutes')
AND status IN ('ACTIVE', 'CLOSED');"

# Check for failed trades
sqlite3 /opt/chimera/data/chimera.db "
SELECT COUNT(*) as failed_trades
FROM trades
WHERE created_at > datetime('now', '-10 minutes')
AND status = 'FAILED';"
```

### Check Spear Strategy Status
**IMPORTANT:** Spear strategy is automatically disabled in fallback mode (Standard RPC doesn't support Jito bundles)

```bash
# Check if Spear trades are being rejected
grep -i "spear.*disabled\|spear.*rejected" /var/log/chimera/operator.log | tail -10

# Verify only Shield trades are executing
sqlite3 /opt/chimera/data/chimera.db "
SELECT strategy, COUNT(*) as count
FROM trades
WHERE created_at > datetime('now', '-10 minutes')
AND status IN ('ACTIVE', 'CLOSED')
GROUP BY strategy;"
```

### Monitor Queue Depth
```bash
# Check if queue is backing up
curl -s http://localhost:8080/api/v1/health | jq '.queue_depth'

# If queue_depth > 800, load shedding is active
```

---

## 4. Root Cause Analysis

### Common Causes

#### 4.1 Helius API Key Issues
**Symptoms:** 401 Unauthorized, 403 Forbidden

**Resolution:**
```bash
# Verify API key is valid
curl -X POST "${HELIUS_RPC_URL}" \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "getHealth"
  }'

# Check API key in config
grep "HELIUS.*API" /opt/chimera/config/.env

# If expired, update and restart
nano /opt/chimera/config/.env
systemctl restart chimera
```

#### 4.2 Network Connectivity Issues
**Symptoms:** Connection refused, timeout errors

**Resolution:**
```bash
# Test network connectivity
ping -c 3 mainnet.helius-rpc.com

# Check DNS resolution
nslookup mainnet.helius-rpc.com

# Check firewall rules
sudo ufw status
sudo iptables -L -n | grep 443
```

#### 4.3 Helius Service Outage
**Symptoms:** 503 Service Unavailable, connection timeouts

**Resolution:**
- Check Helius status page: https://status.helius.dev
- Check Solana network status: https://status.solana.com
- Wait for Helius to recover (system will auto-recover)

#### 4.4 Rate Limit Exceeded
**Symptoms:** 429 Too Many Requests

**Resolution:**
```bash
# Check current rate limit setting
grep "rate_limit_per_second" /opt/chimera/config/config.yaml

# If too high, reduce it
# Default should be 40 req/s for Helius

# Restart to apply changes
systemctl restart chimera
```

#### 4.5 Jito Bundle Service Issues
**Symptoms:** Jito-specific errors, bundle submission failures

**Resolution:**
- Check Jito status: https://jito.wtf/status
- System automatically falls back to Standard RPC (no bundles)
- Spear strategy will be disabled until Jito recovers

---

## 5. Recovery to Primary RPC

### Automatic Recovery
The system automatically attempts to recover to primary RPC every 5 minutes when in fallback mode.

**Recovery conditions:**
- Primary RPC health check passes
- Jito is enabled and healthy
- Enough time has passed since fallback (5 minutes)

### Manual Recovery (if needed)
```bash
# Force recovery attempt by restarting service
# This will trigger immediate health check
systemctl restart chimera

# Monitor recovery
journalctl -u chimera -f | grep -i "recover\|jito\|fallback"
```

### Verify Recovery
```bash
# Check RPC mode after recovery
curl -s http://localhost:8080/api/v1/config | jq '.rpc_status.fallback_triggered'

# Should be: false

# Check config audit for recovery log
sqlite3 /opt/chimera/data/chimera.db "
SELECT * FROM config_audit
WHERE key = 'rpc_mode'
AND change_reason LIKE '%RECOVERY%'
ORDER BY changed_at DESC
LIMIT 1;"
```

---

## 6. When Fallback is Expected

### Normal Scenarios
- **Helius maintenance:** Scheduled maintenance windows
- **Network issues:** Temporary connectivity problems
- **Rate limit spikes:** Burst traffic exceeding limits

**Action:** Monitor and wait for automatic recovery

### Abnormal Scenarios
- **Extended fallback (> 1 hour):** Investigate Helius status
- **Frequent fallbacks:** Check rate limit configuration
- **Fallback during high volatility:** May impact trade execution speed

**Action:** Escalate to platform team

---

## 7. Impact Assessment

### What Still Works
- ✅ Shield strategy trades (Standard RPC)
- ✅ Position tracking
- ✅ Webhook signal processing
- ✅ Database operations
- ✅ API endpoints

### What's Disabled
- ❌ Spear strategy trades (requires Jito bundles)
- ❌ Jito bundle submission
- ❌ Optimal transaction prioritization

### Performance Impact
- **Latency:** Slightly higher (Standard RPC vs Jito)
- **Throughput:** Reduced (no bundle batching)
- **Success Rate:** Should remain similar

---

## 8. Monitoring During Fallback

### Key Metrics to Watch
```bash
# Queue depth (should stay < 1000)
watch -n 5 'curl -s http://localhost:8080/api/v1/health | jq .queue_depth'

# Failed trade rate
watch -n 10 'sqlite3 /opt/chimera/data/chimera.db "SELECT COUNT(*) FROM trades WHERE status = '\''FAILED'\'' AND created_at > datetime('\''now'\'', '\''-10 minutes'\'');"'

# RPC latency
watch -n 5 'curl -s http://localhost:8080/api/v1/health | jq .rpc_latency_ms'
```

### Alert Thresholds
- Queue depth > 900: Load shedding active
- Failed trade rate > 10%: Investigate immediately
- Fallback duration > 2 hours: Escalate

---

## 9. Communication Template

### Internal Notification
```
⚠️ RPC FALLBACK ACTIVATED

Time: [UTC timestamp]
Primary RPC: [Helius/QuickNode]
Fallback RPC: [QuickNode/Helius]
Trigger Reason: [Consecutive failures / Rate limit / etc.]
Status: [Monitoring / Investigating / Resolved]

Impact:
- Spear strategy disabled
- Trading continues with Shield only
- Slightly higher latency

Actions:
- [ ] Verified fallback is working
- [ ] Identified root cause
- [ ] Monitoring for recovery
- [ ] Notified signal provider (if extended)

ETA for Recovery: [if known]
```

---

## 10. Post-Recovery Verification

### After Primary RPC Recovers

1. **Verify RPC Mode**
   ```bash
   curl -s http://localhost:8080/api/v1/config | jq '.rpc_status'
   ```

2. **Check Spear Strategy Resumed**
   ```bash
   # Verify Spear trades are being accepted again
   grep -i "spear" /var/log/chimera/operator.log | tail -10
   ```

3. **Monitor First Few Trades**
   ```bash
   # Watch for successful Jito bundle submissions
   journalctl -u chimera -f | grep -i "bundle\|jito"
   ```

4. **Update Config Audit**
   ```bash
   # Recovery should be auto-logged, but verify
   sqlite3 /opt/chimera/data/chimera.db "
   SELECT * FROM config_audit
   WHERE key = 'rpc_mode'
   ORDER BY changed_at DESC
   LIMIT 3;"
   ```

---

## 11. Prevention Checklist

- [ ] RPC rate limits configured appropriately (40 req/s for Helius)
- [ ] Fallback RPC provider configured and tested
- [ ] Monitoring alerts configured for fallback events
- [ ] Automatic recovery enabled (default: every 5 minutes)
- [ ] Team trained on fallback behavior and impact

---

## 12. Escalation

### When to Escalate
- Fallback duration > 4 hours
- Primary RPC down for > 2 hours
- Multiple fallback events in 24 hours
- Trading performance significantly degraded

### Escalation Path
1. **L1:** On-call engineer (this runbook)
2. **L2:** Platform team lead
3. **L3:** Infrastructure team (if RPC provider issue)

---

## Emergency Contacts

| Role | Contact |
|------|---------|
| Platform Team Lead | @platform-lead |
| Helius Support | support@helius.dev |
| QuickNode Support | support@quicknode.com |
| Infrastructure Team | @infra-team |
