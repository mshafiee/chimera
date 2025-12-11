#!/bin/bash
# Setup and Run Chimera in Mainnet Paper Trading Mode
# This mode uses real mainnet data but simulates trades (no real funds at risk)

set -e

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[✓]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[!]${NC} $1"
}

log_error() {
    echo -e "${RED}[✗]${NC} $1"
}

log_section() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo ""
}

# Check if Helius API key is configured
check_helius_key() {
    local env_file="docker/env.mainnet-paper.local"
    if [ -f "$env_file" ]; then
        if grep -q "YOUR_HELIUS_API_KEY" "$env_file" 2>/dev/null; then
            return 1
        fi
        if grep -q "CHIMERA_RPC__PRIMARY_URL.*helius" "$env_file" 2>/dev/null; then
            return 0
        fi
    fi
    
    # Check base file
    if grep -q "YOUR_HELIUS_API_KEY" docker/env.mainnet-paper 2>/dev/null; then
        return 1
    fi
    return 0
}

log_section "Chimera Mainnet Paper Trading Setup"

log_info "This will set up Chimera for paper trading on Mainnet."
log_info "Paper trading uses real mainnet data but simulates trades (no real funds)."
echo ""

# Step 1: Stop current services
log_section "Step 1: Stopping Current Services"
log_info "Checking for running devnet services..."

if docker ps --format "{{.Names}}" | grep -q "chimera-"; then
    log_warning "Found running Chimera services. Stopping..."
    ./docker/docker-compose.sh stop devnet 2>/dev/null || true
    sleep 2
    log_success "Services stopped"
else
    log_success "No running services found"
fi

# Step 2: Configure environment
log_section "Step 2: Configuring Environment"

ENV_FILE="docker/env.mainnet-paper.local"
if [ ! -f "$ENV_FILE" ]; then
    log_info "Creating local environment file from template..."
    cp docker/env.mainnet-paper "$ENV_FILE"
    log_success "Created $ENV_FILE"
else
    log_info "Local environment file already exists: $ENV_FILE"
fi

# Check for Helius API key
if ! check_helius_key; then
    log_warning "Helius API key not configured!"
    echo ""
    log_info "To get a Helius API key:"
    echo "  1. Visit: https://www.helius.dev/"
    echo "  2. Sign up for a free account"
    echo "  3. Get your API key from the dashboard"
    echo ""
    log_info "Then edit $ENV_FILE and replace YOUR_HELIUS_API_KEY with your actual key:"
    echo "  SOLANA_RPC_URL=https://mainnet.helius-rpc.com/?api-key=YOUR_KEY"
    echo "  CHIMERA_RPC__PRIMARY_URL=https://mainnet.helius-rpc.com/?api-key=YOUR_KEY"
    echo ""
    
    read -p "Do you want to configure the API key now? (y/n): " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        read -p "Enter your Helius API key: " HELIUS_KEY
        if [ -n "$HELIUS_KEY" ]; then
            sed -i.bak "s/YOUR_HELIUS_API_KEY/$HELIUS_KEY/g" "$ENV_FILE"
            log_success "Helius API key configured"
        else
            log_error "No API key provided"
            exit 1
        fi
    else
        log_warning "Skipping API key configuration. You'll need to edit $ENV_FILE manually."
        log_info "You can continue, but RPC calls will fail until the key is configured."
    fi
else
    log_success "Helius API key appears to be configured"
fi

# Generate webhook secret if needed
log_info "Checking webhook secret..."
if grep -q "mainnet-paper-webhook-secret-change-me" "$ENV_FILE" 2>/dev/null; then
    log_info "Generating secure webhook secret..."
    NEW_SECRET=$(openssl rand -hex 32)
    sed -i.bak "s/mainnet-paper-webhook-secret-change-me/$NEW_SECRET/g" "$ENV_FILE"
    log_success "Webhook secret generated"
else
    log_success "Webhook secret already configured"
fi

# Step 3: Initialize database
log_section "Step 3: Initializing Database"
log_info "Initializing database for mainnet-paper..."
if ./docker/docker-compose.sh init-db mainnet-paper; then
    log_success "Database initialized"
else
    log_error "Database initialization failed"
    exit 1
fi

# Step 4: Start services
log_section "Step 4: Starting Services"
log_info "Starting mainnet-paper services..."
if ./docker/docker-compose.sh start mainnet-paper; then
    log_success "Services started"
else
    log_error "Failed to start services"
    exit 1
fi

# Step 5: Wait for services to be ready
log_section "Step 5: Waiting for Services to be Ready"
log_info "Waiting 10 seconds for services to initialize..."
sleep 10

# Step 6: Verify services
log_section "Step 6: Verifying Services"

# Check operator health
log_info "Checking operator health..."
for i in {1..6}; do
    if curl -s http://localhost:8080/api/v1/health > /dev/null 2>&1; then
        log_success "Operator is healthy"
        break
    else
        if [ $i -eq 6 ]; then
            log_warning "Operator health check failed (may still be starting)"
        else
            log_info "Waiting for operator... ($i/6)"
            sleep 5
        fi
    fi
done

# Check other services
log_info "Checking service status..."
docker ps --format "table {{.Names}}\t{{.Status}}" | grep chimera || log_warning "No Chimera services found"

# Display service URLs
log_section "Service URLs"
echo "  Operator API:    http://localhost:8080"
echo "  Web Dashboard:  http://localhost:3000"
echo "  Grafana:        http://localhost:3002 (admin/change-me-secure-password)"
echo "  Prometheus:     http://localhost:9090"
echo ""

log_section "Setup Complete!"
log_success "Chimera is now running in Mainnet Paper Trading mode"
echo ""
log_info "Important Notes:"
echo "  • Paper trading mode: All trades are simulated (no real funds)"
echo "  • Real mainnet data: Uses actual mainnet blockchain data"
echo "  • Monitor logs: ./docker/docker-compose.sh logs mainnet-paper -f"
echo "  • Check health: curl http://localhost:8080/api/v1/health"
echo ""

log_info "To view logs:"
echo "  ./docker/docker-compose.sh logs mainnet-paper -f"
echo ""

log_info "To stop services:"
echo "  ./docker/docker-compose.sh stop mainnet-paper"
echo ""
