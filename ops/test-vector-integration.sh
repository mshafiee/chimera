#!/bin/bash
# Vector Integration Testing Script
# Tests Vector log collection and processing with real logs

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

FAILED=0

EVAL_DIR="${EVAL_DIR:-/opt/chimera/evaluation}"
LOG_DIR="${EVAL_DIR}/logs/evaluation"

echo -e "${BLUE}=========================================="
echo "Vector Integration Testing"
echo -e "==========================================${NC}"
echo ""

# Check if Vector is running
echo -e "${BLUE}[1/6] Checking Vector Status${NC}"

if docker ps -q --filter "name=^/chimera-vector$" | grep -q .; then
    echo -e "${GREEN}✓ Vector container running${NC}"
else
    echo -e "${RED}✗ Vector container not running${NC}"
    echo "Start Vector with: docker-compose --profile evaluation up -d vector"
    exit 1
fi

# Test health endpoint
echo ""
echo -e "${BLUE}[2/6] Testing Health Endpoint${NC}"

if curl -sf --max-time 5 http://localhost:8383/health > /dev/null 2>&1; then
    echo -e "${GREEN}✓ Health endpoint responding${NC}"
else
    echo -e "${RED}✗ Health endpoint not responding${NC}"
    exit 1
fi

# Generate test logs
echo ""
echo -e "${BLUE}[3/6] Generating Test Logs${NC}"

# Preflight: /var/log/chimera must be writable via sudo
if ! command -v sudo >/dev/null 2>&1 || ! sudo test -w /var/log/chimera 2>/dev/null; then
    echo -e "${RED}✗ Cannot write to /var/log/chimera (sudo required)${NC}" >&2
    exit 1
fi
sudo mkdir -p /var/log/chimera

# Unique marker per run so cleanup never removes another run's lines
MARKER="test-integration-123-$$"
OPERATOR_MARKER_LINE='{"timestamp":"'"$(date -u +%Y-%m-%dT%H:%M:%SZ)"'","level":"INFO","message":"Integration test log","trade_uuid":"'"$MARKER"'","wallet_address":"test_wallet","token_address":"test_token","strategy":"shield"}'
SCOUT_MARKER_LINE="$(date -u '+%Y-%m-%d %H:%M:%S') INFO Integration test - wallet analyzed, wqs_score: 75.5, discovery_count: 42"

echo "$OPERATOR_MARKER_LINE" | sudo tee -a /var/log/chimera/operator.log > /dev/null
echo "$SCOUT_MARKER_LINE" | sudo tee -a /var/log/chimera/scout.log > /dev/null

echo -e "${GREEN}✓ Test logs generated (marker: ${MARKER})${NC}"

# Cleanup: remove the injected lines after verification
cleanup_test_logs() {
    sudo sed -i '' "/${MARKER}/d" /var/log/chimera/operator.log 2>/dev/null || \
        sudo sed -i "/${MARKER}/d" /var/log/chimera/operator.log 2>/dev/null || true
}
trap cleanup_test_logs EXIT

# Wait for log processing (poll up to 60s)
echo ""
echo -e "${BLUE}[4/6] Waiting for Log Processing${NC}"
EVAL_TODAY="$(date -u +%Y-%m-%d)"
for _ in $(seq 1 60); do
    if grep -q "$MARKER" "${LOG_DIR}/chimera-${EVAL_TODAY}.log" 2>/dev/null; then
        break
    fi
    sleep 1
done

# Verify main evaluation log
echo ""
echo -e "${BLUE}[5/6] Verifying Log Output${NC}"

if [ -f "${LOG_DIR}/chimera-${EVAL_TODAY}.log" ]; then
    echo -e "${GREEN}✓ Main evaluation log created${NC}"

    # Check if test logs are present
    if grep -q "$MARKER" "${LOG_DIR}/chimera-${EVAL_TODAY}.log"; then
        echo -e "${GREEN}✓ Operator test logs found in evaluation output${NC}"
    else
        echo -e "${RED}✗ Operator test logs not found in evaluation output${NC}"
        FAILED=1
    fi
else
    echo -e "${RED}✗ Main evaluation log not created${NC}"
    FAILED=1
fi

# Verify specialized outputs
if [ -f "${LOG_DIR}/operator-${EVAL_TODAY}.log" ]; then
    if grep -q "$MARKER" "${LOG_DIR}/operator-${EVAL_TODAY}.log"; then
        echo -e "${GREEN}✓ Operator specialized log contains test entry${NC}"
    else
        echo -e "${YELLOW}⚠ Operator specialized log exists but test entry not found${NC}"
        FAILED=1
    fi
else
    echo -e "${RED}✗ Operator specialized log not created${NC}"
    FAILED=1
fi

if [ -f "${LOG_DIR}/scout-${EVAL_TODAY}.log" ]; then
    if grep -q "discovery_count: 42" "${LOG_DIR}/scout-${EVAL_TODAY}.log"; then
        echo -e "${GREEN}✓ Scout specialized log contains test entry${NC}"
    else
        echo -e "${RED}✗ Scout specialized log exists but test entry not found${NC}"
        FAILED=1
    fi
else
    echo -e "${RED}✗ Scout specialized log not created${NC}"
    FAILED=1
fi

# Verify metrics collection
echo ""
echo -e "${BLUE}[6/6] Testing Metrics Collection${NC}"

if curl -sf --max-time 5 http://localhost:8383/metrics | grep -q "vector_"; then
    echo -e "${GREEN}✓ Vector metrics exposed${NC}"

    # Check for throughput metrics
    if curl -sf --max-time 5 http://localhost:8383/metrics | grep -q "vector_processed_events_total"; then
        echo -e "${GREEN}✓ Throughput metrics available${NC}"
    fi
else
    echo -e "${RED}✗ Vector metrics not exposed${NC}"
    FAILED=1
fi

# Summary
echo ""
if [ "$FAILED" -ne 0 ]; then
    echo -e "${RED}=========================================="
    echo "Integration Tests FAILED"
    echo -e "==========================================${NC}"
    exit 1
fi
echo -e "${GREEN}=========================================="
echo "Integration Tests Complete!"
echo -e "==========================================${NC}"
echo ""
echo "Log file status:"
ls -la "${LOG_DIR}" 2>/dev/null | tail -5 || echo "  No log files found yet"
echo ""
echo "Vector metrics:"
curl -sf --max-time 5 http://localhost:8383/metrics | grep "vector_processed_events_total" | tail -1 || echo "  No metrics available"
echo ""
echo "Next steps:"
echo "  1. Monitor Vector: ./ops/monitor-vector.sh"
echo "  2. Run performance tests: ./ops/test-vector-performance.sh"
echo "  3. Deploy to production: ./ops/migrate-to-vector.sh"
