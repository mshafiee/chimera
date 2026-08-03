#!/bin/bash
set -e
set -o pipefail

PRODUCTION_SERVER="root@216.151.164.105"
PRODUCTION_DOMAIN="chimera-01.moez.tech"
DEPLOY_PATH="/opt/chimera"

echo "🚀 Final Production Deployment to ${PRODUCTION_DOMAIN}"

# Ensure remote directory layout exists before any scp
ssh ${PRODUCTION_SERVER} "mkdir -p ${DEPLOY_PATH}/docker/haproxy"

# Build custom images locally
echo "📦 Building custom images locally..."
docker build -f tools/Dockerfile.geoip -t chimera-geoip-lookup:latest .
docker build -f docker/haproxy/Dockerfile -t chimera-haproxy:latest .

# Build security services (fail loudly on error)
for service in attack-detection policy-manager security-log-parser; do
    echo "Building ${service}..."
    docker build -f tools/Dockerfile.${service} -t chimera-${service}:latest .
done

# Push/save images to transfer (exactly the images built above)
echo "📤 Transferring images to production server..."
docker save chimera-geoip-lookup:latest chimera-haproxy:latest chimera-attack-detection:latest chimera-policy-manager:latest chimera-security-log-parser:latest | ssh ${PRODUCTION_SERVER} "docker load"

# Ensure env file exists (create-only; never clobber existing overrides)
echo "⚙️  Ensuring environment files..."
if [ ! -f docker/env.mainnet-prod.local ]; then
    cat > docker/env.mainnet-prod.local << 'ENV'
# Local Production Environment
PRODUCTION_MODE=true
DEBUG_MODE=false
LOG_LEVEL=info
HAPROXY_STATS_ENABLED=true
MONITORING_ENABLED=true
ALERTING_ENABLED=true
ENV
fi

scp docker/env.mainnet-prod.local ${PRODUCTION_SERVER}:${DEPLOY_PATH}/docker/

# Setup monitoring authentication if not exists
echo "🔐 Setting up monitoring authentication..."
if ! ssh ${PRODUCTION_SERVER} "[ -f ${DEPLOY_PATH}/docker/haproxy/monitoring-auth.cfg ]"; then
    MONITORING_ADMIN_PASS=$(openssl rand -hex 32)
    MONITORING_OPERATOR_PASS=$(openssl rand -hex 32)
    MONITORING_VIEWER_PASS=$(openssl rand -hex 32)

    umask 077
    cat > docker/haproxy/monitoring-auth.cfg << AUTH
userlist monitoring_credentials
  user admin insecure-password ${MONITORING_ADMIN_PASS}
  user operator insecure-password ${MONITORING_OPERATOR_PASS}
  user viewer insecure-password ${MONITORING_VIEWER_PASS}
AUTH
    chmod 600 docker/haproxy/monitoring-auth.cfg

    scp docker/haproxy/monitoring-auth.cfg ${PRODUCTION_SERVER}:${DEPLOY_PATH}/docker/haproxy/
    echo "Monitoring credentials generated"
else
    echo "Monitoring authentication already exists"
fi

# Deploy compose files
echo "📤 Deploying compose files..."
scp docker-compose.yml ${PRODUCTION_SERVER}:${DEPLOY_PATH}/
scp docker-compose-haproxy.yml ${PRODUCTION_SERVER}:${DEPLOY_PATH}/

# Start services
echo "🔄 Starting production services..."
ssh ${PRODUCTION_SERVER} "
set -e
cd ${DEPLOY_PATH}
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml --profile mainnet-prod up -d --remove-orphans
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
    echo "❌ Production deployment did not become healthy in time" >&2
    exit 1
fi

echo "✅ Production deployment complete!"
echo "🌐 https://${PRODUCTION_DOMAIN}"
echo "🔍 Health Checks:"
echo "  Web: curl -I https://${PRODUCTION_DOMAIN}/"
echo "  API: curl https://${PRODUCTION_DOMAIN}/api/v1/health"
echo "  HAProxy Stats: http://${PRODUCTION_DOMAIN}:8404/stats"
