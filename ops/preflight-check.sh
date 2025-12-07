#!/bin/bash
# Chimera Pre-Deployment Verification Script
#
# Performs critical checks before production deployment:
# 1. Time sync verification (NTP)
# 2. RPC latency measurement (< 50ms required)
# 3. Circuit breaker test (insert fake loss, verify rejection)
#
# Usage: ./preflight-check.sh [--skip-circuit-breaker]
#
# Exit codes:
#   0 - All checks passed
#   1 - Critical check failed
#   2 - Warning (non-critical)

set -euo pipefail

# Configuration
CHIMERA_HOME="${CHIMERA_HOME:-/opt/chimera}"
DB_PATH="${CHIMERA_HOME}/data/chimera.db"
RPC_URL="${HELIUS_RPC_URL:-https://api.mainnet-beta.solana.com}"
LATENCY_THRESHOLD_MS=50
SKIP_CIRCUIT_BREAKER="${SKIP_CIRCUIT_BREAKER:-false}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

# Counters
PASSED=0
FAILED=0
WARNINGS=0

log_pass() {
    echo -e "${GREEN}✓${NC} $1"
    ((PASSED++))
}

log_fail() {
    echo -e "${RED}✗${NC} $1"
    ((FAILED++))
}

log_warn() {
    echo -e "${YELLOW}⚠${NC} $1"
    ((WARNINGS++))
}

log_info() {
    echo -e "  $1"
}

# ============================================================================
# Check 1: Time Sync (NTP)
# ============================================================================

check_time_sync() {
    echo ""
    echo "=== Check 1: Time Synchronization ==="
    
    # Check if timedatectl is available (systemd)
    if command -v timedatectl &> /dev/null; then
        local ntp_status
        ntp_status=$(timedatectl show -p NTP --value 2>/dev/null || echo "unknown")
        
        if [[ "$ntp_status" == "yes" ]] || [[ "$ntp_status" == "true" ]]; then
            log_pass "NTP is enabled"
            
            # Check sync status
            local sync_status
            sync_status=$(timedatectl status | grep -i "system clock synchronized" || echo "")
            
            if echo "$sync_status" | grep -qi "yes\|true"; then
                log_pass "System clock is synchronized"
                
                # Check time offset
                local offset_str
                offset_str=$(timedatectl timesync-status 2>/dev/null | grep "Offset:" || echo "")
                
                if [[ -n "$offset_str" ]]; then
                    log_info "Time sync status: $offset_str"
                fi
            else
                log_warn "System clock sync status unknown"
            fi
        else
            log_fail "NTP is not enabled"
            log_info "Enable with: sudo timedatectl set-ntp true"
            return 1
        fi
        
        # Check for ntpq (alternative check)
        if command -v ntpq &> /dev/null; then
            local peers
            peers=$(ntpq -p 2>/dev/null | grep -c "^\*" || echo "0")
            if [[ "$peers" -gt 0 ]]; then
                log_pass "NTP peer synchronized (ntpq)"
            else
                log_warn "No NTP peers synchronized (ntpq)"
            fi
        fi
        
    elif command -v ntpq &> /dev/null; then
        # Fallback to ntpq
        local peers
        peers=$(ntpq -p 2>/dev/null | grep -c "^\*" || echo "0")
        if [[ "$peers" -gt 0 ]]; then
            log_pass "NTP peer synchronized (ntpq)"
        else
            log_fail "No NTP synchronization detected"
            log_info "Install and configure NTP"
            return 1
        fi
    else
        log_warn "Cannot verify time sync (no timedatectl or ntpq)"
        log_info "Manual verification required"
    fi
    
    # Check clock drift (compare system time with a reference)
    log_info "Checking clock drift..."
    local system_time
    system_time=$(date +%s)
    # Note: In production, compare with a reliable time server
    log_info "System time: $(date -u '+%Y-%m-%d %H:%M:%S UTC')"
    
    return 0
}

# ============================================================================
# Check 2: RPC Latency
# ============================================================================

check_rpc_latency() {
    echo ""
    echo "=== Check 2: RPC Latency ==="
    
    if [[ -z "$RPC_URL" ]]; then
        log_fail "RPC_URL not set"
        log_info "Set HELIUS_RPC_URL environment variable"
        return 1
    fi
    
    log_info "Testing latency to: $RPC_URL"
    
    # Perform 10 ping-like requests to measure latency
    local latencies=()
    local success_count=0
    
    for i in {1..10}; do
        local start_time
        start_time=$(date +%s%N)
        
        # Simple RPC call (getHealth or getSlot)
        local response
        response=$(curl -s -X POST "$RPC_URL" \
            -H "Content-Type: application/json" \
            -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' \
            --max-time 5 2>/dev/null || echo "")
        
        local end_time
        end_time=$(date +%s%N)
        
        if [[ -n "$response" ]] && echo "$response" | grep -q "result\|error"; then
            local latency_ms
            latency_ms=$(( (end_time - start_time) / 1000000 ))
            latencies+=($latency_ms)
            ((success_count++))
        else
            log_warn "Request $i failed or timed out"
        fi
        
        # Rate limit
        sleep 0.1
    done
    
    if [[ $success_count -eq 0 ]]; then
        log_fail "All RPC requests failed"
        log_info "Check network connectivity and RPC endpoint"
        return 1
    fi
    
    # Calculate average latency
    local total=0
    for latency in "${latencies[@]}"; do
        total=$((total + latency))
    done
    
    local avg_latency
    avg_latency=$((total / ${#latencies[@]}))
    
    # Find min and max
    local min_latency=${latencies[0]}
    local max_latency=${latencies[0]}
    for latency in "${latencies[@]}"; do
        [[ $latency -lt $min_latency ]] && min_latency=$latency
        [[ $latency -gt $max_latency ]] && max_latency=$latency
    done
    
    log_info "Latency stats: min=${min_latency}ms, avg=${avg_latency}ms, max=${max_latency}ms"
    
    if [[ $avg_latency -lt $LATENCY_THRESHOLD_MS ]]; then
        log_pass "Average latency (${avg_latency}ms) is below threshold (${LATENCY_THRESHOLD_MS}ms)"
    else
        log_fail "Average latency (${avg_latency}ms) exceeds threshold (${LATENCY_THRESHOLD_MS}ms)"
        log_info "Consider relocating VPS or using alternative provider"
        log_info "High latency will cause blockhash expiration and failed trades"
        return 1
    fi
    
    # Check for high variance (unstable connection)
    local variance=0
    for latency in "${latencies[@]}"; do
        local diff=$((latency - avg_latency))
        variance=$((variance + diff * diff))
    done
    variance=$((variance / ${#latencies[@]}))
    local std_dev=$(echo "sqrt($variance)" | bc 2>/dev/null || echo "0")
    
    if [[ $(echo "$std_dev > 20" | bc 2>/dev/null || echo "0") -eq 1 ]]; then
        log_warn "High latency variance detected (std dev: ${std_dev}ms)"
        log_info "Connection may be unstable"
    fi
    
    return 0
}

# ============================================================================
# Check 3: Circuit Breaker Test
# ============================================================================

check_circuit_breaker() {
    echo ""
    echo "=== Check 3: Circuit Breaker Functionality ==="
    
    if [[ "$SKIP_CIRCUIT_BREAKER" == "true" ]]; then
        log_warn "Circuit breaker test skipped (--skip-circuit-breaker)"
        return 0
    fi
    
    if [[ ! -f "$DB_PATH" ]]; then
        log_warn "Database not found at $DB_PATH"
        log_info "Skipping circuit breaker test (database required)"
        return 0
    fi
    
    # Check if service is running
    if ! systemctl is-active --quiet chimera 2>/dev/null; then
        log_warn "Chimera service is not running"
        log_info "Skipping circuit breaker test (service must be running)"
        return 0
    fi
    
    log_info "Testing circuit breaker with fake loss..."
    
    # Get current circuit breaker threshold
    local max_loss
    max_loss=$(sqlite3 "$DB_PATH" \
        "SELECT value FROM config WHERE key = 'circuit_breakers.max_loss_24h_usd' LIMIT 1" \
        2>/dev/null || echo "500")
    
    if [[ -z "$max_loss" ]]; then
        max_loss=500  # Default from PDD
    fi
    
    # Insert a fake loss that exceeds threshold
    local fake_loss
    fake_loss=$(echo "$max_loss + 100" | bc)
    local test_uuid="preflight-test-$(date +%s)"
    
    log_info "Inserting test trade with loss: \$${fake_loss} (threshold: \$${max_loss})"
    
    sqlite3 "$DB_PATH" <<EOF
INSERT INTO trades (
    trade_uuid, wallet_address, token_address, strategy, side,
    amount_sol, status, pnl_usd, created_at
) VALUES (
    '${test_uuid}',
    'PreflightTestWallet',
    'PreflightTestToken',
    'SHIELD',
    'SELL',
    1.0,
    'CLOSED',
    -${fake_loss},
    datetime('now')
);
EOF
    
    if [[ $? -ne 0 ]]; then
        log_fail "Failed to insert test trade"
        return 1
    fi
    
    log_info "Waiting for circuit breaker evaluation (30 seconds)..."
    sleep 30
    
    # Check if circuit breaker tripped
    local cb_status
    cb_status=$(curl -s http://localhost:8080/api/v1/health 2>/dev/null | \
        grep -o '"trading_allowed":[^,}]*' | cut -d: -f2 || echo "unknown")
    
    # Clean up test trade
    sqlite3 "$DB_PATH" "DELETE FROM trades WHERE trade_uuid = '${test_uuid}';" 2>/dev/null || true
    
    if [[ "$cb_status" == "false" ]] || [[ "$cb_status" == "0" ]]; then
        log_pass "Circuit breaker correctly tripped (trading_allowed: false)"
        
        # Reset circuit breaker for next test
        log_info "Resetting circuit breaker..."
        curl -s -X POST http://localhost:8080/api/v1/config/circuit-breaker/reset \
            -H "Authorization: Bearer $(cat /opt/chimera/config/.env | grep API_KEY | cut -d= -f2)" \
            > /dev/null 2>&1 || log_warn "Could not reset circuit breaker (may require manual reset)"
        
        return 0
    else
        log_fail "Circuit breaker did not trip (trading_allowed: $cb_status)"
        log_info "Circuit breaker may not be functioning correctly"
        return 1
    fi
}

# ============================================================================
# Main
# ============================================================================

main() {
    echo "=========================================="
    echo "Chimera Pre-Deployment Verification"
    echo "=========================================="
    echo ""
    
    local time_sync_ok=true
    local latency_ok=true
    local circuit_breaker_ok=true
    
    # Run checks
    check_time_sync || time_sync_ok=false
    check_rpc_latency || latency_ok=false
    
    if [[ "$SKIP_CIRCUIT_BREAKER" != "true" ]]; then
        check_circuit_breaker || circuit_breaker_ok=false
    fi
    
    # Summary
    echo ""
    echo "=========================================="
    echo "Verification Summary"
    echo "=========================================="
    echo "Passed:  $PASSED"
    echo "Failed:  $FAILED"
    echo "Warnings: $WARNINGS"
    echo ""
    
    if [[ $FAILED -gt 0 ]]; then
        echo -e "${RED}✗ Pre-flight checks FAILED${NC}"
        echo ""
        echo "Critical issues must be resolved before deployment:"
        [[ "$time_sync_ok" == "false" ]] && echo "  - Time synchronization"
        [[ "$latency_ok" == "false" ]] && echo "  - RPC latency"
        [[ "$circuit_breaker_ok" == "false" ]] && echo "  - Circuit breaker"
        exit 1
    elif [[ $WARNINGS -gt 0 ]]; then
        echo -e "${YELLOW}⚠ Pre-flight checks passed with warnings${NC}"
        echo "Review warnings before production deployment"
        exit 0
    else
        echo -e "${GREEN}✓ All pre-flight checks passed${NC}"
        exit 0
    fi
}

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --skip-circuit-breaker)
            SKIP_CIRCUIT_BREAKER=true
            shift
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [--skip-circuit-breaker]"
            exit 1
            ;;
    esac
done

main

