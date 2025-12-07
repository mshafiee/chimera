# Incident Runbooks Index

This directory contains runbooks for all failure modes and incident scenarios in the Chimera system.

## Runbook Index

### Critical Incidents

1. **Wallet Drained** (`wallet_drained.md`)
   - **Trigger:** Wallet balance drops significantly
   - **Severity:** CRITICAL
   - **SLA:** Immediate response required
   - **Actions:** Kill switch, verify, contain, audit, rotate keys

2. **System Crash** (`system_crash.md`)
   - **Trigger:** Service down or unresponsive
   - **Severity:** CRITICAL
   - **SLA:** Restore within 15 minutes
   - **Actions:** Check logs, restart, verify state, recover stuck positions

3. **Circuit Breaker Tripped** (See `docs/pdd.md` Section 4.4)
   - **Trigger:** Loss thresholds exceeded
   - **Severity:** CRITICAL
   - **SLA:** Review within 1 hour
   - **Actions:** Verify breach, review trades, reset if false positive

### High Priority Incidents

4. **RPC Fallback** (`rpc_fallback.md`)
   - **Trigger:** Primary RPC fails, switched to fallback
   - **Severity:** HIGH
   - **SLA:** Investigate within 30 minutes
   - **Actions:** Verify fallback, check primary RPC, restore when possible

5. **Reconciliation Discrepancies** (`reconciliation_discrepancies.md`)
   - **Trigger:** DB vs on-chain state mismatch
   - **Severity:** MEDIUM to HIGH
   - **SLA:** Investigate within 1 hour, resolve within 24 hours
   - **Actions:** Verify discrepancy, investigate, resolve, update DB

### Medium Priority Incidents

6. **SQLite Lock Issues** (`sqlite_lock.md`)
   - **Trigger:** Database lock errors
   - **Severity:** MEDIUM to HIGH
   - **SLA:** Resolve within 15 minutes
   - **Actions:** Check VACUUM, verify WAL mode, clear locks, restart if needed

7. **Memory Pressure** (`memory_pressure.md`)
   - **Trigger:** Memory usage > 85%
   - **Severity:** HIGH to CRITICAL
   - **SLA:** Resolve within 10 minutes
   - **Actions:** Enable load shedding, clear caches, identify leak, restart

8. **Disk Full** (`disk_full.md`)
   - **Trigger:** Disk space < 10%
   - **Severity:** HIGH to CRITICAL
   - **SLA:** Resolve within 15 minutes
   - **Actions:** Clean logs, archive data, remove old backups, free space

### Operational Issues

9. **Queue Backpressure** (See `docs/pdd.md` Section 4.2)
   - **Trigger:** Queue depth > 800
   - **Severity:** WARNING
   - **SLA:** Investigate within 1 hour
   - **Actions:** Check consumer lag, verify load shedding, investigate source

10. **High Trade Latency** (See `docs/pdd.md` Section 6.2)
    - **Trigger:** p99 latency > 2 seconds
    - **Severity:** CRITICAL
    - **SLA:** Resolve within 30 minutes
    - **Actions:** Check RPC health, verify network, check queue depth

11. **Webhook Rejections** (See `docs/pdd.md` Section 6.2)
    - **Trigger:** High rate of HMAC failures
    - **Severity:** CRITICAL
    - **SLA:** Immediate investigation
    - **Actions:** Check HMAC config, verify clock sync, check for attacks

## Runbook Structure

Each runbook follows this structure:

1. **Overview** - Trigger, severity, SLA, on-call
2. **Identify the Issue** - Symptoms and verification
3. **Immediate Actions** - Quick fixes to stabilize
4. **Resolution Steps** - Detailed resolution by scenario
5. **Prevention Measures** - Long-term prevention
6. **Verification** - Post-resolution checks
7. **Escalation** - When and how to escalate
8. **Post-Resolution** - Documentation and follow-up
9. **Emergency Contacts** - Contact information

## Using Runbooks

### During an Incident

1. **Identify the incident type** from symptoms
2. **Open the relevant runbook**
3. **Follow steps in order**
4. **Document actions taken**
5. **Escalate if needed**

### After an Incident

1. **Update runbook** with lessons learned
2. **Document root cause**
3. **Update monitoring** if gaps found
4. **Review prevention measures**

## Runbook Maintenance

- **Review quarterly** for accuracy
- **Update after incidents** with new learnings
- **Test runbooks** during drills
- **Keep contacts current**

## Related Documentation

- **PDD Section 4.6:** Graceful Degradation Matrix
- **PDD Section 6.2:** Critical Alerts
- **PDD Section 7.2-7.4:** Incident Runbooks
- **Architecture Docs:** `docs/architecture.md`

## Emergency Contacts

| Role | Contact | Escalation |
|------|---------|------------|
| Platform Team | @platform-team | Primary |
| Infrastructure Team | @infra-team | Secondary |
| On-Call Engineer | See schedule | 24/7 |
| Security Team | @security-team | Security incidents |

## Quick Reference

### Most Common Incidents

1. **System Crash** → `system_crash.md`
2. **RPC Fallback** → `rpc_fallback.md`
3. **Reconciliation Issues** → `reconciliation_discrepancies.md`
4. **Memory/Disk Issues** → `memory_pressure.md`, `disk_full.md`

### Incident Severity Levels

- **CRITICAL:** Immediate response, system down or data at risk
- **HIGH:** Response within 30 minutes, significant impact
- **MEDIUM:** Response within 1 hour, moderate impact
- **WARNING:** Response within 4 hours, minor impact

### Response Times

- **CRITICAL:** < 15 minutes
- **HIGH:** < 30 minutes
- **MEDIUM:** < 1 hour
- **WARNING:** < 4 hours
