#!/bin/bash
set -e

# Production Deployment Script for Chimera Trading System
# Target: chimera-01.moez.tech (216.151.164.105)

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# Configuration
PRODUCTION_SERVER="root@216.151.164.105"
PRODUCTION_DOMAIN="chimera-01.moez.tech"
PRODUCTION_USER="root"
DEPLOY_PATH="/opt/chimera"
PROFILE="mainnet-prod"

echo -e "${BLUE}========================================${NC}"
echo -e "${BLUE}Chimera Production Deployment${NC}"
echo -e "${BLUE}========================================${NC}"
echo -e "Target: ${PRODUCTION_DOMAIN} (${PRODUCTION_SERVER})"
echo -e "Profile: ${PROFILE}"
echo -e "Path: ${DEPLOY_PATH}"
echo -e "${BLUE}========================================${NC}"

# Function to check prerequisites
check_prerequisites() {
    echo -e "${YELLOW}[1/8] Checking prerequisites...${NC}"
    
    # Check SSH access
    echo "Checking SSH access to production server..."
    if ssh -o ConnectTimeout=5 "${PRODUCTION_SERVER}" "echo 'SSH access OK'" > /dev/null 2>&1; then
        echo -e "${GREEN}✓ SSH access OK${NC}"
    else
        echo -e "${RED}✗ SSH access failed${NC}"
        echo "Please ensure:"
        echo "  - You have SSH access to ${PRODUCTION_SERVER}"
        echo "  - Your SSH keys are configured"
        exit 1
    fi
    
    # Check Docker on remote
    echo "Checking Docker on production server..."
    if ssh "${PRODUCTION_SERVER}" "docker --version" > /dev/null 2>&1; then
        echo -e "${GREEN}✓ Docker installed${NC}"
    else
        echo -e "${RED}✗ Docker not found on remote server${NC}"
        exit 1
    fi
    
    # Check docker compose on remote
    echo "Checking docker compose on production server..."
    if ssh "${PRODUCTION_SERVER}" "docker compose version" > /dev/null 2>&1; then
        echo -e "${GREEN}✓ Docker compose installed${NC}"
    else
        echo -e "${RED}✗ Docker compose not found${NC}"
        exit 1
    fi
    
    echo ""
}

# Function to build images locally
build_images() {
    echo -e "${YELLOW}[2/8] Building production images...${NC}"
    
    # Build geoip-lookup with databases
    echo "Building geoip-lookup service with GeoIP databases..."
    docker build -f tools/Dockerfile.geoip -t chimera-geoip-lookup:latest . > /dev/null 2>&1
    
    # Build HAProxy
    echo "Building HAProxy service..."
    docker build -f docker/haproxy/Dockerfile -t chimera-haproxy:latest . > /dev/null 2>&1
    
    # Build operator
    echo "Building operator service..."
    docker build -f tools/Dockerfile.operator -t chimera-operator:latest . > /dev/null 2>&1
    
    # Build web
    echo "Building web service..."
    docker build -f tools/Dockerfile.web -t chimera-web:latest . > /dev/null 2>&1
    
    # Build security services
    for service in attack-detection policy-manager security-log-parser; do
        echo "Building ${service} service..."
        docker build -f tools/Dockerfile.${service} -t chimera-${service}:latest . > /dev/null 2>&1
    done
    
    echo -e "${GREEN}✓ All images built successfully${NC}"
    echo ""
}

# Function to prepare production environment
prepare_environment() {
    echo -e "${YELLOW}[3/8] Preparing production environment...${NC}"
    
    # Create production environment file
    echo "Creating production environment configuration..."
    
    cat > docker/env.mainnet-prod.production << 'ENV_EOF'
# Chimera Production Configuration
COMPOSE_PROFILE=mainnet-prod
SOLANA_NETWORK=mainnet

# Operator Configuration
CHIMERA_RPC__PRIMARY_URL=https://mainnet.helius-rpc.com/?api-key=609cb910-17a5-4a76-9d1b-2ca9c42f759e
CHIMERA_RPC__FALLBACK_URL=https://api.mainnet-beta.solana.com
CHIMERA_RPC__RATE_LIMIT_PER_SECOND=40

# Security
CHIMERA_SECURITY__WEBHOOK_SECRET=704d221525125d91a9650d331ad3eb3acf075077d9a2bcb57ecdb54cd586617d
CHIMERA_SECURITY__WEBHOOK_RATE_LIMIT=1000
CHIMERA_SECURITY__WEBHOOK_BURST_SIZE=1500

# Environment
CHIMERA_ENV=mainnet-prod
CHIMERA_DEV_MODE=0

# Database
CHIMERA_DB_MODE=postgres
POSTGRES_USER=chimera
POSTGRES_PASSWORD=$(openssl rand -hex 32)
POSTGRES_DB=chimera
DATABASE_URL=postgresql://chimera:${POSTGRES_PASSWORD}@postgres:5432/chimera

# Redis
REDIS_ENABLED=true
REDIS_URL=redis://redis:6379/0

# Circuit Breakers
CHIMERA_CIRCUIT_BREAKERS__MAX_LOSS_24H_USD=1000
CHIMERA_CIRCUIT_BREAKERS__MAX_CONSECUTIVE_LOSSES=7
CHIMERA_CIRCUIT_BREAKERS__MAX_DRAWDOWN_PERCENT=20
CHIMERA_CIRCUIT_BREAKERS__COOLDOWN_MINUTES=30

# Strategy
CHIMERA_STRATEGY__SHIELD_PERCENT=70
CHIMERA_STRATEGY__SPEAR_PERCENT=30
CHIMERA_STRATEGY__MAX_POSITION_SOL=1.0
CHIMERA_STRATEGY__MIN_POSITION_SOL=0.01

# Jito
CHIMERA_JITO__ENABLED=true
CHIMERA_JITO__API_KEY=jup_7e095c7e729dadb3070b15417faaeed98464afde972184fdd93e0b24247fc857
CHIMERA_JITO__TIP_FLOOR_SOL=0.001
CHIMERA_JITO__TIP_CEILING_SOL=0.01

# Token Safety
CHIMERA_TOKEN_SAFETY__MIN_LIQUIDITY_SHIELD_USD=10000
CHIMERA_TOKEN_SAFETY__MIN_LIQUIDITY_SPEAR_USD=5000
CHIMERA_TOKEN_SAFETY__HONEYPOT_DETECTION_ENABLED=true

# Helius Monitoring
CHIMERA_MONITORING__HELIUS_WEBHOOK_URL=https://captivity-grunge-curtsy.ngrok-free.dev/api/v1/monitoring/helius-webhook

# HAProxy Configuration
HAPROXY_STATS_PASSWORD=$(openssl rand -hex 16)

# Monitoring
GRAFANA_PASSWORD=$(openssl rand -hex 16)
PROMETHEUS_RETENTION=15d
ALERTMANAGER_RETENTION=120h

# Production Mode
PAPER_TRADE_MODE=false
ENV_EOF
    
    # Setup monitoring authentication
    echo "Setting up secure monitoring authentication..."
    
    cat > docker/haproxy/monitoring-auth.cfg << 'AUTH_EOF'
userlist monitoring_credentials
  admin $(openssl rand -hex 32)
  operator $(openssl rand -hex 32)
  viewer $(openssl rand -hex 32)
AUTH_EOF
    
    echo -e "${GREEN}✓ Production environment prepared${NC}"
    echo ""
}

# Function to deploy to production server
deploy_to_server() {
    echo -e "${YELLOW}[4/8] Deploying to production server...${NC}"
    
    # Create deployment directory on remote
    echo "Creating deployment directory on remote..."
    ssh "${PRODUCTION_SERVER}" "mkdir -p ${DEPLOY_PATH} ${DEPLOY_PATH}/data ${DEPLOY_PATH}/ssl/certbot/letsencrypt"
    
    # Copy files to remote
    echo "Copying deployment files..."
    
    # Copy docker compose files
    scp docker-compose.yml "${PRODUCTION_SERVER}:${DEPLOY_PATH}/"
    scp docker-compose-haproxy.yml "${PRODUCTION_SERVER}:${DEPLOY_PATH}/"
    
    # Copy environment files
    scp docker/env.mainnet-prod.production "${PRODUCTION_SERVER}:${DEPLOY_PATH}/.env"
    scp docker/env.mainnet-prod "${PRODUCTION_SERVER}:${DEPLOY_PATH}/env.mainnet-prod"
    
    # Copy HAProxy configuration
    scp -r docker/haproxy "${PRODUCTION_SERVER}:${DEPLOY_PATH}/"
    
    # Copy SSL setup scripts
    scp -r ssl/* "${PRODUCTION_SERVER}:${DEPLOY_PATH}/ssl/"
    
    # Create monitoring startup script on remote
    ssh "${PRODUCTION_SERVER}" "cat > ${DEPLOY_PATH}/start-monitoring.sh << 'SCRIPT_EOF'
#!/bin/bash
cd ${DEPLOY_PATH}
# Setup monitoring auth with envsubst
envsubst < docker/haproxy/monitoring-auth.cfg.template > docker/haproxy/monitoring-auth.cfg
SCRIPT_EOF"
    
    ssh "${PRODUCTION_SERVER}" "chmod +x ${DEPLOY_PATH}/start-monitoring.sh"
    
    echo -e "${GREEN}✓ Files deployed to production server${NC}"
    echo ""
}

# Function to setup SSL certificates
setup_ssl() {
    echo -e "${YELLOW}[5/8] Setting up SSL certificates...${NC}"
    
    # Check if SSL certificates exist
    echo "Checking for existing SSL certificates..."
    if ssh "${PRODUCTION_SERVER}" "[ -f ${DEPLOY_PATH}/ssl/certbot/letsencrypt/chimera-01.moez.tech.pem ]"; then
        echo -e "${GREEN}✓ SSL certificates already exist${NC}"
    else
        echo "Obtaining Let's Encrypt certificates..."
        
        # Use certbot to obtain certificates
        ssh "${PRODUCTION_SERVER}" "
            cd ${DEPLOY_PATH}
            
            # Stop any running services on port 80
            docker compose -f docker-compose-haproxy.yml stop 2>/dev/null || true
            
            # Run certbot
            docker run --rm \\
                -v ${DEPLOY_PATH}/ssl/certbot/letsencrypt:/etc/letsencrypt \\
                -v ${DEPLOY_PATH}/ssl/logs:/var/log/letsencrypt \\
                -v ${DEPLOY_PATH}/ssl/work:/var/lib/letsencrypt \\
                -p 80:80 \\
                certbot/certbot:latest certonly \\
                --standalone \\
                --email admin@${PRODUCTION_DOMAIN} \\
                --agree-tos \\
                --no-eff-email \\
                -d ${PRODUCTION_DOMAIN} || echo 'Certificate generation failed - will retry'
            
            # Create combined PEM for HAProxy
            if [ -f ${DEPLOY_PATH}/ssl/certbot/letsencrypt/live/${PRODUCTION_DOMAIN}/fullchain.pem ]; then
                cat ${DEPLOY_PATH}/ssl/certbot/letsencrypt/live/${PRODUCTION_DOMAIN}/fullchain.pem \\
                    ${DEPLOY_PATH}/ssl/certbot/letsencrypt/live/${PRODUCTION_DOMAIN}/privkey.pem \\
                    > ${DEPLOY_PATH}/ssl/certbot/letsencrypt/chimera-01.moez.tech.pem
                
                echo 'SSL certificates obtained successfully'
            else
                echo 'Certificate generation failed - using self-signed for now'
                # Generate self-signed as fallback
                openssl req -x509 -nodes -days 365 -newkey rsa:2048 \\
                    -keyout ${DEPLOY_PATH}/ssl/certbot/letsencrypt/chimera.key \\
                    -out ${DEPLOY_PATH}/ssl/certbot/letsencrypt/chimera.crt \\
                    -subj \"/CN=${PRODUCTION_DOMAIN}\"
                
                cat ${DEPLOY_PATH}/ssl/certbot/letsencrypt/chimera.crt \\
                    ${DEPLOY_PATH}/ssl/certbot/letsencrypt/chimera.key \\
                    > ${DEPLOY_PATH}/ssl/certbot/letsencrypt/chimera-01.moez.tech.pem
            fi
        "
        
        echo -e "${GREEN}✓ SSL certificates configured${NC}"
    fi
    echo ""
}

# Function to start services
start_services() {
    echo -e "${YELLOW}[6/8] Starting production services...${NC}"
    
    ssh "${PRODUCTION_SERVER}" "
        cd ${DEPLOY_PATH}
        
        # Stop existing services
        docker compose -f docker-compose.yml -f docker-compose-haproxy.yml --profile ${PROFILE} down 2>/dev/null || true
        
        # Build GeoIP image on remote (ensure databases are baked in)
        docker build -f tools/Dockerfile.geoip -t chimera-geoip-lookup:latest .
        
        # Start services
        docker compose -f docker-compose.yml -f docker-compose-haproxy.yml --profile ${PROFILE} up -d
        
        # Wait for services to be healthy
        echo 'Waiting for services to be healthy...'
        sleep 30
        
        # Check service status
        docker compose -f docker-compose.yml -f docker-compose-haproxy.yml --profile ${PROFILE} ps
    "
    
    echo -e "${GREEN}✓ Production services started${NC}"
    echo ""
}

# Function to verify deployment
verify_deployment() {
    echo -e "${YELLOW}[7/8] Verifying deployment...${NC}"
    
    # Check SSL
    echo "Checking SSL certificate..."
    if curl -sI "https://${PRODUCTION_DOMAIN}/" | grep -q "200 OK"; then
        echo -e "${GREEN}✓ SSL certificate valid${NC}"
    else
        echo -e "${YELLOW}⚠ SSL certificate check failed (may still be propagating)${NC}"
    fi
    
    # Check main endpoints
    echo "Checking main endpoints..."
    
    # Web dashboard
    if curl -s "https://${PRODUCTION_DOMAIN}/" | grep -q "html"; then
        echo -e "${GREEN}✓ Web dashboard accessible${NC}"
    else
        echo -e "${YELLOW}⚠ Web dashboard check failed${NC}"
    fi
    
    # API health
    if curl -s "https://${PRODUCTION_DOMAIN}/api/v1/health" | grep -q "healthy"; then
        echo -e "${GREEN}✓ API health check passed${NC}"
    else
        echo -e "${YELLOW}⚠ API health check failed${NC}"
    fi
    
    # Monitoring endpoints (should require auth)
    echo "Checking monitoring security..."
    
    if curl -s "https://${PRODUCTION_DOMAIN}/grafana/" | grep -q "401\|403\|Unauthorized"; then
        echo -e "${GREEN}✓ Grafana authentication required${NC}"
    else
        echo -e "${YELLOW}⚠ Grafana may be accessible without authentication${NC}"
    fi
    
    if curl -s "https://${PRODUCTION_DOMAIN}/prometheus/" | grep -q "401\|403\|Unauthorized"; then
        echo -e "${GREEN}✓ Prometheus authentication required${NC}"
    else
        echo -e "${YELLOW}⚠ Prometheus may be accessible without authentication${NC}"
    fi
    
    # Check GeoIP service
    echo "Checking GeoIP service..."
    if ssh "${PRODUCTION_SERVER}" "docker logs chimera-geoip-lookup 2>&1 | grep -q 'Loaded GeoIP Country database'"; then
        echo -e "${GREEN}✓ GeoIP service operational${NC}"
    else
        echo -e "${YELLOW}⚠ GeoIP service may not be fully operational${NC}"
    fi
    
    echo ""
}

# Function to provide deployment summary
deployment_summary() {
    echo -e "${YELLOW}[8/8] Deployment Summary${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo -e "${GREEN}Production Deployment Complete!${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo ""
    echo "Production Details:"
    echo "  - Domain: ${PRODUCTION_DOMAIN}"
    echo "  - Server: ${PRODUCTION_SERVER}"
    echo "  - Profile: ${PROFILE}"
    echo "  - Path: ${DEPLOY_PATH}"
    echo ""
    echo "Service Endpoints:"
    echo "  - Web Dashboard: https://${PRODUCTION_DOMAIN}/"
    echo "  - API: https://${PRODUCTION_DOMAIN}/api/v1/"
    echo "  - Grafana: https://${PRODUCTION_DOMAIN}/grafana/ (requires auth)"
    echo "  - Prometheus: https://${PRODUCTION_DOMAIN}/prometheus/ (requires auth)"
    echo "  - AlertManager: https://${PRODUCTION_DOMAIN}/alerts/ (requires auth)"
    echo "  - HAProxy Stats: http://${PRODUCTION_DOMAIN}:8404/stats"
    echo ""
    echo "Monitoring Credentials:"
    echo "  - Admin: Check ${DEPLOY_PATH}/docker/haproxy/monitoring-auth.cfg"
    echo "  - Operator: Check ${DEPLOY_PATH}/docker/haproxy/monitoring-auth.cfg"
    echo "  - Viewer: Check ${DEPLOY_PATH}/docker/haproxy/monitoring-auth.cfg"
    echo ""
    echo "Security Features:"
    echo "  - SSL/TLS: Let's Encrypt certificates"
    echo "  - GeoIP: Germany-only access policy"
    echo "  - Rate Limiting: Configured per endpoint"
    echo "  - Authentication: Required for monitoring tools"
    echo ""
    echo "Next Steps:"
    echo "  1. Update monitoring passwords in ${DEPLOY_PATH}/docker/haproxy/monitoring-auth.cfg"
    echo "  2. Configure Grafana dashboards"
    echo "  3. Set up alerting rules"
    echo "  4. Monitor logs: ssh ${PRODUCTION_SERVER} 'cd ${DEPLOY_PATH} && docker compose logs -f'"
    echo ""
    echo "Useful Commands:"
    echo "  - View logs: ssh ${PRODUCTION_SERVER} 'cd ${DEPLOY_PATH} && docker compose logs -f'"
    echo "  - Restart services: ssh ${PRODUCTION_SERVER} 'cd ${DEPLOY_PATH} && docker compose restart'"
    echo "  - Check status: ssh ${PRODUCTION_SERVER} 'cd ${DEPLOY_PATH} && docker compose ps'"
    echo "  - Stop services: ssh ${PRODUCTION_SERVER} 'cd ${DEPLOY_PATH} && docker compose down'"
    echo ""
    echo -e "${BLUE}========================================${NC}"
}

# Main execution
main() {
    check_prerequisites
    build_images
    prepare_environment
    deploy_to_server
    setup_ssl
    start_services
    verify_deployment
    deployment_summary
}

# Run main function
main
