# Runbook: Memory Pressure

## Overview

**Trigger:** System memory usage > 85% or OOM (Out of Memory) errors

**Severity:** HIGH to CRITICAL

**SLA:** Resolve within 10 minutes

**On-Call:** @platform-team

---

## 1. Identify the Issue (2 minutes)

### Symptoms
- Memory usage > 85%
- OOM killer activated
- Application crashes or restarts
- Slow response times
- Swap usage high

### Check Memory Status
```bash
# Check current memory usage
free -h

# Check process memory usage
ps aux --sort=-%mem | head -10

# Check for OOM kills
dmesg | grep -i "out of memory"
journalctl -k | grep -i oom
```

---

## 2. Immediate Actions (5 minutes)

### Step 1: Check Chimera Memory Usage
```bash
# Check Chimera process memory
ps aux | grep chimera | awk '{print $6/1024 " MB"}'

# Check if memory is growing
watch -n 1 'ps aux | grep chimera | grep -v grep'
```

### Step 2: Enable Aggressive Load Shedding
If memory > 90%:
1. **Disable Spear Strategy:**
   ```bash
   curl -X PUT http://localhost:8080/api/v1/config \
     -H "Authorization: Bearer $ADMIN_TOKEN" \
     -H "Content-Type: application/json" \
     -d '{"strategy_allocation": {"shield_percent": 100, "spear_percent": 0}}'
   ```

2. **Reduce Queue Capacity:**
   - Edit `config.yaml`: `queue.capacity: 500` (from 1000)
   - Restart service

### Step 3: Clear Caches
```bash
# Clear token metadata cache (if API available)
curl -X POST http://localhost:8080/api/v1/cache/clear

# Or restart service to clear memory
systemctl restart chimera
```

---

## 3. Resolution Steps (10 minutes)

### Scenario A: Memory Leak

**Problem:** Memory usage continuously growing

**Solution:**
1. **Identify Leaking Component:**
   ```bash
   # Monitor memory over time
   while true; do
     ps aux | grep chimera | awk '{print $6/1024 " MB"}'
     sleep 5
   done
   ```

2. **Check for Unbounded Growth:**
   - Queue depth (should be bounded)
   - Cache sizes (should have TTL/limits)
   - WebSocket connections (should be limited)

3. **Temporary Fix:**
   - Restart service to clear memory
   - Reduce cache sizes in config
   - Enable more aggressive cleanup

4. **Long-term Fix:**
   - Fix memory leak in code
   - Add memory limits to caches
   - Implement connection limits

### Scenario B: High Queue Depth

**Problem:** Queue filled with signals consuming memory

**Solution:**
```bash
# Check queue depth
curl http://localhost:8080/api/v1/health | jq '.queue_depth'

# If queue > 800, load shedding should be active
# Manually clear queue if needed (requires code change or restart)
```

### Scenario C: Too Many Active Positions

**Problem:** Large number of positions tracked in memory

**Solution:**
1. **Check Position Count:**
   ```bash
   sqlite3 /opt/chimera/data/chimera.db "SELECT COUNT(*) FROM positions WHERE state = 'ACTIVE';"
   ```

2. **Reduce Position Tracking:**
   - Close old positions
   - Reduce max positions per strategy
   - Archive historical positions

### Scenario D: System-Wide Memory Pressure

**Problem:** Other processes consuming memory

**Solution:**
```bash
# Identify memory consumers
ps aux --sort=-%mem | head -20

# Kill non-essential processes
# Free up swap
sudo swapoff -a && sudo swapon -a

# Increase VPS RAM (if possible)
```

---

## 4. Prevention Measures

### Configuration
- **Memory Limits:** Set process memory limits
- **Cache Sizes:** Limit cache capacities
- **Queue Capacity:** Reasonable limits (1000)
- **Connection Limits:** Limit concurrent connections

### Monitoring
- Alert on memory > 85%
- Alert on memory growth rate
- Monitor OOM kills
- Track cache sizes

### Best Practices
- Regular service restarts (if needed)
- Memory profiling in development
- Cache TTL enforcement
- Connection pool limits

---

## 5. Verification

After resolution, verify:

```bash
# 1. Memory usage normalized
free -h
# Should be < 80%

# 2. Service is running
systemctl status chimera

# 3. No memory leaks
# Monitor for 30 minutes
watch -n 60 'ps aux | grep chimera | awk "{print \$6/1024 \" MB\"}"'

# 4. Health check passes
curl http://localhost:8080/api/v1/health
```

---

## 6. Escalation

### Escalate if:
- Memory pressure persists > 15 minutes
- OOM kills continue
- Service cannot stay running
- Data loss risk

### Escalation Contacts
- **Platform Team:** @platform-team
- **Infrastructure Team:** @infra-team
- **On-Call:** See on-call schedule

---

## 7. Post-Resolution

1. **Document Incident:**
   - Root cause
   - Memory usage patterns
   - Resolution steps

2. **Update Configuration:**
   - Adjust memory limits
   - Reduce cache sizes if needed
   - Review connection pools

3. **Long-term Actions:**
   - Memory profiling
   - Code review for leaks
   - Consider VPS upgrade

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
- **Disk Full:** `ops/runbooks/disk_full.md`
- **SQLite Lock:** `ops/runbooks/sqlite_lock.md`
