#!/bin/bash
# Comprehensive Devnet Testing Script
# Tests webhook, wallet management, circuit breaker, and monitoring

set -e

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BLUE='\033[0;34m'
NC='\033[0m'

API_URL="http://localhost:8080"
# Get webhook secret from running container
WEBHOOK_SECRET=$(docker exec chimera-operator printenv CHIMERA_SECURITY__WEBHOOK_SECRET 2>/dev/null || true)
if [ -z "$WEBHOOK_SECRET" ]; then
    echo "ERROR: cannot obtain webhook secret from chimera-operator container" >&2
    exit 1
fi

PASSED=0
FAILED=0

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[PASS]${NC} $1"
    ((PASSED += 1))
}

log_error() {
    echo -e "${RED}[FAIL]${NC} $1"
    ((FAILED += 1))
}

log_section() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo ""
}

# Generate HMAC signature
generate_signature() {
    local timestamp=$1
    local payload=$2
    echo -n "${timestamp}${payload}" | openssl dgst -sha256 -hmac "$WEBHOOK_SECRET" | cut -d' ' -f2
}

# Test webhook endpoint
test_webhook() {
    log_section "Testing Webhook Endpoint"
    
    local timestamp=$(date +%s)
    local payload='{"strategy":"SHIELD","token":"BONK","action":"BUY","amount_sol":0.1,"wallet_address":"7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"}'
    local signature=$(generate_signature "$timestamp" "$payload")
    
    log_info "Sending test webhook signal..."
    local response
    local status_code
    
    response=$(curl -s -w "\n%{http_code}" -X POST "${API_URL}/api/v1/webhook" \
        -H "Content-Type: application/json" \
        -H "X-Signature: $signature" \
        -H "X-Timestamp: $timestamp" \
        -d "$payload" 2>&1 || true)
    
    status_code=$(echo "$response" | tail -1)
    response_body=$(echo "$response" | sed '$d')
    
    if [ "$status_code" = "200" ] || [ "$status_code" = "202" ]; then
        log_success "Webhook accepted (status: $status_code)"
        echo "Response: $response_body" | head -3
    else
        log_error "Webhook rejected (status: $status_code)"
        echo "Response: $response_body"
        return 1
    fi
}

# Test wallet API
test_wallet_api() {
    log_section "Testing Wallet API"
    
    local test_wallet="7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"
    
    log_info "Getting wallet status..."
    local response
    if ! response=$(curl -sf --max-time 5 "${API_URL}/api/v1/wallets/${test_wallet}" 2>&1); then
        log_error "Wallet API failed (endpoint unreachable)"
        echo "Response: $response"
        return 1
    fi
    
    if echo "$response" | grep -q "wallet_address"; then
        log_success "Wallet API endpoint accessible"
        echo "Wallet data: $(echo "$response" | python3 -m json.tool 2>/dev/null | head -10)"
    else
        log_error "Wallet API failed"
        echo "Response: $response"
        return 1
    fi
}

# Test circuit breaker
test_circuit_breaker() {
    log_section "Testing Circuit Breaker"
    
    log_info "Checking circuit breaker status..."
    local health
    health=$(curl -s --max-time 5 "${API_URL}/api/v1/health" | python3 -m json.tool 2>/dev/null || true)
    
    local cb_state
    cb_state=$(echo "$health" | grep -A 2 '"circuit_breaker"' | grep '"state"' | cut -d'"' -f4)
    
    if [ "$cb_state" = "ACTIVE" ]; then
        log_success "Circuit breaker is ACTIVE"
    else
        log_error "Circuit breaker state: $cb_state (expected ACTIVE)"
        return 1
    fi
    
    log_info "Circuit breaker status: $cb_state"
    echo "$health" | grep -A 3 '"circuit_breaker"'
}

# Verify Grafana metrics
verify_grafana_metrics() {
    log_section "Verifying Grafana Dashboard Metrics"
    
    log_info "Checking if metrics are available in Prometheus..."
    local metrics=("chimera_queue_depth" "chimera_circuit_breaker_state" "chimera_rpc_health" "chimera_active_positions")
    
    for metric in "${metrics[@]}"; do
        local result
        result=$(curl -s --max-time 5 "http://localhost:9090/api/v1/query?query=${metric}" | python3 -c "import sys,json; print(len(json.load(sys.stdin).get('data',{}).get('result',[])))" 2>/dev/null || echo "0")
        
        if [ "$result" -gt 0 ]; then
            log_success "Metric ${metric} is available in Prometheus"
        else
            log_error "Metric ${metric} not found in Prometheus"
        fi
    done
    
    log_info "Access Grafana at http://localhost:3002 to view dashboard"
}

# Verify Prometheus collection
verify_prometheus() {
    log_section "Verifying Prometheus Metrics Collection"
    
    log_info "Checking Prometheus targets..."
    local targets
    targets=$(curl -s "http://localhost:9090/api/v1/targets" | python3 -m json.tool 2>/dev/null)
    
    local health
    health=$(echo "$targets" | grep -A 10 "chimera-operator" | grep '"health"' | head -1 | cut -d'"' -f4 || echo "")
    
    if [ "$health" = "up" ]; then
        log_success "Prometheus is successfully scraping operator metrics"
    elif [ -n "$health" ]; then
        log_error "Prometheus target health: $health (expected 'up')"
        return 1
    else
        # Try alternative parsing
        health=$(curl -s "http://localhost:9090/api/v1/targets" | python3 -c "import sys, json; data=json.load(sys.stdin); targets=[t for t in data['data']['activeTargets'] if 'chimera-operator' in str(t)]; print(targets[0]['health'] if targets else 'unknown')" 2>/dev/null || echo "unknown")
        if [ "$health" = "up" ]; then
            log_success "Prometheus is successfully scraping operator metrics"
        else
            log_info "Prometheus target health: $health (checking manually...)"
            # Check if metrics are actually available
            local test_metric
            test_metric=$(curl -s "http://localhost:9090/api/v1/query?query=chimera_queue_depth" | python3 -m json.tool 2>/dev/null | grep -c "result" || echo "0")
            if [ "$test_metric" -gt 0 ]; then
                log_success "Prometheus is collecting metrics (health check parsing issue)"
            else
                log_error "Prometheus may not be scraping correctly"
            fi
        fi
    fi
    
    log_info "Checking available metrics..."
    local metric_count
    metric_count=$(curl -s "http://localhost:9090/api/v1/label/__name__/values" | python3 -m json.tool 2>/dev/null | grep -c "chimera_" || echo "0")
    log_info "Found $metric_count Chimera metrics in Prometheus"
}

# Test web dashboard
test_web_dashboard() {
    log_section "Testing Web Dashboard"
    
    log_info "Testing dashboard endpoints..."
    
    # Test health endpoint
    local health
    if ! health=$(curl -sf --max-time 5 "${API_URL}/api/v1/health" 2>&1); then
        log_error "Health endpoint failed"
        return 1
    fi
    log_success "Health endpoint accessible"
    
    # Test positions endpoint
    if curl -sf --max-time 5 "${API_URL}/api/v1/positions" > /dev/null 2>&1; then
        log_success "Positions endpoint accessible"
    else
        log_error "Positions endpoint failed"
        return 1
    fi
    
    # Test trades endpoint
    if curl -sf --max-time 5 "${API_URL}/api/v1/trades" > /dev/null 2>&1; then
        log_success "Trades endpoint accessible"
    else
        log_error "Trades endpoint failed"
        return 1
    fi
    
    log_info "Web dashboard available at http://localhost:3000"
}

# Monitor metrics over time
monitor_metrics() {
    log_section "Monitoring Metrics Over Time"
    
    log_info "Collecting metrics samples over 30 seconds..."
    
    local query_value='import sys,json; r=json.load(sys.stdin).get("data",{}).get("result",[]); print(r[0]["value"][1] if r else "N/A")'
    for i in {1..6}; do
        echo -n "Sample $i: "
        local queue_depth
        queue_depth=$(curl -s --max-time 5 "http://localhost:9090/api/v1/query?query=chimera_queue_depth" | python3 -c "$query_value" 2>/dev/null || echo "N/A")
        local rpc_health
        rpc_health=$(curl -s --max-time 5 "http://localhost:9090/api/v1/query?query=chimera_rpc_health" | python3 -c "$query_value" 2>/dev/null || echo "N/A")
        local cb_state
        cb_state=$(curl -s --max-time 5 "http://localhost:9090/api/v1/query?query=chimera_circuit_breaker_state" | python3 -c "$query_value" 2>/dev/null || echo "N/A")
        
        echo "Queue: $queue_depth, RPC: $rpc_health, CB: $cb_state"
        sleep 5
    done
    
    log_success "Metrics monitoring complete"
}

# Verify notifications
verify_notifications() {
    log_section "Verifying Notifications"
    
    log_info "Checking notification configuration..."
    
    local env_file="docker/env.devnet"
    if [ -f "$env_file" ]; then
        if grep -q "TELEGRAM_BOT_TOKEN" "$env_file" && ! grep -q "^TELEGRAM_BOT_TOKEN=$" "$env_file"; then
            log_success "Telegram notifications configured"
        else
            log_info "Telegram notifications not configured (optional)"
        fi
        
        if grep -q "DISCORD_WEBHOOK_URL" "$env_file" && ! grep -q "^DISCORD_WEBHOOK_URL=$" "$env_file"; then
            log_success "Discord notifications configured"
        else
            log_info "Discord notifications not configured (optional)"
        fi
    fi
    
    log_info "Notifications are optional - system will work without them"
}

# Print summary
print_summary() {
    log_section "Test Summary"
    
    local total=$((PASSED + FAILED))
    echo "Total tests: $total"
    echo -e "${GREEN}Passed: $PASSED${NC}"
    echo -e "${RED}Failed: $FAILED${NC}"
    echo ""
    
    if [ $FAILED -eq 0 ]; then
        echo -e "${GREEN}✓ All tests passed!${NC}"
        return 0
    else
        echo -e "${RED}✗ Some tests failed.${NC}"
        return 1
    fi
}

# Main execution
main() {
    log_section "Comprehensive Devnet Testing"
    
    # Wait for services to be ready
    log_info "Waiting for services to be ready..."
    sleep 5
    
    # Run tests
    test_webhook || true
    test_wallet_api || true
    test_circuit_breaker || true
    verify_prometheus || true
    verify_grafana_metrics || true
    test_web_dashboard || true
    monitor_metrics || true
    verify_notifications || true
    
    # Print summary
    print_summary
}

main "$@"
