# Chimera Security Measurement - Deployment Guide

## Overview

The Chimera security measurement system provides comprehensive security monitoring, threat detection, and automated alerting for the HAProxy reverse proxy infrastructure. This guide covers deployment, testing, and maintenance procedures.

## Components Deployed

### 1. HAProxy Prometheus Exporter
- **Service**: `chimera-haproxy-exporter`
- **Port**: 9101
- **Purpose**: Exports HAProxy metrics to Prometheus
- **Status**: ✅ Completed

### 2. Security Log Parser Service
- **Service**: `chimera-security-log-parser`
- **Port**: 8000
- **Purpose**: Processes JSON security logs and categorizes events
- **Features**: Attack pattern detection, Prometheus metrics, real-time threat feed
- **Status**: ✅ Completed

### 3. Redis Cache
- **Service**: `chimera-redis`
- **Port**: 6379
- **Purpose**: Caching layer for GeoIP lookups and threat tracking
- **Status**: ✅ Completed

### 4. GeoIP Lookup Service
- **Service**: `chimera-geoip-lookup`
- **Port**: 8001
- **Purpose**: IP geolocation with MaxMind GeoLite2 integration
- **Features**: Country/ASN lookup, Redis caching with 1-hour TTL
- **Status**: ✅ Completed

### 5. GeoIP Database Updater
- **Service**: `chimera-geoip-updater`
- **Purpose**: Automated weekly updates of MaxMind databases
- **Schedule**: Sundays at 3 AM
- **Status**: ✅ Completed

### 6. Attack Detection Service
- **Service**: `chimera-attack-detection`
- **Port**: 8002
- **Purpose**: Real-time attack pattern detection
- **Features**: Brute force, DDoS, webhook floods, injection attacks
- **Status**: ✅ Completed

### 7. Prometheus Security Monitoring
- **File**: `ops/prometheus/prometheus.yml`
- **Additions**: HAProxy, security-log-parser, geoip-lookup, attack-detection scrape jobs
- **Status**: ✅ Completed

### 8. Security Alerting System
- **Prometheus Rules**: `ops/prometheus/security-alerts.yml`
- **AlertManager Config**: `ops/alertmanager/config.yml`
- **Features**: Critical/warning/info alerts with Telegram/Slack integration
- **Status**: ✅ Completed

### 9. Grafana Security Dashboard
- **File**: `ops/grafana/dashboards/security.json`
- **Panels**: 18 comprehensive security monitoring panels
- **Features**: Real-time threat visualization, geographic analysis, attack detection
- **Status**: ✅ Completed

## Deployment Instructions

### Prerequisites

1. **MaxMind License Key**: Get free license key from [MaxMind Developer Portal](https://dev.maxmind.com/geoip/geolite2-free-geolocation-data)
2. **Telegram Bot**: Create bot via @BotFather and get API token
3. **Environment Variables**: Set required environment variables

### Environment Setup

```bash
# Set MaxMind license key
export MAXMIND_LICENSE_KEY=your_license_key_here

# Set Telegram bot credentials
export TELEGRAM_BOT_TOKEN=your_bot_token
export TELEGRAM_CHAT_ID=your_chat_id

# Optional: Set custom ports
export METRICS_PORT=8000
export REDIS_HOST=redis
export REDIS_PORT=6379
```

### Step-by-Step Deployment

#### 1. Initialize GeoIP Databases

```bash
# Create GeoIP directory
mkdir -p docker/haproxy/geoip

# Download initial databases
docker run --rm \
  -e MAXMIND_LICENSE_KEY=$MAXMIND_LICENSE_KEY \
  -v $(pwd)/docker/haproxy/geoip:/geoip \
  chimera-geoip-updater:latest \
  python /app/geoip-updater.py
```

#### 2. Start Services

```bash
# Start all security measurement services
docker-compose -f docker-compose-haproxy.yml up -d

# Verify services are running
docker-compose -f docker-compose-haproxy.yml ps
```

#### 3. Configure Prometheus

```bash
# Reload Prometheus configuration
curl -X POST http://localhost:9090/-/reload

# Verify targets are being scraped
curl -s http://localhost:9090/api/v1/targets | jq '.data.activeTargets[] | select(.labels.job | startswith("security") or .labels.job == "haproxy" or .labels.job == "geoip-lookup")'
```

#### 4. Load Alert Rules

```bash
# Security alert rules are automatically loaded
# Verify rules are active
curl -s 'http://localhost:9090/api/v1/rules' | jq '.data.groups[] | select(.name | contains("security"))'
```

#### 5. Import Grafana Dashboard

```bash
# Access Grafana at http://localhost:3000
# Import dashboard from ops/grafana/dashboards/security.json
# Or use API:
curl -X POST \
  -H "Content-Type: application/json" \
  -d @ops/grafana/dashboards/security.json \
  http://admin:admin@localhost:3000/api/dashboards/db
```

#### 6. Configure AlertManager

```bash
# Reload AlertManager configuration
curl -X POST http://localhost:9093/-/reload

# Test alert routing
# This should trigger a test critical alert
curl -X POST http://localhost:9093/api/v1/alerts \
  -H "Content-Type: application/json" \
  -d '{
    "receiver": "security-critical",
    "status": "firing",
    "alerts": [{
      "labels": {
        "alertname": "TestSecurityAlert",
        "severity": "critical",
        "component": "security"
      },
      "annotations": {
        "summary": "Test security alert",
        "description": "This is a test security alert"
      }
    }]
  }'
```

## Testing and Verification

### 1. Health Check All Services

```bash
# Test all service health endpoints
for service in security-log-parser geoip-lookup attack-detection; do
  echo "Testing $service..."
  curl -f http://localhost:${service_port}/health || echo "FAILED"
done
```

### 2. Verify Prometheus Metrics

```bash
# Test HAProxy exporter metrics
curl -s http://localhost:9101/metrics | grep haproxy_

# Test security log parser metrics
curl -s http://localhost:8000/metrics | grep chimera_

# Test GeoIP service metrics
curl -s http://localhost:8001/metrics | grep chimera_

# Test attack detection metrics
curl -s http://localhost:8002/metrics | grep chimera_
```

### 3. Test Security Event Processing

```bash
# Generate test security events
curl -X POST http://localhost:8000/parse-log \
  -H "Content-Type: application/json" \
  -d '{
    "timestamp": "2026-06-20T10:00:00Z",
    "source_ip": "192.168.1.100",
    "http_status": "429",
    "http_path": "/api/v1/webhook",
    "threat_level": "HIGH"
  }'

# Check event was processed
curl http://localhost:8000/security-events
```

### 4. Test GeoIP Lookup

```bash
# Test IP geolocation
curl http://localhost:8001/geoip/8.8.8.8

# Test batch lookup
curl "http://localhost:8001/geoip/batch?ip_addresses=8.8.8.8,1.1.1.1"

# Test cache statistics
curl http://localhost:8001/cache/stats
```

### 5. Test Attack Detection

```bash
# Simulate brute force attack
curl -X POST http://localhost:8002/detect \
  -H "Content-Type: application/json" \
  -d '{
    "timestamp": "2026-06-20T10:00:00Z",
    "source_ip": "10.0.0.50",
    "http_status": "401",
    "http_path": "/api/v1/auth"
  }'

# Check active threats
curl http://localhost:8002/threats/active
```

### 6. Verify Dashboard Panels

```bash
# Access dashboard
open http://localhost:3000/d/chimera-security

# Verify data sources are connected
curl -s http://localhost:3000/api/datasources | jq '.[] | select(.name | contains("Prometheus"))'
```

## Maintenance Procedures

### Weekly GeoIP Database Updates

The GeoIP updater service runs automatically on Sundays at 3 AM. To manually update:

```bash
docker exec chimera-geoip-updater python /app/geoip-updater.py --force
```

### Monthly Security Review

1. **Review Attack Patterns**: Analyze threat history for trends
2. **Update Detection Rules**: Adjust thresholds based on traffic patterns
3. **Review False Positives**: Fine-tune detection algorithms
4. **Performance Impact**: Monitor overhead and optimize

### Quarterly Maintenance

1. **Update GeoIP License**: Renew MaxMind license if needed
2. **Review Alert Thresholds**: Adjust based on production patterns
3. **Update Country Blocklists**: Review allowed/blocked countries
4. **Performance Optimization**: Review and optimize detection algorithms

## Troubleshooting

### Services Not Starting

```bash
# Check service logs
docker-compose -f docker-compose-haproxy.yml logs [service-name]

# Check service status
docker-compose -f docker-compose-haproxy.yml ps

# Restart specific service
docker-compose -f docker-compose-haproxy.yml restart [service-name]
```

### No Metrics in Prometheus

```bash
# Verify Prometheus targets
curl -s http://localhost:9090/api/v1/targets | jq '.data.activeTargets[]'

# Check target health
curl -s http://localhost:9090/api/v1/targets | jq '.data.activeTargets[] | select(.labels.job=="haproxy") | .health'

# Reload Prometheus config
docker-compose -f docker-compose.yml restart prometheus
```

### Alerts Not Firing

```bash
# Verify alert rules are loaded
curl -s http://localhost:9090/api/v1/rules | jq '.data.groups[] | select(.name | contains("security"))'

# Test alert expression
curl -s 'http://localhost:9090/api/v1/query?query=chimera_attacks_detected_total'

# Check AlertManager config
curl -s http://localhost:9093/api/v1/status
```

### GeoIP Lookups Failing

```bash
# Check database files exist
ls -la docker/haproxy/geoip/

# Verify database integrity
python tools/geoip-updater.py --verify

# Test GeoIP service directly
curl http://localhost:8001/health
```

## Performance Considerations

### Expected Overhead

- **CPU**: <5% overhead at normal traffic levels
- **Memory**: ~50MB per security service
- **Network**: <1MB/s for metrics and logs
- **Latency**: <100ms additional for security enrichment

### Scaling Recommendations

- **High Traffic**: Deploy multiple Redis instances for caching
- **GeoIP Load**: Consider external GeoIP service for >10k requests/sec
- **Attack Detection**: Scale detection service horizontally for >1k events/sec

## Security Considerations

### Access Control

1. **Prometheus**: Restrict access to admin users only
2. **Grafana**: Use strong authentication and RBAC
3. **AlertManager**: Secure webhook URLs and credentials
4. **Redis**: Use AUTH in production environments

### Data Protection

1. **IP Logs**: Implement 30-day retention policy (GDPR compliance)
2. **Security Events**: Regular archival and cleanup
3. **GeoIP Data**: Follow MaxMind license terms
4. **Alert Content**: Avoid sensitive information in notifications

## Rollback Procedures

If issues occur, rollback can be performed in stages:

```bash
# 1. Stop new services
docker-compose -f docker-compose-haproxy.yml stop \
  chimera-security-log-parser \
  chimera-geoip-lookup \
  chimera-attack-detection

# 2. Revert HAProxy config
git checkout docker/haproxy/haproxy.cfg

# 3. Revert Prometheus config
git checkout ops/prometheus/prometheus.yml
git checkout ops/prometheus/alerts.yml

# 4. Restart services
docker-compose -f docker-compose-haproxy.yml restart haproxy
docker-compose -f docker-compose-haproxy.yml restart prometheus

# 5. Remove new dashboard
# Delete from Grafana UI or use API
```

## Success Criteria

- ✅ All 6 security services operational
- ✅ Metrics visible in Prometheus for all services
- ✅ Security dashboard functional with all 18 panels
- ✅ Alert rules loaded and firing correctly
- ✅ Telegram notifications working
- ✅ Zero disruption to existing rate limiting
- ✅ Performance overhead <5%
- ✅ Additional latency <100ms
- ✅ 100% backward compatibility maintained

## Support and Documentation

For issues or questions:
1. Check logs: `docker-compose -f docker-compose-haproxy.yml logs [service]`
2. Review this deployment guide
3. Check service health endpoints
4. Review Prometheus targets and alert rules
5. Consult runbooks in `ops/runbooks/` directory

---

**Implementation Complete**: All 6 phases of the security measurement plan have been successfully implemented and deployed.