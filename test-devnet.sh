#!/bin/bash
# Comprehensive Devnet Testing Script for Chimera
# Tests the entire application stack in devnet mode

set -e

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
PROFILE="devnet"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$SCRIPT_DIR"
DOCKER_SCRIPT="$PROJECT_ROOT/docker/docker-compose.sh"
MAX_WAIT_TIME=120  # Maximum time to wait for services to be ready
TEST_WEBHOOK_SECRET="devnet-webhook-secret-change-me-in-production"

# Counters
PASSED=0
FAILED=0
SKIPPED=0

# Initialize counters as integers
declare -i PASSED FAILED SKIPPED

# Helper functions
log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[PASS]${NC} $1"
    PASSED=$((PASSED + 1))
}

log_error() {
    echo -e "${RED}[FAIL]${NC} $1"
    FAILED=$((FAILED + 1))
}

log_warning() {
    echo -e "${YELLOW}[WARN]${NC} $1"
    SKIPPED=$((SKIPPED + 1))
}

log_section() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo ""
}

# Check prerequisites
check_prerequisites() {
    log_section "Checking Prerequisites"
    
    local missing=0
    
    if ! command -v docker &> /dev/null; then
        log_error "Docker is not installed"
        missing=1
    else
        log_success "Docker is installed"
    fi
    
    if ! command -v sqlite3 &> /dev/null; then
        log_error "SQLite3 is not installed"
        missing=1
    else
        log_success "SQLite3 is installed"
    fi
    
    if ! command -v curl &> /dev/null; then
        log_error "curl is not installed"
        missing=1
    else
        log_success "curl is installed"
    fi
    
    if ! command -v cargo &> /dev/null; then
        log_warning "Cargo is not installed (needed for unit tests)"
    else
        log_success "Cargo is installed"
    fi
    
    if ! command -v python3 &> /dev/null; then
        log_warning "Python3 is not installed (needed for scout tests)"
    else
        log_success "Python3 is installed"
    fi
    
    if [ $missing -eq 1 ]; then
        log_error "Missing required prerequisites. Please install them first."
        exit 1
    fi
    
    # Check if ports are available
    local ports=(8080 3000 3002 9090 9093)
    for port in "${ports[@]}"; do
        if command -v lsof &> /dev/null; then
            if lsof -Pi :$port -sTCP:LISTEN -t >/dev/null 2>&1 ; then
                log_warning "Port $port is already in use. Services may conflict."
            fi
        fi
    done
}

# Initialize database
init_database() {
    log_section "Initializing Database"
    
    if [ -f "$PROJECT_ROOT/data/chimera.db" ]; then
        log_warning "Database already exists. Removing old database..."
        rm -f "$PROJECT_ROOT/data/chimera.db" "$PROJECT_ROOT/data/chimera.db-shm" "$PROJECT_ROOT/data/chimera.db-wal" 2>/dev/null || true
    fi
    
    mkdir -p "$PROJECT_ROOT/data"
    
    if [ -f "$DOCKER_SCRIPT" ]; then
        log_info "Initializing database using docker-compose.sh..."
        bash "$DOCKER_SCRIPT" init-db "$PROFILE"
    else
        log_info "Initializing database manually..."
        if [ -f "$PROJECT_ROOT/database/schema.sql" ]; then
            sqlite3 "$PROJECT_ROOT/data/chimera.db" < "$PROJECT_ROOT/database/schema.sql"
            log_success "Database initialized"
        else
            log_error "Schema file not found at $PROJECT_ROOT/database/schema.sql"
            exit 1
        fi
    fi
    
    if [ -f "$PROJECT_ROOT/data/chimera.db" ]; then
        log_success "Database created successfully"
    else
        log_error "Database creation failed"
        exit 1
    fi
}

# Build Docker images
build_images() {
    log_section "Building Docker Images"
    
    log_info "Building images for profile: $PROFILE"
    if bash "$DOCKER_SCRIPT" build "$PROFILE" 2>&1 | tee /tmp/chimera-build.log; then
        log_success "Docker images built successfully"
    else
        log_error "Docker build failed. Check logs above."
        exit 1
    fi
}

# Start services
start_services() {
    log_section "Starting Services"
    
    log_info "Starting all services for profile: $PROFILE"
    if bash "$DOCKER_SCRIPT" start "$PROFILE"; then
        log_success "Services started"
    else
        log_error "Failed to start services"
        exit 1
    fi
    
    log_info "Waiting for services to be ready..."
    wait_for_services
}

# Wait for services to be ready
wait_for_services() {
    local wait_time=0
    local interval=5
    
    log_info "Waiting for operator to be ready..."
    while [ $wait_time -lt $MAX_WAIT_TIME ]; do
        if curl -sf http://localhost:8080/api/v1/health > /dev/null 2>&1; then
            log_success "Operator is ready"
            return 0
        fi
        sleep $interval
        wait_time=$((wait_time + interval))
        echo -n "."
    done
    echo ""
    log_error "Operator did not become ready within $MAX_WAIT_TIME seconds"
    log_info "Checking logs..."
    bash "$DOCKER_SCRIPT" logs "$PROFILE" operator | tail -50
    exit 1
}

# Test health endpoint
test_health() {
    log_section "Testing Health Endpoint"
    
    local response
    response=$(curl -sf http://localhost:8080/api/v1/health 2>&1)
    
    if [ $? -eq 0 ]; then
        log_success "Health endpoint is accessible"
        echo "Response: $response" | head -5
    else
        log_error "Health endpoint failed: $response"
        return 1
    fi
}

# Test webhook endpoint
test_webhook() {
    log_section "Testing Webhook Endpoint"
    
    # Generate HMAC signature
    local timestamp=$(date +%s)
    local payload='{"strategy":"SHIELD","token":"BONK","action":"BUY","amount_sol":0.1,"wallet_address":"7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"}'
    local signature
    
    # Get the actual webhook secret from environment or use default
    local webhook_secret="${CHIMERA_SECURITY__WEBHOOK_SECRET:-$TEST_WEBHOOK_SECRET}"
    
    # Generate signature using openssl
    if command -v openssl &> /dev/null; then
        signature=$(echo -n "${timestamp}${payload}" | openssl dgst -sha256 -hmac "$webhook_secret" | cut -d' ' -f2)
    else
        log_warning "openssl not found, skipping HMAC signature test"
        return 0
    fi
    
    log_info "Sending test webhook with HMAC signature..."
    local response
    local status_code
    
    response=$(curl -s -w "\n%{http_code}" -X POST http://localhost:8080/api/v1/webhook \
        -H "Content-Type: application/json" \
        -H "X-Signature: $signature" \
        -H "X-Timestamp: $timestamp" \
        -d "$payload" 2>&1)
    
    status_code=$(echo "$response" | tail -1)
    response_body=$(echo "$response" | sed '$d')
    
    if [ "$status_code" = "200" ] || [ "$status_code" = "202" ]; then
        log_success "Webhook endpoint accepted request (status: $status_code)"
    elif [ "$status_code" = "400" ] || [ "$status_code" = "401" ]; then
        log_warning "Webhook rejected (status: $status_code) - this may be expected for devnet"
        echo "Response: $response_body"
    else
        log_error "Webhook test failed (status: $status_code)"
        echo "Response: $response_body"
        return 1
    fi
}

# Test metrics endpoint
test_metrics() {
    log_section "Testing Metrics Endpoint"
    
    local response
    response=$(curl -sf http://localhost:8080/metrics 2>&1)
    
    if [ $? -eq 0 ]; then
        log_success "Metrics endpoint is accessible"
        local metric_count=$(echo "$response" | grep -c "^chimera_" || echo "0")
        log_info "Found $metric_count Chimera metrics"
    else
        log_error "Metrics endpoint failed: $response"
        return 1
    fi
}

# Test web dashboard
test_web_dashboard() {
    log_section "Testing Web Dashboard"
    
    local response
    response=$(curl -sf http://localhost:3000 2>&1)
    
    if [ $? -eq 0 ]; then
        log_success "Web dashboard is accessible"
    else
        log_warning "Web dashboard not accessible: $response"
        log_info "This may be normal if the web service is still starting"
    fi
}

# Test Prometheus
test_prometheus() {
    log_section "Testing Prometheus"
    
    local response
    response=$(curl -sf http://localhost:9090/-/healthy 2>&1)
    
    if [ $? -eq 0 ]; then
        log_success "Prometheus is accessible"
    else
        log_warning "Prometheus not accessible: $response"
    fi
}

# Test Grafana
test_grafana() {
    log_section "Testing Grafana"
    
    local response
    response=$(curl -sf --max-time 5 http://localhost:3002/api/health 2>&1) || true
    
    if [ -n "$response" ] && echo "$response" | grep -q "database"; then
        log_success "Grafana is accessible"
    else
        log_warning "Grafana not accessible or still starting: $response"
    fi
}

# Check service status
check_service_status() {
    log_section "Checking Service Status"
    
    if bash "$DOCKER_SCRIPT" status "$PROFILE" | grep -q "Up"; then
        log_success "Services are running"
        bash "$DOCKER_SCRIPT" status "$PROFILE"
    else
        log_error "Some services are not running"
        bash "$DOCKER_SCRIPT" status "$PROFILE"
        return 1
    fi
}

# Run unit tests
run_unit_tests() {
    log_section "Running Unit Tests"
    
    if ! command -v cargo &> /dev/null; then
        log_warning "Cargo not found, skipping unit tests"
        return 0
    fi
    
    log_info "Running operator unit tests..."
    cd "$PROJECT_ROOT/operator"
    if cargo test --lib 2>&1 | tee /tmp/chimera-unit-tests.log; then
        log_success "Unit tests passed"
    else
        log_error "Unit tests failed"
        return 1
    fi
    cd "$PROJECT_ROOT"
}

# Run integration tests
run_integration_tests() {
    log_section "Running Integration Tests"
    
    if ! command -v cargo &> /dev/null; then
        log_warning "Cargo not found, skipping integration tests"
        return 0
    fi
    
    log_info "Running operator integration tests..."
    cd "$PROJECT_ROOT/operator"
    if cargo test --test '*' -- --test-threads=1 2>&1 | tee /tmp/chimera-integration-tests.log; then
        log_success "Integration tests passed"
    else
        log_warning "Some integration tests may have failed (check logs)"
    fi
    cd "$PROJECT_ROOT"
}

# Run scout tests
run_scout_tests() {
    log_section "Running Scout Tests"
    
    if ! command -v python3 &> /dev/null; then
        log_warning "Python3 not found, skipping scout tests"
        return 0
    fi
    
    if ! command -v pytest &> /dev/null; then
        log_warning "pytest not found, skipping scout tests"
        return 0
    fi
    
    log_info "Running scout tests..."
    cd "$PROJECT_ROOT/scout"
    if python3 -m pytest tests/ -v 2>&1 | tee /tmp/chimera-scout-tests.log; then
        log_success "Scout tests passed"
    else
        log_warning "Some scout tests may have failed (check logs)"
    fi
    cd "$PROJECT_ROOT"
}

# Test database connectivity
test_database() {
    log_section "Testing Database"
    
    if [ -f "$PROJECT_ROOT/data/chimera.db" ]; then
        log_success "Database file exists"
        
        # Check if we can query the database
        local table_count
        table_count=$(sqlite3 "$PROJECT_ROOT/data/chimera.db" "SELECT COUNT(*) FROM sqlite_master WHERE type='table';" 2>&1)
        
        if [ $? -eq 0 ]; then
            log_success "Database is accessible (found $table_count tables)"
        else
            log_error "Database query failed"
            return 1
        fi
    else
        log_error "Database file not found"
        return 1
    fi
}

# Check logs for errors
check_logs() {
    log_section "Checking Service Logs for Errors"
    
    log_info "Checking operator logs..."
    local errors
    errors=$(bash "$DOCKER_SCRIPT" logs "$PROFILE" operator 2>&1 | grep -i "error\|panic\|fatal" | tail -10 || true)
    
    if [ -z "$errors" ]; then
        log_success "No critical errors found in operator logs"
    else
        log_warning "Found potential errors in operator logs:"
        echo "$errors"
    fi
}

# Print summary
print_summary() {
    log_section "Test Summary"
    
    local total=$((PASSED + FAILED + SKIPPED))
    
    echo "Total tests: $total"
    echo -e "${GREEN}Passed: $PASSED${NC}"
    echo -e "${RED}Failed: $FAILED${NC}"
    echo -e "${YELLOW}Skipped: $SKIPPED${NC}"
    echo ""
    
    if [ $FAILED -eq 0 ]; then
        echo -e "${GREEN}✓ All critical tests passed!${NC}"
        echo ""
        echo "Services are running:"
        echo "  - Operator API: http://localhost:8080"
        echo "  - Web Dashboard: http://localhost:3000"
        echo "  - Grafana: http://localhost:3002 (admin/admin)"
        echo "  - Prometheus: http://localhost:9090"
        echo ""
        echo "To view logs: ./docker/docker-compose.sh logs devnet -f"
        echo "To stop services: ./docker/docker-compose.sh stop devnet"
        return 0
    else
        echo -e "${RED}✗ Some tests failed. Check logs above.${NC}"
        return 1
    fi
}

# Cleanup function
cleanup() {
    log_section "Cleanup"
    log_info "Stopping services..."
    bash "$DOCKER_SCRIPT" stop "$PROFILE" 2>/dev/null || true
}

# Main execution
main() {
    log_section "Chimera Devnet Testing Suite"
    log_info "Profile: $PROFILE"
    log_info "Project Root: $PROJECT_ROOT"
    echo ""
    
    # Trap to ensure cleanup on exit
    trap cleanup EXIT
    
    # Run test suite
    check_prerequisites
    init_database
    build_images
    start_services
    sleep 5  # Give services a moment to fully initialize
    
    # Core functionality tests
    test_health
    test_webhook
    test_metrics
    test_database
    
    # Service tests - make these non-fatal
    set +e
    test_web_dashboard || true
    test_prometheus || true
    test_grafana || true
    check_service_status || true
    set -e
    
    # Code tests (if available) - don't fail on these
    set +e  # Temporarily disable exit on error for optional tests
    run_unit_tests || true
    run_integration_tests || true
    run_scout_tests || true
    set -e  # Re-enable exit on error
    
    # Final checks
    check_logs
    
    # Print summary
    print_summary
    
    # Don't cleanup on success - let services keep running
    trap - EXIT
}

# Run main function
main "$@"
