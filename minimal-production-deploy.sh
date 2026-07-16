#!/bin/bash
set -e

PRODUCTION_SERVER="root@216.151.164.105"
PRODUCTION_DOMAIN="chimera-01.moez.tech"
DEPLOY_PATH="/opt/chimera"

echo "🚀 Minimal Production Deployment to ${PRODUCTION_DOMAIN}"

# Build only working images
echo "📦 Building working images..."
docker build -f tools/Dockerfile.geoip -t chimera-geoip-lookup:latest . 2>&1 | tail -5
docker build -f docker/haproxy/Dockerfile -t chimera-haproxy:latest . 2>&1 | tail -5

# Transfer only working images
echo "📤 Transferring images..."
docker save chimera-geoip-lookup:latest chimera-haproxy:latest | ssh ${PRODUCTION_SERVER} "docker load"

# Setup env files
echo "⚙️  Setting up environment..."
cat > docker/env.mainnet-prod.prod << 'ENV'
COMPOSE_PROFILE=mainnet-prod
SOLANA_NETWORK=mainnet
CHIMERA_ENV=mainnet-prod
CHIMERA_DEV_MODE=0
PAPER_TRADE_MODE=false
HAPROXY_STATS_PASSWORD=stats123
GRAFANA_PASSWORD=grafana123
POSTGRES_PASSWORD=postgres123
DATABASE_URL=postgresql://chimera:postgres123@postgres:5432/chimera
REDIS_ENABLED=true
REDIS_URL=redis://redis:6379/0
ENV

cat > docker/env.mainnet-prod.local << 'ENV'
PRODUCTION_MODE=true
DEBUG_MODE=false
MONITORING_ENABLED=true
ENV

scp docker/env.mainnet-prod.prod ${PRODUCTION_SERVER}:${DEPLOY_PATH}/.env
scp docker/env.mainnet-prod.local ${PRODUCTION_SERVER}:${DEPLOY_PATH}/docker/

# Setup monitoring auth
cat > docker/haproxy/monitoring-auth.cfg << 'AUTH'
userlist monitoring_credentials
  admin admin123
  operator operator123
  viewer viewer123
AUTH

scp docker/haproxy/monitoring-auth.cfg ${PRODUCTION_SERVER}:${DEPLOY_PATH}/docker/haproxy/

# Deploy compose files
scp docker-compose.yml ${PRODUCTION_SERVER}:${DEPLOY_PATH}/
scp docker-compose-haproxy.yml ${PRODUCTION_SERVER}:${DEPLOY_PATH}/

# Start only core services
echo "🔄 Starting core services..."
ssh ${PRODUCTION_SERVER} "
cd ${DEPLOY_PATH}
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml --profile mainnet-prod down 2>/dev/null || true
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml --profile mainnet-prod up -d postgres redis operator web prometheus grafana alertmanager haproxy geoip-lookup certbot
sleep 30
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml --profile mainnet-prod ps
"

echo "✅ Core services deployed!"
echo "🌐 https://${PRODUCTION_DOMAIN}"
