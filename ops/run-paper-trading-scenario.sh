#!/bin/bash
# Chimera Paper Trading Scenario Runner
# Comprehensive evaluation of the trading system

set -e
set -o pipefail

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
EVALUATION_DIR="evaluation/day-${DAY_NUM}"
RESULT_FILE="evaluation/signal-run-summary.json"

echo "Evaluation Parameters:"
echo "  Signals to process: $SIGNALS_TO_PROCESS"
echo "  Evaluation Day: $DAY_NUM"
echo "  Operator URL: $OPERATOR_URL"
echo ""

# System health check
echo "Step 1/5: System Health Check"
echo "================================"
HEALTH_OK=0
if curl -sf --max-time 5 "$OPERATOR_URL/api/v1/health" > /dev/null 2>&1; then
    HEALTH=$(curl -s --max-time 5 "$OPERATOR_URL/api/v1/health" 2>/dev/null || true)
    if echo "$HEALTH" | grep -q '"status".*"healthy"\|"status":"healthy"'; then
        HEALTH_OK=1
        echo "✅ Operator is healthy"
    else
        echo "⚠ Operator responded but health payload is not 'healthy'"
    fi
else
    echo "❌ Operator health check failed"
    exit 1
fi

# Get initial metrics
echo ""
echo "Step 2/5: Initial Metrics Collection"
echo "================================"
echo "Collecting baseline metrics..."
curl -s --max-time 5 "$METRICS_URL" 2>/dev/null | grep -E "chimera_queue_depth|chimera_trade_" | head -5 || echo "Some metrics unavailable"

# Process signals
echo ""
echo "Step 3/5: Signal Processing Simulation"
echo "================================"
echo "Processing $SIGNALS_TO_PROCESS historical signals..."
echo ""

rm -f "$RESULT_FILE"

# Use Python for more robust signal processing
SIGNAL_FILE="$SIGNAL_FILE" OPERATOR_URL="$OPERATOR_URL" SIGNALS_TO_PROCESS="$SIGNALS_TO_PROCESS" RESULT_FILE="$RESULT_FILE" python3 <<'PYTHON_SCRIPT'
import json
import os
import requests
import time

# Read historical signals
signal_file = os.environ['SIGNAL_FILE']
operator_url = os.environ['OPERATOR_URL'] + "/api/v1/signal"
signals_to_process = int(os.environ['SIGNALS_TO_PROCESS'])
result_file = os.environ['RESULT_FILE']

signals = []
try:
    with open(signal_file, 'r') as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                signals.append(json.loads(line))
            except json.JSONDecodeError as e:
                print(f"Skipping malformed signal line: {e}")
except FileNotFoundError:
    print(f"Error: signal file not found: {signal_file}")

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
if processed > 0:
    print(f"  Success Rate: {successful * 100.0 / processed:.1f}%")
else:
    print("  Success Rate: N/A (no signals processed)")

# Persist results for the shell summary
with open(result_file, 'w') as f:
    json.dump({
        "processed": processed,
        "successful": successful,
        "failed": failed,
        "success_rate": (successful * 100.0 / processed) if processed > 0 else None
    }, f)

PYTHON_SCRIPT

# Read results produced by the Python step
PROCESSED=0
SUCCESSFUL=0
FAILED=0
if [ -f "$RESULT_FILE" ]; then
    PROCESSED=$(python3 -c "import json; print(json.load(open('$RESULT_FILE')).get('processed', 0))" 2>/dev/null || echo "0")
    SUCCESSFUL=$(python3 -c "import json; print(json.load(open('$RESULT_FILE')).get('successful', 0))" 2>/dev/null || echo "0")
    FAILED=$(python3 -c "import json; print(json.load(open('$RESULT_FILE')).get('failed', 0))" 2>/dev/null || echo "0")
fi

# Collect final metrics
echo ""
echo "Step 4/5: Post-Processing Metrics"
echo "================================"
echo "Collecting final system metrics..."
echo ""

echo "Queue Depth:"
curl -s --max-time 5 "$METRICS_URL" 2>/dev/null | grep "chimera_queue_depth" || echo "  Queue depth metrics unavailable"

echo ""
echo "Trade Count Metrics:"
curl -s --max-time 5 "$METRICS_URL" 2>/dev/null | grep "chimera_trades_total" || echo "  Trade count metrics unavailable"

echo ""
echo "RPC Latency:"
curl -s --max-time 5 "$METRICS_URL" 2>/dev/null | grep "chimera_rpc_latency" | head -3 || echo "  RPC latency metrics unavailable"

# Check system health
echo ""
echo "Step 5/5: Final Health Assessment"
echo "================================"

HEALTH=$(curl -s --max-time 5 "$OPERATOR_URL/api/v1/health" | python3 -c "import sys, json; print(json.dumps(json.load(sys.stdin), indent=2))" 2>/dev/null || echo "{}")

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
if [ -d "$EVALUATION_DIR" ]; then
    echo "✅ Day ${DAY_NUM} data directory exists"
    echo "  Files in directory:"
    ls -la "$EVALUATION_DIR/" | grep -v "^total" | grep -v "^d" | head -3 || echo "  No files yet"
else
    echo "⚠️  Day ${DAY_NUM} data directory not found"
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
ANOMALY_ROWS=$(sqlite3 evaluation/evaluation.db "SELECT severity, metric_name, metric_value, anomaly_time FROM evaluation_anomalies ORDER BY anomaly_time DESC LIMIT 3" 2>/dev/null || true)
if [ -n "$ANOMALY_ROWS" ]; then
    echo "$ANOMALY_ROWS" | while IFS='|' read -r severity metric value time; do
        echo "  [$severity] $metric: $value at ${time:0:19}"
    done
else
    echo "No recent anomalies"
fi

echo ""
echo "=========================================="
echo "Paper Trading Evaluation Complete"
echo "=========================================="
echo ""
echo "📊 Summary:"
if [ "$HEALTH_OK" -eq 1 ]; then
    echo "  ✅ System Health: Operational"
else
    echo "  ❌ System Health: DEGRADED"
fi
if [ "$PROCESSED" -gt 0 ]; then
    echo "  📨 Signal Processing: $SUCCESSFUL/$PROCESSED accepted ($FAILED failed)"
else
    echo "  ⚠ Signal Processing: No signals were processed"
fi
if [ -d "$EVALUATION_DIR" ]; then
    echo "  ✅ Data Collection: Running"
else
    echo "  ⚠ Data Collection: No data directory for day ${DAY_NUM}"
fi
if [ "$ANOMALY_COUNT" -gt 0 ]; then
    echo "  ✅ Anomaly Detection: Monitoring ($ANOMALY_COUNT anomalies)"
else
    echo "  ⚠ Anomaly Detection: No anomalies recorded"
fi
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
