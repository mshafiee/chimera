#!/bin/bash
set -e

PRODUCTION_SERVER="root@216.151.164.105"
PRODUCTION_DOMAIN="chimera-01.moez.tech"
DEPLOY_PATH="/opt/chimera"

echo "🚀 Final Production Deployment to ${PRODUCTION_DOMAIN}"

# Build custom images locally
echo "📦 Building custom images locally..."
docker build -f tools/Dockerfile.geoip -t chimera-geoip-lookup:latest . 
docker build -f docker/haproxy/Dockerfile -t chimera-haproxy:latest .

# Build security services
for service in attack-detection policy-manager security-log-parser; do
    echo "Building ${service}..."
    docker build -f tools/Dockerfile.${service} -t chimera-${service}:latest . 2>/dev/null || echo "Skipping ${service}"
done

# Push/save images to transfer
echo "📤 Transferring images to production server..."
docker save chimera-geoip-lookup:latest chimera-haproxy:latest chimera-operator:latest chimera-web:latest chimera-scout:latest | ssh ${PRODUCTION_SERVER} "docker load"

# Ensure env file exists
echo "⚙️  Ensuring environment files..."
cat > docker/env.mainnet-prod.local << 'ENV'
# Local Production Environment
PRODUCTION_MODE=true
DEBUG_MODE=false
LOG_LEVEL=info
HAPROXY_STATS_ENABLED=true
MONITORING_ENABLED=true
ALERTING_ENABLED=true
ENV

scp docker/env.mainnet-prod.local ${PRODUCTION_SERVER}:${DEPLOY_PATH}/docker/

# Setup monitoring authentication if not exists
echo "🔐 Setting up monitoring authentication..."
if ! ssh ${PRODUCTION_SERVER} "[ -f ${DEPLOY_PATH}/docker/haproxy/monitoring-auth.cfg ]"; then
    MONITORING_ADMIN_PASS=$(openssl rand -hex 32)
    MONITORING_OPERATOR_PASS=$(openssl rand -hex 32)
    MONITORING_VIEWER_PASS=$(openssl rand -hex 32)
    
    cat > docker/haproxy/monitoring-auth.cfg << AUTH
userlist monitoring_credentials
  admin ${MONITORING_ADMIN_PASS}
  operator ${MONITORING_OPERATOR_PASS}
  viewer ${MONITORING_VIEWER_PASS}
AUTH
    
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
cd ${DEPLOY_PATH}
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml --profile mainnet-prod down 2>/dev/null || true
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml --profile mainnet-prod up -d
sleep 60
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml --profile mainnet-prod ps
"

echo "✅ Production deployment complete!"
echo "🌐 https://${PRODUCTION_DOMAIN}"
echo "🔍 Health Checks:"
echo "  Web: curl -I https://${PRODUCTION_DOMAIN}/"
echo "  API: curl https://${PRODUCTION_DOMAIN}/api/v1/health"
echo "  HAProxy Stats: http://${PRODUCTION_DOMAIN}:8404/stats"
