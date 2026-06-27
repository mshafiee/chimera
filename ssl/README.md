# Chimera Trading System - HAProxy and SSL Setup Guide

This guide covers the implementation of HAProxy as a reverse proxy with SSL/TLS termination and Helius webhook monitoring for the Chimera trading system.

## Prerequisites

- Docker and Docker Compose
- Domain name (for production) or use chimera.local (for development)
- Basic understanding of SSL certificates and reverse proxies
- Helius API key (already configured in environment files)

## Quick Start

### 1. Generate SSL Certificates

**For Development (Self-Signed):**
```bash
./ssl/generate-certificates.sh chimera.local trading@chimera.local
# Select option 2 for self-signed certificate
```

**For Production (Let's Encrypt):**
```bash
./ssl/generate-certificates.sh yourdomain.com admin@yourdomain.com
# Select option 1 for Let's Encrypt certificate
```

### 2. Update Environment Configuration

Edit `docker/env.mainnet-paper.local` or `docker/env.mainnet-prod`:
```bash
CHIMERA_MONITORING__HELIUS_WEBHOOK_URL=https://yourdomain.com/api/v1/monitoring/helius-webhook
```

### 3. Start Services

**Start with HAProxy:**
```bash
# Start main services (without external ports)
docker-compose --profile mainnet-paper up -d

# Start HAProxy reverse proxy
docker-compose -f docker-compose-haproxy.yml --profile mainnet-paper up -d
```

**Traditional deployment (without HAProxy):**
```bash
# Start services with external ports exposed
docker-compose --profile mainnet-paper up -d
```

### 4. Test Webhook Endpoint

```bash
# Test webhook connectivity
./tools/test-helius-webhook.sh https://yourdomain.com/api/v1/monitoring/helius-webhook

# Or for local development
./tools/test-helius-webhook.sh https://chimera.local/api/v1/monitoring/helius-webhook
```

### 5. Register Wallets with Helius

```bash
# Register ACTIVE wallets from database
python tools/register_helius_webhooks.py

# Register specific wallets
python tools/register_helius_webhooks.py wallet1 wallet2 wallet3

# List existing webhooks
python tools/register_helius_webhooks.py --list
```

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    External Traffic                      │
│              (HTTPS:443 via HAProxy)                    │
└─────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────┐
│              HAProxy (SSL Termination)                  │
│         ┌──────────────────────────────────┐            │
│         │  • Rate limiting                 │            │
│         │  • Health checks                 │            │
│         │  • WebSocket support             │            │
│         │  • HMAC header preservation       │            │
│         └──────────────────────────────────┘            │
└─────────────────────────────────────────────────────────┘
                            │
        ┌───────────────────┼───────────────────┐
        │                   │                   │
        ▼                   ▼                   ▼
┌──────────────┐   ┌──────────────┐   ┌──────────────┐
│   Operator    │   │  Web UI      │   │  Helius     │
│   (8080)      │   │  (80)        │   │  Webhooks   │
└──────────────┘   └──────────────┘   └──────────────┘
```

## HAProxy Features

### SSL/TLS Termination
- Certificates stored in `ssl/certbot/letsencrypt/chimera.pem`
- Automatic renewal via `ssl/renew-certificates.sh`
- HTTPS enforced with HTTP redirect

### Rate Limiting
- Trading signal webhooks: 100 req/s
- Helius webhooks: 45 req/s
- Per-IP rate limiting with stick tables

### Routing Rules
- `/api/v1/webhook` → Trading signals (HMAC authenticated)
- `/api/v1/monitoring/helius-webhook` → Helius notifications
- `/ws` → WebSocket connections
- `/api/v1/*` → General API endpoints
- `/` → Web dashboard

### Health Checks
- Continuous monitoring of backend services
- Automatic failover on backend failure
- Circuit breaker awareness

### Statistics Dashboard
- Access at `http://your-server:8404/stats`
- Real-time metrics and connection statistics
- Backend health monitoring

## Configuration Files

### Main Configuration
- `docker/haproxy/haproxy.cfg` - HAProxy main configuration
- `docker-compose-haproxy.yml` - HAProxy service definition
- `docker-compose.yml` - Main services (updated for internal communication)

### SSL Certificate Management
- `ssl/generate-certificates.sh` - Certificate generation script
- `ssl/renew-certificates.sh` - Certificate renewal automation
- `ssl/certbot/letsencrypt/` - Certificate storage directory

### Webhook Tools
- `tools/register_helius_webhooks.py` - Helius webhook registration
- `tools/test-helius-webhook.sh` - Webhook endpoint testing

## SSL Certificate Management

### Development Certificates
```bash
# Generate self-signed certificate
./ssl/generate-certificates.sh chimera.local trading@chimera.local
```

### Production Certificates
```bash
# Generate Let's Encrypt certificate
./ssl/generate-certificates.sh yourdomain.com admin@yourdomain.com

# Set up automatic renewal (add to crontab)
0 3 * * * /path/to/chimera/ssl/renew-certificates.sh
```

### Certificate Renewal
```bash
# Manual renewal
./ssl/renew-certificates.sh yourdomain.com

# Check certificate expiry
openssl x509 -enddate -noout -in ssl/certbot/letsencrypt/chimera.pem
```

## Helius Webhook Integration

### Registration
```bash
# Register ACTIVE wallets from database
python tools/register_helius_webhooks.py

# Register specific wallets
python tools/register_helius_webhooks.py wallet_address_1 wallet_address_2

# List existing webhooks
python tools/register_helius_webhooks.py --list

# Test webhook URL
python tools/register_helius_webhooks.py --test
```

### Webhook Payload Format
Helius sends POST requests with transaction data:
```json
{
  "accountData": [...],
  "nativeTransfers": [...],
  "signature": "signature...",
  "slot": 12345,
  "timestamp": 1234567890,
  "type": "SWAP",
  "transaction": {...}
}
```

### Testing Webhooks
```bash
# Test webhook connectivity
./tools/test-helius-webhook.sh https://yourdomain.com/api/v1/monitoring/helius-webhook

# Test with local development server
./tools/test-helius-webhook.sh https://chimera.local/api/v1/monitoring/helius-webhook
```

## Troubleshooting

### HAProxy Issues

**Configuration validation:**
```bash
docker run --rm -v $(pwd)/docker/haproxy:/usr/local/etc/haproxy haproxy:2.8-alpine haproxy -c -f /usr/local/etc/haproxy/haproxy.cfg
```

**Check HAProxy logs:**
```bash
docker logs chimera-haproxy
```

**Access HAProxy stats:**
```
http://your-server:8404/stats
Username: admin
Password: CHANGE_ME_PASSWORD (update in haproxy.cfg)
```

### SSL Certificate Issues

**Certificate not found:**
```bash
ls -la ssl/certbot/letsencrypt/chimera.pem
# Should exist and contain combined certificate and key
```

**Certificate expired:**
```bash
# Check expiry date
openssl x509 -enddate -noout -in ssl/certbot/letsencrypt/chimera.pem

# Renew certificate
./ssl/renew-certificates.sh yourdomain.com
```

**Browser warnings for self-signed certificates:**
- This is expected for development
- For production, use Let's Encrypt certificates
- Browsers will warn about untrusted certificates

### Webhook Issues

**No webhook signals received:**
1. Check webhook URL is accessible: `./tools/test-helius-webhook.sh`
2. Verify Helius webhook registration: `python tools/register_helius_webhooks.py --list`
3. Check operator logs for webhook activity
4. Ensure monitored wallets are actively trading

**HMAC authentication failures:**
1. Verify webhook secret matches between sender and receiver
2. Check timestamp is within ±60 seconds
3. Ensure headers are preserved by HAProxy

### Service Communication Issues

**Services cannot reach each other:**
```bash
# Check network connectivity
docker network inspect chimera-network

# Test service connectivity
docker exec chimera-haproxy nc -z operator 8080
docker exec chimera-haproxy nc -z web 80
```

## Monitoring

### HAProxy Metrics
- URL: `http://your-server:8404/stats`
- Metrics include:
  - Backend health status
  - Connection counts
  - Response times
  - Rate limiting violations

### Application Health
- Operator health: `https://yourdomain.com/api/v1/health`
- Web dashboard: `https://yourdomain.com/`
- WebSocket test: `wss://yourdomain.com/ws`

### Logging
```bash
# HAProxy logs
docker logs chimera-haproxy

# Operator logs
docker logs chimera-operator

# Webhook activity
docker logs chimera-operator | grep -i webhook
```

## Production Deployment Checklist

- [ ] SSL certificates obtained and installed
- [ ] HAProxy configuration validated
- [ ] Health checks passing for all backends
- [ ] Rate limiting configured and tested
- [ ] WebSocket connections working
- [ ] HMAC headers preserved
- [ ] Helius webhook URLs updated to HTTPS
- [ ] Certificate renewal automated
- [ ] Monitoring dashboards configured
- [ ] Security headers validated
- [ ] Load testing completed
- [ ] Rollback plan documented
- [ ] Team trained on new architecture

## Rollback Procedure

If issues occur after HAProxy deployment:

1. **Stop HAProxy services:**
```bash
docker-compose -f docker-compose-haproxy.yml down
```

2. **Restore original configuration:**
```bash
git checkout docker-compose.yml
```

3. **Restart services directly:**
```bash
docker-compose --profile mainnet-paper up -d
```

4. **Update webhook URLs:**
```bash
# Change back to HTTP in environment files
CHIMERA_MONITORING__HELIUS_WEBHOOK_URL=http://your-server:8080/api/v1/monitoring/helius-webhook
```

## Security Considerations

### HAProxy
- Change default stats password in `haproxy.cfg`
- Use strong SSL/TLS ciphers (already configured)
- Enable rate limiting to prevent DoS attacks
- Monitor HAProxy stats for unusual activity

### SSL/TLS
- Use Let's Encrypt for production certificates
- Set up automatic renewal
- Monitor certificate expiry (30 days before)
- Use strong certificate private keys

### Webhooks
- Preserve HMAC signatures (HAProxy configuration)
- Implement IP whitelisting for Helius webhooks
- Use HTTPS for all webhook endpoints
- Monitor webhook processing logs

## Performance Optimization

### HAProxy Tuning
- Adjust `maxconn` based on expected load
- Optimize `nbthread` for CPU cores
- Tune timeout values for your use case
- Monitor connection statistics

### Rate Limiting
- Adjust limits based on actual traffic patterns
- Monitor rate limit violations
- Implement IP-based whitelisting if needed

### Caching
- Consider HAProxy caching for static content
- Implement CDN for static assets
- Cache webhook responses if appropriate

## Support and Documentation

For more information, see:
- [HAProxy Documentation](http://www.haproxy.org/)
- [Let's Encrypt Documentation](https://letsencrypt.org/docs/)
- [Helius API Documentation](https://helius.xyz/docs/)
- [Chimera Architecture](./docs/core/architecture.md)

## Contact

For issues or questions:
- Check HAProxy logs: `docker logs chimera-haproxy`
- Check operator logs: `docker logs chimera-operator`
- Test webhook connectivity: `./tools/test-helius-webhook.sh`
- Validate configuration: `haproxy -c -f docker/haproxy/haproxy.cfg`