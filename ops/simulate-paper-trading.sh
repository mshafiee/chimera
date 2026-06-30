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
    # Parse JSON signal
    timestamp=$(echo "$line" | jq -r '.timestamp')
    wallet=$(echo "$line" | jq -r '.wallet_address')
    token=$(echo "$line" | jq -r '.token_address')
    action=$(echo "$line" | jq -r '.action')
    amount=$(echo "$line" | jq -r '.amount_sol')
    strategy=$(echo "$line" jq -r '.strategy')
    price=$(echo "$line | jq -r '.price_usd')

    # Shorten wallet address for display
    wallet_short="${wallet:0:8}..."

    # Simulate signal submission to operator
    SIGNAL_DATA=$(cat <<EOF
{
  "wallet_address": "$wallet",
  "token_address": "$token",
  "action": "$action",
  "amount_sol": $amount,
  "strategy": "$strategy",
  "timestamp": "$timestamp",
  "price_usd": $price
}
EOF
)

    # Submit signal to operator
    HTTP_CODE=$(curl -s -w "%{http_code}" -o /dev/null -X POST "${OPERATOR_URL}/api/v1/signal" \
      -H "Content-Type: application/json" \
      -H "X-Webhook-Signature: simulation_$(date +%s)" \
      -d "$SIGNAL_DATA")

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
echo "Success rate: $(echo "scale=1; $SUCCESSFUL * 100 / $PROCESSED" | bc)%"
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
ls -la "${EVAL_DIR}/day-${DAY_NUM}/" 2>/dev/null | grep -c "json\|log" || echo "No data files yet"

echo ""
echo "=========================================="
echo "Paper Trading Simulation Complete"
echo "=========================================="
echo "Results saved to evaluation database"
echo "Monitor real-time progress with:"
echo "  docker logs chimera-operator"
echo "  docker logs chimera-anomaly-detector"
echo "  docker logs chimera-data-collector"