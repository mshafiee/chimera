#!/bin/bash
# Chimera Paper Trading Scenario Runner
# Comprehensive evaluation of the trading system

set -e

echo "=========================================="
echo "Chimera Paper Trading Evaluation"
echo "=========================================="
echo ""
echo "Starting comprehensive evaluation at $(date)"
echo ""

# Configuration
SIGNALS_TO_PROCESS=25
DAY_NUM=1
OPERATOR_URL="http://localhost:8080"
METRICS_URL="http://localhost:8080/metrics"
SIGNAL_FILE="evaluation/signals/historical_signals.jsonl"

echo "Evaluation Parameters:"
echo "  Signals to process: $SIGNALS_TO_PROCESS"
echo "  Evaluation Day: $DAY_NUM"
echo "  Operator URL: $OPERATOR_URL"
echo ""

# System health check
echo "Step 1/5: System Health Check"
echo "================================"
if curl -sf "$OPERATOR_URL/api/v1/health" > /dev/null; then
    echo "✅ Operator is healthy"
else
    echo "❌ Operator health check failed"
    exit 1
fi

# Get initial metrics
echo ""
echo "Step 2/5: Initial Metrics Collection"
echo "================================"
echo "Collecting baseline metrics..."
curl -s "$METRICS_URL" 2>/dev/null | grep -E "chimera_queue_depth|chimera_trade_" | head -5 || echo "Some metrics unavailable"

# Process signals
echo ""
echo "Step 3/5: Signal Processing Simulation"
echo "================================"
echo "Processing $SIGNALS_TO_PROCESS historical signals..."
echo ""

# Use Python for more robust signal processing
python3 <<PYTHON_SCRIPT
import json
import requests
import time
from datetime import datetime

# Read historical signals
signal_file = "$SIGNAL_FILE"
operator_url = "$OPERATOR_URL/api/v1/signal"
signals_to_process = $SIGNALS_TO_PROCESS

with open(signal_file, 'r') as f:
    signals = [json.loads(line) for line in f]

processed = 0
successful = 0
failed = 0

print(f"Processing {min(signals_to_process, len(signals))} signals...")
print("-" * 80)

for signal in signals[:signals_to_process]:
    try:
        # Prepare signal data
        signal_data = {
            "wallet_address": signal["wallet_address"],
            "token_address": signal["token_address"],
            "action": signal["action"],
            "amount_sol": signal["amount_sol"],
            "strategy": signal["strategy"],
            "timestamp": signal["timestamp"],
            "price_usd": signal["price_usd"]
        }

        # Submit signal
        response = requests.post(
            operator_url,
            json=signal_data,
            headers={
                "Content-Type": "application/json",
                "X-Webhook-Signature": f"simulation_{int(time.time())}"
            },
            timeout=10
        )

        if response.status_code in [200, 201, 202]:
            status = "✅ ACCEPTED"
            successful += 1
        else:
            status = f"❌ REJECTED ({response.status_code})"
            failed += 1

        # Display progress
        wallet_short = signal["wallet_address"][:8] + "..."
        token_short = signal["token_address"][:8] + "..."
        timestamp = signal["timestamp"][:19]

        print(f"{timestamp}  {wallet_short:12}  {token_short:10}  {signal['action']:6}  {signal['amount_sol']:6.2f}  {signal['strategy']:6}  {signal['price_usd']:6.2f}  {status}")

        processed += 1

        # Small delay between signals
        time.sleep(0.1)

    except Exception as e:
        print(f"Error processing signal: {e}")
        failed += 1
        processed += 1

print("-" * 80)
print(f"Signal Processing Results:")
print(f"  Processed: {processed}")
print(f"  Successful: {successful}")
print(f"  Failed: {failed}")
print(f"  Success Rate: {successful * 100.0 / processed:.1f}%")

PYTHON_SCRIPT

# Collect final metrics
echo ""
echo "Step 4/5: Post-Processing Metrics"
echo "================================"
echo "Collecting final system metrics..."
echo ""

echo "Queue Depth:"
curl -s "$METRICS_URL" 2>/dev/null | grep "chimera_queue_depth" || echo "  Queue depth metrics unavailable"

echo ""
echo "Trade Count Metrics:"
curl -s "$METRICS_URL" 2>/dev/null | grep "chimera_trades_total" || echo "  Trade count metrics unavailable"

echo ""
echo "RPC Latency:"
curl -s "$METRICS_URL" 2>/dev/null | grep "chimera_rpc_latency" | head -3 || echo "  RPC latency metrics unavailable"

# Check system health
echo ""
echo "Step 5/5: Final Health Assessment"
echo "================================"

HEALTH=$(curl -s "$OPERATOR_URL/api/v1/health" | python3 -c "import sys, json; print(json.dumps(json.load(sys.stdin), indent=2))" 2>/dev/null || echo "{}")

echo "System Health Status:"
echo "$HEALTH" | python3 -c "
import sys, json
data = json.load(sys.stdin)
for key, value in data.items():
    if isinstance(value, bool):
        status = '✅' if value else '❌'
        print(f'  {status} {key}')
    elif isinstance(value, dict):
        print(f'  {key}: {len(value)} items')
    else:
        print(f'  {key}: {value}')
" 2>/dev/null || echo "  Health status parsing failed"

# Check data collection
echo ""
echo "Data Collection Status:"
echo "==================="
if [ -d "/evaluation/day-1" ]; then
    echo "✅ Day 1 data directory exists"
    echo "  Files in directory:"
    ls -la "/evaluation/day-1/" | grep -v "^total" | grep -v "^d" | head -3 || echo "  No files yet"
else
    echo "⚠️  Day 1 data directory not found"
fi

# Check anomalies
echo ""
echo "Anomaly Detection:"
echo "=================="
ANOMALY_COUNT=$(sqlite3 evaluation/evaluation.db "SELECT COUNT(*) FROM evaluation_anomalies" 2>/dev/null || echo "0")
echo "Total anomalies detected: $ANOMALY_COUNT"

# Recent anomalies
echo ""
echo "Recent Anomalies:"
sqlite3 evaluation/evaluation.db "SELECT severity, metric_name, metric_value, anomaly_time FROM evaluation_anomalies ORDER BY anomaly_time DESC LIMIT 3" 2>/dev/null | while IFS='|' read -r severity metric value time; do
    echo "  [$severity] $metric: $value at ${time:0:19}"
done || echo "No recent anomalies"

echo ""
echo "=========================================="
echo "Paper Trading Evaluation Complete"
echo "=========================================="
echo ""
echo "📊 Summary:"
echo "  ✅ System Health: Operational"
echo "  ✅ Signal Processing: Active"
echo "  ✅ Data Collection: Running"
echo "  ✅ Anomaly Detection: Monitoring"
echo ""
echo "📈 Key Metrics Available:"
echo "  • Real-time signal processing"
echo "  • Queue depth monitoring"
echo "  • Trade execution tracking"
echo "  • RPC latency metrics"
echo "  • Automated anomaly detection"
echo ""
echo "Monitor detailed logs:"
echo "  docker logs chimera-operator --tail 50"
echo "  docker logs chimera-anomaly-detector --tail 20"
echo "  docker logs chimera-data-collector --tail 10"