#!/bin/bash
# Chimera Paper Trading Simulation Script
# Simulates signal processing for comprehensive evaluation

set -e

echo "=========================================="
echo "Chimera Paper Trading Simulation"
echo "=========================================="
echo ""
echo "Starting comprehensive paper trading scenario..."
echo "Simulation Time: $(date)"
echo ""

# Configuration
SIGNAL_COUNT=50  # Process 50 signals for simulation
DAY_NUM=1
HOUR_START=9
EVAL_DIR="/evaluation"
SIGNAL_FILE="/evaluation/signals/historical_signals.jsonl"
OPERATOR_URL="http://localhost:8080"
METRICS_URL="http://localhost:8080/metrics"

echo "Configuration:"
echo "  Signals to process: ${SIGNAL_COUNT}"
echo "  Day number: ${DAY_NUM}"
echo "  Hour start: ${HOUR_START}"
echo "  Evaluation directory: ${EVAL_DIR}"
echo ""

# Check if operator is accessible
echo "Testing operator connectivity..."
if curl -sf "${OPERATOR_URL}/api/v1/health" > /dev/null; then
    echo "✅ Operator is accessible"
else
    echo "❌ Operator is not accessible"
    exit 1
fi

# Get initial metrics
echo ""
echo "Collecting initial system metrics..."
curl -s "${METRICS_URL}" | grep -E "chimera_" | head -10 || echo "Some metrics unavailable"

echo ""
echo "=========================================="
echo "Signal Processing Simulation"
echo "=========================================="

# Read and process signals
PROCESSED=0
SUCCESSFUL=0
FAILED=0

echo ""
echo "Processing ${SIGNAL_COUNT} historical signals..."
echo "Timestamp              Wallet        Token     Action  Amount   Strategy  Price   Status"
echo "-------------------  ------------  --------  ------  ------  --------  -----   ------"

while IFS= read -r line && [ $PROCESSED -lt $SIGNAL_COUNT ]; do
    # Parse JSON signal (tolerate malformed lines)
    timestamp=$(echo "$line" | jq -r '.timestamp' 2>/dev/null || true)
    wallet=$(echo "$line" | jq -r '.wallet_address' 2>/dev/null || true)
    token=$(echo "$line" | jq -r '.token_address' 2>/dev/null || true)
    action=$(echo "$line" | jq -r '.action' 2>/dev/null || true)
    amount=$(echo "$line" | jq -r '.amount_sol' 2>/dev/null || true)
    strategy=$(echo "$line" | jq -r '.strategy' 2>/dev/null || true)
    price=$(echo "$line" | jq -r '.price_usd' 2>/dev/null || true)

    if [ -z "$wallet" ] || [ -z "$token" ]; then
        FAILED=$((FAILED + 1))
        PROCESSED=$((PROCESSED + 1))
        printf "%-19s  %-12s  %-8s  %-7s  %-7s  %-9s  %-7s   %s\n" "" "" "" "" "" "" "" "❌ MALFORMED LINE"
        continue
    fi

    # Shorten wallet address for display
    wallet_short="${wallet:0:8}..."

    # Simulate signal submission to operator (payload built with jq so all
    # values are properly escaped for JSON)
    SIGNAL_DATA=$(jq -n --arg w "$wallet" --arg t "$token" --arg a "$action" --arg amt "$amount" \
        --arg s "$strategy" --arg ts "$timestamp" --arg p "$price" \
        '{wallet_address:$w, token_address:$t, action:$a, amount_sol:$amt, strategy:$s, timestamp:$ts, price_usd:$p}')

    # Submit signal to operator (guard transport failures so one bad request
    # cannot abort the whole simulation)
    HTTP_CODE=$(curl -s -w "%{http_code}" -o /dev/null -X POST "${OPERATOR_URL}/api/v1/signal" \
      -H "Content-Type: application/json" \
      -H "X-Webhook-Signature: simulation_$(date +%s)" \
      -d "$SIGNAL_DATA" || echo 000)

    if [ "$HTTP_CODE" = "200" ] || [ "$HTTP_CODE" = "201" ] || [ "$HTTP_CODE" = "202" ]; then
        STATUS="✅ ACCEPTED"
        SUCCESSFUL=$((SUCCESSFUL + 1))
    else
        STATUS="❌ REJECTED ($HTTP_CODE)"
        FAILED=$((FAILED + 1))
    fi

    printf "%-19s  %-12s  %-8s  %-7s  %-7s  %-9s  %-7s   %s\n" \
           "${timestamp:0:19}" "${wallet_short}" "${token:0:8}..." \
           "$action" "$amount" "$strategy" "$price" "$STATUS"

    PROCESSED=$((PROCESSED + 1))

    # Small delay between signals
    sleep 0.1
done < "$SIGNAL_FILE"

echo ""
echo "=========================================="
echo "Simulation Summary"
echo "=========================================="
echo "Total signals processed: $PROCESSED"
echo "Successful submissions: $SUCCESSFUL"
echo "Failed submissions: $FAILED"
if [ "$PROCESSED" -gt 0 ]; then
    echo "Success rate: $(awk -v s="$SUCCESSFUL" -v p="$PROCESSED" 'BEGIN { printf "%.1f%%", s*100/p }')"
else
    echo "Success rate: N/A"
fi
echo ""

# Collect final metrics
echo "Collecting final system metrics..."
echo "Operator Metrics:"
curl -s "${METRICS_URL}" | grep -E "chimera_trade_" | head -5 || echo "Trade metrics unavailable"

echo ""
echo "System Health:"
curl -s "${OPERATOR_URL}/api/v1/health" | jq -r '.' 2>/dev/null || echo "Health endpoint unavailable"

echo ""
echo "Anomaly Detection Status:"
docker logs chimera-anomaly-detector --tail 5 | grep -E "(Detected|anomalies)" | tail -2 || echo "No recent anomalies"

echo ""
echo "Data Collection Status:"
DATA_COUNT=$(ls -la "${EVAL_DIR}/day-${DAY_NUM}/" 2>/dev/null | grep -c "json\|log" || true)
if [ "$DATA_COUNT" -gt 0 ]; then
    echo "$DATA_COUNT data files in ${EVAL_DIR}/day-${DAY_NUM}/"
else
    echo "No data files yet"
fi

echo ""
echo "=========================================="
echo "Paper Trading Simulation Complete"
echo "=========================================="
echo "Results saved to evaluation database"
echo "Monitor real-time progress with:"
echo "  docker logs chimera-operator"
echo "  docker logs chimera-anomaly-detector"
echo "  docker logs chimera-data-collector"