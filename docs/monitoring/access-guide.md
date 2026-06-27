# Chimera Monitoring Gateway Access Guide

## Overview

The Chimera trading system now provides **unified gateway access** to all monitoring tools through HAProxy. This guide explains how to access Prometheus, Grafana, and AlertManager through the secure gateway.

## Architecture

```
External User → HAProxy Gateway (HTTPS) → Monitoring Tools
                          ↓
                   SSL/TLS Termination
                   Authentication (Basic Auth)
                   Geographic Access Control
                   Rate Limiting & Security
                          ↓
              ┌───────────┼───────────┐
              ↓           ↓           ↓
         Prometheus   Grafana   AlertManager
```

## Access URLs

### Primary Gateway URLs (Recommended)

**Prometheus:**
- URL: `https://localhost/monitoring/prometheus/`
- Authentication: Admin or Operator credentials
- Features: Metrics querying, target inspection, configuration management

**Grafana:**
- URL: `https://localhost/monitoring/grafana/`
- Authentication: Any role (Admin, Operator, Viewer)
- Features: Dashboard visualization, alerting, data source management

**AlertManager:**
- URL: `https://localhost/monitoring/alerts/`
- Authentication: Admin only
- Features: Alert management, silence configuration, notification routing

### Legacy URLs (Deprecated - Still Functional)

**Prometheus:**
- Legacy URL: `https://localhost/metrics/`
- Status: ⚠️ Deprecated - Use `/monitoring/prometheus/` instead
- Removal: Planned for v8.0

**Grafana:**
- Legacy URL: `https://localhost/grafana/`
- Status: ⚠️ Deprecated - Use `/monitoring/grafana/` instead
- Removal: Planned for v8.0

**AlertManager:**
- Legacy URL: `https://localhost/alerts/`
- Status: ⚠️ Deprecated - Use `/monitoring/alerts/` instead
- Removal: Planned for v8.0

## Authentication

### Default Credentials

**⚠️ SECURITY WARNING:** Change these passwords immediately after first deployment!

| Role | Username | Default Password | Access Level |
|------|----------|------------------|--------------|
| Admin | `admin` | `changeme_asap` | Full access to all tools |
| Operator | `operator` | `changeme_asap` | Read metrics, view dashboards |
| Viewer | `viewer` | `changeme_asap` | Dashboard view only |

### Role-Based Access Control

**Admin Role:**
- ✅ Full access to Prometheus (query, manage)
- ✅ Full access to Grafana (create, edit dashboards)
- ✅ Full access to AlertManager (manage alerts)
- ✅ Rate limit: 100 req/min

**Operator Role:**
- ✅ Read-only access to Prometheus metrics
- ✅ View Grafana dashboards
- ✅ View AlertManager alerts
- ✅ Rate limit: 50 req/min

**Viewer Role:**
- ❌ No Prometheus access
- ✅ View Grafana dashboards only
- ❌ No AlertManager access
- ✅ Rate limit: 20 req/min

### Authentication Methods

**1. HTTP Basic Authentication (Current):**
```bash
# Using curl
curl -u admin:password https://localhost/monitoring/prometheus/api/v1/targets

# Using browser
# Browser will prompt for username/password
```

**2. Browser Integration:**
- Modern browsers will cache authentication credentials
- Use browser password manager for convenience
- Session persists until browser closed

### Password Rotation

**Rotation Schedule:** Every 90 days (quarterly)

**Rotation Procedure:**
```bash
# 1. Update environment variables
export MONITORING_ADMIN_PASSWORD="new_secure_password_here"
export MONITORING_OPERATOR_PASSWORD="new_secure_password_here"
export MONITORING_VIEWER_PASSWORD="new_secure_password_here"

# 2. Update docker-compose configuration
# Edit docker-compose-haproxy.yml environment section

# 3. Restart HAProxy service
docker-compose -f docker-compose-haproxy.yml restart haproxy

# 4. Notify all users of new credentials
# 5. Update documentation
```

## Geographic Access Control

### Allowed Countries

Monitoring tools enforce the same geographic restrictions as application endpoints:

**Whitelisted Countries:**
- US (United States)
- GB (United Kingdom)
- DE (Germany)
- FR (France)
- JP (Japan)
- SG (Singapore)
- CH (Switzerland)
- CA (Canada)
- AU (Australia)
- NL (Netherlands)
- SE (Sweden)
- NO (Norway)
- DK (Denmark)
- FI (Finland)
- IT (Italy)
- ES (Spain)
- PT (Portugal)
- IE (Ireland)
- AT (Austria)
- BE (Belgium)
- LU (Luxembourg)

**Blocked Countries:**
- CN (China) - OFAC compliance
- RU (Russia) - OFAC compliance
- KP (North Korea) - OFAC compliance
- IR (Iran) - OFAC compliance

### Geographic Testing

**Test from allowed country:**
```bash
# Test with US IP (should be allowed)
curl -H "X-Forwarded-For: 8.8.8.8" https://localhost/monitoring/prometheus/
```

**Test from blocked country:**
```bash
# Test with Chinese IP (should be blocked)
curl -H "X-Forwarded-For: 1.2.4.8" https://localhost/monitoring/prometheus/
```

## SSL/TLS Security

### Certificate Information

**Gateway Certificate:**
- Location: `/etc/haproxy/certs/chimera.pem`
- Type: SSL/TLS certificate
- Protocols: TLS 1.2, TLS 1.3
- Ciphers: Modern cipher suite (ECDHE, AES-GCM)

### HTTPS Enforcement

All monitoring traffic is encrypted:
- HTTP (port 80) automatically redirects to HTTPS (port 443)
- No plaintext access to monitoring tools
- Strict Transport Security headers enforced

### SSL Configuration
```yaml
ssl-default-bind-ciphers:
  ECDHE-ECDSA-AES128-GCM-SHA256
  ECDHE-RSA-AES128-GCM-SHA256
  ECDHE-ECDSA-AES256-GCM-SHA384
  ECDHE-RSA-AES256-GCM-SHA384

ssl-default-bind-options:
  no-sslv3
  no-tlsv10
  no-tlsv11
```

## Usage Examples

### Prometheus Queries

**Basic metric query:**
```bash
curl -u admin:password https://localhost/monitoring/prometheus/api/v1/query?query=up
```

**Range query:**
```bash
curl -u admin:password "https://localhost/monitoring/prometheus/api/v1/query_range?query=up&start=2026-06-20T00:00:00Z&end=2026-06-20T01:00:00Z&step=1m"
```

**Target inspection:**
```bash
curl -u admin:password https://localhost/monitoring/prometheus/api/v1/targets
```

### Grafana Dashboard Access

**Via Browser:**
1. Navigate to `https://localhost/monitoring/grafana/`
2. Authenticate with credentials
3. Access dashboards and create visualizations

**Via API:**
```bash
# List dashboards
curl -u admin:password https://localhost/monitoring/grafana/api/search

# Get dashboard by ID
curl -u admin:password https://localhost/monitoring/grafana/api/dashboards/uid/{dashboard_uid}
```

### AlertManager Management

**View alerts:**
```bash
curl -u admin:password https://localhost/monitoring/alerts/api/v1/alerts
```

**Manage silences:**
```bash
# Create silence
curl -u admin:password -X POST \
  https://localhost/monitoring/alerts/api/v1/silences \
  -d '{"matchers":[{"name":"alertname","value":"HighErrorRate","isRegex":false}],"startsAt":"2026-06-20T10:00:00Z","comment":"Planned maintenance"}'

# List silences
curl -u admin:password https://localhost/monitoring/alerts/api/v1/silences
```

## Troubleshooting

### Common Issues

**1. Authentication Failed (401):**
- **Problem:** Invalid credentials
- **Solution:** Verify username/password, check for typos
- **Debug:** `curl -v -u user:pass https://localhost/monitoring/prometheus/`

**2. Geographic Access Denied (403):**
- **Problem:** IP-based geographic restriction
- **Solution:** Check if your IP is from allowed country
- **Debug:** Check `X-Forwarded-For` header, verify GeoIP database

**3. SSL Certificate Errors:**
- **Problem:** Certificate validation failed
- **Solution:** Ensure valid certificate, check certificate chain
- **Debug:** `openssl s_client -connect localhost:443 -showcerts`

**4. Gateway Timeouts:**
- **Problem:** Request taking too long
- **Solution:** Optimize queries, increase timeout if needed
- **Debug:** Check HAProxy logs for timeout errors

**5. Legacy URLs Not Working:**
- **Problem:** Deprecated URLs returning errors
- **Solution:** Use new `/monitoring/` prefixed URLs
- **Debug:** Check for deprecation headers in response

### Debug Commands

**Check HAProxy status:**
```bash
# View HAProxy statistics
curl -u admin:password http://localhost:8404/stats

# View Prometheus metrics
curl http://localhost:8404/metrics
```

**Test gateway connectivity:**
```bash
# Run comprehensive gateway tests
python tools/monitoring-tester.py --url https://localhost

# Quick test only
python tools/monitoring-tester.py --url https://localhost --quick
```

**View access logs:**
```bash
# Check recent access attempts
curl http://localhost:8003/audit/log?limit=50

# View blocked access attempts
docker logs chimera-haproxy | grep "403"
```

### Performance Issues

**High Latency:**
- Check network connectivity between gateway and monitoring tools
- Verify monitoring tools aren't overloaded
- Monitor HAProxy connection metrics

**Rate Limiting:**
- Default rate limits: 100 req/min (admin), 50 req/min (operator), 20 req/min (viewer)
- Adjust limits in config.yaml if needed
- Use batching for API calls

## Security Best Practices

### Password Security
1. **Use strong passwords:** Minimum 32 characters with mixed case, numbers, symbols
2. **Rotate regularly:** Quarterly password rotation mandatory
3. **Never share credentials:** Individual accounts preferred
4. **Use password manager:** Store credentials securely

### Access Control
1. **Principle of least privilege:** Use viewer role when possible
2. **Monitor access logs:** Review audit logs weekly
3. **Geo-fencing enforcement:** Ensure geographic restrictions active
4. **Session management:** Close browser when done

### Network Security
1. **VPN for remote access:** Use VPN when accessing from outside
2. **Firewall rules:** Restrict access to gateway from trusted networks
3. **DNS security:** Use DNSSEC to prevent DNS spoofing
4. **Network monitoring:** Monitor for suspicious access patterns

### Operational Security
1. **Regular updates:** Keep HAProxy and monitoring tools updated
2. **Backup configurations:** Maintain backup of gateway configurations
3. **Incident response:** Have procedures ready for security incidents
4. **Compliance audits:** Quarterly security access reviews

## Migration Guide

### From Direct Port Access

**Step 1: Update bookmarks and scripts**
- Replace `http://localhost:9090` with `https://localhost/monitoring/prometheus/`
- Replace `http://localhost:3002` with `https://localhost/monitoring/grafana/`
- Replace `http://localhost:9093` with `https://localhost/monitoring/alerts/`

**Step 2: Update Prometheus configurations**
```yaml
# Update Prometheus scrape configs
scrape_configs:
  - job_name: 'chimera'
    static_configs:
      - targets: ['localhost:8080']
    scheme: https
    basic_auth:
      username: admin
      password: your_password
```

**Step 3: Update Grafana data sources**
- Navigate to Grafana → Configuration → Data Sources
- Update Prometheus URL to `https://localhost/monitoring/prometheus/`
- Configure basic authentication
- Test connection

**Step 4: Update AlertManager configurations**
- Update Prometheus alertmanager URL
- Configure authentication
- Test alert routing

### Testing Migration

**1. Test gateway access:**
```bash
python tools/monitoring-tester.py --url https://localhost
```

**2. Verify monitoring functionality:**
- Check Prometheus targets are being scraped
- Verify Grafana dashboards display data
- Confirm AlertManager receives alerts

**3. Validate SSL/TLS:**
```bash
# Check SSL certificate
openssl s_client -connect localhost:443 -servername localhost

# Verify HTTPS redirect
curl -I http://localhost/monitoring/prometheus/
```

**4. Test authentication:**
```bash
# Test with valid credentials
curl -u admin:password https://localhost/monitoring/prometheus/api/v1/targets

# Test without credentials (should fail)
curl https://localhost/monitoring/prometheus/api/v1/targets
```

## Advanced Configuration

### Custom Authentication

**Integrate with existing authentication:**
```yaml
# Extend to use admin wallet authentication
monitoring_auth:
  method: "admin_wallet"  # Use Solana wallet auth
  wallet_required: true
  session_duration: 3600
```

### Custom Rate Limits

**Per-endpoint rate limiting:**
```yaml
# In config.yaml
endpoint_policies:
  /monitoring/prometheus:
    rate_limit_override: 100  # Admin queries
  /monitoring/grafana:
    rate_limit_override: 50   # Dashboard loads
  /monitoring/alerts:
    rate_limit_override: 30   # Alert checks
```

### Custom Geographic Rules

**City-level restrictions:**
```bash
# Block specific cities
echo "Moscow" >> docker/haproxy/policies/blacklists/cities.lst
echo "Beijing" >> docker/haproxy/policies/blacklists/cities.lst

# Reload policies
curl -X POST http://localhost:8003/policies/reload
```

## Support

### Getting Help

**Documentation:**
- Main documentation: `docs/`
- Runbooks: `ops/runbooks/`
- Architecture: `docs/core/architecture.md`

**Troubleshooting:**
- Access control issues: `docs/guides/access-control-guide.md`
- HAProxy issues: `docker/haproxy/README.md`
- Monitoring tools: Vendor documentation

**Contact:**
- Infrastructure team: infrastructure@chimera
- Security issues: security@chimera
- Emergency contacts: `ops/emergency-contacts.md`

---

**Last Updated:** 2026-06-20
**Version:** 1.0.0
**Maintained By:** Chimera Infrastructure Team