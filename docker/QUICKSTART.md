# Quick Start Guide

Get Chimera running with Docker Compose in 5 minutes!

## Prerequisites

- Docker and Docker Compose installed
- At least 4GB RAM available

## Devnet (Fastest Start)

```bash
# 1. Initialize database
./docker/docker-compose.sh init-db devnet

# 2. Start services
./docker/docker-compose.sh start devnet

# 3. Check status
./docker/docker-compose.sh status devnet

# 4. View logs
./docker/docker-compose.sh logs devnet -f
```

**Access Points:**
- Operator API: http://localhost:8080
- Web Dashboard: http://localhost:3000
- Grafana: http://localhost:3001 (admin/admin)
- Prometheus: http://localhost:9090

## Mainnet Paper Trading

```bash
# 1. Configure environment
cp docker/env.mainnet-paper docker/env.mainnet-paper.local
# Edit docker/env.mainnet-paper.local and add your Helius API key

# 2. Initialize database
./docker/docker-compose.sh init-db mainnet-paper

# 3. Start services
./docker/docker-compose.sh start mainnet-paper
```

## Mainnet Production

⚠️ **WARNING: Uses REAL funds!**

```bash
# 1. Configure environment
cp docker/env.mainnet-prod docker/env.mainnet-prod.local
# REQUIRED: Edit and configure:
#   - Helius API key
#   - Webhook secret (generate: openssl rand -hex 32)
#   - Telegram/Discord notifications
#   - Secure Grafana password

# 2. Initialize database
./docker/docker-compose.sh init-db mainnet-prod

# 3. Start services
./docker/docker-compose.sh start mainnet-prod
```

## Common Commands

```bash
# View logs
./docker/docker-compose.sh logs <profile> -f

# Stop services
./docker/docker-compose.sh stop <profile>

# Restart services
./docker/docker-compose.sh restart <profile>

# Access operator shell
./docker/docker-compose.sh shell <profile> operator

# Check health
curl http://localhost:8080/api/v1/health
```

## Troubleshooting

**Services won't start:**
```bash
# Check logs
./docker/docker-compose.sh logs <profile>

# Verify configuration
docker-compose --profile <profile> config
```

**Database issues:**
```bash
# Reinitialize database (⚠️ deletes data)
rm -f data/chimera.db*
./docker/docker-compose.sh init-db <profile>
```

**Port conflicts:**
Edit `docker-compose.yml` and change port mappings.

## Next Steps

- Read [Full Documentation](../docker-compose.README.md)
- Configure notifications (Telegram/Discord)
- Set up monitoring alerts
- Review [Pre-Deployment Checklist](../docs/pre-deployment-checklist.md)
