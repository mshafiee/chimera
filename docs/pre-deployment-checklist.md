# Pre-Deployment Checklist

## Overview

This checklist must be completed before deploying Chimera to production with real funds. All items are mandatory unless explicitly marked as optional.

## Deployment Gate

**All three critical verifications must pass before production deployment.**

---

## 1. Time Synchronization Verification

### Requirement
System time must be synced via NTP (Network Time Protocol) with clock drift < 1 second.

### Why Critical
HMAC replay protection rejects requests if timestamp drift > 60 seconds. A 5-minute clock drift = all webhooks rejected.

### Verification Steps

1. **Check NTP Status:**
   ```bash
   timedatectl status
   # Should show: "NTP service: active"
   # Should show: "System clock synchronized: yes"
   ```

2. **Verify NTP Sync:**
   ```bash
   ntpq -p
   # Should show at least one peer with "*" (synchronized)
   ```

3. **Check Clock Drift:**
   ```bash
   timedatectl timesync-status
   # Check "Offset" - should be < 1 second
   ```

4. **Enable NTP (if not enabled):**
   ```bash
   sudo timedatectl set-ntp true
   ```

### ✅ Pass Criteria
- [ ] NTP service is active
- [ ] System clock is synchronized
- [ ] Clock drift < 1 second
- [ ] NTP peers are reachable

### ❌ Fail Action
**DO NOT DEPLOY** if time sync is not verified. Fix NTP configuration and re-verify.

---

## 2. RPC Latency Verification

### Requirement
Average latency to Helius Jito endpoint must be < 50ms.

### Why Critical
Latency > 50ms defeats the "<5ms internal latency" optimization and increases risk of blockhash expiration and failed trades. Spear strategy requires low latency.

### Verification Steps

1. **Measure Latency:**
   ```bash
   # Replace with actual Helius Jito endpoint
   HELIUS_ENDPOINT="https://mainnet.helius-rpc.com/?api-key=YOUR_KEY"
   
   # Perform 10 RPC calls and measure latency
   for i in {1..10}; do
     time curl -s -X POST "$HELIUS_ENDPOINT" \
       -H "Content-Type: application/json" \
       -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' > /dev/null
   done
   ```

2. **Use Preflight Script:**
   ```bash
   ./ops/preflight-check.sh
   # This will test latency automatically
   ```

3. **Check Latency Stats:**
   - Average latency should be < 50ms
   - Maximum latency should be < 100ms
   - Standard deviation should be < 20ms

### ✅ Pass Criteria
- [ ] Average latency < 50ms
- [ ] Maximum latency < 100ms
- [ ] Low latency variance (stable connection)

### ❌ Fail Action
**DO NOT DEPLOY SPEAR STRATEGY** if latency > 50ms. Options:
1. Relocate VPS to US-East (Ashburn, VA) or Amsterdam
2. Use alternative provider (Latitude.sh, Cherry Servers)
3. Deploy with Spear strategy disabled
4. Verify latency to alternative RPC endpoints

### Alternative Providers
If Hetzner Ashburn is unavailable:
- **Latitude.sh** (formerly Maxihost): Bare metal in Ashburn/NY
- **Cherry Servers**: Bare metal in US-East

---

## 3. Circuit Breaker Test

### Requirement
Circuit breaker must automatically halt trading when loss thresholds are exceeded.

### Why Critical
Ensures automatic trading halts work correctly before real money is at risk.

### Verification Steps

1. **Get Current Threshold:**
   ```bash
   sqlite3 /opt/chimera/data/chimera.db "
   SELECT value FROM config WHERE key = 'circuit_breakers.max_loss_24h_usd' LIMIT 1;
   "
   # Default: 500 USD
   ```

2. **Insert Test Loss:**
   ```bash
   # Insert fake loss exceeding threshold
   TEST_UUID="preflight-circuit-breaker-test-$(date +%s)"
   MAX_LOSS=500  # Or get from config
   TEST_LOSS=$((MAX_LOSS + 100))
   
   sqlite3 /opt/chimera/data/chimera.db "
   INSERT INTO trades (
       trade_uuid, wallet_address, token_address, strategy, side,
       amount_sol, status, pnl_usd, created_at
   ) VALUES (
       '${TEST_UUID}',
       'PreflightTestWallet',
       'PreflightTestToken',
       'SHIELD',
       'SELL',
       1.0,
       'CLOSED',
       -${TEST_LOSS},
       datetime('now')
   );
   "
   ```

3. **Wait for Evaluation:**
   ```bash
   # Circuit breaker evaluates every 30 seconds
   sleep 35
   ```

4. **Verify Circuit Breaker Tripped:**
   ```bash
   # Check health endpoint
   curl http://localhost:8080/api/v1/health | jq '.trading_allowed'
   # Should return: false
   
   # Or check database
   sqlite3 /opt/chimera/data/chimera.db "
   SELECT * FROM config_audit 
   WHERE key = 'circuit_breaker' 
   AND changed_by = 'SYSTEM_CIRCUIT_BREAKER'
   ORDER BY changed_at DESC 
   LIMIT 1;
   "
   ```

5. **Test Webhook Rejection:**
   ```bash
   # Send test webhook
   curl -X POST http://localhost:8080/api/v1/webhook \
     -H "X-Signature: $(generate_signature)" \
     -H "X-Timestamp: $(date +%s)" \
     -H "Content-Type: application/json" \
     -d '{"strategy":"SHIELD","token":"BONK","action":"BUY","amount_sol":0.5}'
   
   # Should return: {"status":"rejected","reason":"circuit_breaker_triggered"}
   ```

6. **Cleanup:**
   ```bash
   # Delete test trade
   sqlite3 /opt/chimera/data/chimera.db "
   DELETE FROM trades WHERE trade_uuid = '${TEST_UUID}';
   "
   
   # Reset circuit breaker
   curl -X POST http://localhost:8080/api/v1/config/circuit-breaker/reset \
     -H "Authorization: Bearer $ADMIN_TOKEN"
   ```

### ✅ Pass Criteria
- [ ] Circuit breaker trips when threshold exceeded
- [ ] Trading is halted (trading_allowed = false)
- [ ] New webhooks are rejected with "circuit_breaker_triggered"
- [ ] Circuit breaker can be reset via API

### ❌ Fail Action
**DO NOT DEPLOY** if circuit breaker does not function. Fix circuit breaker logic and re-test.

---

## 4. Additional Pre-Deployment Checks

### 4.1 Configuration Verification

- [ ] All secrets set (not defaults)
- [ ] Webhook secret configured
- [ ] RPC endpoints configured
- [ ] Circuit breaker thresholds set appropriately
- [ ] Strategy allocation configured (Shield/Spear)
- [ ] Notification rules configured (if using)

### 4.2 Database Verification

- [ ] Database schema initialized
- [ ] WAL mode enabled
- [ ] Indexes created
- [ ] Admin wallets configured (if using wallet auth)
- [ ] Database permissions correct (600)

### 4.3 Service Verification

- [ ] Systemd service installed
- [ ] Service starts without errors
- [ ] Health endpoint responds
- [ ] Metrics endpoint accessible
- [ ] Logs are being written

### 4.4 Security Verification

- [ ] HMAC secret is strong (32+ bytes)
- [ ] API keys are set (not defaults)
- [ ] Database file permissions restricted
- [ ] Firewall rules configured
- [ ] HTTPS/TLS enabled (if applicable)

### 4.5 Monitoring Verification

- [ ] Prometheus can scrape metrics
- [ ] Alertmanager configured
- [ ] Grafana dashboards set up
- [ ] Notifications working (Telegram/Discord)

### 4.6 Backup Verification

- [ ] Backup script works
- [ ] Backup cron job installed
- [ ] Backup location accessible
- [ ] Backup restoration tested

---

## 5. Deployment Gate Process

### Gate Criteria

**ALL of the following must be true:**

1. ✅ Time sync verified (< 1 second drift)
2. ✅ RPC latency verified (< 50ms average)
3. ✅ Circuit breaker tested and working
4. ✅ All configuration verified
5. ✅ All tests passing
6. ✅ Security audit completed
7. ✅ Documentation reviewed

### Approval Process

1. **Engineering Lead** reviews checklist completion
2. **Security Team** reviews security audit
3. **Operations Team** verifies infrastructure
4. **Final Approval** from project owner

### Deployment Gate Enforcement

- **Automated:** Preflight script blocks deployment if checks fail
- **Manual:** Deployment requires sign-off from all stakeholders
- **Documentation:** All verification results logged

### Pre-Deployment Script

Run the automated preflight check:

```bash
./ops/preflight-check.sh
```

**Expected Output:**
```
==========================================
Chimera Pre-Deployment Verification
==========================================

=== Check 1: Time Synchronization ===
✓ NTP is enabled
✓ System clock is synchronized

=== Check 2: RPC Latency ===
✓ Average latency (45ms) is below threshold (50ms)

=== Check 3: Circuit Breaker Functionality ===
✓ Circuit breaker correctly tripped (trading_allowed: false)

==========================================
Verification Summary
==========================================
Passed:  9
Failed:  0
Warnings: 0

✓ All pre-flight checks passed
```

---

## 6. Post-Deployment Verification

After deployment, verify:

- [ ] Service is running
- [ ] Health endpoint returns "healthy"
- [ ] Metrics are being collected
- [ ] Logs are being written
- [ ] Webhook endpoint accepts test signal
- [ ] Database operations working
- [ ] Notifications working

### Smoke Tests

```bash
# 1. Health check
curl http://localhost:8080/api/v1/health

# 2. Metrics
curl http://localhost:8080/metrics | head -20

# 3. Test webhook (with valid signature)
curl -X POST http://localhost:8080/api/v1/webhook \
  -H "X-Signature: $(generate_signature)" \
  -H "X-Timestamp: $(date +%s)" \
  -d '{"strategy":"SHIELD","token":"BONK","action":"BUY","amount_sol":0.1}'

# 4. Check logs
tail -f /var/log/chimera/operator.log
```

---

## 7. Rollback Plan

If deployment fails:

1. **Stop Service:**
   ```bash
   systemctl stop chimera
   ```

2. **Restore Database:**
   ```bash
   ./ops/rollback.sh
   ```

3. **Restore Configuration:**
   ```bash
   cp /opt/chimera/config/.env.backup /opt/chimera/config/.env
   ```

4. **Verify Rollback:**
   - Check database state
   - Verify configuration
   - Test service startup

---

## 8. Documentation

### Required Documentation

- [ ] Deployment runbook reviewed
- [ ] Incident runbooks accessible
- [ ] API documentation updated
- [ ] Configuration guide reviewed
- [ ] Security audit completed

### Sign-Off

**Deployment Approval:**

- [ ] Engineering Lead: _________________ Date: _______
- [ ] Security Team: _________________ Date: _______
- [ ] Operations Team: _________________ Date: _______
- [ ] Project Owner: _________________ Date: _______

---

## 9. Emergency Contacts

| Role | Contact | Availability |
|------|---------|--------------|
| Engineering Lead | @eng-lead | Business hours |
| On-Call Engineer | See schedule | 24/7 |
| Security Team | @security-team | Business hours |
| Infrastructure Team | @infra-team | 24/7 |

---

## References

- **PDD Section 7.4:** Pre-Deployment Verification Steps
- **Preflight Script:** `ops/preflight-check.sh`
- **Deployment Script:** `ops/deploy.sh`
- **Rollback Script:** `ops/rollback.sh`

---

## Checklist Summary

### Critical (Must Pass)
- [ ] Time sync verified
- [ ] RPC latency verified (< 50ms)
- [ ] Circuit breaker tested

### Important (Should Pass)
- [ ] Configuration verified
- [ ] Database initialized
- [ ] Service starts correctly
- [ ] Security audit completed
- [ ] Monitoring configured

### Optional (Nice to Have)
- [ ] Load tests completed
- [ ] Chaos tests completed
- [ ] Performance benchmarks met

---

**Deployment Gate:** All critical items must pass before production deployment with real funds.
