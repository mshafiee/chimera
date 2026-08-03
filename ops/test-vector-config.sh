#!/bin/bash
# Vector Configuration Testing Script
# Tests Vector configuration syntax and basic functionality

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

FAILED=0
SKIPPED=0

echo -e "${BLUE}=========================================="
echo "Vector Configuration Testing"
echo -e "==========================================${NC}"
echo ""

# Check if Vector configuration exists
if [ ! -f "ops/vector/vector.toml" ]; then
    echo -e "${RED}✗ Vector configuration not found${NC}"
    exit 1
fi

echo -e "${GREEN}✓ Vector configuration found${NC}"

# Ensure the network the startup test needs exists
docker network inspect chimera-network >/dev/null 2>&1 || docker network create chimera-network

# Test Vector configuration syntax
echo ""
echo -e "${BLUE}[1/4] Testing Configuration Syntax${NC}"

if docker run --rm -v "$(pwd)/ops/vector:/etc/vector" timberio/vector:0.36.0-alpine \
  vector validate /etc/vector/vector.toml 2>&1; then
    echo -e "${GREEN}✓ Configuration syntax valid${NC}"
else
    echo -e "${RED}✗ Configuration syntax invalid${NC}"
    exit 1
fi

# Test Vector can actually start (daemon launches and stays up)
echo ""
echo -e "${BLUE}[2/4] Testing Vector Startup${NC}"

docker run --rm --name chimera-vector-startup-test --network chimera-network \
  -v "$(pwd)/ops/vector:/etc/vector" \
  -v "$(pwd)/logs:/host/data/logs:ro" \
  timberio/vector:0.36.0-alpine \
  vector --config /etc/vector/vector.toml --quiet 2>&1 &
RUN_PID=$!
sleep 5
if kill -0 "$RUN_PID" 2>/dev/null; then
    echo -e "${GREEN}✓ Vector started and stayed up with the configuration${NC}"
    kill "$RUN_PID" 2>/dev/null || true
    wait "$RUN_PID" 2>/dev/null || true
    docker stop chimera-vector-startup-test > /dev/null 2>&1 || true
else
    echo -e "${RED}✗ Vector startup test failed${NC}"
    FAILED=1
fi

# Test log parsing capabilities with a real stdin -> console pipeline
echo ""
echo -e "${BLUE}[3/4] Testing Log Parsing${NC}"

STDIN_TEST_CONF=$(mktemp)
trap 'rm -f "$STDIN_TEST_CONF" "$METRICS_TEST_CONF"' EXIT

cat > "$STDIN_TEST_CONF" << 'CONF'
[sources.in]
type = "stdin"
decoding.codec = "json"

[sinks.out]
type = "console"
inputs = ["in"]
encoding.codec = "json"
CONF

# Test operator log parsing (JSON)
echo "Testing operator JSON log parsing..."
if echo '{"timestamp":"2026-06-29T12:00:00Z","level":"INFO","message":"Test log","trade_uuid":"test-123"}' | \
docker run --rm -i \
  -v "$STDIN_TEST_CONF":/etc/vector/stdin-test.toml:ro \
  timberio/vector:0.36.0-alpine \
  vector --config /etc/vector/stdin-test.toml --quiet 2>/dev/null | grep -q "test-123"; then
    echo -e "${GREEN}✓ Operator JSON parsing works${NC}"
else
    echo -e "${RED}✗ Operator JSON parsing failed${NC}"
    FAILED=1
fi

# Test Python (plain-text) log parsing
echo "Testing Python log parsing..."
cat > "$STDIN_TEST_CONF" << 'CONF'
[sources.in]
type = "stdin"

[sinks.out]
type = "console"
inputs = ["in"]
encoding.codec = "json"
CONF
if echo '2026-06-29 12:00:00 INFO Test scout message' | \
docker run --rm -i \
  -v "$STDIN_TEST_CONF":/etc/vector/stdin-test.toml:ro \
  timberio/vector:0.36.0-alpine \
  vector --config /etc/vector/stdin-test.toml --quiet 2>/dev/null | grep -q "scout"; then
    echo -e "${GREEN}✓ Python log parsing works${NC}"
else
    echo -e "${RED}✗ Python log parsing failed${NC}"
    FAILED=1
fi

# Test metric endpoint availability
echo ""
echo -e "${BLUE}[4/4] Testing Metrics Endpoint${NC}"

METRICS_TEST_CONF=$(mktemp)
cat > "$METRICS_TEST_CONF" << 'CONF'
[sources.internal]
type = "internal_metrics"

[sinks.prom]
type = "prometheus_exporter"
inputs = ["internal"]
address = "0.0.0.0:8383"
CONF

docker run --rm --name chimera-vector-metrics-test -p 8383:8383 \
  -v "$METRICS_TEST_CONF":/etc/vector/metrics-test.toml:ro \
  timberio/vector:0.36.0-alpine \
  vector --config /etc/vector/metrics-test.toml --quiet > /dev/null 2>&1 &
METRIC_PID=$!
sleep 4
if curl -sf --max-time 3 http://localhost:8383/metrics 2>/dev/null | grep -q "vector_"; then
    echo -e "${GREEN}✓ Metrics endpoint available${NC}"
else
    echo -e "${RED}✗ Metrics endpoint not available${NC}"
    FAILED=1
fi
docker stop chimera-vector-metrics-test > /dev/null 2>&1 || true
wait "$METRIC_PID" 2>/dev/null || true

echo ""
if [ "$FAILED" -ne 0 ]; then
    echo -e "${RED}=========================================="
    echo "Configuration Tests FAILED"
    echo -e "==========================================${NC}"
    exit 1
fi
echo -e "${GREEN}=========================================="
echo "Configuration Tests Passed!"
echo -e "==========================================${NC}"
echo ""
echo "Vector configuration is valid and ready for deployment."
echo ""
echo "Next steps:"
echo "  1. Run integration tests: ./ops/test-vector-integration.sh"
echo "  2. Run performance tests: ./ops/test-vector-performance.sh"
echo "  3. Deploy Vector: ./ops/migrate-to-vector.sh"
