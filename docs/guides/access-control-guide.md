# Chimera Access Control Guide

## Overview

Chimera implements comprehensive **geographic and IP-based access control** through HAProxy reverse proxy, providing regulatory compliance, risk management, and operational security capabilities. The system supports multiple enforcement modes and granular policy configuration per endpoint.

## Architecture

```
External Traffic → HAProxy Access Control Layer → Policy Evaluation → Backend Services
                    ↓
              GeoIP Lookup (City-level)
                    ↓
              Policy Enforcement (Allow/Deny)
                    ↓
              Audit Logging & Metrics
```

## Access Control Modes

### 1. Whitelist Mode (Most Restrictive)
- **Behavior:** Only explicitly allowed entities can access
- **Use Case:** High-security endpoints, admin interfaces
- **Configuration:** Set `mode: "whitelist"` in policy config
- **Example:**
  ```bash
  # Only allow specific IPs
  acl is_whitelisted_ip src -f /etc/haproxy/policies/whitelists/ips.lst
  http-request deny if !is_whitelisted_ip
  ```

### 2. Blacklist Mode (Least Restrictive)
- **Behavior:** Block only explicitly denied entities
- **Use Case:** Public endpoints with known bad actors
- **Configuration:** Set `mode: "blacklist"` in policy config
- **Example:**
  ```bash
  # Block blacklisted IPs and countries
  http-request deny if is_blacklisted_ip
  http-request deny if is_blocked_country
  ```

### 3. Mixed Mode (Balanced)
- **Behavior:** Combine whitelist and blacklist rules
- **Use Case:** API endpoints with geographic restrictions
- **Configuration:** Set `mode: "mixed"` in policy config (default)
- **Example:**
  ```bash
  # Block blacklisted entities, require geographic compliance
  http-request deny if is_blacklisted_ip
  http-request deny if is_blocked_country
  ```

### 4. Off Mode (No Restrictions)
- **Behavior:** No access control enforcement
- **Use Case:** Development, testing, emergency situations
- **Configuration:** Set `mode: "off"` in policy config

## Geographic Access Control

### Country-Based Filtering

**Default Whitelisted Countries:**
```yaml
allowed_countries:
  - US    # United States
  - GB    # United Kingdom
  - DE    # Germany
  - FR    # France
  - JP    # Japan
  - SG    # Singapore
  - CH    # Switzerland
  - CA    # Canada
  - AU    # Australia
  - NL    # Netherlands
  - SE    # Sweden
  - NO    # Norway
  - DK    # Denmark
  - FI    # Finland
  - IT    # Italy
  - ES    # Spain
  - PT    # Portugal
  - IE    # Ireland
  - AT    # Austria
  - BE    # Belgium
  - LU    # Luxembourg
```

**Default Blacklisted Countries (OFAC Compliance):**
```yaml
blocked_countries:
  - CN    # China
  - RU    # Russia
  - KP    # North Korea
  - IR    # Iran
```

### City-Level Filtering

**Blocked Cities (High-Risk Areas):**
```yaml
blocked_cities:
  - Moscow        # Russia
  - Beijing       # China
  - Shanghai      # China
  - Tianjin       # China
  - Shenzhen      # China
```

### Usage Examples

**Block specific country:**
```bash
curl -X PUT "http://localhost:8003/policies/blacklist/countries" \
  -H "Content-Type: application/json" \
  -d '["CN", "RU"]'
```

**Add country to whitelist:**
```bash
curl -X PUT "http://localhost:8003/policies/whitelist/countries" \
  -H "Content-Type: application/json" \
  -d '["US", "GB", "DE"]'
```

## IP Range Access Control

### IP Whitelisting

**Format:** One IP address or CIDR range per line
```
# docker/haproxy/policies/whitelists/ips.lst
192.168.1.0/24
10.0.0.0/8
172.16.0.0/12
```

### IP Blacklisting

**Format:** One IP address or CIDR range per line
```
# docker/haproxy/policies/blacklists/ips.lst
192.0.2.0/24     # TEST-NET-1 (documentation)
198.51.100.0/24  # TEST-NET-2 (documentation)
```

### Usage Examples

**Add IP to whitelist:**
```bash
curl -X PUT "http://localhost:8003/policies/whitelist/ips" \
  -H "Content-Type: application/json" \
  -d '["192.168.1.100", "10.0.0.50"]'
```

**Block IP range:**
```bash
curl -X PUT "http://localhost:8003/policies/blacklist/ips" \
  -H "Content-Type: application/json" \
  -d '["203.0.113.0/24"]'
```

## ASN-Based Filtering

### ASN Whitelisting

**Format:** AS{number} or just the number
```
# docker/haproxy/policies/whitelists/asns.lst
AS13335  # Cloudflare
AS15169  # Google Cloud
AS16509  # Amazon AWS
```

### Usage Examples

**Add ASN to whitelist:**
```bash
curl -X PUT "http://localhost:8003/policies/whitelist/asns" \
  -H "Content-Type: application/json" \
  -d '["AS13335", "AS15169"]'
```

## Per-Endpoint Policies

Different endpoints can have different access control requirements:

### Admin Dashboard (Strict IP Whitelist)
```yaml
/api/v1/admin:
  mode: "strict_whitelist"
  allowed_ips: ["192.168.1.0/24", "10.0.0.0/8"]
  require_authentication: true
```

### Webhook Endpoints (Geographic Restrictions)
```yaml
/api/v1/webhook:
  mode: "geo_restricted"
  allowed_countries: ["US", "GB", "DE", "FR", "JP", "SG", "CH"]
  rate_limit_override: 50  # More restrictive
```

### Trading API (Strict Country Restrictions)
```yaml
/api/v1/trading:
  mode: "strict"
  allowed_countries: ["US", "GB"]
  require_authentication: true
```

### Public Dashboard (Permissive)
```yaml
/:
  mode: "permissive"
  rate_limit_only: true
```

## Policy Management API

### Endpoints

**List all policies:**
```bash
curl http://localhost:8003/policies
```

**Get policy configuration:**
```bash
curl http://localhost:8003/policies/config
```

**Validate policy:**
```bash
curl -X POST "http://localhost:8003/policies/validate" \
  -H "Content-Type: application/json" \
  -d '{
    "mode": "whitelist",
    "allowed_countries": ["US", "GB"],
    "blocked_countries": ["CN", "RU"]
  }'
```

**Update endpoint policy:**
```bash
curl -X PUT "http://localhost:8003/policies/endpoint/api/v1/webhook" \
  -H "Content-Type: application/json" \
  -d '{
    "endpoint": "/api/v1/webhook",
    "policy": {
      "mode": "geo_restricted",
      "allowed_countries": ["US", "GB", "DE"]
    }
  }'
```

**Reload policies (triggers HAProxy reload):**
```bash
curl -X POST http://localhost:8003/policies/reload
```

## GeoIP Lookup Service

### Endpoints

**Get GeoIP information:**
```bash
curl http://localhost:8001/geoip/8.8.8.8
```

**Response:**
```json
{
  "status": "success",
  "data": {
    "ip_address": "8.8.8.8",
    "city": "Mountain View",
    "subdivision": "California",
    "country_code": "US",
    "country_name": "United States",
    "continent_code": "NA",
    "latitude": 37.4056,
    "longitude": -122.0775,
    "timezone": "America/Los_Angeles",
    "asn": "AS15169",
    "asn_organization": "Google LLC",
    "cache_status": "miss",
    "lookup_time": 0.023
  }
}
```

**Evaluate access policy:**
```bash
curl "http://localhost:8001/geoip/evaluate/1.2.4.8?policy_type=strict"
```

**Response:**
```json
{
  "status": "success",
  "decision": {
    "ip_address": "1.2.4.8",
    "decision": "deny",
    "reason": "Country CN not in strict whitelist",
    "policy_type": "strict",
    "details": {
      "country_code": "CN",
      "city": "Beijing"
    }
  }
}
```

## Testing Access Control

### 1. Test Geographic Blocking

**Test Chinese IP (should be blocked):**
```bash
curl -H "X-Forwarded-For: 1.2.4.8" http://localhost/api/v1/health
# Expected: 403 Forbidden
```

**Test US IP (should be allowed):**
```bash
curl -H "X-Forwarded-For: 8.8.8.8" http://localhost/api/v1/health
# Expected: 200 OK
```

### 2. Test City-Level Restrictions

**Test Moscow IP (should be blocked):**
```bash
curl -H "X-Forwarded-For: 185.12.12.12" http://localhost/api/v1/health
# Expected: 403 Forbidden
```

### 3. Test IP Whitelisting

**Test whitelisted IP:**
```bash
curl -H "X-Forwarded-For: 192.168.1.100" http://localhost/api/v1/health
# Expected: 200 OK (if in whitelist)
```

**Test non-whitelisted IP:**
```bash
curl -H "X-Forwarded-For: 8.8.8.8" http://localhost/admin
# Expected: 403 Forbidden (admin requires IP whitelist)
```

### 4. Test Policy Evaluation

**Evaluate policy for specific IP:**
```bash
curl "http://localhost:8001/geoip/evaluate/8.8.8.8?policy_type=default"
```

## Monitoring & Metrics

### Access Control Metrics (Prometheus)

**Available metrics:**
```bash
# Policy changes
curl http://localhost:8003/metrics | grep chimera_policy_changes_total

# Policy validations
curl http://localhost:8003/metrics | grep chimera_policy_validations_total

# HAProxy reloads
curl http://localhost:8003/metrics | grep chimera_haproxy_reloads_total

# Active policies
curl http://localhost:8003/metrics | grep chimera_active_policies
```

### Access Control Dashboard

**Grafana Dashboard:** Import `ops/grafana/dashboards/access-control.json`

**Key Panels:**
- Denied requests by country and reason
- Active policies and their effectiveness
- Geographic access patterns
- Policy change timeline
- Access control performance metrics

## Audit Logging

### View Audit Log

```bash
curl http://localhost:8003/audit/log?limit=50
```

**Sample Audit Entry:**
```json
{
  "timestamp": "2026-06-20T10:30:00",
  "action": "update",
  "policy_type": "blacklist_countries",
  "changes": {
    "blocked_countries": ["CN", "RU", "KP", "IR"]
  },
  "performed_by": "admin@chimera",
  "validation_result": {
    "valid": true,
    "errors": [],
    "warnings": []
  }
}
```

## Maintenance

### GeoIP Database Updates

**Manual update:**
```bash
# Download and install latest GeoIP databases
docker exec chimera-geoip-updater python /app/geoip-updater.py --force
```

**Automatic updates:** Weekly (Sundays at 3 AM)

### Policy Review Schedule

**Monthly:**
- Review access denied patterns
- Update whitelist/blacklist based on threat intelligence
- Validate compliance requirements

**Quarterly:**
- Comprehensive policy review
- Update geographic restrictions based on business requirements
- Audit trail review

## Troubleshooting

### Issue: Legitimate Traffic Blocked

**Symptoms:** Users from allowed countries being blocked

**Diagnosis:**
```bash
# Check current policies
curl http://localhost:8003/policies/config

# Test specific IP evaluation
curl "http://localhost:8001/geoip/evaluate/{user_ip}?policy_type=default"

# Check audit log for recent denials
curl http://localhost:8003/audit/log?limit=20
```

**Solutions:**
1. Verify country is in whitelist: `curl http://localhost:8003/policies/whitelist/countries`
2. Check for policy conflicts in config.yaml
3. Review per-endpoint policies
4. Add IP to emergency whitelist if needed

### Issue: GeoIP Lookups Failing

**Symptoms:** All traffic being blocked or allowed indiscriminately

**Diagnosis:**
```bash
# Check GeoIP service health
curl http://localhost:8001/health

# Test GeoIP lookup
curl http://localhost:8001/geoip/8.8.8.8

# Verify database files exist
ls -la docker/haproxy/geoip/
```

**Solutions:**
1. Verify GeoIP databases exist and are valid
2. Check MaxMind license key is valid
3. Restart GeoIP lookup service
4. Check Redis cache connectivity

### Issue: HAProxy Configuration Errors

**Symptoms:** HAProxy fails to start or reload

**Diagnosis:**
```bash
# Check HAProxy configuration syntax
docker-compose -f docker-compose-haproxy.yml config haproxy

# Check HAProxy logs
docker-compose -f docker-compose-haproxy.yml logs haproxy
```

**Solutions:**
1. Fix configuration syntax errors
2. Verify all referenced files exist
3. Check file permissions
4. Validate policy file formats

## Emergency Procedures

### Emergency Access Override

**Scenario:** Legitimate users locked out due to access control misconfiguration

**Solution 1: Emergency Whitelist**
```bash
# Add emergency IPs to whitelist
curl -X PUT "http://localhost:8003/policies/whitelist/ips" \
  -H "Content-Type: application/json" \
  -d '["EMERGENCY_IP_1", "EMERGENCY_IP_2"]'

# Reload policies
curl -X POST http://localhost:8003/policies/reload
```

**Solution 2: Disable Access Control**
```yaml
# Edit docker/haproxy/policies/config.yaml
access_control:
  mode: "off"  # Temporary disable
```

**Solution 3: Direct HAProxy Bypass**
```bash
# Comment out access control rules in haproxy.cfg
# http-request deny if is_blacklisted_ip
# http-request deny if is_blocked_country
```

### Policy Rollback

**If new policies cause issues:**
```bash
# View policy change history
curl http://localhost:8003/audit/log?limit=100

# Restore previous configuration
git checkout docker/haproxy/policies/config.yaml

# Reload policies
curl -X POST http://localhost:8003/policies/reload
```

## Security Considerations

### Compliance

**OFAC Compliance:**
- Blocks access from sanctioned countries (CN, RU, KP, IR)
- Audit trail for compliance reporting
- Regular policy reviews required

**GDPR Considerations:**
- GeoIP data processed for access control only
- 30-day log retention policy
- Data minimization principles

### Performance

**Expected Overhead:**
- **IP filtering:** <1ms per request
- **Country filtering:** <5ms per request (cached)
- **City filtering:** <10ms per request (cached)
- **Overall:** <2% performance impact

**Cache Effectiveness:**
- GeoIP lookup cache: 95%+ hit rate
- Policy decision cache: 90%+ hit rate
- Redis cache reduces database queries significantly

### Rate Limiting Integration

Access control works with existing rate limiting:
- Trusted regions: 1.5x rate limit multiplier
- Restricted regions: 0.5x rate limit multiplier
- Per-endpoint rate limit overrides supported

## Best Practices

1. **Start with blacklist mode** for testing
2. **Monitor access denied patterns** for first 48 hours
3. **Use specific policies** rather than broad restrictions
4. **Review audit logs weekly** for suspicious patterns
5. **Test policy changes** in staging before production
6. **Document emergency access procedures**
7. **Keep GeoIP databases updated** (weekly automatic)
8. **Use per-endpoint policies** for different security levels
9. **Review compliance requirements quarterly**
10. **Maintain emergency access procedures**

## Quick Reference

**Common Commands:**
```bash
# Check policy status
curl http://localhost:8003/policies

# Test IP access
curl -H "X-Forwarded-For: 8.8.8.8" http://localhost/api/v1/health

# View blocked countries
curl http://localhost:8003/policies/blacklist/countries

# Add to whitelist
curl -X PUT "http://localhost:8003/policies/whitelist/ips" \
  -H "Content-Type: application/json" \
  -d '["192.168.1.100"]'

# Reload policies
curl -X POST http://localhost:8003/policies/reload

# View audit log
curl http://localhost:8003/audit/log?limit=20
```

**File Locations:**
- Policies: `docker/haproxy/policies/`
- HAProxy Config: `docker/haproxy/haproxy.cfg`
- GeoIP Databases: `docker/haproxy/geoip/`
- Audit Log: `docker/haproxy/policies/audit.log`

**Service Ports:**
- HAProxy: 80, 443, 8404
- Policy Manager: 8003
- GeoIP Lookup: 8001
- Security Log Parser: 8000
- Attack Detection: 8002

---

**For questions or issues:** Contact infrastructure team or consult `ops/runbooks/access-control.md`