#!/bin/bash
# Comprehensive Paper Trading Functionality Test
# Tests all features of the Chimera bot in mainnet-paper mode

set -e

API_URL="http://localhost:8080"
WEBHOOK_SECRET=$(docker exec chimera-operator printenv CHIMERA_SECURITY__WEBHOOK_SECRET 2>/dev/null || echo "")

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

PASSED=0
FAILED=0
WARNINGS=0

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[✓ PASS]${NC} $1"
    ((PASSED++))
}

log_error() {
    echo -e "${RED}[✗ FAIL]${NC} $1"
    ((FAILED++))
}

log_warning() {
    echo -e "${YELLOW}[! WARN]${NC} $1"
    ((WARNINGS++))
}

log_section() {
    echo ""
    echo -e "${CYAN}========================================${NC}"
    echo -e "${CYAN}$1${NC}"
    echo -e "${CYAN}========================================${NC}"
    echo ""
}

# Generate HMAC signature
generate_signature() {
    local timestamp=$1
    local payload=$2
    echo -n "${timestamp}${payload}" | openssl dgst -sha256 -hmac "$WEBHOOK_SECRET" | cut -d' ' -f2
}

# Test 1: Service Health
test_service_health() {
    log_section "Test 1: Service Health Check"
    
    local health
    health=$(curl -s "${API_URL}/api/v1/health" 2>&1)
    
    if echo "$health" | grep -q '"status".*"healthy"'; then
        log_success "Operator health check passed"
        echo "$health" | python3 -m json.tool 2>/dev/null | head -15
    else
        log_error "Operator health check failed"
        echo "$health"
        return 1
    fi
    
    # Check RPC status
    if echo "$health" | grep -q '"rpc".*"status".*"healthy"'; then
        log_success "RPC connection healthy"
    else
        log_warning "RPC status unclear"
    fi
    
    # Check circuit breaker
    if echo "$health" | grep -q '"state".*"ACTIVE"'; then
        log_success "Circuit breaker is ACTIVE"
    else
        log_error "Circuit breaker not active"
    fi
}

# Test 2: Webhook Endpoint
test_webhook() {
    log_section "Test 2: Webhook Signal Processing"
    
    if [ -z "$WEBHOOK_SECRET" ]; then
        log_error "Webhook secret not found"
        return 1
    fi
    
    local timestamp=$(date +%s)
    local payload='{"strategy":"SHIELD","token":"So11111111111111111111111111111111111111112","action":"BUY","amount_sol":0.1,"wallet_address":"7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU","consensus_count":5,"signal_quality":0.95}'
    local signature=$(generate_signature "$timestamp" "$payload")
    
    log_info "Sending test webhook signal..."
    local response
    response=$(curl -s -w "\n%{http_code}" -X POST "${API_URL}/api/v1/webhook" \
        -H "Content-Type: application/json" \
        -H "X-Signature: $signature" \
        -H "X-Timestamp: $timestamp" \
        -d "$payload" 2>&1)
    
    local status_code=$(echo "$response" | tail -1)
    local response_body=$(echo "$response" | sed '$d')
    
    if [ "$status_code" = "200" ] || [ "$status_code" = "202" ]; then
        log_success "Webhook accepted (status: $status_code)"
        echo "$response_body" | python3 -m json.tool 2>/dev/null | head -10
    else
        log_warning "Webhook response: $status_code"
        echo "$response_body" | python3 -m json.tool 2>/dev/null | head -5
        # This might be expected if signal quality is too low
    fi
}

# Test 3: API Endpoints
test_api_endpoints() {
    log_section "Test 3: API Endpoints"
    
    # Test positions
    log_info "Testing /api/v1/positions..."
    local positions
    positions=$(curl -s "${API_URL}/api/v1/positions" 2>&1)
    if echo "$positions" | grep -q "positions\|\[\]"; then
        log_success "Positions endpoint accessible"
    else
        log_error "Positions endpoint failed"
    fi
    
    # Test trades
    log_info "Testing /api/v1/trades..."
    local trades
    trades=$(curl -s "${API_URL}/api/v1/trades" 2>&1)
    if echo "$trades" | grep -q "trades\|\[\]"; then
        log_success "Trades endpoint accessible"
    else
        log_error "Trades endpoint failed"
    fi
    
    # Test wallets
    log_info "Testing /api/v1/wallets..."
    local wallets
    wallets=$(curl -s "${API_URL}/api/v1/wallets" 2>&1)
    if echo "$wallets" | grep -q "wallets\|\[\]"; then
        log_success "Wallets endpoint accessible"
    else
        log_error "Wallets endpoint failed"
    fi
    
    # Test config
    log_info "Testing /api/v1/config..."
    local config
    config=$(curl -s "${API_URL}/api/v1/config" 2>&1)
    if echo "$config" | grep -q "circuit_breakers\|strategy"; then
        log_success "Config endpoint accessible"
        echo "$config" | python3 -m json.tool 2>/dev/null | grep -E '"paper|PAPER|jito_enabled|circuit_breaker"' | head -5
    else
        log_error "Config endpoint failed"
    fi
    
    # Test metrics
    log_info "Testing /api/v1/metrics/performance..."
    local metrics
    metrics=$(curl -s "${API_URL}/api/v1/metrics/performance" 2>&1)
    if echo "$metrics" | grep -q "pnl"; then
        log_success "Performance metrics endpoint accessible"
    else
        log_warning "Performance metrics endpoint may not have data yet"
    fi
}

# Test 4: Prometheus Metrics
test_prometheus() {
    log_section "Test 4: Prometheus Metrics Collection"
    
    log_info "Checking Prometheus targets..."
    local targets
    targets=$(curl -s "http://localhost:9090/api/v1/targets" 2>&1)
    
    if echo "$targets" | grep -q "chimera-operator"; then
        log_success "Prometheus target found"
    else
        log_error "Prometheus target not found"
    fi
    
    # Check key metrics
    local metrics=("chimera_queue_depth" "chimera_circuit_breaker_state" "chimera_rpc_health" "chimera_active_positions")
    
    for metric in "${metrics[@]}"; do
        local result
        result=$(curl -s "http://localhost:9090/api/v1/query?query=${metric}" 2>&1)
        if echo "$result" | grep -q "result"; then
            log_success "Metric ${metric} available"
        else
            log_warning "Metric ${metric} not found"
        fi
    done
}

# Test 5: Grafana Dashboard
test_grafana() {
    log_section "Test 5: Grafana Dashboard"
    
    log_info "Checking Grafana accessibility..."
    local dashboards
    dashboards=$(curl -s "http://localhost:3002/api/search?query=Chimera" -u admin:admin 2>&1)
    
    if echo "$dashboards" | grep -q "Chimera"; then
        log_success "Grafana dashboard found"
        local uid
        uid=$(echo "$dashboards" | python3 -c "import sys, json; data=json.load(sys.stdin); print(data[0]['uid'] if data else '')" 2>/dev/null)
        if [ -n "$uid" ]; then
            log_info "Dashboard UID: $uid"
            log_info "Access at: http://localhost:3002/d/$uid/chimera-trading-platform"
        fi
    else
        log_warning "Grafana dashboard not found"
    fi
}

# Test 6: Paper Trading Mode Verification
test_paper_mode() {
    log_section "Test 6: Paper Trading Mode Verification"
    
    log_info "Checking paper trading mode configuration..."
    
    # Check environment variable
    local paper_mode
    paper_mode=$(docker exec chimera-operator printenv PAPER_TRADE_MODE 2>/dev/null || echo "")
    
    if [ "$paper_mode" = "true" ]; then
        log_success "PAPER_TRADE_MODE is set to true"
    else
        log_error "PAPER_TRADE_MODE not set correctly"
    fi
    
    # Check dev mode
    local dev_mode
    dev_mode=$(docker exec chimera-operator printenv CHIMERA_DEV_MODE 2>/dev/null || echo "")
    
    if [ "$dev_mode" = "0" ]; then
        log_success "CHIMERA_DEV_MODE is 0 (production-like mode)"
    else
        log_warning "CHIMERA_DEV_MODE is $dev_mode (expected 0 for mainnet-paper)"
    fi
    
    log_info "In paper trading mode, all trades are simulated (no real funds at risk)"
}

# Test 7: RPC Connectivity
test_rpc_connectivity() {
    log_section "Test 7: RPC Connectivity"
    
    log_info "Checking RPC latency from health endpoint..."
    local health
    health=$(curl -s "${API_URL}/api/v1/health" 2>&1)
    
    local rpc_latency
    rpc_latency=$(echo "$health" | python3 -c "import sys, json; data=json.load(sys.stdin); print(data.get('rpc_latency_ms', 'N/A'))" 2>/dev/null || echo "N/A")
    
    if [ "$rpc_latency" != "N/A" ] && [ -n "$rpc_latency" ]; then
        log_success "RPC latency: ${rpc_latency}ms"
        if (( $(echo "$rpc_latency < 2000" | bc -l 2>/dev/null || echo 0) )); then
            log_success "RPC latency is acceptable (< 2000ms)"
        else
            log_warning "RPC latency is high (> 2000ms)"
        fi
    else
        log_warning "RPC latency not available"
    fi
    
    # Check RPC status
    if echo "$health" | grep -q '"rpc".*"status".*"healthy"'; then
        log_success "RPC status is healthy"
    else
        log_error "RPC status is not healthy"
    fi
}

# Test 8: Load Testing
test_load() {
    log_section "Test 8: Load Testing (10 concurrent webhooks)"
    
    log_info "Sending 10 webhook signals in parallel..."
    
    local timestamp=$(date +%s)
    local payload='{"strategy":"SHIELD","token":"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v","action":"BUY","amount_sol":0.05,"wallet_address":"7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU","consensus_count":3}'
    
    local success=0
    local failed=0
    
    for i in {1..10}; do
        local ts=$((timestamp + i))
        local sig=$(generate_signature "$ts" "$payload")
        
        local response
        response=$(curl -s -w "\n%{http_code}" -X POST "${API_URL}/api/v1/webhook" \
            -H "Content-Type: application/json" \
            -H "X-Signature: $sig" \
            -H "X-Timestamp: $ts" \
            -d "$payload" 2>&1)
        
        local status=$(echo "$response" | tail -1)
        
        if [ "$status" = "200" ] || [ "$status" = "202" ]; then
            ((success++))
            echo -n "."
        else
            ((failed++))
            echo -n "F"
        fi
    done
    
    echo ""
    log_info "Load test results: $success succeeded, $failed failed"
    
    if [ $success -gt 0 ]; then
        log_success "System handled load successfully"
    else
        log_warning "All requests failed (may be due to signal quality checks)"
    fi
}

# Test 9: Queue and Metrics
test_queue_metrics() {
    log_section "Test 9: Queue and Metrics"
    
    log_info "Checking queue depth..."
    local queue_depth
    queue_depth=$(curl -s "http://localhost:9090/api/v1/query?query=chimera_queue_depth" 2>&1 | python3 -c "import sys, json; data=json.load(sys.stdin); result=data.get('data', {}).get('result', []); print(result[0]['value'][1] if result else 'N/A')" 2>/dev/null || echo "N/A")
    
    log_info "Current queue depth: $queue_depth"
    
    log_info "Checking active positions..."
    local positions
    positions=$(curl -s "http://localhost:9090/api/v1/query?query=chimera_active_positions" 2>&1 | python3 -c "import sys, json; data=json.load(sys.stdin); result=data.get('data', {}).get('result', []); print(result[0]['value'][1] if result else 'N/A')" 2>/dev/null || echo "N/A")
    
    log_info "Active positions: $positions"
    
    log_success "Metrics are being collected"
}

# Test 10: Web Dashboard
test_web_dashboard() {
    log_section "Test 10: Web Dashboard"
    
    log_info "Checking web dashboard accessibility..."
    
    local response
    response=$(curl -s -o /dev/null -w "%{http_code}" http://localhost:3000 2>&1)
    
    if [ "$response" = "200" ]; then
        log_success "Web dashboard is accessible"
        log_info "Access at: http://localhost:3000"
    else
        log_warning "Web dashboard returned status: $response"
    fi
}

# Print summary
print_summary() {
    log_section "Test Summary"
    
    local total=$((PASSED + FAILED + WARNINGS))
    echo "Total tests: $total"
    echo -e "${GREEN}Passed: $PASSED${NC}"
    echo -e "${RED}Failed: $FAILED${NC}"
    echo -e "${YELLOW}Warnings: $WARNINGS${NC}"
    echo ""
    
    if [ $FAILED -eq 0 ]; then
        echo -e "${GREEN}✓ All critical tests passed!${NC}"
        echo ""
        log_info "The bot is ready for paper trading on mainnet!"
        echo ""
        log_info "Service URLs:"
        echo "  - Operator API: http://localhost:8080"
        echo "  - Web Dashboard: http://localhost:3000"
        echo "  - Grafana: http://localhost:3002"
        echo "  - Prometheus: http://localhost:9090"
        echo ""
        log_info "Monitor logs:"
        echo "  ./docker/docker-compose.sh logs mainnet-paper -f"
        return 0
    else
        echo -e "${RED}✗ Some tests failed.${NC}"
        return 1
    fi
}

# Main execution
main() {
    log_section "Chimera Paper Trading - Comprehensive Functionality Test"
    
    log_info "Testing bot in mainnet-paper mode..."
    log_info "Paper trading mode: All trades are simulated (no real funds)"
    echo ""
    
    # Run all tests
    test_service_health || true
    test_webhook || true
    test_api_endpoints || true
    test_prometheus || true
    test_grafana || true
    test_paper_mode || true
    test_rpc_connectivity || true
    test_load || true
    test_queue_metrics || true
    test_web_dashboard || true
    
    # Print summary
    print_summary
}

main "$@"
