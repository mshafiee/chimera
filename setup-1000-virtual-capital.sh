#!/bin/bash
# Setup Chimera for $1000 Virtual Capital Paper Trading
# This configures the system for risk-free testing with simulated $1000 capital

set -e

# Error handler
cleanup_on_error() {
    local exit_code=$?
    if [ $exit_code -ne 0 ]; then
        log_error "Script failed with exit code: $exit_code"
        echo ""
        log_info "Recovery steps:"
        echo "  1. Check the error message above for specific issue"
        echo "  2. Verify dependencies are installed: docker, openssl, bc, curl, python3"
        echo "  3. Ensure you're running from the Chimera project root"
        echo "  4. Check required files exist: docker/env.mainnet-paper, operator/config.yaml"
        echo "  5. Review logs: ./docker/docker-compose.sh logs mainnet-paper"
    fi
}

trap cleanup_on_error EXIT

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

log_info() { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[✓]${NC} $1"; }
log_warning() { echo -e "${YELLOW}[!]${NC} $1"; }
log_error() { echo -e "${RED}[✗]${NC} $1"; }
log_section() {
    echo ""
    echo -e "${CYAN}========================================${NC}"
    echo -e "${CYAN}$1${NC}"
    echo -e "${CYAN}========================================${NC}"
    echo ""
}

# Configuration
VIRTUAL_CAPITAL_USD=1000
SOL_PRICE_USD=150  # Approximate SOL price (will be updated from RPC)
VIRTUAL_CAPITAL_SOL=$((VIRTUAL_CAPITAL_USD / SOL_PRICE_USD))
ENV_FILE="docker/env.mainnet-paper.local"

# Dependency validation
check_dependencies() {
    local missing=()

    for cmd in docker openssl bc curl python3; do
        if ! command -v "$cmd" &> /dev/null; then
            missing+=("$cmd")
        fi
    done

    if [ ${#missing[@]} -gt 0 ]; then
        log_error "Missing required dependencies: ${missing[*]}"
        echo ""
        log_info "Install missing dependencies:"
        echo "  macOS: brew install ${missing[*]}"
        echo "  Ubuntu/Debian: sudo apt-get install ${missing[*]}"
        echo "  CentOS/RHEL: sudo yum install ${missing[*]}"
        exit 1
    fi

    # Check docker-compose.sh
    if [ ! -f "./docker/docker-compose.sh" ]; then
        log_error "docker-compose.sh not found - run from project root"
        exit 1
    fi

    log_success "All dependencies installed"
}

# File safety validation
check_required_files() {
    local missing=()

    [ ! -f "docker/env.mainnet-paper" ] && missing+=("docker/env.mainnet-paper")
    [ ! -f "operator/config.yaml" ] && missing+=("operator/config.yaml")

    if [ ${#missing[@]} -gt 0 ]; then
        log_error "Missing required files: ${missing[*]}"
        echo ""
        log_info "Please run this script from the Chimera project root directory"
        log_info "Expected structure:"
        echo "  chimera/"
        echo "    ├── setup-1000-virtual-capital.sh"
        echo "    ├── docker/"
        echo "    │   ├── env.mainnet-paper"
        echo "    │   └── docker-compose.sh"
        echo "    └── operator/"
        echo "        └── config.yaml"
        exit 1
    fi

    log_success "All required files found"
}

log_section "🚀 Chimera $1000 Virtual Capital Setup"

log_info "This will configure Chimera for paper trading with:"
echo "  • Virtual Capital: $${VIRTUAL_CAPITAL_USD} USD"
echo "  • Equivalent SOL: ~${VIRTUAL_CAPITAL_SOL} SOL (based on \$$SOL_PRICE_USD/SOL)"
echo "  • Trading Mode: Paper Trading (no real funds at risk)"
echo "  • Network: Mainnet (real market data)"
echo ""

# Pre-flight checks
log_section "Pre-flight Checks"
check_dependencies
check_required_files
echo ""

# Step 1: Stop current services
log_section "Step 1: Stop Current Services"
log_info "Checking for running services..."

if docker ps --format "{{.Names}}" | grep -q "chimera-"; then
    log_warning "Found running Chimera services. Stopping..."
    ./docker/docker-compose.sh stop mainnet-paper 2>/dev/null || true
    sleep 2
    log_success "Services stopped"
else
    log_success "No running services found"
fi

# Step 2: Setup environment file
log_section "Step 2: Configure Environment"

if [ ! -f "$ENV_FILE" ]; then
    log_info "Creating local environment file from template..."
    cp docker/env.mainnet-paper "$ENV_FILE"
    log_success "Created $ENV_FILE"
else
    log_info "Local environment file already exists: $ENV_FILE"
fi

# Configure Helius API key
log_info "Checking Helius API key..."
if grep -q "YOUR_HELIUS_API_KEY" "$ENV_FILE" 2>/dev/null; then
    log_warning "Helius API key not configured!"
    echo ""
    log_info "Get your free API key from: https://www.helius.dev/"
    echo ""

    while true; do
        read -p "Enter your Helius API key (or 'skip' to continue): " HELIUS_KEY

        if [ "$HELIUS_KEY" = "skip" ]; then
            log_warning "Skipping API key configuration - RPC calls will fail"
            break
        fi

        # Validate UUID format
        if [[ $HELIUS_KEY =~ ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$ ]]; then
            sed -i.bak "s/YOUR_HELIUS_API_KEY/$HELIUS_KEY/g" "$ENV_FILE"
            log_success "Helius API key configured"
            break
        else
            log_error "Invalid API key format - should be UUID (e.g., 12345678-1234-1234-1234-123456789abc)"
        fi
    done
else
    log_success "Helius API key already configured"
fi

# Generate webhook secret
log_info "Checking webhook secret..."
if grep -q "mainnet-paper-webhook-secret-change-me" "$ENV_FILE" 2>/dev/null; then
    log_info "Generating secure webhook secret..."
    NEW_SECRET=$(openssl rand -hex 32)
    sed -i.bak "s/mainnet-paper-webhook-secret-change-me/$NEW_SECRET/g" "$ENV_FILE"
    log_success "Webhook secret generated"
else
    log_success "Webhook secret already configured"
fi

# Step 3: Configure Virtual Capital Settings
log_section "Step 3: Configure Virtual Capital Settings"

# Update environment file with capital settings
log_info "Configuring environment variables for $${VIRTUAL_CAPITAL_USD} capital..."

# Ensure paper trading mode is enabled
if ! grep -q "PAPER_TRADE_MODE=true" "$ENV_FILE"; then
    echo "PAPER_TRADE_MODE=true" >> "$ENV_FILE"
    log_success "Enabled paper trading mode"
fi

# Add capital configuration
if ! grep -q "VIRTUAL_CAPITAL_USD" "$ENV_FILE"; then
    cat >> "$ENV_FILE" << EOF

# Virtual Capital Configuration
VIRTUAL_CAPITAL_USD=$VIRTUAL_CAPITAL_USD
VIRTUAL_CAPITAL_SOL=$VIRTUAL_CAPITAL_SOL
EOF
    log_success "Added virtual capital configuration"
fi

log_success "Environment configured for $${VIRTUAL_CAPITAL_USD} virtual capital"

# Configuration update helper
validate_and_update_config() {
    local config_file="operator/config.yaml"
    local field_name=$1
    local new_value=$2

    # Check if field exists
    if ! grep -q "${field_name}:" "$config_file" 2>/dev/null; then
        log_warning "Field '${field_name}' not found in config.yaml"
        read -p "Continue anyway? (y/n): " -n 1 -r
        echo
        if [[ ! $REPLY =~ ^[Yy]$ ]]; then
            log_error "Configuration update cancelled"
            exit 1
        fi
        return 1
    fi

    # Update with precise pattern matching
    sed -i.bak "s/^[[:space:]]*${field_name}:[[:space:]]*.*/${field_name}: ${new_value}/" "$config_file"
    log_success "Updated ${field_name}: ${new_value}"
}

# Step 4: Update operator configuration for virtual capital
log_section "Step 4: Update Operator Configuration"

log_info "Updating operator/config.yaml for virtual capital..."

# Backup original config
if [ -f "operator/config.yaml" ]; then
    cp operator/config.yaml operator/config.yaml.backup
    log_success "Backed up original config.yaml"
fi

# Update position sizing for virtual capital
if [ -f "operator/config.yaml" ]; then
    # Calculate position sizes based on $1000 capital
    # Conservative sizing: Max 10% per position, Min 1% per position
    MAX_POSITION_SOL=$(echo "scale=2; $VIRTUAL_CAPITAL_SOL * 0.10" | bc)
    MIN_POSITION_SOL=$(echo "scale=3; $VIRTUAL_CAPITAL_SOL * 0.01" | bc)
    BASE_SIZE_SOL=$(echo "scale=2; $VIRTUAL_CAPITAL_SOL * 0.02" | bc)
    
    # Update config values using safe validation function
    validate_and_update_config "total_capital_sol" "$VIRTUAL_CAPITAL_SOL"
    validate_and_update_config "max_size_sol" "$MAX_POSITION_SOL"
    validate_and_update_config "min_size_sol" "$MIN_POSITION_SOL"
    validate_and_update_config "base_size_sol" "$BASE_SIZE_SOL"
    
    log_success "Updated position sizing:"
    echo "  • Total Capital: ${VIRTUAL_CAPITAL_SOL} SOL (~$${VIRTUAL_CAPITAL_USD})"
    echo "  • Max Position: ${MAX_POSITION_SOL} SOL (~$$(echo "scale=0; $MAX_POSITION_SOL * $SOL_PRICE_USD" | bc))"
    echo "  • Min Position: ${MIN_POSITION_SOL} SOL (~$$(echo "scale=0; $MIN_POSITION_SOL * $SOL_PRICE_USD" | bc))"
    echo "  • Base Size: ${BASE_SIZE_SOL} SOL (~$$(echo "scale=0; $BASE_SIZE_SOL * $SOL_PRICE_USD" | bc))"
fi

# Adjust circuit breaker for virtual capital
log_info "Adjusting circuit breaker for virtual capital..."
MAX_LOSS_24H=$(echo "scale=0; $VIRTUAL_CAPITAL_USD * 0.05" | bc)  # 5% max daily loss
if [ -f "operator/config.yaml" ]; then
    validate_and_update_config "max_loss_24h_usd" "$MAX_LOSS_24H"
    log_success "Set max daily loss: $${MAX_LOSS_24H} (5% of capital)"
fi

# Step 5: Initialize fresh database
log_section "Step 5: Initialize Fresh Database"

log_info "Cleaning old database if exists..."
if [ -f "data/chimera.db" ]; then
    rm -f data/chimera.db data/chimera.db-shm data/chimera.db-wal
    log_success "Old database removed"
fi

log_info "Initializing new database..."
if ./docker/docker-compose.sh init-db mainnet-paper; then
    log_success "Database initialized successfully"
else
    log_error "Database initialization failed"
    exit 1
fi

# Validate database was created
if [ ! -f "data/chimera.db" ]; then
    log_error "Database file not created after initialization"
    echo ""
    log_info "Troubleshooting:"
    echo "  1. Check docker logs: ./docker/docker-compose.sh logs mainnet-paper"
    echo "  2. Verify docker-compose.sh init-db is implemented"
    echo "  3. Try manual initialization: sqlite3 data/chimera.db < database/schema.sql"
    exit 1
fi
log_success "Database file verified"

# Step 6: Start services
log_section "Step 6: Start Services"

log_info "Building Docker images (if needed)..."
./docker/docker-compose.sh build mainnet-paper 2>&1 | grep -E "(Building|Successfully|ERROR)" || true

log_info "Starting Chimera services..."
if ./docker/docker-compose.sh start mainnet-paper; then
    log_success "Services started"
else
    log_error "Failed to start services"
    exit 1
fi

# Step 7: Wait for services and verify
log_section "Step 7: Verify Services"

log_info "Waiting for services to be ready..."
for i in {1..30}; do
    if curl -s http://localhost:8080/api/v1/health > /dev/null 2>&1; then
        log_success "Operator is healthy and ready"
        break
    fi
    if [ $i -eq 30 ]; then
        log_warning "Operator health check timeout"
    else
        echo -n "."
        sleep 2
    fi
done

echo ""
log_info "Checking system health..."
HEALTH=$(curl -s http://localhost:8080/api/v1/health 2>/dev/null)
if [ -n "$HEALTH" ]; then
    echo "$HEALTH" | python3 -m json.tool 2>/dev/null | head -20 || echo "$HEALTH"
fi

# Step 8: Run Scout for wallet discovery
log_section "Step 8: Run Scout for Wallet Discovery"

log_info "Starting Scout to discover profitable wallets..."
log_warning "This may take 5-10 minutes to analyze wallet performance..."

read -p "Press Enter to start Scout discovery, or Ctrl+C to skip..." 

if docker exec chimera-scout python main.py \
    --output /app/data/roster_new.db \
    --verbose 2>&1 | tee /tmp/scout-discovery.log; then
    log_success "Scout discovery completed"
    
    # Show results
    if [ -f /tmp/scout-discovery.log ]; then
        log_info "Discovery Summary:"
        grep -E "(Total analyzed|ACTIVE|CANDIDATE|Wallet Quality Score)" /tmp/scout-discovery.log | tail -10 || true
    fi
    
    # Merge roster
    log_info "Merging discovered wallets into operator..."
    if curl -s -X POST http://localhost:8080/api/v1/roster/merge \
        -H "Content-Type: application/json" \
        -H "Authorization: Bearer dev-admin-key" \
        -d '{}' > /dev/null 2>&1; then
        log_success "Wallet roster merged successfully"
    else
        log_warning "Manual merge may be required"
    fi
else
    log_warning "Scout discovery failed or skipped"
    log_info "You can run scout manually later"
fi

# Final Summary
log_section "🎉 Setup Complete!"

log_success "Chimera is now running with $${VIRTUAL_CAPITAL_USD} virtual capital!"
echo ""
log_info "Virtual Capital Configuration:"
echo "  • Total Capital: $${VIRTUAL_CAPITAL_USD} USD (~${VIRTUAL_CAPITAL_SOL} SOL)"
echo "  • Max Position Size: ~$$(echo "scale=0; $MAX_POSITION_SOL * $SOL_PRICE_USD" | bc) USD"
echo "  • Min Position Size: ~$$(echo "scale=0; $MIN_POSITION_SOL * $SOL_PRICE_USD" | bc) USD"
echo "  • Daily Loss Limit: $${MAX_LOSS_24H} (5%)"
echo ""

log_info "Service URLs:"
echo "  • Web Dashboard: http://localhost:3000"
echo "  • Operator API: http://localhost:8080"
echo "  • Grafana: http://localhost:3002 (admin/change-me-secure-password)"
echo "  • Prometheus: http://localhost:9090"
echo ""

log_info "Monitoring Commands:"
echo "  • View logs: ./docker/docker-compose.sh logs mainnet-paper -f"
echo "  • Check health: curl http://localhost:8080/api/v1/health"
echo "  • View trades: curl http://localhost:8080/api/v1/trades"
echo "  • Check positions: curl http://localhost:8080/api/v1/positions"
echo ""

log_warning "Important Notes:"
echo "  • This is PAPER TRADING - No real funds are at risk"
echo "  • All trades are simulated but use REAL market data"
echo "  • Monitor performance via the web dashboard"
echo "  • System will automatically copy trades from high-quality wallets"
echo "  • Circuit breakers will halt trading if losses exceed 5% daily"
echo ""

log_success "Your virtual trading bot is now ready! 🚀"
echo ""
log_info "Next steps:"
echo "  1. Open http://localhost:3000 to view the dashboard"
echo "  2. Monitor the 'Wallets' page to see tracked wallets"
echo "  3. Watch 'Positions' for simulated trades"
echo "  4. Check 'Performance' for P&L tracking"
echo "  5. Review 'Trades' for complete trade history"
echo ""