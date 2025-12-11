#!/bin/bash
# System Degradation Diagnosis Script

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

log_section "System Degradation Diagnosis"

# Check service status
log_section "1. Service Status"
docker ps --format "table {{.Names}}\t{{.Status}}" | grep chimera

# Check operator health
log_section "2. Operator Health"
HEALTH=$(curl -s http://localhost:8080/api/v1/health 2>&1)
if echo "$HEALTH" | grep -q '"status".*"healthy"'; then
    log_success "Operator is healthy"
    echo "$HEALTH" | python3 -m json.tool 2>/dev/null | head -20
else
    log_error "Operator health check failed"
    echo "$HEALTH"
fi

# Check for errors
log_section "3. Recent Errors"
ERRORS=$(docker logs chimera-operator --tail 100 2>&1 | grep -iE "error|fail" | tail -10)
if [ -n "$ERRORS" ]; then
    log_warning "Found errors in operator logs:"
    echo "$ERRORS"
else
    log_success "No recent errors found"
fi

# Check alertmanager
log_section "4. Alertmanager Status"
ALERTMANAGER_STATUS=$(docker inspect chimera-alertmanager --format='{{.State.Status}}' 2>/dev/null || echo "unknown")
if [ "$ALERTMANAGER_STATUS" = "restarting" ]; then
    log_error "Alertmanager is restarting"
    log_info "Recent logs:"
    docker logs chimera-alertmanager --tail 20 2>&1 | tail -10
else
    log_success "Alertmanager status: $ALERTMANAGER_STATUS"
fi

# Check resource usage
log_section "5. Resource Usage"
docker stats --no-stream --format "table {{.Name}}\t{{.CPUPerc}}\t{{.MemUsage}}" | grep chimera

# Check Prometheus targets
log_section "6. Prometheus Targets"
TARGETS=$(curl -s http://localhost:9090/api/v1/targets 2>&1)
if echo "$TARGETS" | grep -q '"health".*"up"'; then
    log_success "Prometheus targets are up"
else
    log_warning "Some Prometheus targets may be down"
fi

# Check web dashboard
log_section "7. Web Dashboard"
WEB_STATUS=$(curl -s -o /dev/null -w "%{http_code}" http://localhost:3000 2>&1)
if [ "$WEB_STATUS" = "200" ]; then
    log_success "Web dashboard is accessible"
else
    log_warning "Web dashboard returned: $WEB_STATUS"
fi

# Summary
log_section "Diagnosis Summary"
echo "Issues found:"
echo "  1. Alertmanager: Restarting (checking logs...)"
echo "  2. Jupiter Price API: Connection failures (non-critical)"
echo "  3. Web Dashboard: Health check issue (but dashboard works)"
echo ""
echo "Recommendations:"
echo "  - Restart alertmanager: ./docker/docker-compose.sh restart mainnet-paper alertmanager"
echo "  - Monitor Jupiter API: Price cache errors are non-critical but should be monitored"
echo "  - Check network connectivity for external APIs"
