#!/bin/bash
# Vector Performance Testing Script
# Tests Vector throughput and resource usage under load

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

TOTAL_GENERATED=11000

echo -e "${BLUE}=========================================="
echo "Vector Performance Testing"
echo -e "==========================================${NC}"
echo ""

# Check if Vector is running
echo -e "${BLUE}[1/5] Checking Vector Status${NC}"

if docker ps -q --filter "name=^/chimera-vector$" | grep -q .; then
    echo -e "${GREEN}✓ Vector container running${NC}"
else
    echo -e "${RED}✗ Vector container not running${NC}"
    echo "Start Vector with: docker-compose --profile evaluation up -d vector"
    exit 1
fi

# Generate performance test logs
echo ""
echo -e "${BLUE}[2/5] Generating High-Volume Test Logs${NC}"

echo "Generating 10,000 test log entries..."
TMP_OPERATOR=$(mktemp)
TMP_SCOUT=$(mktemp)
trap 'rm -f "$TMP_OPERATOR" "$TMP_SCOUT"' EXIT

# Generate entries locally with plain shell redirection, then flush into the
# live log files with a single sudo tee each (avoids 11,000 subprocess spawns
# and any interactive sudo prompts in the loop).
for i in {1..10000}; do
    TIMESTAMP=$(date -u +%Y-%m-%dT%H:%M:%S.%3NZ)

    # Mix of different log types
    case $((i % 4)) in
        0)
            # Trade event
            printf '{"timestamp":"%s","level":"INFO","message":"Trade executed","trade_uuid":"test-%s","wallet_address":"wallet_%s","token_address":"token_%s","strategy":"shield","trade_size":%s}\n' "$TIMESTAMP" "$i" "$i" "$i" "$((i % 5000))" >> "$TMP_OPERATOR"
            ;;
        1)
            # Performance metric
            printf '{"timestamp":"%s","level":"INFO","message":"Performance metric","latency":%s,"p95":%s,"p99":%s,"queue_depth":%s}\n' "$TIMESTAMP" "$((i % 100))" "$((i % 120))" "$((i % 150))" "$((i % 100))" >> "$TMP_OPERATOR"
            ;;
        2)
            # Security event
            printf '{"timestamp":"%s","level":"INFO","message":"Authentication successful","security_event":true,"source_ip":"192.168.1.%s"}\n' "$TIMESTAMP" "$((i % 255))" >> "$TMP_OPERATOR"
            ;;
        3)
            # Error event
            printf '{"timestamp":"%s","level":"ERROR","message":"Test error %s","error_level":true}\n' "$TIMESTAMP" "$i" >> "$TMP_OPERATOR"
            ;;
    esac
done

# Generate scout logs
for i in {1..1000}; do
    echo "$(date '+%Y-%m-%d %H:%M:%S') INFO Wallet analysis completed - wallet_analyzed: wallet_${i}, wqs_score: $((60 + i % 40)), discovery_count: $((10 + i % 90))" >> "$TMP_SCOUT"
done

OPERATOR_LINES=$(wc -l < "$TMP_OPERATOR")
SCOUT_LINES=$(wc -l < "$TMP_SCOUT")
TOTAL_GENERATED=$((OPERATOR_LINES + SCOUT_LINES))

# Flush into the live log files (fresh per run so counts reflect this run)
sudo mkdir -p /var/log/chimera
sudo tee -a /var/log/chimera/operator.log < "$TMP_OPERATOR" > /dev/null
sudo tee -a /var/log/chimera/scout.log < "$TMP_SCOUT" > /dev/null

echo -e "${GREEN}✓ $TOTAL_GENERATED test logs flushed (operator: $OPERATOR_LINES, scout: $SCOUT_LINES)${NC}"

# Measure processing time
echo ""
echo -e "${BLUE}[3/5] Measuring Processing Performance${NC}"

EVAL_TODAY="$(date +%Y-%m-%d)"
MAIN_OUTPUT="evaluation/logs/evaluation/chimera-${EVAL_TODAY}.log"

# Count the lines currently in the main output (cumulative across runs)
count_lines() {
    if [ -f "$1.gz" ]; then
        gzip -dc "$1.gz" 2>/dev/null | wc -l
    elif [ -f "$1" ]; then
        wc -l < "$1"
    else
        echo 0
    fi
}

BASELINE=$(count_lines "$MAIN_OUTPUT")

START_TIME=$(date +%s)
echo "Waiting for Vector to process the generated logs..."
PROCESSED=0
for i in $(seq 1 24); do
    sleep 5
    CURRENT=$(count_lines "$MAIN_OUTPUT")
    PROCESSED=$((CURRENT - BASELINE))
    if [ "$PROCESSED" -ge "$TOTAL_GENERATED" ]; then
        break
    fi
done
END_TIME=$(date +%s)
DURATION=$((END_TIME - START_TIME))

if [ "$PROCESSED" -ge "$TOTAL_GENERATED" ]; then
    echo -e "${GREEN}✓ Vector processed $PROCESSED logs in ~${DURATION} seconds${NC}"
    echo "  Average throughput: $((PROCESSED / DURATION)) logs/second (measured)"
else
    echo -e "${RED}✗ Only $PROCESSED/$TOTAL_GENERATED logs processed within 120 seconds${NC}"
    exit 1
fi

# Check resource usage
echo ""
echo -e "${BLUE}[4/5] Measuring Resource Usage${NC}"

STATS=$(docker stats chimera-vector --no-stream --format "{{.MemUsage}} {{.CPUPerc}}" 2>/dev/null || true)
if [ -n "$STATS" ]; then
    echo "  $STATS"
    CPU_PERC=$(echo "$STATS" | awk '{print $2}' | tr -d '%')
    MEM_MIB=$(echo "$STATS" | awk '{print $1}' | sed 's/[^0-9.]*$//')
    if [ "${CPU_PERC%%.*}" -lt 100 ] && [ "${MEM_MIB%%.*}" -lt 1024 ]; then
        echo -e "${GREEN}✓ Resource usage within limits (CPU < 100%, memory < 1GiB)${NC}"
    else
        echo -e "${RED}✗ Resource usage exceeds limits (CPU: ${CPU_PERC}%, memory: ${MEM_MIB}MiB)${NC}"
        exit 1
    fi
else
    echo -e "${YELLOW}⚠ Could not collect resource metrics${NC}"
fi

# Verify output quality
echo ""
echo -e "${BLUE}[5/5] Verifying Output Quality${NC}"

TOTAL_OUTPUT_FILES=0

# Check main output: this run must have produced at least the generated lines
MAIN_LINES=$(count_lines "$MAIN_OUTPUT")
MAIN_DELTA=$((MAIN_LINES - BASELINE))
if [ "$MAIN_DELTA" -ge "$TOTAL_GENERATED" ]; then
    echo -e "${GREEN}✓ Main output grew by $MAIN_DELTA lines (generated: $TOTAL_GENERATED)${NC}"
    TOTAL_OUTPUT_FILES=$((TOTAL_OUTPUT_FILES + 1))
else
    echo -e "${RED}✗ Main output grew by only $MAIN_DELTA lines (expected >= $TOTAL_GENERATED)${NC}"
    exit 1
fi

# Check specialized outputs
for file in operator scout performance errors security; do
    if [ -f "evaluation/logs/evaluation/${file}-${EVAL_TODAY}.log" ] || [ -f "evaluation/logs/evaluation/${file}-${EVAL_TODAY}.log.gz" ]; then
        LINES=$(count_lines "evaluation/logs/evaluation/${file}-${EVAL_TODAY}.log")
        echo -e "${GREEN}✓ ${file} output: ${LINES} lines${NC}"
        TOTAL_OUTPUT_FILES=$((TOTAL_OUTPUT_FILES + 1))
    fi
done

echo -e "${GREEN}✓ Total output files found: ${TOTAL_OUTPUT_FILES}${NC}"

# Performance summary
echo ""
echo -e "${GREEN}=========================================="
echo "Performance Tests Complete!"
echo -e "==========================================${NC}"
echo ""
echo "Performance Summary:"
echo "  Logs generated: $TOTAL_GENERATED"
echo "  Processing time: ~${DURATION} seconds (measured)"
echo "  Average throughput: $((PROCESSED / DURATION)) logs/second (measured)"
echo "  Output files found: ${TOTAL_OUTPUT_FILES}"
echo ""
echo "Expected Vector Performance:"
echo "  Throughput: 10x better than Fluentd"
echo "  Memory usage: 50% lower than Fluentd"
echo "  CPU efficiency: Significantly improved"
echo ""
echo "Next steps:"
echo "  1. Monitor resource usage: ./ops/monitor-vector.sh"
echo "  2. Compare with Fluentd baseline (if available)"
echo "  3. Deploy to production: ./ops/migrate-to-vector.sh"
