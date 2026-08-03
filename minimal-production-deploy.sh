#!/bin/bash
set -euo pipefail

PRODUCTION_SERVER="root@216.151.164.105"
PRODUCTION_DOMAIN="chimera-01.moez.tech"
DEPLOY_PATH="/opt/chimera"

TMPDIR_OVERRIDE=$(mktemp -d)
trap 'rm -rf "$TMPDIR_OVERRIDE"' EXIT

echo "🚀 Minimal Production Deployment to ${PRODUCTION_DOMAIN}"

# Build only working images
echo "📦 Building working images..."
docker build -f tools/Dockerfile.geoip -t chimera-geoip-lookup:latest . 2>&1 | tail -5
docker image inspect chimera-geoip-lookup:latest > /dev/null
docker build -f docker/haproxy/Dockerfile -t chimera-haproxy:latest . 2>&1 | tail -5
docker image inspect chimera-haproxy:latest > /dev/null

# Transfer only working images
echo "📤 Transferring images..."
docker save chimera-geoip-lookup:latest chimera-haproxy:latest | ssh ${PRODUCTION_SERVER} "docker load"

# Generate per-deploy credentials (never hardcoded)
HAPROXY_STATS_PASSWORD=$(openssl rand -hex 16)
GRAFANA_PASSWORD=$(openssl rand -hex 16)
POSTGRES_PASSWORD=$(openssl rand -hex 32)
DATABASE_URL="postgresql://chimera:${POSTGRES_PASSWORD}@postgres:5432/chimera"
MONITORING_ADMIN_PASS=$(openssl rand -hex 16)
MONITORING_OPERATOR_PASS=$(openssl rand -hex 16)
MONITORING_VIEWER_PASS=$(openssl rand -hex 16)

# Ensure remote directory layout exists before any scp
ssh ${PRODUCTION_SERVER} "mkdir -p ${DEPLOY_PATH}/docker/haproxy"

# Setup env files in a throwaway directory (secrets never touch the repo tree)
echo "⚙️  Setting up environment..."
cat > "$TMPDIR_OVERRIDE/env.mainnet-prod.prod" << ENV
COMPOSE_PROFILE=mainnet-prod
SOLANA_NETWORK=mainnet
CHIMERA_ENV=mainnet-prod
CHIMERA_DEV_MODE=0
PAPER_TRADE_MODE=false
POSTGRES_USER=chimera
HAPROXY_STATS_PASSWORD=${HAPROXY_STATS_PASSWORD}
GRAFANA_PASSWORD=${GRAFANA_PASSWORD}
POSTGRES_PASSWORD=${POSTGRES_PASSWORD}
DATABASE_URL=${DATABASE_URL}
REDIS_ENABLED=true
REDIS_URL=redis://redis:6379/0
ENV

cat > "$TMPDIR_OVERRIDE/env.mainnet-prod.local" << 'ENV'
PRODUCTION_MODE=true
DEBUG_MODE=false
MONITORING_ENABLED=true
ENV

chmod 600 "$TMPDIR_OVERRIDE/env.mainnet-prod.prod"
scp "$TMPDIR_OVERRIDE/env.mainnet-prod.prod" ${PRODUCTION_SERVER}:${DEPLOY_PATH}/.env
ssh ${PRODUCTION_SERVER} "chmod 600 ${DEPLOY_PATH}/.env"
scp "$TMPDIR_OVERRIDE/env.mainnet-prod.local" ${PRODUCTION_SERVER}:${DEPLOY_PATH}/docker/

# Setup monitoring auth with strong random credentials
cat > "$TMPDIR_OVERRIDE/monitoring-auth.cfg" << AUTH
userlist monitoring_credentials
  user admin insecure-password ${MONITORING_ADMIN_PASS}
  user operator insecure-password ${MONITORING_OPERATOR_PASS}
  user viewer insecure-password ${MONITORING_VIEWER_PASS}
AUTH
chmod 600 "$TMPDIR_OVERRIDE/monitoring-auth.cfg"
scp "$TMPDIR_OVERRIDE/monitoring-auth.cfg" ${PRODUCTION_SERVER}:${DEPLOY_PATH}/docker/haproxy/

# Deploy compose files
scp docker-compose.yml ${PRODUCTION_SERVER}:${DEPLOY_PATH}/
scp docker-compose-haproxy.yml ${PRODUCTION_SERVER}:${DEPLOY_PATH}/

# Start only core services
echo "🔄 Starting core services..."
ssh ${PRODUCTION_SERVER} "
set -e
cd ${DEPLOY_PATH}
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml --profile mainnet-prod up -d postgres redis operator web prometheus grafana alertmanager haproxy geoip-lookup certbot
"

# Verify health with a polling loop instead of a blind sleep
echo "⏳ Waiting for services to become healthy..."
HEALTHY=0
for i in $(seq 1 30); do
    if ssh ${PRODUCTION_SERVER} "curl -sf --max-time 5 http://localhost:8080/api/v1/health > /dev/null 2>&1"; then
        HEALTHY=1
        break
    fi
    sleep 5
done

ssh ${PRODUCTION_SERVER} "cd ${DEPLOY_PATH} && docker compose -f docker-compose.yml -f docker-compose-haproxy.yml --profile mainnet-prod ps"

if [ "$HEALTHY" -ne 1 ]; then
    echo "❌ Core services did not become healthy in time" >&2
    exit 1
fi

echo "✅ Core services deployed!"
echo "🌐 https://${PRODUCTION_DOMAIN}"
