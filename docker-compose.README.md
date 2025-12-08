# Docker Compose Setup for Chimera

This Docker Compose configuration provides a complete testing and deployment solution for Chimera across three environments:

1. **Devnet** - Development and testing on Solana Devnet
2. **Mainnet Paper Trading** - Testing on Mainnet with simulated trades (no real funds)
3. **Mainnet Production** - Live trading on Mainnet with real funds

## Prerequisites

- Docker Engine 20.10+
- Docker Compose 2.0+
- At least 4GB RAM available
- 10GB free disk space

## Quick Start

### 1. Devnet (Development/Testing)

**Using the helper script (recommended):**

```bash
# Initialize database
./docker/docker-compose.sh init-db devnet

# Start all services
./docker/docker-compose.sh start devnet

# View logs
./docker/docker-compose.sh logs devnet -f

# Stop services
./docker/docker-compose.sh stop devnet
```

**Or using docker-compose directly:**

```bash
# Set the profile
export COMPOSE_PROFILE=devnet

# Copy and configure environment (if needed)
cp docker/env.devnet docker/env.devnet.local
# Edit docker/env.devnet.local with your settings

# Initialize database
mkdir -p data
sqlite3 data/chimera.db < database/schema.sql

# Start all services
docker-compose --profile devnet up -d

# View logs
docker-compose --profile devnet logs -f

# Stop services
docker-compose --profile devnet down
```

### 2. Mainnet Paper Trading

**Using the helper script (recommended):**

```bash
# Copy and configure environment
cp docker/env.mainnet-paper docker/env.mainnet-paper.local
# Edit docker/env.mainnet-paper.local:
#   - Add your Helius API key
#   - Configure Telegram/Discord notifications
#   - Set secure passwords

# Initialize database
./docker/docker-compose.sh init-db mainnet-paper

# Start services
./docker/docker-compose.sh start mainnet-paper

# Monitor logs
./docker/docker-compose.sh logs mainnet-paper -f operator
```

**Or using docker-compose directly:**

```bash
# Set the profile
export COMPOSE_PROFILE=mainnet-paper

# Copy and configure environment
cp docker/env.mainnet-paper docker/env.mainnet-paper.local
# Edit docker/env.mainnet-paper.local with your settings

# Initialize database
mkdir -p data
sqlite3 data/chimera.db < database/schema.sql

# Start services
docker-compose --profile mainnet-paper up -d

# Monitor logs
docker-compose --profile mainnet-paper logs -f operator
```

### 3. Mainnet Production

⚠️ **WARNING: This uses REAL funds. Use with extreme caution.**

**Using the helper script (recommended):**

```bash
# Copy and configure environment
cp docker/env.mainnet-prod docker/env.mainnet-prod.local
# REQUIRED: Edit docker/env.mainnet-prod.local:
#   - Add your Helius API key
#   - Generate secure webhook secret: openssl rand -hex 32
#   - Configure Telegram/Discord notifications
#   - Set secure Grafana password
#   - Configure wallet private key (encrypted)

# Initialize database
./docker/docker-compose.sh init-db mainnet-prod

# Run preflight checks
./docker/docker-compose.sh exec mainnet-prod operator /app/chimera_operator --preflight || true

# Start services
./docker/docker-compose.sh start mainnet-prod

# Monitor closely
./docker/docker-compose.sh logs mainnet-prod -f
```

**Or using docker-compose directly:**

```bash
# Set the profile
export COMPOSE_PROFILE=mainnet-prod

# Copy and configure environment
cp docker/env.mainnet-prod docker/env.mainnet-prod.local
# REQUIRED: Edit docker/env.mainnet-prod.local with your settings

# Initialize database
mkdir -p data
sqlite3 data/chimera.db < database/schema.sql

# Run preflight checks
docker-compose --profile mainnet-prod run --rm operator /app/chimera_operator --preflight

# Start services
docker-compose --profile mainnet-prod up -d

# Monitor closely
docker-compose --profile mainnet-prod logs -f
```

## Service Architecture

### Services Overview

| Service | Port | Description |
|---------|------|-------------|
| **operator** | 8080 | Core trading engine (Rust) |
| **scout** | - | Wallet intelligence layer (Python, runs on schedule) |
| **web** | 3000 | Web dashboard (React) |
| **prometheus** | 9090 | Metrics collection |
| **grafana** | 3001 | Metrics visualization |
| **alertmanager** | 9093 | Alert routing |

### Service Details

#### Operator
- **Image**: Built from `operator/Dockerfile`
- **Health Check**: `GET /api/v1/health`
- **Data**: Persisted in `./data/chimera.db`
- **Config**: Mounted from `./config/`

#### Scout
- **Image**: Built from `scout/Dockerfile`
- **Schedule**: Runs daily at 2 AM UTC (configurable)
- **Output**: Writes to `./data/roster_new.db`

#### Web Dashboard
- **Image**: Built from `web/Dockerfile`
- **API**: Connects to operator at `http://operator:8080`
- **Static**: Served via nginx

#### Monitoring Stack
- **Prometheus**: Scrapes metrics from operator
- **Grafana**: Visualizes metrics (default: admin/admin)
- **Alertmanager**: Routes alerts to Telegram/Discord

## Environment Configuration

### Environment Files

Each environment has its own configuration file in the `docker/` directory:

- `docker/env.devnet` - Devnet configuration template
- `docker/env.mainnet-paper` - Mainnet paper trading template
- `docker/env.mainnet-prod` - Mainnet production template

**Note:** The docker-compose.yml automatically loads the appropriate file based on `COMPOSE_PROFILE`. You can create local overrides (e.g., `docker/env.devnet.local`) if needed.

### Key Configuration Variables

#### Required for All Environments

```bash
# RPC Endpoint
CHIMERA_RPC__PRIMARY_URL=https://api.devnet.solana.com

# Webhook Secret (generate with: openssl rand -hex 32)
CHIMERA_SECURITY__WEBHOOK_SECRET=your-secret-here
```

#### Mainnet-Specific

```bash
# Helius API Key (required for mainnet)
CHIMERA_RPC__PRIMARY_URL=https://mainnet.helius-rpc.com/?api-key=YOUR_KEY

# Fallback RPC
CHIMERA_RPC__FALLBACK_URL=https://your-quicknode-endpoint.com
```

#### Production-Specific

```bash
# Notifications (highly recommended)
TELEGRAM_BOT_TOKEN=your-bot-token
TELEGRAM_CHAT_ID=your-chat-id
DISCORD_WEBHOOK_URL=your-webhook-url

# Grafana Password
GRAFANA_PASSWORD=secure-password
```

## Database Management

### Initialize Database

```bash
# Create data directory
mkdir -p data

# Initialize schema
sqlite3 data/chimera.db < database/schema.sql
```

### Backup Database

```bash
# Backup while services are running
docker-compose exec operator sqlite3 /app/data/chimera.db ".backup /app/data/chimera.backup.db"

# Copy backup to host
docker cp chimera-operator:/app/data/chimera.backup.db ./data/
```

### Access Database

```bash
# SQLite shell
docker-compose exec operator sqlite3 /app/data/chimera.db

# Or from host
sqlite3 data/chimera.db
```

## Monitoring & Observability

### Access Dashboards

- **Grafana**: http://localhost:3001 (admin/admin by default)
- **Prometheus**: http://localhost:9090
- **Alertmanager**: http://localhost:9093

### View Metrics

```bash
# Operator metrics
curl http://localhost:8080/metrics

# Prometheus query
curl 'http://localhost:9090/api/v1/query?query=chimera_queue_depth'
```

### Import Grafana Dashboard

1. Access Grafana at http://localhost:3001
2. Go to Dashboards → Import
3. Upload `ops/grafana/dashboard.json`

## Development Workflow

### Build Images

```bash
# Build all images
docker-compose build

# Build specific service
docker-compose build operator
```

### View Logs

```bash
# All services
docker-compose logs -f

# Specific service
docker-compose logs -f operator

# Last 100 lines
docker-compose logs --tail=100 operator
```

### Execute Commands

```bash
# Run scout manually
docker-compose run --rm scout python main.py --verbose

# Access operator shell
docker-compose exec operator sh

# Run tests
docker-compose run --rm operator cargo test
```

### Update Services

```bash
# Rebuild and restart
docker-compose up -d --build

# Restart specific service
docker-compose restart operator

# Update without downtime (zero-downtime deployment)
docker-compose up -d --no-deps --build operator
```

## Troubleshooting

### Services Won't Start

```bash
# Check logs
docker-compose logs

# Check service status
docker-compose ps

# Verify environment variables
docker-compose config
```

### Database Locked

```bash
# Check for locks
docker-compose exec operator sqlite3 /app/data/chimera.db "PRAGMA busy_timeout;"

# Restart operator
docker-compose restart operator
```

### RPC Connection Issues

```bash
# Test RPC from operator container
docker-compose exec operator curl -X POST ${CHIMERA_RPC__PRIMARY_URL} \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}'
```

### Port Conflicts

If ports are already in use, modify `docker-compose.yml`:

```yaml
ports:
  - "8081:8080"  # Change host port
```

## Security Considerations

### Production Deployment

1. **Change Default Passwords**
   - Grafana admin password
   - Webhook secrets
   - API keys

2. **Use Secrets Management**
   - Consider Docker secrets or external secret managers
   - Never commit `.env.*.local` files

3. **Network Security**
   - Use reverse proxy (nginx/traefik) for HTTPS
   - Restrict access to monitoring ports
   - Use firewall rules

4. **Backup Strategy**
   - Regular database backups
   - Encrypted backups for production
   - Test restore procedures

## Performance Tuning

### Resource Limits

Add to `docker-compose.yml`:

```yaml
services:
  operator:
    deploy:
      resources:
        limits:
          cpus: '2'
          memory: 2G
        reservations:
          cpus: '1'
          memory: 1G
```

### Database Optimization

```bash
# Enable WAL mode (already in schema)
docker-compose exec operator sqlite3 /app/data/chimera.db "PRAGMA journal_mode=WAL;"

# Optimize database
docker-compose exec operator sqlite3 /app/data/chimera.db "VACUUM; ANALYZE;"
```

## Cleanup

### Stop All Services

```bash
docker-compose --profile devnet down
docker-compose --profile mainnet-paper down
docker-compose --profile mainnet-prod down
```

### Remove Volumes (⚠️ Deletes Data)

```bash
docker-compose down -v
```

### Remove Images

```bash
docker-compose down --rmi all
```

## Additional Resources

- [Chimera README](../README.md)
- [Architecture Documentation](../docs/architecture.md)
- [Pre-Deployment Checklist](../docs/pre-deployment-checklist.md)
- [Runbooks](../ops/runbooks/)

## Support

For issues or questions:
1. Check logs: `docker-compose logs`
2. Review runbooks: `ops/runbooks/`
3. Check documentation: `docs/`
