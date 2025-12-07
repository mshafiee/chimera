# Security Audit Checklist

## Overview

This document provides a comprehensive security audit checklist for the Chimera trading system. Use this checklist before production deployment and during regular security reviews.

## 1. Secret Management

### 1.1 Secret Storage
- [ ] All secrets stored in encrypted vault (AES-256)
- [ ] No secrets in plaintext in code or config files
- [ ] Secrets not committed to version control
- [ ] `.env` files in `.gitignore`
- [ ] Encrypted config files use strong encryption keys

### 1.2 Secret Access
- [ ] Secrets loaded from secure vault at runtime
- [ ] No secrets logged in application logs
- [ ] Secrets not exposed in error messages
- [ ] Secrets not accessible via API endpoints
- [ ] Secrets rotated on schedule (see rotation schedule)

### 1.3 Secret Rotation
- [ ] Webhook HMAC key rotated every 30 days
- [ ] RPC API keys rotated every 90 days
- [ ] Database encryption key rotated annually
- [ ] Rotation process tested and documented
- [ ] Grace period implemented (24h for HMAC, immediate for others)

**Verification:**
```bash
# Check secret rotation status
sqlite3 chimera.db "SELECT * FROM config_audit WHERE key LIKE '%secret%' ORDER BY changed_at DESC LIMIT 10;"
```

## 2. Authentication & Authorization

### 2.1 HMAC Signature Verification
- [ ] HMAC-SHA256 signature verification implemented
- [ ] Signature includes timestamp + payload
- [ ] Constant-time comparison to prevent timing attacks
- [ ] Signature validation before processing requests
- [ ] Invalid signatures rejected with generic error (no info leak)

**Test:**
```bash
# Test invalid signature
curl -X POST http://localhost:8080/api/v1/webhook \
  -H "X-Signature: invalid-signature" \
  -H "X-Timestamp: $(date +%s)" \
  -d '{"strategy":"SHIELD","token":"BONK","action":"BUY","amount_sol":0.5}'
# Should return 401 Unauthorized
```

### 2.2 Replay Attack Prevention
- [ ] Timestamp validation implemented (±60 seconds)
- [ ] Old timestamps rejected
- [ ] Future timestamps rejected
- [ ] Clock synchronization verified (NTP enabled)
- [ ] Idempotency check (trade_uuid deduplication)

**Test:**
```bash
# Test old timestamp
curl -X POST http://localhost:8080/api/v1/webhook \
  -H "X-Signature: $(generate_signature)" \
  -H "X-Timestamp: $(($(date +%s) - 120))" \
  -d '{"strategy":"SHIELD","token":"BONK","action":"BUY","amount_sol":0.5}'
# Should return 401 Unauthorized (expired)
```

### 2.3 API Key Authentication
- [ ] Bearer token authentication implemented
- [ ] API keys stored securely (encrypted)
- [ ] Role-based access control (readonly, operator, admin)
- [ ] Admin-only endpoints protected
- [ ] Invalid tokens rejected

**Test:**
```bash
# Test without token
curl http://localhost:8080/api/v1/positions
# Should return 401 Unauthorized

# Test with invalid token
curl -H "Authorization: Bearer invalid-token" http://localhost:8080/api/v1/positions
# Should return 401 Unauthorized
```

### 2.4 Admin Wallet Authorization
- [ ] Admin wallets stored in `admin_wallets` table
- [ ] Wallet-based authentication for web UI
- [ ] Role lookup from database
- [ ] Unauthorized access denied

## 3. Input Validation

### 3.1 Webhook Payload Validation
- [ ] Strategy validation (SHIELD, SPEAR, EXIT only)
- [ ] Action validation (BUY, SELL only)
- [ ] Amount validation (min/max bounds)
- [ ] Token address format validation
- [ ] Wallet address format validation
- [ ] Trade UUID format validation (if provided)

### 3.2 SQL Injection Prevention
- [ ] All database queries use parameterized statements
- [ ] No string concatenation in SQL queries
- [ ] User input sanitized before database operations
- [ ] SQLite prepared statements used throughout

**Verification:**
```bash
# Search for potential SQL injection risks
grep -r "format!.*SELECT\|format!.*INSERT\|format!.*UPDATE" operator/src/
# Should return no results
```

### 3.3 XSS Prevention (Web UI)
- [ ] User input sanitized before display
- [ ] React's default escaping used
- [ ] No `dangerouslySetInnerHTML` without sanitization
- [ ] Content Security Policy (CSP) headers set

## 4. Rate Limiting

### 4.1 Webhook Rate Limiting
- [ ] Rate limiting implemented (100 req/sec)
- [ ] Burst size configured (150)
- [ ] Rate limit headers returned
- [ ] Rate limit exceeded returns 429

**Test:**
```bash
# Send 200 requests rapidly
for i in {1..200}; do
  curl -X POST http://localhost:8080/api/v1/webhook \
    -H "X-Signature: $(generate_signature)" \
    -H "X-Timestamp: $(date +%s)" \
    -d '{"strategy":"SHIELD","token":"BONK","action":"BUY","amount_sol":0.5}' &
done
# Some should return 429 Too Many Requests
```

### 4.2 API Rate Limiting
- [ ] API endpoints rate limited
- [ ] Different limits for different roles
- [ ] Rate limit headers in responses

## 5. Token Safety Checks

### 5.1 Freeze Authority Check
- [ ] Freeze authority checked before BUY
- [ ] Whitelist for known safe tokens (USDC, USDT)
- [ ] Tokens with freeze authority rejected

### 5.2 Mint Authority Check
- [ ] Mint authority checked before BUY
- [ ] Whitelist for known safe tokens
- [ ] Tokens with mint authority rejected

### 5.3 Liquidity Validation
- [ ] Minimum liquidity threshold enforced
- [ ] Shield: $10,000 minimum
- [ ] Spear: $5,000 minimum
- [ ] Liquidity checked before execution

### 5.4 Honeypot Detection
- [ ] Transaction simulation before execution
- [ ] Sell simulation verifies token can be sold
- [ ] Honeypot tokens rejected
- [ ] Cache for verified tokens (1 hour TTL)

## 6. Database Security

### 6.1 Database Access Control
- [ ] Database file permissions restricted (600)
- [ ] Database not accessible via web server
- [ ] Connection pool limits configured
- [ ] SQLite WAL mode enabled

### 6.2 Database Encryption
- [ ] Sensitive data encrypted at rest (if applicable)
- [ ] Database backups encrypted
- [ ] Encryption keys stored securely

### 6.3 SQL Injection Prevention
- [ ] Parameterized queries used
- [ ] No dynamic SQL construction
- [ ] Input validation before database operations

## 7. Network Security

### 7.1 HTTPS/TLS
- [ ] HTTPS enabled in production
- [ ] TLS 1.2+ required
- [ ] Certificate validation enabled
- [ ] HSTS headers set

### 7.2 RPC Security
- [ ] RPC API keys stored securely
- [ ] RPC endpoints use HTTPS
- [ ] RPC rate limiting configured
- [ ] Fallback RPC credentials rotated

## 8. Error Handling

### 8.1 Error Messages
- [ ] No sensitive information in error messages
- [ ] Generic error messages for authentication failures
- [ ] Stack traces not exposed in production
- [ ] Error logging doesn't include secrets

### 8.2 Logging Security
- [ ] No secrets in logs
- [ ] No API keys in logs
- [ ] No private keys in logs
- [ ] Structured logging with sanitization

**Verification:**
```bash
# Check logs for secrets
grep -r "secret\|key\|password\|token" /var/log/chimera/ | grep -v "redacted\|masked"
# Should return no results
```

## 9. Dependency Security

### 9.1 Dependency Audits
- [ ] `cargo audit` run regularly
- [ ] `npm audit` run for web dependencies
- [ ] Vulnerable dependencies updated
- [ ] Security advisories monitored

**Commands:**
```bash
# Rust dependencies
cd operator && cargo audit

# Web dependencies
cd web && npm audit
```

### 9.2 Dependency Updates
- [ ] Dependencies updated regularly
- [ ] Security patches applied promptly
- [ ] Breaking changes tested

## 10. Access Control

### 10.1 Role-Based Access
- [ ] Readonly role: view only
- [ ] Operator role: wallet management
- [ ] Admin role: full access
- [ ] Roles enforced at API level

### 10.2 Admin Functions
- [ ] Circuit breaker reset: admin only
- [ ] Configuration updates: admin only
- [ ] Secret rotation: admin only
- [ ] Emergency kill switch: admin only

## 11. Penetration Testing Procedures

### 11.1 Webhook Endpoint Testing
1. **Signature Bypass Attempts**
   - Test with missing signature
   - Test with invalid signature format
   - Test with signature for different payload
   - Test with signature for different timestamp

2. **Replay Attack Testing**
   - Capture valid webhook
   - Replay with same timestamp
   - Replay with old timestamp
   - Replay with future timestamp

3. **Rate Limit Testing**
   - Send requests at exactly 100 req/sec
   - Send burst of 200 requests
   - Verify rate limiting works
   - Verify priority queuing (EXIT > SHIELD > SPEAR)

### 11.2 API Endpoint Testing
1. **Authentication Bypass**
   - Test endpoints without token
   - Test with invalid token format
   - Test with expired token
   - Test token reuse after revocation

2. **Authorization Testing**
   - Test admin endpoints with readonly token
   - Test operator endpoints with readonly token
   - Test role escalation attempts

3. **Input Validation Testing**
   - Test with malformed JSON
   - Test with SQL injection attempts
   - Test with XSS payloads
   - Test with extremely large values
   - Test with negative values
   - Test with special characters

### 11.3 Database Security Testing
1. **SQL Injection Testing**
   - Test all user inputs for SQL injection
   - Verify parameterized queries used
   - Test with special SQL characters

2. **Database Access Testing**
   - Verify database file permissions
   - Test unauthorized database access
   - Verify connection pool limits

### 11.4 Token Safety Testing
1. **Honeypot Detection**
   - Test with known honeypot token
   - Verify rejection
   - Test cache behavior

2. **Authority Checks**
   - Test with freeze authority token
   - Test with mint authority token
   - Verify whitelist works

## 12. Security Monitoring

### 12.1 Attack Detection
- [ ] Failed authentication attempts logged
- [ ] Rate limit violations logged
- [ ] Suspicious patterns detected
- [ ] Alerts configured for security events

### 12.2 Audit Logging
- [ ] All configuration changes logged
- [ ] All admin actions logged
- [ ] All authentication failures logged
- [ ] Logs retained for compliance period

## 13. Incident Response

### 13.1 Security Incident Procedures
- [ ] Incident response plan documented
- [ ] Security team contacts defined
- [ ] Escalation procedures clear
- [ ] Post-incident review process

### 13.2 Compromise Response
- [ ] Secret rotation procedure documented
- [ ] System isolation procedure
- [ ] Forensic data collection process
- [ ] Communication plan for users

## 14. Compliance

### 14.1 Data Protection
- [ ] Personal data handling compliant
- [ ] Data retention policies defined
- [ ] Data deletion procedures documented

### 14.2 Audit Trail
- [ ] All trades logged with timestamps
- [ ] Configuration changes audited
- [ ] Admin actions tracked
- [ ] Audit logs tamper-proof

## 15. Pre-Deployment Security Checklist

Before production deployment, verify:

- [ ] All secrets rotated from defaults
- [ ] All default passwords changed
- [ ] HTTPS/TLS configured
- [ ] Rate limiting enabled
- [ ] Authentication required for all endpoints
- [ ] Error messages don't leak information
- [ ] Logs don't contain secrets
- [ ] Database permissions restricted
- [ ] Firewall rules configured
- [ ] Security updates applied
- [ ] Dependencies audited
- [ ] Penetration testing completed
- [ ] Security review signed off

## Security Testing Tools

### Recommended Tools
1. **OWASP ZAP** - Web application security scanner
2. **Burp Suite** - Web vulnerability scanner
3. **SQLMap** - SQL injection testing (for validation)
4. **Nmap** - Network security scanner
5. **cargo-audit** - Rust dependency audit
6. **npm audit** - Node.js dependency audit

### Testing Schedule
- **Weekly**: Dependency audits
- **Monthly**: Security configuration review
- **Quarterly**: Full penetration test
- **Annually**: Comprehensive security audit

## Security Contacts

- **Security Team**: security@chimera.example
- **On-Call**: See `ops/runbooks/`
- **Emergency**: See incident response plan

## References

- OWASP Top 10: https://owasp.org/www-project-top-ten/
- Solana Security Best Practices
- Rust Security Guidelines
- SQLite Security Documentation
