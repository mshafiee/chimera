#!/bin/bash
# collect-evaluation-data.sh - Automated hourly data collection for Chimera evaluation
# This script collects comprehensive metrics and stores them for investigation-ready analysis

set -euo pipefail

# ===================================================================
# CONFIGURATION
# ===================================================================
EVAL_DIR="${EVAL_DIR:-/opt/chimera/evaluation}"
DAY_NUM="${DAY_NUM:-1}"
HOUR_NUM="${HOUR_NUM:-0}"
DB_PATH="${EVAL_DIR}/evaluation.db"
CHIMERA_DB_PATH="${CHIMERA_DB_PATH:-/opt/chimera/data/chimera.db}"

# API endpoints
OPERATOR_METRICS_URL="${OPERATOR_METRICS_URL:-http://localhost:8080/metrics}"
SCOUT_METRICS_URL="${SCOUT_METRICS_URL:-http://localhost:8081/metrics}"
PROMETHEUS_URL="${PROMETHEUS_URL:-http://localhost:9090}"

# Create evaluation directory structure
mkdir -p "${EVAL_DIR}/day-${DAY_NUM}"
mkdir -p "${EVAL_DIR}/day-${DAY_NUM}/metrics"
mkdir -p "${EVAL_DIR}/day-${DAY_NUM}/database"
mkdir -p "${EVAL_DIR}/day-${DAY_NUM}/logs"
mkdir -p "${EVAL_DIR}/day-${DAY_NUM}/system"

# Timestamp for this collection
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
TIMESTAMP_FILE=$(date -u +"%Y%m%d_%H%M%S")

echo "=========================================="
echo "Evaluation Data Collection"
echo "=========================================="
echo "Day: ${DAY_NUM}, Hour: ${HOUR_NUM}"
echo "Timestamp: ${TIMESTAMP}"
echo "Output Directory: ${EVAL_DIR}/day-${DAY_NUM}"
echo ""

# ===================================================================
# COLLECT PROMETHEUS METRICS
# ===================================================================
echo "Collecting Prometheus metrics..."

# Operator metrics
if curl -sf "${OPERATOR_METRICS_URL}" > "${EVAL_DIR}/day-${DAY_NUM}/metrics/operator-metrics-${TIMESTAMP_FILE}.txt" 2>/dev/null; then
    echo "✓ Operator metrics collected"
else
    echo "✗ Failed to collect operator metrics"
fi

# Scout metrics
if curl -sf "${SCOUT_METRICS_URL}" > "${EVAL_DIR}/day-${DAY_NUM}/metrics/scout-metrics-${TIMESTAMP_FILE}.txt" 2>/dev/null; then
    echo "✓ Scout metrics collected"
else
    echo "✗ Failed to collect scout metrics"
fi

# Main Prometheus metrics (if available)
if curl -sf "${PROMETHEUS_URL}/api/v1/query?query=up" > /dev/null 2>&1; then
    curl -sf "${PROMETHEUS_URL}/api/v1/query?query={__name__=~\".*\"}" | jq '.data.result' > "${EVAL_DIR}/day-${DAY_NUM}/metrics/prometheus-snapshot-${TIMESTAMP_FILE}.json" 2>/dev/null
    echo "✓ Prometheus snapshot collected"
fi

# ===================================================================
# COLLECT DATABASE SNAPSHOTS
# ===================================================================
echo "Collecting database snapshots..."

# Main Chimera database
if [ -f "${CHIMERA_DB_PATH}" ]; then
    # Create a compressed backup
    sqlite3 "${CHIMERA_DB_PATH}" ".backup ${EVAL_DIR}/day-${DAY_NUM}/database/chimera-snapshot-${TIMESTAMP_FILE}.db" 2>/dev/null
    if [ $? -eq 0 ]; then
        # Compress the database
        gzip -f "${EVAL_DIR}/day-${DAY_NUM}/database/chimera-snapshot-${TIMESTAMP_FILE}.db"
        echo "✓ Chimera database snapshot created and compressed"
    else
        echo "✗ Failed to create Chimera database snapshot"
    fi

    # Export current trades and positions for quick analysis
    sqlite3 "${CHIMERA_DB_PATH}" "SELECT * FROM trades ORDER BY created_at DESC LIMIT 1000;" > "${EVAL_DIR}/day-${DAY_NUM}/database/recent-trades-${TIMESTAMP_FILE}.csv" 2>/dev/null
    sqlite3 "${CHIMERA_DB_PATH}" "SELECT * FROM positions;" > "${EVAL_DIR}/day-${DAY_NUM}/database/active-positions-${TIMESTAMP_FILE}.csv" 2>/dev/null
    echo "✓ Database exports created"
else
    echo "✗ Chimera database not found at ${CHIMERA_DB_PATH}"
fi

# Evaluation database
if [ -f "${DB_PATH}" ]; then
    sqlite3 "${DB_PATH}" ".backup ${EVAL_DIR}/day-${DAY_NUM}/database/evaluation-snapshot-${TIMESTAMP_FILE}.db" 2>/dev/null
    if [ $? -eq 0 ]; then
        gzip -f "${EVAL_DIR}/day-${DAY_NUM}/database/evaluation-snapshot-${TIMESTAMP_FILE}.db"
        echo "✓ Evaluation database snapshot created"
    fi
fi

# ===================================================================
# COLLECT SYSTEM RESOURCES
# ===================================================================
echo "Collecting system resources..."

# Docker container stats
if command -v docker &> /dev/null; then
    docker stats --no-stream --format "table {{.Container}}\t{{.CPUPerc}}\t{{.MemUsage}}\t{{.MemPerc}}\t{{.NetIO}}\t{{.BlockIO}}" > "${EVAL_DIR}/day-${DAY_NUM}/system/docker-stats-${TIMESTAMP_FILE}.txt" 2>/dev/null
    echo "✓ Docker stats collected"

    # Individual container stats (JSON format for analysis)
    for container in $(docker ps --format "{{.Names}}"); do
        docker inspect "$container" | jq '.[0]' > "${EVAL_DIR}/day-${DAY_NUM}/system/container-${container}-${TIMESTAMP_FILE}.json" 2>/dev/null
    done
    echo "✓ Container details collected"
else
    echo "✗ Docker not available"
fi

# System resource usage
top -l 1 -n 0 -b -o cpu | head -20 > "${EVAL_DIR}/day-${DAY_NUM}/system/cpu-usage-${TIMESTAMP_FILE}.txt" 2>/dev/null
vm_stat > "${EVAL_DIR}/day-${DAY_NUM}/system/memory-usage-${TIMESTAMP_FILE}.txt" 2>/dev/null
df -h > "${EVAL_DIR}/day-${DAY_NUM}/system/disk-usage-${TIMESTAMP_FILE}.txt" 2>/dev/null
echo "✓ System resources collected"

# ===================================================================
# COLLECT LOGS
# ===================================================================
echo "Collecting recent logs..."

# Recent operator logs (last 1000 lines)
if [ -f "/var/log/chimera/operator.log" ]; then
    tail -1000 "/var/log/chimera/operator.log" > "${EVAL_DIR}/day-${DAY_NUM}/logs/operator-recent-${TIMESTAMP_FILE}.log"
    echo "✓ Operator logs collected"
fi

# Recent scout logs
if [ -f "/var/log/chimera/scout.log" ]; then
    tail -1000 "/var/log/chimera/scout.log" > "${EVAL_DIR}/day-${DAY_NUM}/logs/scout-recent-${TIMESTAMP_FILE}.log"
    echo "✓ Scout logs collected"
fi

# Error logs (if available)
if [ -d "/var/log/chimera/errors" ]; then
    find "/var/log/chimera/errors" -name "*.log" -mtime -1 -exec cp {} "${EVAL_DIR}/day-${DAY_NUM}/logs/" \;
    echo "✓ Error logs collected"
fi

# ===================================================================
# COLLECT TRADE EXECUTION DETAILS
# ===================================================================
echo "Collecting trade execution details..."

if [ -f "${CHIMERA_DB_PATH}" ]; then
    # Recent trade details with performance metrics
    sqlite3 "${CHIMERA_DB_PATH}" "SELECT
        trade_uuid,
        wallet_address,
        token_address,
        strategy,
        status,
        created_at as signal_time,
        updated_at as execution_time,
        julianday('now') - julianday(created_at) as execution_delay_seconds
    FROM trades
    WHERE created_at >= datetime('now', '-1 hour')
    ORDER BY created_at DESC;" > "${EVAL_DIR}/day-${DAY_NUM}/metrics/recent-trades-${TIMESTAMP_FILE}.csv" 2>/dev/null
    echo "✓ Trade execution details collected"
fi

# ===================================================================
# COLLECT CIRCUIT BREAKER STATUS
# ===================================================================
echo "Collecting circuit breaker status..."

if curl -sf "http://localhost:8080/api/v1/config/circuit-breaker" > "${EVAL_DIR}/day-${DAY_NUM}/system/circuit-breaker-${TIMESTAMP_FILE}.json" 2>/dev/null; then
    echo "✓ Circuit breaker status collected"
else
    echo "✗ Failed to collect circuit breaker status"
fi

# ===================================================================
# COLLECT SYSTEM HEALTH
# ===================================================================
echo "Collecting system health status..."

if curl -sf "http://localhost:8080/api/v1/health" > "${EVAL_DIR}/day-${DAY_NUM}/system/health-status-${TIMESTAMP_FILE}.json" 2>/dev/null; then
    echo "✓ System health status collected"
else
    echo "✗ Failed to collect system health status"
fi

# ===================================================================
# PROCESS AND STORE IN EVALUATION DATABASE
# ===================================================================
echo "Processing evaluation metrics..."

# Check if Python script exists
if [ -f "/opt/chimera/ops/process-evaluation-metrics.py" ]; then
    python3 /opt/chimera/ops/process-evaluation-metrics.py \
        --day "${DAY_NUM}" \
        --hour "${HOUR_NUM}" \
        --metrics-dir "${EVAL_DIR}/day-${DAY_NUM}" \
        --database "${DB_PATH}" \
        --timestamp "${TIMESTAMP}"
    echo "✓ Metrics processed and stored in evaluation database"
else
    echo "⚠ Processing script not found, skipping database storage"
fi

# ===================================================================
# COMPRESS OLD FILES
# ===================================================================
echo "Compressing previous hourly data..."

# Compress files from previous hours (except the current one)
find "${EVAL_DIR}/day-${DAY_NUM}" -name "*.txt" -mmin +120 -exec gzip -f {} \; 2>/dev/null || true
find "${EVAL_DIR}/day-${DAY_NUM}" -name "*.log" -mmin +120 -exec gzip -f {} \; 2>/dev/null || true
find "${EVAL_DIR}/day-${DAY_NUM}" -name "*.csv" -mmin +120 -exec gzip -f {} \; 2>/dev/null || true
echo "✓ Old files compressed"

# ===================================================================
# GENERATE COLLECTION SUMMARY
# ===================================================================
echo "Generating collection summary..."

cat > "${EVAL_DIR}/day-${DAY_NUM}/collection-summary-${TIMESTAMP_FILE}.json" <<EOF
{
  "collection_time": "${TIMESTAMP}",
  "day_number": ${DAY_NUM},
  "hour_number": ${HOUR_NUM},
  "files_collected": $(find "${EVAL_DIR}/day-${DAY_NUM}" -type f | wc -l),
  "total_size_mb": $(du -sm "${EVAL_DIR}/day-${DAY_NUM}" | cut -f1),
  "metrics_files": $(find "${EVAL_DIR}/day-${DAY_NUM}/metrics" -type f | wc -l),
  "database_files": $(find "${EVAL_DIR}/day-${DAY_NUM}/database" -type f | wc -l),
  "log_files": $(find "${EVAL_DIR}/day-${DAY_NUM}/logs" -type f | wc -l),
  "system_files": $(find "${EVAL_DIR}/day-${DAY_NUM}/system" -type f | wc -l)
}
EOF

echo "✓ Collection summary created"

# ===================================================================
# CLEANUP OLD FILES
# ===================================================================
echo "Cleaning up old data..."

# Remove files older than 12 days (keep 2 extra days for safety)
find "${EVAL_DIR}" -type f -mtime +12 -delete 2>/dev/null || true
echo "✓ Old data cleaned up"

# ===================================================================
# FINAL SUMMARY
# ===================================================================
echo ""
echo "=========================================="
echo "Data Collection Complete"
echo "=========================================="
echo "Day: ${DAY_NUM}, Hour: ${HOUR_NUM}"
echo "Files collected: $(find "${EVAL_DIR}/day-${DAY_NUM}" -type f | wc -l)"
echo "Total size: $(du -sh "${EVAL_DIR}/day-${DAY_NUM}" | cut -f1)"
echo "Data stored in: ${EVAL_DIR}/day-${DAY_NUM}"
echo ""

# Send completion notification if Telegram is configured
if [ -n "${TELEGRAM_BOT_TOKEN:-}" ] && [ -n "${TELEGRAM_CHAT_ID:-}" ]; then
    MESSAGE="📊 Evaluation Data Collection Complete
Day: ${DAY_NUM}, Hour: ${HOUR_NUM}
Files: $(find "${EVAL_DIR}/day-${DAY_NUM}" -type f | wc -l)
Size: $(du -sh "${EVAL_DIR}/day-${DAY_NUM}" | cut -f1)"

    curl -s "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/sendMessage" \
        -d "chat_id=${TELEGRAM_CHAT_ID}" \
        -d "text=${MESSAGE}" > /dev/null 2>&1
fi

exit 0