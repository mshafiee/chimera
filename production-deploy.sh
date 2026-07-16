#!/bin/bash
set -e

PRODUCTION_SERVER="root@216.151.164.105"
PRODUCTION_DOMAIN="chimera-01.moez.tech"
DEPLOY_PATH="/opt/chimera"

echo "🚀 Production Deployment to ${PRODUCTION_DOMAIN}"

# Build images
echo "📦 Building production images..."
docker build -f tools/Dockerfile.geoip -t chimera-geoip-lookup:latest . 
docker build -f docker/haproxy/Dockerfile -t chimera-haproxy:latest .
docker build -f tools/Dockerfile.operator -t chimera-operator:latest .
docker build -f tools/Dockerfile.web -t chimera-web:latest .

# Build security services
echo "🔒 Building security services..."
for service in attack-detection policy-manager security-log-parser; do
    docker build -f tools/Dockerfile.${service} -t chimera-${service}:latest . || echo "Warning: ${service} build failed"
done

# Setup production environment
echo "⚙️  Setting up production environment..."
cat > docker/env.mainnet-prod.prod << 'ENV'
COMPOSE_PROFILE=mainnet-prod
SOLANA_NETWORK=mainnet
HELIUS_API_KEY=609cb910-17a5-4a76-9d1b-2ca9c42f759e
CHIMERA_RPC__PRIMARY_URL=https://mainnet.helius-rpc.com/?api-key=609cb910-17a5-4a76-9d1b-2ca9c42f759e
CHIMERA_RPC__FALLBACK_URL=https://api.mainnet-beta.solana.com
CHIMERA_RPC__RATE_LIMIT_PER_SECOND=40
CHIMERA_SECURITY__WEBHOOK_SECRET=704d221525125d91a9650d331ad3eb3acf075077d9a2bcb57ecdb54cd586617d
CHIMERA_ENV=mainnet-prod
CHIMERA_DEV_MODE=0
CHIMERA_JUPITER__API_KEY=jup_7e095c7e729dadb3070b15417faaeed98464afde972184fdd93e0b24247fc857
CHIMERA_MONITORING__HELIUS_WEBHOOK_URL=https://captivity-grunge-curtsy.ngrok-free.dev/api/v1/monitoring/helius-webhook
HAPROXY_STATS_PASSWORD=$(openssl rand -hex 16)
GRAFANA_PASSWORD=$(openssl rand -hex 16)
POSTGRES_PASSWORD=$(openssl rand -hex 32)
DATABASE_URL=postgresql://chimera:${POSTGRES_PASSWORD}@postgres:5432/chimera
PAPER_TRADE_MODE=false
ENV

# Deploy to server
echo "📤 Deploying to production server..."
ssh ${PRODUCTION_SERVER} "mkdir -p ${DEPLOY_PATH}/{data,ssl/certbot/letsencrypt,logs,policies}"

scp docker-compose.yml ${PRODUCTION_SERVER}:${DEPLOY_PATH}/
scp docker-compose-haproxy.yml ${PRODUCTION_SERVER}:${DEPLOY_PATH}/
scp docker/env.mainnet-prod.prod ${PRODUCTION_SERVER}:${DEPLOY_PATH}/.env
scp -r docker/haproxy ${PRODUCTION_SERVER}:${DEPLOY_PATH}/

# Setup secure monitoring authentication
echo "🔐 Setting up secure monitoring authentication..."
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

# Update SSL certificates if needed
echo "🔐 Updating SSL certificates..."
ssh ${PRODUCTION_SERVER} "
cd ${DEPLOY_PATH}
if [ ! -f ssl/certbot/letsencrypt/chimera.pem ]; then
  docker run --rm -v \${PWD}/ssl/certbot/letsencrypt:/etc/letsencrypt -p 80:80 certbot/certbot:latest certonly --standalone --email admin@${PRODUCTION_DOMAIN} --agree-tos --no-eff-email -d ${PRODUCTION_DOMAIN} --non-interactive || true
fi

if [ -f ssl/certbot/letsencrypt/live/${PRODUCTION_DOMAIN}/fullchain.pem ]; then
  cat ssl/certbot/letsencrypt/live/${PRODUCTION_DOMAIN}/fullchain.pem ssl/certbot/letsencrypt/live/${PRODUCTION_DOMAIN}/privkey.pem > ssl/certbot/letsencrypt/chimera.pem
  echo '✓ SSL certificates configured'
else
  echo '⚠ Using existing certificates'
fi
"

# Start services
echo "🔄 Starting production services..."
ssh ${PRODUCTION_SERVER} "
cd ${DEPLOY_PATH}
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml --profile mainnet-prod down 2>/dev/null || true
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml --profile mainnet-prod pull 2>/dev/null || true
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml --profile mainnet-prod up -d
sleep 45
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml --profile mainnet-prod ps
"

echo ""
echo "✅ Production deployment complete!"
echo "🌐 https://${PRODUCTION_DOMAIN}"
echo ""
echo "📊 Monitoring Credentials (save these securely!):"
echo "  Admin: ${MONITORING_ADMIN_PASS}"
echo "  Operator: ${MONITORING_OPERATOR_PASS}"
echo "  Viewer: ${MONITORING_VIEWER_PASS}"
echo ""
echo "🔗 Service Endpoints:"
echo "  Web Dashboard: https://${PRODUCTION_DOMAIN}/"
echo "  API: https://${PRODUCTION_DOMAIN}/api/v1/"
echo "  Grafana: https://${PRODUCTION_DOMAIN}/grafana/ (admin:${MONITORING_ADMIN_PASS})"
echo "  Prometheus: https://${PRODUCTION_DOMAIN}/prometheus/ (admin:${MONITORING_ADMIN_PASS})"
echo "  AlertManager: https://${PRODUCTION_DOMAIN}/alerts/ (admin:${MONITORING_ADMIN_PASS})"
echo ""
echo "🔍 Health Checks:"
echo "  Web: curl -I https://${PRODUCTION_DOMAIN}/"
echo "  API: curl https://${PRODUCTION_DOMAIN}/api/v1/health"
echo "  GeoIP: curl https://${PRODUCTION_DOMAIN}/api/v1/geoip/8.8.8.8"
