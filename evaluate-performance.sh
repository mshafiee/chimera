#!/bin/bash
# Performance Evaluation Script
# Evaluates logs and performance metrics for the Chimera trading bot

set -e

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
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
    echo -e "${CYAN}========================================${NC}"
    echo -e "${CYAN}$1${NC}"
    echo -e "${CYAN}========================================${NC}"
    echo ""
}

PROJECT_ROOT="$(cd "$(dirname "$0")" && pwd)"
DB_PATH="$PROJECT_ROOT/data/chimera.db"

log_section "Chimera Performance Evaluation"

# 1. System Health
log_section "1. System Health Status"
HEALTH=$(curl -s http://localhost:8080/api/v1/health 2>/dev/null)
if [ -n "$HEALTH" ]; then
    echo "$HEALTH" | python3 -m json.tool 2>/dev/null || echo "$HEALTH"
else
    log_error "Could not fetch health status"
fi

# 2. Wallet Roster Status
log_section "2. Wallet Roster Analysis"
if [ -f "$DB_PATH" ]; then
    TOTAL=$(sqlite3 "$DB_PATH" "SELECT COUNT(*) FROM wallets;" 2>/dev/null || echo "0")
    ACTIVE=$(sqlite3 "$DB_PATH" "SELECT COUNT(*) FROM wallets WHERE status='ACTIVE';" 2>/dev/null || echo "0")
    CANDIDATE=$(sqlite3 "$DB_PATH" "SELECT COUNT(*) FROM wallets WHERE status='CANDIDATE';" 2>/dev/null || echo "0")
    REJECTED=$(sqlite3 "$DB_PATH" "SELECT COUNT(*) FROM wallets WHERE status='REJECTED';" 2>/dev/null || echo "0")
    
    log_info "Total wallets: $TOTAL"
    log_info "  ACTIVE: $ACTIVE"
    log_info "  CANDIDATE: $CANDIDATE"
    log_info "  REJECTED: $REJECTED"
    
    if [ "$TOTAL" -gt 0 ]; then
        AVG_WQS=$(sqlite3 "$DB_PATH" "SELECT ROUND(AVG(wqs_score), 2) FROM wallets WHERE wqs_score IS NOT NULL;" 2>/dev/null || echo "N/A")
        log_info "Average WQS Score: $AVG_WQS"
        
        sqlite3 "$DB_PATH" "SELECT status, COUNT(*) as count, ROUND(AVG(wqs_score), 2) as avg_wqs, ROUND(AVG(trade_count_30d), 1) as avg_trades FROM wallets GROUP BY status;" 2>/dev/null | column -t -s '|' || true
    fi
else
    log_warning "Database not found"
fi

# 3. Trading Performance
log_section "3. Trading Performance Metrics"
PERF=$(curl -s http://localhost:8080/api/v1/metrics/performance 2>/dev/null)
if [ -n "$PERF" ]; then
    echo "$PERF" | python3 -m json.tool 2>/dev/null || echo "$PERF"
else
    log_warning "Could not fetch performance metrics"
fi

# 4. Cost Analysis
log_section "4. Cost Metrics (30-day)"
COSTS=$(curl -s http://localhost:8080/api/v1/metrics/costs 2>/dev/null)
if [ -n "$COSTS" ]; then
    echo "$COSTS" | python3 -m json.tool 2>/dev/null || echo "$COSTS"
else
    log_warning "Could not fetch cost metrics"
fi

# 5. Recent Trades
log_section "5. Recent Trading Activity"
TRADES=$(curl -s "http://localhost:8080/api/v1/trades?limit=10" 2>/dev/null)
if [ -n "$TRADES" ]; then
    TRADE_COUNT=$(echo "$TRADES" | python3 -c "import sys, json; data=json.load(sys.stdin); print(len(data.get('trades', [])))" 2>/dev/null || echo "0")
    log_info "Recent trades: $TRADE_COUNT"
    if [ "$TRADE_COUNT" -gt 0 ]; then
        echo "$TRADES" | python3 -m json.tool 2>/dev/null | head -50 || echo "$TRADES" | head -30
    else
        log_info "No trades yet - bot is waiting for signals"
    fi
else
    log_warning "Could not fetch trades"
fi

# 6. Error Analysis
log_section "6. Error Analysis (Last 50 lines)"
ERRORS=$(docker logs chimera-operator --tail 50 2>&1 | grep -iE "ERROR|WARN" | tail -20)
if [ -n "$ERRORS" ]; then
    echo "$ERRORS"
    ERROR_COUNT=$(echo "$ERRORS" | grep -i "ERROR" | wc -l | tr -d ' ')
    WARN_COUNT=$(echo "$ERRORS" | grep -i "WARN" | wc -l | tr -d ' ')
    log_info "Errors in last 50 lines: $ERROR_COUNT"
    log_info "Warnings in last 50 lines: $WARN_COUNT"
else
    log_success "No recent errors or warnings found"
fi

# 7. Resource Usage
log_section "7. Resource Usage"
docker stats --no-stream --format "table {{.Name}}\t{{.CPUPerc}}\t{{.MemUsage}}\t{{.NetIO}}" 2>/dev/null | grep chimera || log_warning "Could not fetch resource stats"

# 8. Service Status
log_section "8. Service Status"
docker ps --format "table {{.Names}}\t{{.Status}}\t{{.Ports}}" | grep chimera || log_warning "No Chimera services found"

# 9. Log Summary
log_section "9. Recent Activity Summary"
log_info "Operator logs (last 10 lines):"
docker logs chimera-operator --tail 10 2>&1 | tail -10 || log_warning "Could not fetch operator logs"

echo ""
log_info "Scout logs (last 5 lines):"
docker logs chimera-scout --tail 5 2>&1 | tail -5 || log_warning "Could not fetch scout logs"

# 10. Recommendations
log_section "10. Recommendations"
if [ "$TOTAL" -eq 0 ]; then
    log_warning "No wallets in roster - run scout to discover wallets"
fi

if [ "$ACTIVE" -eq 0 ] && [ "$TOTAL" -gt 0 ]; then
    log_warning "No ACTIVE wallets - check WQS thresholds or scout configuration"
fi

log_info "Monitor live activity:"
echo "  - Live logs: ./docker/docker-compose.sh logs mainnet-paper -f"
echo "  - Dashboard: http://localhost:3000"
echo "  - API: http://localhost:8080"
echo "  - Metrics: http://localhost:9090 (Prometheus)"
echo "  - Grafana: http://localhost:3002"

log_section "Evaluation Complete"
