# Security Monitoring Guide

**Purpose:** Operational procedures for monitoring security events, managing logs, and responding to security incidents in Chimera trading platform.

**Target Audience:** DevOps engineers, SREs, and security operators

**Last Updated:** 2026-07-04

---

## Overview

Chimera implements multiple security monitoring layers:

1. **Application-level monitoring** - Rust operator security events
2. **Proxy-level monitoring** - HAProxy/Nginx access control events
3. **Database audit logging** - Configuration and authentication changes
4. **Security event services** - Attack detection and threat classification

This guide covers how to monitor, analyze, and respond to security events across all layers.

---

## Critical Security Events to Monitor

### 1. Bearer Token Exposure (WARN Level)

**Location:** `operator/src/middleware/auth.rs:272-280`

**What to monitor:**
```rust
tracing::warn!(
    identifier = %user.identifier,
    role = %user.role,
    "User authenticated via QUERY PARAMETER (security risk - token may be in logs)"
);
```

**Why it's critical:**
- Bearer tokens in URL query parameters appear in:
  - Web server access logs (`/var/log/nginx/access.log`)
  - Proxy logs (HAProxy, load balancers)
  - Browser history and bookmarks
  - Referer headers to external sites

**How to monitor:**
```bash
# Check for WARN level auth events
tail -f /var/log/chimera/operator.log | grep -i "query parameter"

# Count occurrences per hour
awk '/WARN.*query parameter/ {print $1" "$2}' operator.log | sort | uniq -c

# Find affected users
grep "QUERY PARAMETER" operator.log | jq -r '.identifier' | sort | uniq
```

**Response actions:**
1. **Immediate:** Rotate affected bearer tokens
2. **Investigation:** Check which logs contain the tokens
3. **Remediation:** 
   - Secure/Delete affected log files
   - Revoke exposed tokens from database
   - Notify user of token rotation

**Prevention:**
- Use `Authorization: Bearer <token>` header for API calls
- For WebSocket: Use `Sec-WebSocket-Protocol` subprotocol instead of query params
- Enable log sanitization (see HAProxy/Nginx configs)

---

### 2. Oversized Header Rejections

**Location:** `operator/src/middleware/hmac.rs:145-151`

**What to monitor:**
```rust
if sig.len() > MAX_HEADER_SIZE {
    return error_response(StatusCode::BAD_REQUEST, "Signature header too large");
}
```

**Why it's critical:**
- Indicates potential DoS attack via memory exhaustion
- Attacker attempting to exhaust server memory with large headers

**How to monitor:**
```bash
# Check for header size rejections
tail -f /var/log/chimera/operator.log | grep "Signature header too large"

# Count rejections per source IP
grep "header too large" operator.log | jq -r '.source_ip' | sort | uniq -c | sort -rn

# Alert on bursts (10+ per minute)
awk '/header too large/ {print $1" "$2}' operator.log | uniq -c | awk '$1 > 10 {print}'
```

**Response actions:**
1. **Immediate:** Block source IP at firewall level
2. **Investigation:** Analyze attack pattern (single IP vs distributed)
3. **Mitigation:** 
   - Add IP to HAProxy blacklist
   - Consider implementing CAPTCHA for suspected bot traffic
   - Monitor for coordinated attack

**Prevention:**
- Header size limits already enforced (4KB per header)
- HAProxy rate limiting provides additional protection
- Consider implementing IP-based throttling for repeat offenders

---

### 3. Replay Attack Detection

**Location:** `operator/src/middleware/hmac.rs:220-238`

**What to monitor:**
```rust
if let Some(nonce) = self.nonce_store.get(&signature_bytes).await {
    return error_response(StatusCode::UNAUTHORIZED, "Replay attack detected");
}
```

**Why it's critical:**
- Indicates attacker attempting to replay captured webhook signatures
- Could lead to unauthorized trade execution if successful

**How to monitor:**
```bash
# Check for replay attack warnings
tail -f /var/log/chimera/operator.log | grep -i "replay attack"

# Analyze attack patterns
grep "replay attack" operator.log | jq -r '.source_ip' | sort | uniq -c | sort -rn

# Check for correlation with trade failures
grep -A 5 "replay attack" operator.log | grep "trade_uuid"
```

**Response actions:**
1. **Immediate:** 
   - Block source IP
   - Temporarily disable webhook endpoint for that IP
2. **Investigation:**
   - Capture the replayed signature for analysis
   - Determine if signatures are being leaked (logs, Referer headers)
   - Check if webhook secret was compromised
3. **Remediation:**
   - Rotate webhook secret immediately
   - Alert all webhook integrators of secret rotation
   - Monitor for additional replay attempts

**Prevention:**
- Nonce tracking with automatic eviction (already implemented)
- 60-second timestamp drift window prevents stale replay
- Consider implementing signature request rate limiting per client

---

### 4. IP Spoofing Attempts

**Location:** `operator/src/middleware/rate_limit.rs:56-82`

**What to monitor:**
```rust
// Look for patterns of X-Forwarded-For abuse
// Multiple IPs from single source, rapid rotation
```

**Why it's critical:**
- Indicates attacker attempting to bypass rate limiting via IP spoofing
- Could lead to excessive webhook submissions (DoS or unauthorized trades)

**How to monitor:**
```bash
# Check for suspicious X-Forwarded-For patterns
grep "X-Forwarded-For" /var/log/nginx/access.log | awk -F',' '{print NF}' | sort | uniq -c

# Identify IPs with many different forwarded IPs
awk -F'"' '{print $2}' /var/log/nginx/access.log | grep -o 'X-Forwarded-For: [^ ]*' | cut -d' ' -f2 | awk -F',' '{print $1}' | sort | uniq -c | sort -rn | head

# Check for rate limit violations correlated with IP spoofing
grep "429" /var/log/nginx/access.log | awk -F'"' '{print $6}' | sort | uniq -c | sort -rn
```

**Response actions:**
1. **Immediate:** Block source IP at firewall
2. **Investigation:** Analyze pattern (sophisticated attack vs misconfigured proxy)
3. **Mitigation:**
   - Add to HAProxy IP blacklist
   - Implement stricter X-Forwarded-For validation
   - Consider requiring mTLS for high-volume integrations

**Prevention:**
- IP extraction fixed to use rightmost IP (closest to trusted proxy)
- HAProxy access control lists provide additional filtering
- Consider implementing authenticated proxy-only mode for integrations

---

### 5. Circuit Breaker Triggers

**Location:** `operator/src/circuit_breaker.rs`

**What to monitor:**
```rust
tracing::error!(
    reason = %reason,
    consecutive_failures = consecutive_failures,
    "Circuit breaker triggered - trading halted"
);
```

**Why it's critical:**
- Trading halt indicates serious system issue or attack
- Could be legitimate (market conditions) or malicious (manipulation)

**How to monitor:**
```bash
# Check for circuit breaker events
tail -f /var/log/chimera/operator.log | grep -i "circuit breaker"

# Analyze trigger reasons
grep "circuit breaker" operator.log | jq -r '.reason' | sort | uniq -c

# Correlate with trade failures
grep -B 10 "circuit breaker" operator.log | grep "trade.*failed"
```

**Response actions:**
1. **Immediate:** 
   - Review circuit breaker reason
   - Check system health (RPC latency, queue depth)
   - Determine if manual intervention needed
2. **Investigation:**
   - If loss-based: Review recent trades for manipulation
   - If failure-based: Check for RPC issues or token problems
   - If manual: Review who triggered and why
3. **Resolution:**
   - Fix underlying issue before resetting
   - Document incident for postmortem
   - Update monitoring/alerting if new pattern detected

**Prevention:**
- Circuit breaker thresholds configured appropriately
- Health checks provide early warning of degradation
- Consider implementing predictive scaling based on queue depth

---

## Log Analysis Procedures

### Daily Security Log Review

**Frequency:** Daily (automated + manual review)

**Scope:**
```bash
# Daily security summary script
#!/bin/bash
DATE=$(date -d yesterday +%Y-%m-%d)
LOG_FILE="/var/log/chimera/operator.log"

echo "=== Security Summary for $DATE ==="

echo "1. Bearer Token Exposure"
grep "QUERY PARAMETER" $LOG_FILE | wc -l

echo "2. Oversized Header Rejections"
grep "header too large" $LOG_FILE | wc -l

echo "3. Replay Attack Attempts"
grep "replay attack" $LOG_FILE | wc -l

echo "4. Circuit Breaker Triggers"
grep "circuit breaker" $LOG_FILE | wc -l

echo "5. Rate Limit Violations"
grep "429" /var/log/nginx/access.log | wc -l

echo "6. IP Blacklist Hits"
grep "blacklisted_ip" /var/log/haproxy.log | wc -l
```

**Output:** Email to security team daily at 09:00 UTC

### Weekly Security Trend Analysis

**Frequency:** Weekly (Monday 09:00 UTC)

**Metrics to track:**
```sql
-- Weekly security metrics (SQLite)
CREATE TABLE security_metrics (
    date TEXT PRIMARY KEY,
    bearer_token_exposures INTEGER,
    oversized_header_rejections INTEGER,
    replay_attempts INTEGER,
    circuit_breaker_triggers INTEGER,
    rate_limit_violations INTEGER,
    ip_blacklist_hits INTEGER
);

-- Example query for trend analysis
SELECT 
    date,
    bearer_token_exposures,
    replay_attempts,
    rate_limit_violations
FROM security_metrics
WHERE date >= date('now', '-7 days')
ORDER BY date;
```

**Visualization:** Grafana dashboard at `grafana.chimera.internal/d/security-metrics`

### Monthly Security Posture Assessment

**Frequency:** Monthly (first Monday 09:00 UTC)

**Scope:**
1. Review all security incidents from previous month
2. Identify patterns (trending attacks, new vectors)
3. Update security controls if needed
4. Review and update access control policies
5. Audit log retention and access policies
6. Test security monitoring procedures (tabletop exercise)

---

## Log Access Policies

### Log Classification

**CRITICAL Logs (Restricted access):**
- `operator.log` - Application logs (may contain sensitive data)
- `audit.log` - Configuration audit trail
- `error.log` - Error messages (may contain stack traces)

**MODERATE Logs (Team access):**
- `/var/log/nginx/access.log` - Web access logs (sanitized)
- `/var/log/haproxy.log` - Proxy access logs (sanitized)
- `/var/log/chimera/webhook.log` - Webhook-specific logs

**PUBLIC Logs (Monitoring):**
- Prometheus metrics (`/metrics` endpoint)
- Health check logs
- Performance metrics

### Access Control

**Who can access what:**

| Role | Critical Logs | Moderate Logs | Public Logs |
|------|--------------|---------------|-------------|
| Admin | ✅ | ✅ | ✅ |
| Operator | ✅ | ✅ | ✅ |
| Security Analyst | ✅ | ✅ | ✅ |
| SRE | 🟡 (see below) | ✅ | ✅ |
| Developer | ❌ | 🟡 (see below) | ✅ |

**Legend:**
- ✅ Full access
- 🟡 Restricted access (time-limited, audit-logged)
- ❌ No access

**SRE Access to Critical Logs:**
- Time-limited grants (max 4 hours)
- Requires approval ticket
- Audit-logged (who accessed, when, what)
- Auto-revocation after time limit

**Developer Access to Moderate Logs:**
- Read-only access to sanitized logs
- No access to sensitive fields (already redacted)
- Audit-logged for compliance

### Log Retention Policies

**Production Logs:**
- **Critical logs:** 90 days (compliance requirement)
- **Moderate logs:** 30 days (operational need)
- **Public logs:** 7 days (monitoring need)

**Development/Stage Logs:**
- All logs: 7 days (debugging need)

**Archival:**
- After retention period: Compress and move to cold storage
- Critical logs: Encrypt before archival (AES-256)
- Access to archived logs requires separate approval

**Disposal:**
- Secure delete after archival period (1 year for critical, 6 months for moderate)
- Certificate of destruction for compliance

### Log Access Audit

**Every log access is recorded:**
```sql
-- Log access audit table
CREATE TABLE log_access_audit (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp TEXT NOT NULL,
    user TEXT NOT NULL,
    role TEXT NOT NULL,
    log_file TEXT NOT NULL,
    access_type TEXT NOT NULL, -- 'read', 'copy', 'export'
    justification TEXT,
    approved_by TEXT, -- NULL for self-approval
    ip_address TEXT NOT NULL,
    session_id TEXT NOT NULL
);
```

**Monthly audit review:**
- Review all log access for previous month
- Flag unusual patterns (off-hours access, large exports)
- Report to security committee

---

## Security Incident Response

### Incident Classification

**SEVERITY 1 (CRITICAL):**
- Active trading exploitation
- Unauthorized trades executed
- Circuit breaker triggered by attack
- Successful replay attack
- **Response time:** < 15 minutes

**SEVERITY 2 (HIGH):**
- Bearer token exposure in logs
- Rate limiting bypass detected
- Suspicious IP patterns
- **Response time:** < 1 hour

**SEVERITY 3 (MEDIUM):**
- Oversized header rejections (single occurrence)
- Failed replay attempts
- Log access policy violations
- **Response time:** < 4 hours

**SEVERITY 4 (LOW):**
- Configuration errors
- Minor security control gaps
- Documentation updates needed
- **Response time:** < 24 hours

### Incident Response Playbook

**For SEV1/SEV2 incidents:**

1. **DETECT (0-5 minutes)**
   - Alert received via monitoring/PagerDuty
   - Confirm incident scope
   - Initialize incident response Slack channel

2. **CONTAIN (5-15 minutes)**
   - Stop the bleeding:
     - Block attacking IPs
     - Disable affected endpoints
     - Trigger circuit breaker (if not already)
   - Preserve evidence:
     - Enable debug logging
     - Export relevant logs
     - Snapshot system state

3. **ERADICATE (15-60 minutes)**
   - Identify root cause
   - Implement permanent fix
   - Test fix in staging

4. **RECOVER (1-4 hours)**
   - Deploy fix to production
   - Monitor for recurrence
   - Restore normal operations

5. **POST-MORTEM (within 5 days)**
   - Document incident timeline
   - Identify improvements needed
   - Update monitoring/alerting
   - Share with team

**Escalation path:**
```
SRE on-call → Engineering Manager → CTO → CEO
(5 min)       (15 min)             (30 min)   (1 hour)
```

---

## Monitoring Tools & Dashboards

### Grafana Dashboards

**Security Overview:** `grafana.chimera.internal/d/security-overview`
- Bearer token exposure rate
- Header rejection rate
- Replay attempt rate
- Circuit breaker status
- Rate limit violations

**Access Control:** `grafana.chimera.internal/d/access-control`
- IP blacklist hits
- Country blacklist hits
- ASN-based violations
- Admin access attempts

**Log Analysis:** `grafana.chimera.internal/d/log-analysis`
- Log volume by severity
- Top error patterns
- Log access audit (who accessed what)

### Alerting Rules

**Prometheus alerts in `ops/prometheus/alerts.yml`:**

```yaml
groups:
  - name: security
    rules:
      # Bearer token exposure
      - alert: BearerTokenInQueryParams
        expr: rate(bearer_token_query_param_count[5m]) > 0
        for: 5m
        annotations:
          summary: "Bearer tokens being passed in query parameters (security risk)"
      
      # Replay attacks
      - alert: ReplayAttackDetected
        expr: rate(replay_attack_count[1m]) > 0
        for: 1m
        annotations:
          summary: "Replay attack detected - trading may be compromised"
      
      # Circuit breaker
      - alert: CircuitBreakerTriggered
        expr: circuit_breaker_state == 1
        for: 1m
        annotations:
          summary: "Circuit breaker triggered - trading halted"
      
      # Rate limit violations
      - alert: HighRateLimitViolations
        expr: rate(rate_limit_violations[5m]) > 10
        for: 5m
        annotations:
          summary: "High rate of rate limit violations - possible attack"
```

**PagerDuty integration:**
- SEV1 alerts: Immediate page
- SEV2 alerts: SMS + email
- SEV3/SEV4 alerts: Email only

---

## Compliance & Legal Considerations

### Data Protection

**GDPR considerations:**
- Logs may contain EU user data (IP addresses, user IDs)
- Implement data minimization (sanitize sensitive fields)
- Provide data access/export on user request
- Delete on user request (right to be forgotten)

**SOX compliance (if applicable):**
- Immutable audit trail for configuration changes
- Log access controls and audit logging
- Retention periods (90 days for financial data)
- Tamper-evident log storage

### Log Storage Security

**Encryption at rest:**
- Critical logs encrypted with AES-256
- Keys managed via AWS KMS or HashiCorp Vault
- Key rotation quarterly

**Encryption in transit:**
- Log shipping via TLS (rsync over SSH, or secure S3)
- No plaintext log transmission

**Access control:**
- Role-based access control (RBAC)
- Multi-factor authentication required for critical logs
- Just-in-time access grants (auto-expiration)

### Third-Party Log Access

**Audit firms/Regulators:**
- Require written request and legal basis
- Provide sanitized logs only (remove tokens, secrets)
- Time-limited access (revoked after review)
- Audit log of all data provided

**Security researchers:**
- Bug bounty program only
- Provide sanitized logs for vulnerability reproduction
- No production data access

---

## Continuous Improvement

### Monthly Security Reviews

**Agenda:**
1. Review security incidents from previous month
2. Assess monitoring effectiveness (false positives/negatives)
3. Update alerting thresholds based on traffic patterns
4. Review new security features/configurations
5. Schedule upcoming security improvements

**Output:** Monthly security report to CTO

### Quarterly Security Assessments

**Scope:**
1. Penetration testing (external firm)
2. Security control audit
3. Log retention compliance check
4. Access control review
5. Threat modeling update

**Output:** Quarterly security board presentation

### Annual Security Strategy

**Review:**
- Threat landscape changes
- New security tools/techniques
- Regulatory compliance updates
- Industry best practices
- Budget for security improvements

**Output:** Annual security strategy document

---

## Contact Information

**Security Team:**
- **Security Lead:** security@chimera.internal
- **On-Call SRE:** sre-oncall@chimera.internal (PagerDuty)
- **Incident Response:** security-incidents@chimera.internal

**Emergency Contacts:**
- **CTO:** cto@chimera.internal
- **Engineering Manager:** eng-manager@chimera.internal

**External:**
- **Security Researcher Disclosure:** security@chimera.internal (PGP key available)
- **Bug Bounty:** https://hackerone.com/chimera (if applicable)

---

## Appendix: Quick Reference

### Critical Log Commands

```bash
# Real-time security monitoring
tail -f /var/log/chimera/operator.log | grep -E "WARN|ERROR"

# Count security events by type
grep -E "replay attack|header too large|QUERY PARAMETER" /var/log/chimera/operator.log | wc -l

# Find IPs with high rate limit violations
grep "429" /var/log/nginx/access.log | awk -F'"' '{print $6}' | sort | uniq -c | sort -rn | head -20

# Check recent circuit breaker events
grep "circuit breaker" /var/log/chimera/operator.log | tail -20

# Monitor bearer token exposure
grep "QUERY PARAMETER" /var/log/chimera/operator.log | jq -r '.identifier' | sort | uniq -c

# Export logs for incident analysis (secure method)
sudo awk '/2026-07-04T10:00:00/,/2026-07-04T11:00:00/' /var/log/chimera/operator.log | gpg --encrypt --recipient security@chimera.internal > incident-logs.gpg
```

### Alert Thresholds

**Alert if:**
- Bearer token exposure rate > 0 per 5 minutes
- Replay attempts > 0 per minute
- Header rejections > 10 per minute
- Circuit breaker triggered
- Rate limit violations > 100 per minute
- IP blacklist hits > 50 per hour

**Warning if:**
- Bearer token exposure rate > 0.1 per hour
- Replay attempts > 0 per hour
- Header rejections > 1 per minute
- Rate limit violations > 10 per minute
- IP blacklist hits > 10 per hour

---

**Document Version:** 1.0  
**Last Updated:** 2026-07-04  
**Next Review:** 2026-08-04  
**Approved By:** CTO, Chimera Trading Platform  
