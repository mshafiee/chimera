# Runbook: Disk Full

## Overview

**Trigger:** Disk space < 10% free or write operations fail

**Severity:** HIGH to CRITICAL

**SLA:** Resolve within 15 minutes

**On-Call:** @platform-team

---

## 1. Identify the Issue (2 minutes)

### Symptoms
- Database writes fail
- Log writes fail
- Error: "No space left on device"
- Application crashes

### Check Disk Status
```bash
# Check disk usage
df -h /

# Check specific directories
du -sh /opt/chimera/data/*
du -sh /var/log/chimera/*

# Check for large files
find /opt/chimera -type f -size +100M -exec ls -lh {} \;
find /var/log/chimera -type f -size +100M -exec ls -lh {} \;
```

---

## 2. Immediate Actions (5 minutes)

### Step 1: Free Up Space Quickly
```bash
# Remove old log files (> 7 days)
find /var/log/chimera -type f -mtime +7 -delete

# Remove old backups (> 30 days)
find /opt/chimera/backups -type f -mtime +30 -delete

# Clear temporary files
rm -rf /tmp/chimera-*
rm -rf /var/tmp/chimera-*
```

### Step 2: Check Largest Consumers
```bash
# Find largest directories
du -h /opt/chimera | sort -rh | head -10
du -h /var/log | sort -rh | head -10
```

### Step 3: Stop Non-Essential Services
If disk is critically full (< 5%):
```bash
# Stop logging temporarily (if safe)
# Edit logrotate to be more aggressive
# Or disable verbose logging
```

---

## 3. Resolution Steps (10 minutes)

### Scenario A: Log Files

**Problem:** Log files consuming too much space

**Solution:**
```bash
# 1. Rotate logs immediately
logrotate -f /etc/logrotate.d/chimera

# 2. Compress old logs
find /var/log/chimera -name "*.log" -mtime +3 -exec gzip {} \;

# 3. Delete very old logs (> 14 days)
find /var/log/chimera -name "*.log.gz" -mtime +14 -delete

# 4. Update logrotate config for more aggressive rotation
# Edit /etc/logrotate.d/chimera:
# - Rotate daily instead of weekly
# - Keep only 7 days of logs
# - Compress immediately
```

### Scenario B: Database Growth

**Problem:** Database file growing too large

**Solution:**
```bash
# 1. Check database size
ls -lh /opt/chimera/data/chimera.db

# 2. Archive old trades (> 90 days)
sqlite3 /opt/chimera/data/chimera.db "
CREATE TABLE IF NOT EXISTS trades_archive AS 
SELECT * FROM trades 
WHERE created_at < datetime('now', '-90 days');
"

# 3. Delete archived trades from main table
sqlite3 /opt/chimera/data/chimera.db "
DELETE FROM trades 
WHERE created_at < datetime('now', '-90 days');
"

# 4. Run VACUUM to reclaim space
sqlite3 /opt/chimera/data/chimera.db "VACUUM;"
```

### Scenario C: Backup Files

**Problem:** Too many backup files

**Solution:**
```bash
# 1. Keep only last 7 days of daily backups
find /opt/chimera/backups -name "chimera-*.db" -mtime +7 -delete

# 2. Keep only last 4 weeks of weekly backups
find /opt/chimera/backups -name "chimera-weekly-*.db" -mtime +30 -delete

# 3. Compress old backups
find /opt/chimera/backups -name "*.db" -mtime +3 -exec gzip {} \;
```

### Scenario D: WAL File Growth

**Problem:** SQLite WAL file growing large

**Solution:**
```bash
# 1. Check WAL file size
ls -lh /opt/chimera/data/chimera.db-wal

# 2. Force checkpoint to merge WAL
sqlite3 /opt/chimera/data/chimera.db "PRAGMA wal_checkpoint(TRUNCATE);"

# 3. If WAL is very large, may need to restart service
systemctl restart chimera
```

---

## 4. Prevention Measures

### Log Rotation
- **Daily rotation** for active logs
- **Keep 7 days** of uncompressed logs
- **Keep 30 days** of compressed logs
- **Auto-delete** logs > 30 days

**Configuration:** `/etc/logrotate.d/chimera`

### Database Maintenance
- **Archive old trades** (> 90 days)
- **Regular VACUUM** (weekly)
- **WAL checkpoint** (daily)
- **Backup cleanup** (keep 7 days daily, 4 weeks weekly)

### Backup Strategy
- **Daily backups:** Keep 7 days
- **Weekly backups:** Keep 4 weeks
- **Monthly backups:** Keep 12 months
- **Compress** backups > 3 days old

### Monitoring
- Alert on disk < 15%
- Alert on disk < 10% (critical)
- Monitor log file sizes
- Monitor database size growth

---

## 5. Verification

After cleanup, verify:

```bash
# 1. Disk space freed
df -h /
# Should be > 15% free

# 2. Service can write
sqlite3 /opt/chimera/data/chimera.db "INSERT INTO config_audit (key, new_value, changed_by) VALUES ('disk_cleanup_test', 'test', 'SYSTEM');"

# 3. Logs can be written
echo "test" >> /var/log/chimera/test.log && rm /var/log/chimera/test.log

# 4. Service is healthy
curl http://localhost:8080/api/v1/health
```

---

## 6. Escalation

### Escalate if:
- Disk space cannot be freed
- Critical data at risk
- Service cannot write to database
- System stability compromised

### Escalation Contacts
- **Platform Team:** @platform-team
- **Infrastructure Team:** @infra-team
- **On-Call:** See on-call schedule

---

## 7. Post-Resolution

1. **Document Incident:**
   - Root cause (logs, database, backups)
   - Space freed
   - Prevention measures taken

2. **Update Configuration:**
   - More aggressive log rotation
   - Database archiving schedule
   - Backup retention policy

3. **Long-term Actions:**
   - Consider larger disk
   - Implement log aggregation (external)
   - Database archiving automation
   - Backup off-site storage

---

## Emergency Contacts

| Role | Contact |
|------|---------|
| Platform Team | @platform-team |
| Infrastructure Team | @infra-team |
| On-Call Engineer | See on-call schedule |

---

## Related Runbooks

- **System Crash:** `ops/runbooks/system_crash.md`
- **Memory Pressure:** `ops/runbooks/memory_pressure.md`
- **SQLite Lock:** `ops/runbooks/sqlite_lock.md`

---

## Quick Reference

### Free Space Immediately
```bash
# Logs
find /var/log/chimera -mtime +7 -delete

# Backups
find /opt/chimera/backups -mtime +7 -delete

# Database cleanup
sqlite3 /opt/chimera/data/chimera.db "DELETE FROM trades WHERE created_at < datetime('now', '-90 days'); VACUUM;"
```

### Check Space Usage
```bash
df -h
du -sh /opt/chimera/* | sort -rh
du -sh /var/log/chimera/* | sort -rh
```
