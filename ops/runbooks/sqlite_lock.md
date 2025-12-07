# Runbook: SQLite Lock Issues

## Overview

**Trigger:** Database operations fail with "database is locked" errors

**Severity:** MEDIUM to HIGH (depends on duration)

**SLA:** Resolve within 15 minutes

**On-Call:** @platform-team

---

## 1. Identify the Issue (2 minutes)

### Symptoms
- Database write operations fail
- Error messages: "database is locked" or "SQLITE_BUSY"
- Application logs show lock timeouts
- Trades fail to be recorded

### Check Database Status
```bash
# Check for active connections
sqlite3 /opt/chimera/data/chimera.db "PRAGMA busy_timeout;"

# Check WAL mode
sqlite3 /opt/chimera/data/chimera.db "PRAGMA journal_mode;"
# Should return: wal

# Check for locks
lsof /opt/chimera/data/chimera.db
```

---

## 2. Immediate Actions (5 minutes)

### Step 1: Check for Long-Running Queries
```bash
# Check process list for SQLite operations
ps aux | grep sqlite
ps aux | grep chimera

# Check for VACUUM operations
ps aux | grep -i vacuum
```

### Step 2: Verify WAL Mode
```bash
sqlite3 /opt/chimera/data/chimera.db "PRAGMA journal_mode;"
```

If not in WAL mode:
```bash
# Enable WAL mode (requires exclusive access)
sqlite3 /opt/chimera/data/chimera.db "PRAGMA journal_mode=WAL;"
```

### Step 3: Check Busy Timeout
```bash
# Verify busy_timeout is set (should be 5000ms)
sqlite3 /opt/chimera/data/chimera.db "PRAGMA busy_timeout;"
```

---

## 3. Resolution Steps (10 minutes)

### Scenario A: VACUUM Running

**Problem:** VACUUM operation is blocking writes

**Solution:**
```bash
# Check if VACUUM is running
ps aux | grep -i vacuum

# If VACUUM is stuck, kill it (carefully)
# First, check if it's safe to kill
# VACUUM should not be running during active trading

# Kill VACUUM process
pkill -f "VACUUM"

# Restart service to clear locks
systemctl restart chimera
```

### Scenario B: Multiple Writers

**Problem:** Multiple processes writing simultaneously

**Solution:**
1. **Verify Scout Pattern:**
   - Scout should write to `roster_new.db`, not main DB
   - Check if Scout is writing directly: `ps aux | grep scout`

2. **Check Connection Pool:**
   ```bash
   # Verify max_connections is reasonable (5-10)
   grep max_connections /opt/chimera/config/config.yaml
   ```

3. **Reduce Concurrent Writes:**
   - Temporarily reduce connection pool size
   - Restart service to clear connections

### Scenario C: Stuck Transaction

**Problem:** Transaction is stuck and holding lock

**Solution:**
```bash
# Check for stuck transactions
sqlite3 /opt/chimera/data/chimera.db "
SELECT * FROM sqlite_master WHERE type='table';
PRAGMA wal_checkpoint;
"

# Force checkpoint to release locks
sqlite3 /opt/chimera/data/chimera.db "PRAGMA wal_checkpoint(TRUNCATE);"

# If still locked, restart service
systemctl restart chimera
```

### Scenario D: Disk I/O Issues

**Problem:** Slow disk I/O causing lock timeouts

**Solution:**
```bash
# Check disk I/O
iostat -x 1 5

# Check disk space
df -h /opt/chimera/data

# If disk is full or slow:
# 1. Free up disk space
# 2. Check for disk errors: dmesg | grep -i error
# 3. Consider moving database to faster storage
```

---

## 4. Prevention Measures

### Configuration
- **WAL Mode:** Always enabled
- **Busy Timeout:** 5000ms minimum
- **Connection Pool:** Limit to 5-10 connections
- **Scout Pattern:** Write to `roster_new.db`, merge via SQL

### Monitoring
- Alert on lock timeouts > 1 second
- Monitor database file locks: `lsof /opt/chimera/data/chimera.db`
- Track VACUUM operations

### Best Practices
- Schedule VACUUM during low-traffic periods
- Use connection pooling
- Avoid long-running transactions
- Use WAL mode for concurrent access

---

## 5. Verification

After resolution, verify:

```bash
# 1. Database is accessible
sqlite3 /opt/chimera/data/chimera.db "SELECT COUNT(*) FROM trades;"

# 2. WAL mode is enabled
sqlite3 /opt/chimera/data/chimera.db "PRAGMA journal_mode;"

# 3. No active locks
lsof /opt/chimera/data/chimera.db | wc -l
# Should be low (1-5 connections)

# 4. Service is healthy
curl http://localhost:8080/api/v1/health
```

---

## 6. Escalation

### Escalate if:
- Locks persist > 15 minutes
- Database corruption suspected
- Multiple services affected
- Data loss risk

### Escalation Contacts
- **Platform Team:** @platform-team
- **Database Admin:** @dba-team
- **On-Call:** See `ops/runbooks/README.md`

---

## 7. Post-Resolution

1. **Document Incident:**
   - Root cause
   - Resolution steps taken
   - Prevention measures

2. **Update Monitoring:**
   - Add alerts for lock timeouts
   - Monitor connection pool usage

3. **Review Configuration:**
   - Verify WAL mode
   - Check busy_timeout settings
   - Review connection pool size

---

## Emergency Contacts

| Role | Contact |
|------|---------|
| Platform Team | @platform-team |
| Database Admin | @dba-team |
| On-Call Engineer | See on-call schedule |

---

## Related Runbooks

- **System Crash:** `ops/runbooks/system_crash.md`
- **Disk Full:** `ops/runbooks/disk_full.md`
- **Memory Pressure:** `ops/runbooks/memory_pressure.md`
