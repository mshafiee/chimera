#!/bin/bash
# Run Fresh Paper Trading Session
# This script cleans the database, starts a fresh bot, runs scout for wallet discovery,
# starts trading, and evaluates logs and performance.

set -e

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

PROFILE="mainnet-paper"
PROJECT_ROOT="$(cd "$(dirname "$0")" && pwd)"
DOCKER_SCRIPT="$PROJECT_ROOT/docker/docker-compose.sh"
DATA_DIR="$PROJECT_ROOT/data"
DB_PATH="$DATA_DIR/chimera.db"

# Admin token must not be a committed default; read from env or local file
ADMIN_TOKEN="${ADMIN_TOKEN:-$(cat "$PROJECT_ROOT/.admin-token" 2>/dev/null || true)}"

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
    echo -e "${CYAN}========================================${NC}"
    echo -e "${CYAN}$1${NC}"
    echo -e "${CYAN}========================================${NC}"
    echo ""
}

# Step 1: Clean up database
clean_database() {
    log_section "Step 1: Cleaning Database"
    
    log_info "Stopping services first..."
    if bash "$DOCKER_SCRIPT" stop "$PROFILE" >/dev/null 2>&1; then
        log_info "Services stopped"
    else
        log_warning "Could not stop services (may not be running)"
    fi
    
    # Wait until the SQLite files are actually gone so WAL/SHM handles are released
    if [ -f "$DB_PATH" ]; then
        log_warning "Database exists at $DB_PATH"
        log_info "Removing old database..."
        rm -f "$DB_PATH" "$DB_PATH-shm" "$DB_PATH-wal" "$DB_PATH-journal" 2>/dev/null || true
        for i in $(seq 1 15); do
            [ ! -e "$DB_PATH" ] && [ ! -e "$DB_PATH-wal" ] && [ ! -e "$DB_PATH-shm" ] && break
            sleep 1
        done
        log_success "Old database removed"
    else
        log_info "No existing database found"
    fi
    
    # If the operator uses PostgreSQL, deleting the SQLite file does not reset
    # trading state; clear the live tables instead.
    DB_MODE=$(grep -E '^CHIMERA_DB_MODE=' "$PROJECT_ROOT/docker/env.mainnet-paper" 2>/dev/null | tail -1 | cut -d= -f2 || true)
    DB_MODE="${DB_MODE:-postgres}"
    if [ "$DB_MODE" != "sqlite" ]; then
        log_info "Operator uses $DB_MODE backend; truncating trading tables..."
        (cd "$PROJECT_ROOT" && docker compose --profile "$PROFILE" exec -T postgres psql -U chimera -d chimera \
            -c "TRUNCATE trades, positions, dead_letter_queue RESTART IDENTITY CASCADE;" 2>/dev/null) \
            || log_warning "Could not truncate PostgreSQL tables (is postgres running?)"
    fi
    
    log_info "Initializing fresh database..."
    if bash "$DOCKER_SCRIPT" init-db "$PROFILE"; then
        log_success "Database initialized successfully"
    else
        log_error "Database initialization failed"
        exit 1
    fi
    
    # Also clean roster_new.db if it exists
    if [ -f "$DATA_DIR/roster_new.db" ]; then
        log_info "Removing old roster_new.db..."
        rm -f "$DATA_DIR/roster_new.db" 2>/dev/null || true
    fi
}

# Step 2: Start fresh bot for paper trading
start_paper_trading_bot() {
    log_section "Step 2: Starting Paper Trading Bot"
    
    log_info "Building images if needed..."
    bash "$DOCKER_SCRIPT" build "$PROFILE" 2>&1 | grep -E "(Building|Successfully|ERROR)" || true
    if [ ${PIPESTATUS[0]} -ne 0 ]; then
        log_error "Docker build failed"
        exit 1
    fi
    
    log_info "Starting services for $PROFILE..."
    if bash "$DOCKER_SCRIPT" start "$PROFILE"; then
        log_success "Services started"
    else
        log_error "Failed to start services"
        exit 1
    fi
    
    log_info "Waiting for services to be ready..."
    local max_attempts=30
    local attempt=0
    
    while [ $attempt -lt $max_attempts ]; do
        if curl -sf --max-time 5 http://localhost:8080/api/v1/health > /dev/null 2>&1; then
            log_success "Operator is healthy and ready"
            break
        fi
        attempt=$((attempt + 1))
        if [ $attempt -eq $max_attempts ]; then
            log_error "Operator did not become healthy within ${max_attempts} attempts"
            exit 1
        fi
        log_info "Waiting for operator... ($attempt/$max_attempts)"
        sleep 2
    done
    
    # Show service status
    log_info "Service status:"
    bash "$DOCKER_SCRIPT" status "$PROFILE" 2>/dev/null || true
}

# Step 3: Run scout for wallet discovery
run_scout_discovery() {
    log_section "Step 3: Running Scout for Wallet Discovery"
    
    log_info "Running scout with wallet discovery enabled..."
    log_info "This may take several minutes..."
    
    # Run scout in the scout container, capturing its real exit status
    if (cd "$PROJECT_ROOT" && docker compose --profile "$PROFILE" exec -T scout python main.py \
        --output /app/data/roster_new.db \
        --verbose > /tmp/scout-output.log 2>&1); then
        log_success "Scout completed wallet discovery"
        
        # Check if roster_new.db was created
        if [ -f "$DATA_DIR/roster_new.db" ]; then
            log_success "Roster file created: $DATA_DIR/roster_new.db"
            
            # Show summary from scout output
            if [ -f /tmp/scout-output.log ]; then
                log_info "Scout Summary:"
                grep -E "(Total analyzed|ACTIVE|CANDIDATE|REJECTED|Wallets discovered)" /tmp/scout-output.log | tail -10 || true
            fi
            
            # Merge roster into main database
            log_info "Merging roster into main database..."
            if curl -sf -X POST http://localhost:8080/api/v1/roster/merge \
                -H "Content-Type: application/json" \
                -H "Authorization: Bearer $ADMIN_TOKEN" \
                -d '{}' > /dev/null 2>&1; then
                log_success "Roster merged successfully"
            else
                log_warning "Roster merge API call failed (may need manual merge)"
            fi
        else
            log_warning "Roster file not created - scout may have failed"
        fi
    else
        log_error "Scout execution failed"
        log_info "Check logs with: docker compose logs scout"
        return 1
    fi
}

# Step 4: Verify trading is active
verify_trading_active() {
    log_section "Step 4: Verifying Trading is Active"
    
    log_info "Checking operator health..."
    HEALTH=$(curl -fsS --max-time 5 http://localhost:8080/api/v1/health 2>/dev/null || true)
    if echo "$HEALTH" | grep -q '"status".*"healthy"'; then
        log_success "Operator is healthy"
        echo "$HEALTH" | python3 -m json.tool 2>/dev/null | head -15 || echo "$HEALTH" | head -10
    else
        log_warning "Operator health check returned unexpected result"
        echo "$HEALTH" | head -10
    fi
    
    log_info "Checking roster status..."
    ROSTER=$(curl -fsS --max-time 5 http://localhost:8080/api/v1/roster 2>/dev/null || true)
    if [ -n "$ROSTER" ]; then
        WALLET_COUNT=$(echo "$ROSTER" | python3 -c "import sys, json; data=json.load(sys.stdin); print(len(data.get('wallets', [])))" 2>/dev/null || echo "unknown")
        log_info "Wallets in roster: $WALLET_COUNT"
        
        ACTIVE_COUNT=$(echo "$ROSTER" | python3 -c "import sys, json; data=json.load(sys.stdin); wallets=data.get('wallets', []); print(sum(1 for w in wallets if w.get('status') == 'ACTIVE'))" 2>/dev/null || echo "unknown")
        log_info "Active wallets: $ACTIVE_COUNT"
    else
        log_warning "Could not fetch roster"
    fi
    
    log_info "Checking recent trades..."
    TRADES=$(curl -fsS --max-time 5 "http://localhost:8080/api/v1/trades?limit=5" 2>/dev/null || true)
    if [ -n "$TRADES" ]; then
        TRADE_COUNT=$(echo "$TRADES" | python3 -c "import sys, json; data=json.load(sys.stdin); print(len(data.get('trades', [])))" 2>/dev/null || echo "0")
        log_info "Recent trades: $TRADE_COUNT"
    fi
}

# Step 5: Evaluate logs and performance
evaluate_performance() {
    log_section "Step 5: Evaluating Logs and Performance"
    
    log_info "Collecting performance metrics..."
    
    # Get performance metrics from API
    log_info "Performance Metrics (24H, 7D, 30D):"
    PERF=$(curl -fsS --max-time 5 http://localhost:8080/api/v1/metrics/performance 2>/dev/null || true)
    if [ -n "$PERF" ]; then
        echo "$PERF" | python3 -m json.tool 2>/dev/null || echo "$PERF"
    else
        log_warning "Could not fetch performance metrics"
    fi
    
    echo ""
    log_info "Cost Metrics (30-day):"
    COSTS=$(curl -fsS --max-time 5 http://localhost:8080/api/v1/metrics/costs 2>/dev/null || true)
    if [ -n "$COSTS" ]; then
        echo "$COSTS" | python3 -m json.tool 2>/dev/null || echo "$COSTS"
    else
        log_warning "Could not fetch cost metrics"
    fi
    
    echo ""
    log_info "Recent Operator Logs (last 20 lines):"
    (cd "$PROJECT_ROOT" && docker compose --profile "$PROFILE" logs operator --tail 20 2>&1 | tail -20) || log_warning "Could not fetch operator logs"
    
    echo ""
    log_info "Recent Scout Logs (last 10 lines):"
    (cd "$PROJECT_ROOT" && docker compose --profile "$PROFILE" logs scout --tail 10 2>&1 | tail -10) || log_warning "Could not fetch scout logs"
    
    echo ""
    log_info "Service Resource Usage:"
    docker stats --no-stream --format "table {{.Name}}\t{{.CPUPerc}}\t{{.MemUsage}}" 2>/dev/null | grep chimera || log_warning "Could not fetch resource stats"
    
    echo ""
    log_info "Monitoring URLs:"
    echo "  - Operator API:    http://localhost:8080"
    echo "  - Web Dashboard:    http://localhost:3000"
    echo "  - Grafana:         http://localhost:3002"
    echo "  - Prometheus:      http://localhost:9090"
    echo ""
    log_info "View live logs with:"
    echo "  ./docker/docker-compose.sh logs $PROFILE -f"
    echo ""
    log_info "View operator logs:"
    echo "  docker compose --profile $PROFILE logs operator -f"
    echo ""
    log_info "View scout logs:"
    echo "  docker compose --profile $PROFILE logs scout -f"
}

# Main execution
main() {
    if [ -z "$ADMIN_TOKEN" ]; then
        log_error "No admin token available: set ADMIN_TOKEN or create $PROJECT_ROOT/.admin-token"
        exit 1
    fi

    log_section "Chimera Fresh Paper Trading Session"
    log_info "This will:"
    log_info "  1. Clean the database"
    log_info "  2. Start fresh paper trading bot"
    log_info "  3. Run scout for wallet discovery"
    log_info "  4. Verify trading is active"
    log_info "  5. Evaluate logs and performance"
    echo ""
    
    # Execute steps
    clean_database
    start_paper_trading_bot
    sleep 5  # Give services a moment to fully start
    
    # Run scout (with retry logic)
    if ! run_scout_discovery; then
        log_warning "Scout failed, but continuing with evaluation..."
    fi
    
    sleep 3  # Give time for roster merge if it happened
    
    verify_trading_active
    evaluate_performance
    
    log_section "Setup Complete!"
    log_success "Paper trading session is ready"
    log_info "The bot will automatically start trading when it receives signals"
    log_info "Monitor the logs and dashboard for trading activity"
}

# Run main function
main "$@"
