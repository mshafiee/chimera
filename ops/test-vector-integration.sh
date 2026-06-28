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

EVAL_DIR="${EVAL_DIR:-/opt/chimera/evaluation}"

echo -e "${BLUE}=========================================="
echo "Vector Integration Testing"
echo -e "==========================================${NC}"
echo ""

# Check if Vector is running
echo -e "${BLUE}[1/6] Checking Vector Status${NC}"

if docker ps | grep -q "chimera-vector"; then
    echo -e "${GREEN}✓ Vector container running${NC}"
else
    echo -e "${RED}✗ Vector container not running${NC}"
    echo "Start Vector with: docker-compose --profile evaluation up -d vector"
    exit 1
fi

# Test health endpoint
echo ""
echo -e "${BLUE}[2/6] Testing Health Endpoint${NC}"

if curl -sf http://localhost:8383/health > /dev/null 2>&1; then
    echo -e "${GREEN}✓ Health endpoint responding${NC}"
else
    echo -e "${RED}✗ Health endpoint not responding${NC}"
    exit 1
fi

# Generate test logs
echo ""
echo -e "${BLUE}[3/6] Generating Test Logs${NC}"

# Create test operator log
sudo mkdir -p /var/log/chimera
echo '{"timestamp":"'"$(date -u +%Y-%m-%dT%H:%M:%SZ)"'","level":"INFO","message":"Integration test log","trade_uuid":"test-integration-123","wallet_address":"test_wallet","token_address":"test_token","strategy":"shield"}' | \
  sudo tee -a /var/log/chimera/operator.log > /dev/null

# Create test scout log
echo "$(date '+%Y-%m-%d %H:%M:%S') INFO Integration test - wallet analyzed, wqs_score: 75.5, discovery_count: 42" | \
  sudo tee -a /var/log/chimera/scout.log > /dev/null

echo -e "${GREEN}✓ Test logs generated${NC}"

# Wait for log processing
echo ""
echo -e "${BLUE}[4/6] Waiting for Log Processing${NC}"
echo "Waiting 30 seconds for Vector to process logs..."
sleep 30

# Verify main evaluation log
echo ""
echo -e "${BLUE}[5/6] Verifying Log Output${NC}"

EVAL_TODAY="$(date +%Y-%m-%d)"
if [ -f "evaluation/logs/evaluation/chimera-${EVAL_TODAY}.log" ]; then
    echo -e "${GREEN}✓ Main evaluation log created${NC}"

    # Check if test logs are present
    if grep -q "test-integration-123" "evaluation/logs/evaluation/chimera-${EVAL_TODAY}.log"; then
        echo -e "${GREEN}✓ Operator test logs found in evaluation output${NC}"
    else
        echo -e "${YELLOW}⚠ Operator test logs not found yet${NC}"
    fi
else
    echo -e "${YELLOW}⚠ Main evaluation log not created yet${NC}"
fi

# Verify specialized outputs
if [ -f "evaluation/logs/evaluation/operator-${EVAL_TODAY}.log" ]; then
    echo -e "${GREEN}✓ Operator specialized log created${NC}"
else
    echo -e "${YELLOW}⚠ Operator specialized log not created yet${NC}"
fi

if [ -f "evaluation/logs/evaluation/scout-${EVAL_TODAY}.log" ]; then
    echo -e "${GREEN}✓ Scout specialized log created${NC}"
else
    echo -e "${YELLOW}⚠ Scout specialized log not created yet${NC}"
fi

# Verify metrics collection
echo ""
echo -e "${BLUE}[6/6] Testing Metrics Collection${NC}"

if curl -sf http://localhost:8383/metrics | grep -q "vector_"; then
    echo -e "${GREEN}✓ Vector metrics exposed${NC}"

    # Check for throughput metrics
    if curl -sf http://localhost:8383/metrics | grep -q "vector_processed_events_total"; then
        echo -e "${GREEN}✓ Throughput metrics available${NC}"
    fi
else
    echo -e "${RED}✗ Vector metrics not exposed${NC}"
fi

# Summary
echo ""
echo -e "${GREEN}=========================================="
echo "Integration Tests Complete!"
echo -e "==========================================${NC}"
echo ""
echo "Log file status:"
ls -la evaluation/logs/evaluation/ 2>/dev/null | tail -5 || echo "  No log files found yet"
echo ""
echo "Vector metrics:"
curl -sf http://localhost:8383/metrics | grep "vector_processed_events_total" | tail -1 || echo "  No metrics available"
echo ""
echo "Next steps:"
echo "  1. Monitor Vector: ./ops/monitor-vector.sh"
echo "  2. Run performance tests: ./ops/test-vector-performance.sh"
echo "  3. Deploy to production: ./ops/migrate-to-vector.sh"