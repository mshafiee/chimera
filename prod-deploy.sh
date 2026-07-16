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

# Setup production environment
echo "⚙️  Setting up production environment..."
cat > docker/env.mainnet-prod.prod << 'ENV'
COMPOSE_PROFILE=mainnet-prod
SOLANA_NETWORK=mainnet
CHIMERA_RPC__PRIMARY_URL=https://mainnet.helius-rpc.com/?api-key=609cb910-17a5-4a76-9d1b-2ca9c42f759e
CHIMERA_RPC__FALLBACK_URL=https://api.mainnet-beta.solana.com
CHIMERA_SECURITY__WEBHOOK_SECRET=704d221525125d91a9650d331ad3eb3acf075077d9a2bcb57ecdb54cd586617d
CHIMERA_ENV=mainnet-prod
CHIMERA_DEV_MODE=0
HAPROXY_STATS_PASSWORD=$(openssl rand -hex 16)
GRAFANA_PASSWORD=$(openssl rand -hex 16)
POSTGRES_PASSWORD=$(openssl rand -hex 32)
DATABASE_URL=postgresql://chimera:${POSTGRES_PASSWORD}@postgres:5432/chimera
CHIMERA_JUPITER__API_KEY=jup_7e095c7e729dadb3070b15417faaeed98464afde972184fdd93e0b24247fc857
PAPER_TRADE_MODE=false
ENV

# Deploy to server
echo "📤 Deploying to production server..."
ssh ${PRODUCTION_SERVER} "mkdir -p ${DEPLOY_PATH}/{data,ssl/certbot/letsencrypt,logs}"

scp docker-compose.yml ${PRODUCTION_SERVER}:${DEPLOY_PATH}/
scp docker-compose-haproxy.yml ${PRODUCTION_SERVER}:${DEPLOY_PATH}/
scp docker/env.mainnet-prod.prod ${PRODUCTION_SERVER}:${DEPLOY_PATH}/.env
scp -r docker/haproxy ${PRODUCTION_SERVER}:${DEPLOY_PATH}/

# Setup SSL certificates
echo "🔐 Setting up SSL certificates..."
ssh ${PRODUCTION_SERVER} "
cd ${DEPLOY_PATH}
docker run --rm -v \${PWD}/ssl/certbot/letsencrypt:/etc/letsencrypt -p 80:80 certbot/certbot:latest certonly --standalone --email admin@${PRODUCTION_DOMAIN} --agree-tos --no-eff-email -d ${PRODUCTION_DOMAIN} || true

if [ -f ssl/certbot/letsencrypt/live/${PRODUCTION_DOMAIN}/fullchain.pem ]; then
  cat ssl/certbot/letsencrypt/live/${PRODUCTION_DOMAIN}/fullchain.pem ssl/certbot/letsencrypt/live/${PRODUCTION_DOMAIN}/privkey.pem > ssl/certbot/letsencrypt/chimera.pem
  echo '✓ SSL certificates obtained'
else
  openssl req -x509 -nodes -days 365 -newkey rsa:2048 -keyout ssl/certbot/letsencrypt/chimera.key -out ssl/certbot/letsencrypt/chimera.crt -subj '/CN=${PRODUCTION_DOMAIN}'
  cat ssl/certbot/letsencrypt/chimera.crt ssl/certbot/letsencrypt/chimera.key > ssl/certbot/letsencrypt/chimera.pem
  echo '✓ Self-signed certificates generated'
fi
"

# Start services
echo "🔄 Starting production services..."
ssh ${PRODUCTION_SERVER} "
cd ${DEPLOY_PATH}
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml --profile mainnet-prod down 2>/dev/null || true
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml --profile mainnet-prod up -d
sleep 30
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml --profile mainnet-prod ps
"

echo "✅ Production deployment complete!"
echo "🌐 https://${PRODUCTION_DOMAIN}"
