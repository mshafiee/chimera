#!/bin/bash
# Vector Monitoring Script
# Monitor Vector log aggregation performance and health for Chimera evaluation

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
VECTOR_HOST="${VECTOR_HOST:-localhost}"
VECTOR_PORT="${VECTOR_PORT:-8383}"
VECTOR_URL="http://${VECTOR_HOST}:${VECTOR_PORT}"

echo -e "${BLUE}=========================================="
echo "Vector Monitoring Dashboard"
echo -e "==========================================${NC}"
echo ""

# Health check
echo -e "${BLUE}[Health Check]${NC}"
echo -n "Vector Service: "
if curl -sf "${VECTOR_URL}/health" > /dev/null 2>&1; then
    echo -e "${GREEN}✓ Healthy${NC}"
else
    echo -e "${RED}✗ Unhealthy${NC}"
    exit 1
fi

echo ""
echo -e "${BLUE}[Throughput Metrics]${NC}"
echo "Events processed per second:"
curl -sf "${VECTOR_URL}/metrics" 2>/dev/null | grep "vector_processed_events_total" | tail -5 || echo "  No metrics available"

echo ""
echo -e "${BLUE}[Buffer Usage]${NC}"
echo "Buffer utilization by component:"
curl -sf "${VECTOR_URL}/metrics" 2>/dev/null | grep "vector_buffer_byte_size" | grep -v "#" || echo "  No buffer metrics available"

echo ""
echo -e "${BLUE}[Error Rates]${NC}"
echo "Errors in the last minute:"
curl -sf "${VECTOR_URL}/metrics" 2>/dev/null | grep "vector_errors_total" | grep -v "#" || echo "  No error metrics available"

echo ""
echo -e "${BLUE}[Resource Usage]${NC}"
if docker ps | grep -q "chimera-vector"; then
    echo "Container Status:"
    docker stats chimera-vector --no-stream --format "  CPU: {{.CPUPerc}}\n  Memory: {{.MemUsage}}\n  Network: {{.NetIO}}}"
else
    echo -e "${YELLOW}⚠ Vector container not running${NC}"
fi

echo ""
echo -e "${BLUE}[Pipeline Status]${NC}"
echo "Active components:"
curl -sf "${VECTOR_URL}/metrics" 2>/dev/null | grep "vector_component" | grep -v "#" | head -10 || echo "  No component metrics available"

echo ""
echo -e "${BLUE}[Log Output Status]${NC}"
if [ -d "evaluation/logs/evaluation" ]; then
    echo "Recent log files:"
    ls -lth evaluation/logs/evaluation/ | head -8
    echo ""
    echo "Total log size:"
    du -sh evaluation/logs/evaluation/
else
    echo -e "${YELLOW}⚠ Log directory not found${NC}"
fi

echo ""
echo -e "${BLUE}[Top Sources]${NC}"
echo "Highest volume log sources:"
curl -sf "${VECTOR_URL}/metrics" 2>/dev/null | grep "vector_events_in_total" | sort -t: -k2 -rn | head -5 || echo "  No source metrics available"

echo ""
echo -e "${BLUE}[Top Sinks]${NC}"
echo "Highest output destinations:"
curl -sf "${VECTOR_URL}/metrics" 2>/dev/null | grep "vector_events_out_total" | sort -t: -k2 -rn | head -5 || echo "  No sink metrics available"

echo ""
echo -e "${BLUE}=========================================="
echo "Monitoring Complete"
echo -e "==========================================${NC}"
echo ""
echo "Quick Commands:"
echo "  Real-time metrics: watch -n 5 'curl -s ${VECTOR_URL}/metrics | grep vector_'"
echo "  Vector logs: docker logs -f chimera-vector"
echo "  View topology: curl -s ${VECTOR_URL}/topology | jq ."
echo "  Restart Vector: docker-compose restart vector"