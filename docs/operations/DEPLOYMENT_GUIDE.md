# Chimera Operator - Production Deployment Guide

## Table of Contents
1. [Infrastructure Requirements](#infrastructure-requirements)
2. [Pre-Deployment Checklist](#pre-deployment-checklist)
3. [Deployment Steps](#deployment-steps)
4. [Configuration](#configuration)
5. [Monitoring & Verification](#monitoring--verification)
6. [Troubleshooting](#troubleshooting)
7. [Scaling & Maintenance](#scaling--maintenance)

---

## Infrastructure Requirements

### Datacenter Location (Critical)
**Requirement:** Server must be located in **US-East (Ashburn, VA)** or **Amsterdam, Netherlands**

**Why:** Solana RPC endpoints are distributed across these regions. Sub-50ms latency is critical for:
- Jito bundle atomicity (Spear strategy)
- Blockhash expiration handling
- Trade execution within network conditions

**Latency Targets:**
| Component | Threshold | Impact |
|-----------|-----------|--------|
| Helius RPC latency | < 50ms | Full Spear strategy enabled |
| 50-100ms | Spear disabled, Shield only |
| > 100ms | Not recommended for production |

### Recommended Providers

#### US-East (Ashburn, VA)
| Provider | Service | Notes |
|----------|---------|-------|
| [Latitude.sh](https://latitude.sh) | Bare Metal | Dedicated hardware, lowest latency |
| [Cherry Servers](https://www.cherryservers.com) | Bare Metal | US-East availability |
| [Vultr](https://www.vultr.com) | Cloud | $5-20/month, minimal latency |
| [Linode](https://www.linode.com) | Cloud | Ashburn DC, reliable |

#### Amsterdam, Netherlands
| Provider | Service | Notes |
|----------|---------|-------|
| [Hetzner](https://www.hetzner.com) | Bare Metal | Affordable, excellent EU latency |
| [OVH](https://www.ovh.com) | Bare Metal | European leader |
| [DigitalOcean](https://www.digitalocean.com) | Cloud | Amsterdam region available |

### Hardware Specifications

**Minimum Specs:**
```
CPU:      2-core (4-core recommended)
RAM:      4GB minimum (8GB recommended)
Storage:  50GB SSD (for SQLite WAL + logs)
Network:  1Gbps (100Mbps minimum)
Bandwidth: Unlimited or > 1TB/month
```

**Recommended Production Specs:**
```
CPU:      8-core AMD EPYC or Intel Xeon
RAM:      16GB
Storage:  500GB NVMe SSD
Network:  1Gbps+
OS:       Ubuntu 22.04 LTS or similar
```

### Network Requirements

**Outbound Ports:**
- `443` (HTTPS) — Helius RPC, Jito, Discord, Telegram
- `53` (DNS) — Domain resolution

**Inbound Ports:**
- `8080` (HTTP) — Operator API (recommended: behind reverse proxy)
- `22` (SSH) — Administration

**Firewall Rules:**
```bash
# Allow SSH from admin IP only
ufw allow from <YOUR_IP> to any port 22

# Allow API traffic (consider using WAF/proxy)
ufw allow 8080

# Default deny
ufw default deny incoming
```

---

## Pre-Deployment Checklist

### 1. Code & Tests
```bash
cd /path/to/chimera

# Verify all tests pass
cargo test                              # Rust: 82/82 passing
cd scout && python -m pytest tests/    # Python: 98/98 passing

# Verify clippy passes
cargo clippy -- -D warnings            # 0 errors

# Build release binary
make build                             # Clean release build
```

### 2. Git Status
```bash
# Verify commit is clean and pushed
git log -1 --oneline
git push origin main
```

### 3. Configuration Files
```bash
# Copy environment template
cd operator
cp config/.env.example .env

# Edit with production values
# Required variables:
#   CHIMERA_SECURITY__WEBHOOK_SECRET (64-char hex)
#   CHIMERA_RPC__PRIMARY_URL (Helius endpoint)
#   TELEGRAM_BOT_TOKEN (if using Telegram)
#   DISCORD_WEBHOOK_URL (if using Discord)
```

### 4. RPC Endpoint Test
```bash
# Test latency to Helius endpoint
ping -c 3 api.mainnet-beta.solana.com

# Expected: < 50ms (US-East/Amsterdam)
# If > 100ms: Relocate or use different provider
```

### 5. Database Initialization
```bash
# Initialize schema
make db-init

# Verify schema
sqlite3 data/chimera.db ".tables"
# Expected: admin_wallets, config_audit, dead_letter_queue, positions, trades, wallets
```

---

## Deployment Steps

### Step 1: Provision Server

**1a. Launch Instance**
```bash
# Example: Vultr US-East (Ashburn)
# - Choose Ubuntu 22.04 LTS
# - 4GB+ RAM, 2+ CPU cores
# - Enable IPv6 if available
# - Add SSH key
```

**1b. Connect & Update**
```bash
ssh root@<SERVER_IP>

# Update system packages
apt update && apt upgrade -y

# Install dependencies
apt install -y \
  build-essential \
  pkg-config \
  libssl-dev \
  sqlite3 \
  curl \
  git \
  htop

# Install Rust (if not pre-installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source $HOME/.cargo/env
```

### Step 2: Clone Repository & Build

```bash
# Clone repository
git clone https://github.com/mshafiee/chimera.git
cd chimera/operator

# Create .env with production values
cp config/.env.example .env
nano .env  # Edit with your settings

# Build release binary
cargo build --release
# Output: target/release/chimera_operator
```

### Step 3: Create Data Directory

```bash
# Create data directories
mkdir -p /opt/chimera/data
mkdir -p /var/log/chimera

# Set permissions
chown -R chimera:chimera /opt/chimera
chmod 700 /opt/chimera/data

# Initialize database
cd /opt/chimera
/path/to/chimera/operator/target/release/chimera_operator --init-db
```

### Step 4: Install as Systemd Service

**Create service file:**
```bash
sudo tee /etc/systemd/system/chimera.service > /dev/null << 'EOF'
[Unit]
Description=Chimera Copy-Trading Operator
After=network.target

[Service]
Type=simple
User=chimera
WorkingDirectory=/opt/chimera
EnvironmentFile=/opt/chimera/.env
ExecStart=/opt/chimera/chimera_operator
Restart=on-failure
RestartSec=10s

# Logging
StandardOutput=journal
StandardError=journal
SyslogIdentifier=chimera

# Resource limits
LimitNOFILE=65536
LimitNPROC=4096

# Security
NoNewPrivileges=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
EOF

# Enable & start
sudo systemctl daemon-reload
sudo systemctl enable chimera
sudo systemctl start chimera
```

**Verify:**
```bash
sudo systemctl status chimera
sudo journalctl -u chimera -f  # Follow logs
```

### Step 5: Install Monitoring & Cron Jobs

```bash
# Install Scout cron job (wallet analysis)
cd /path/to/chimera/ops
sudo bash install-crons.sh

# Install Prometheus metrics scraper
sudo bash install-monitoring.sh

# Verify cron jobs
sudo crontab -l
```

### Step 6: Configure Reverse Proxy (Recommended)

**Using Nginx:**
```bash
sudo apt install -y nginx

sudo tee /etc/nginx/sites-available/chimera > /dev/null << 'EOF'
server {
    listen 80;
    server_name _;

    # Rate limiting
    limit_req_zone $binary_remote_addr zone=api:10m rate=100r/s;
    limit_req zone=api burst=200 nodelay;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        
        # Timeouts
        proxy_connect_timeout 10s;
        proxy_send_timeout 30s;
        proxy_read_timeout 30s;
    }

    # Health check endpoint (no rate limit)
    location /api/v1/health {
        proxy_pass http://127.0.0.1:8080;
    }
}
EOF

sudo ln -s /etc/nginx/sites-available/chimera /etc/nginx/sites-enabled/
sudo nginx -t
sudo systemctl restart nginx
```

---

## Configuration

### Environment Variables (.env)

**Required:**
```bash
# RPC Configuration (must be < 50ms latency)
CHIMERA_RPC__PRIMARY_URL=https://mainnet.helius-rpc.com/?api-key=YOUR_API_KEY
CHIMERA_RPC__FALLBACK_URL=https://api.mainnet-beta.solana.com

# Security
CHIMERA_SECURITY__WEBHOOK_SECRET=<64-char hex: openssl rand -hex 32>

# Notifications (optional)
TELEGRAM_BOT_TOKEN=<your_telegram_bot_token>
TELEGRAM_CHAT_ID=<your_telegram_chat_id>
DISCORD_WEBHOOK_URL=<your_discord_webhook>

# Database
CHIMERA_DATABASE__PATH=/opt/chimera/data/chimera.db

# Logging
RUST_LOG=chimera_operator=info
```

### config.yaml Settings

**Critical for Production:**
```yaml
server:
  host: 127.0.0.1        # Only expose via proxy
  port: 8080

database:
  path: /opt/chimera/data/chimera.db
  max_connections: 20

rpc:
  primary_url: "https://mainnet.helius-rpc.com/?api-key=YOUR_KEY"
  rate_limit_per_second: 40  # Helius Developer Plan limit

security:
  webhook_rate_limit: 100    # requests/second
  max_timestamp_drift_secs: 60

circuit_breaker:
  daily_loss_limit_sol: 10.0
  loss_threshold_percent: 5.0
  recovery_interval_minutes: 5

strategy:
  shield_percent: 70
  spear_percent: 30

jito:
  tip_floor_sol: 0.0005      # Minimum tip
  tip_ceiling_sol: 0.01      # Maximum tip

token_safety:
  honeypot_detection_enabled: true
  min_liquidity_shield_usd: 10000
  min_liquidity_spear_usd: 5000
```

---

## Monitoring & Verification

### Pre-Flight Checks

```bash
make preflight
# Checks:
# ✅ Time synchronization (NTP)
# ✅ RPC latency < 50ms
# ✅ Circuit breaker functionality
```

### Health Endpoint

```bash
curl http://localhost:8080/api/v1/health

# Expected response:
# {
#   "status": "ok",
#   "trading_allowed": true,
#   "uptime_seconds": 3600
# }
```

### Monitoring Status

```bash
curl http://localhost:8080/api/v1/monitoring/status

# Returns:
# - RPC health
# - Queue depth
# - Circuit breaker state
# - Signal aggregation stats
```

### Log Monitoring

```bash
# Follow logs in real-time
sudo journalctl -u chimera -f

# Search for errors
sudo journalctl -u chimera --grep ERROR

# Last 100 lines
sudo journalctl -u chimera -n 100
```

### Prometheus Metrics

```bash
curl http://localhost:8080/metrics

# Key metrics:
# - chimera_queue_depth
# - chimera_trade_latency_ms
# - chimera_circuit_breaker_trips
# - chimera_rpc_latency_ms
```

---

## Troubleshooting

### Issue: RPC Latency > 50ms

**Diagnosis:**
```bash
# Test RPC endpoint latency
curl -w "Response time: %{time_total}\n" -o /dev/null -s https://mainnet.helius-rpc.com/?api-key=KEY

# Check routing
mtr -c 10 mainnet.helius-rpc.com
```

**Solutions:**
1. **Relocate VPS** to US-East or Amsterdam (preferred)
2. **Switch RPC provider** (QuickNode, Triton)
3. **Disable Spear strategy** (Shield only):
   ```yaml
   strategy:
     shield_percent: 100
     spear_percent: 0
   ```

### Issue: Circuit Breaker Triggered

**Reset via API:**
```bash
curl -X POST http://localhost:8080/api/v1/config/circuit-breaker/reset \
  -H "Authorization: Bearer <TOKEN>"
```

**Or via database:**
```bash
sqlite3 /opt/chimera/data/chimera.db \
  "UPDATE circuit_breaker SET tripped = 0 WHERE id = 1;"
```

### Issue: Database Locked

**Verify WAL mode:**
```bash
sqlite3 /opt/chimera/data/chimera.db "PRAGMA journal_mode;"
# Output: wal
```

**Enable WAL mode:**
```bash
sqlite3 /opt/chimera/data/chimera.db << 'EOF'
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 5000;
EOF
```

### Issue: High Memory Usage

**Check memory:**
```bash
top -p $(pgrep -f chimera_operator)

# If > 500MB:
# 1. Check position_cache size (monitoring/status endpoint)
# 2. Reduce price_cache TTL in config
# 3. Scale cache pruning intervals
```

---

## Scaling & Maintenance

### Backup Strategy

```bash
# Daily database backup
0 2 * * * /opt/chimera/backup-db.sh

# Backup script:
#!/bin/bash
BACKUP_DIR=/opt/chimera/backups
mkdir -p $BACKUP_DIR
cp /opt/chimera/data/chimera.db $BACKUP_DIR/chimera-$(date +%Y%m%d).db
# Keep last 30 days
find $BACKUP_DIR -mtime +30 -delete
```

### Log Rotation

```bash
sudo tee /etc/logrotate.d/chimera > /dev/null << 'EOF'
/var/log/chimera/*.log {
    daily
    rotate 7
    compress
    delaycompress
    missingok
    notifempty
}
EOF
```

### Version Updates

```bash
# Pull latest code
cd /path/to/chimera
git pull origin main

# Rebuild
make build-operator

# Stop service
sudo systemctl stop chimera

# Copy new binary
sudo cp target/release/chimera_operator /opt/chimera/

# Start service
sudo systemctl start chimera

# Verify
curl http://localhost:8080/api/v1/health
```

### High Availability Setup

**For HA deployment:**
1. **Primary + Standby** servers (same datacenter)
2. **Shared database** (PostgreSQL or managed SQLite)
3. **Load balancer** (HAProxy, Nginx)
4. **Wallet key replication** (encrypted vault sync)

See `docs/operations/high-availability.md` for details.

---

## Support & References

- **Architecture:** `docs/core/architecture.md`
- **API Reference:** `docs/core/api.md`
- **Runbooks:** `ops/runbooks/`
- **Incident Response:** `ops/runbooks/incident-response.md`

---

## Deployment Checklist (Final)

- [ ] Server located in US-East or Amsterdam
- [ ] RPC latency tested and < 50ms
- [ ] All tests pass (cargo test, pytest)
- [ ] Code committed and pushed
- [ ] .env configured with production secrets
- [ ] Database initialized
- [ ] Systemd service installed
- [ ] Reverse proxy configured
- [ ] Monitoring enabled
- [ ] Health endpoint responding
- [ ] Cron jobs installed
- [ ] Backups configured
- [ ] Log rotation configured
- [ ] Alerts configured
- [ ] Documentation reviewed

**Status:** ✅ Ready for Production
