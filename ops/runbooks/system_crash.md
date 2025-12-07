# Runbook: System Crash

## Overview

**Trigger:** `up{job="operator"} == 0` (Prometheus alert) or service not responding

**Severity:** CRITICAL

**SLA:** Restore service within 30 minutes

**On-Call:** @platform-team

---

## 1. Initial Assessment (2 minutes)

### Check Service Status
```bash
# Check if service is running
systemctl status chimera

# Check recent logs
journalctl -u chimera -n 100 --no-pager

# Check if process exists
pgrep -f chimera_operator
```

### Quick Health Check
```bash
# Test API health endpoint
curl -s http://localhost:8080/health | jq .

# Check database accessibility
sqlite3 /opt/chimera/data/chimera.db "SELECT 1;"
```

---

## 2. Common Failure Modes

### 2.1 Out of Memory (OOM)

**Symptoms:**
- `journalctl` shows "Killed" or "oom-killer"
- `dmesg | grep -i oom` shows entries

**Resolution:**
```bash
# Check memory status
free -h

# Check what killed it
dmesg | tail -50 | grep -i oom

# Restart with monitoring
systemctl restart chimera
watch -n 5 'ps aux | grep chimera'
```

**Prevention:**
- Increase `MemoryMax` in systemd unit
- Enable load shedding (`queue_depth > 800`)
- Add swap space if needed

### 2.2 Database Locked / Corrupted

**Symptoms:**
- Logs show "database is locked" or "SQLITE_BUSY"
- Logs show "malformed" or "corrupt"

**Resolution:**
```bash
# Check database integrity
sqlite3 /opt/chimera/data/chimera.db "PRAGMA integrity_check;"

# If locked, check for stale processes
fuser /opt/chimera/data/chimera.db

# If corrupted, restore from backup
ls -la /opt/chimera/backups/
sqlite3 /opt/chimera/data/chimera.db.backup "PRAGMA integrity_check;"
cp /opt/chimera/backups/chimera_YYYYMMDD.db /opt/chimera/data/chimera.db

# Restart
systemctl restart chimera
```

### 2.3 RPC Connection Failure

**Symptoms:**
- Logs show "connection refused" or "timeout"
- Helius/QuickNode API errors

**Resolution:**
```bash
# Test RPC connectivity
curl -X POST https://mainnet.helius-rpc.com/?api-key=YOUR_KEY \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}'

# Check if fallback is configured
grep -i fallback /opt/chimera/config/.env

# Restart to trigger fallback
systemctl restart chimera
```

### 2.4 Configuration Error

**Symptoms:**
- Logs show "Configuration error" or "Failed to load"
- Service fails immediately on start

**Resolution:**
```bash
# Validate config
cat /opt/chimera/config/config.yaml

# Check environment file
cat /opt/chimera/config/.env

# Test config loading
cd /opt/chimera/operator && ./target/release/chimera_operator --check-config

# Fix and restart
systemctl restart chimera
```

### 2.5 Disk Full

**Symptoms:**
- Logs show "No space left on device"
- `df -h` shows full disk

**Resolution:**
```bash
# Check disk usage
df -h
du -sh /opt/chimera/*
du -sh /var/log/chimera/*

# Clear old logs
journalctl --vacuum-time=3d
rm /var/log/chimera/*.log.*.gz

# Clear old backups (keep last 3)
ls -t /opt/chimera/backups/chimera_*.db | tail -n +4 | xargs rm -f

# Restart
systemctl restart chimera
```

---

## 3. Restart Procedure

### Standard Restart
```bash
# Stop gracefully (wait for position cleanup)
systemctl stop chimera

# Wait for clean shutdown
sleep 5

# Start
systemctl start chimera

# Verify
systemctl status chimera
journalctl -u chimera -f
```

### Force Restart (if unresponsive)
```bash
# Force kill
systemctl kill -s SIGKILL chimera

# Wait for process to fully exit
sleep 2

# Clean up any stale locks
fuser -k /opt/chimera/data/chimera.db 2>/dev/null || true

# Start
systemctl start chimera
```

---

## 4. Post-Restart Verification

### Check System Health
```bash
# API health
curl -s http://localhost:8080/health | jq .

# Check circuit breaker status
curl -s http://localhost:8080/api/v1/health | jq '.circuit_breaker'

# Check queue depth
curl -s http://localhost:8080/api/v1/health | jq '.queue_depth'
```

### Check for Stuck Positions
```bash
# Query positions in limbo states
sqlite3 /opt/chimera/data/chimera.db "
SELECT trade_uuid, state, last_updated 
FROM positions 
WHERE state IN ('EXECUTING', 'EXITING')
AND last_updated < datetime('now', '-5 minutes');"
```

### Verify Trades Processing
```bash
# Watch for new trades being processed
tail -f /var/log/chimera/operator.log | grep -E "(Processing signal|Trade executed)"
```

---

## 5. Escalation

### When to Escalate

- Service doesn't start after 3 attempts
- Database corruption that can't be restored from backup
- Unknown error patterns
- Multiple systems affected

### Escalation Path

1. **L1:** On-call engineer (this runbook)
2. **L2:** Platform team lead
3. **L3:** Architecture team

### Communication Template

```
INCIDENT: Chimera System Crash
TIME: [timestamp UTC]
STATUS: [Investigating | Mitigating | Resolved]
IMPACT: Trading halted, no new positions being opened
ROOT CAUSE: [if known]
ACTIONS TAKEN: [list actions]
NEXT STEPS: [if ongoing]
```

---

## 6. Post-Incident

### Required Actions

1. [ ] Verify all active positions are correctly tracked
2. [ ] Run reconciliation to check for discrepancies
3. [ ] Review logs for root cause
4. [ ] Update config_audit with incident record
5. [ ] Create incident report if downtime > 15 minutes

### Log Incident
```bash
sqlite3 /opt/chimera/data/chimera.db "
INSERT INTO config_audit (key, old_value, new_value, changed_by, change_reason)
VALUES ('incident', 'none', 'system_crash', 'ONCALL_ENGINEER', 
        'System crash at [TIME]. Root cause: [CAUSE]. Resolution: [ACTIONS]');"
```

---

## 7. Prevention Checklist

- [ ] Memory limits configured appropriately
- [ ] Disk space monitoring in place
- [ ] Database backups running daily
- [ ] RPC fallback configured and tested
- [ ] Log rotation preventing disk fill
- [ ] Circuit breakers configured
