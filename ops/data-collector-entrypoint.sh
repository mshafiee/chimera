#!/bin/bash
set -e

# Chimera Data Collector Service
# Fetches metrics from operator and stores them as files for processing

# Initialize counters from environment variables or defaults
DAY_NUM=${DAY_NUM:-1}
HOUR_START=${HOUR_START:-0}
EVAL_DIR=${EVAL_DIR:-/evaluation}
DB_PATH=${EVAL_DB_PATH:-/evaluation/evaluation.db}
OPERATOR_URL=${OPERATOR_URL:-http://chimera-operator:8080}

echo "=================================="
echo "Chimera Data Collector Service"
echo "=================================="

echo "Configuration:"
echo "  Day Number: ${DAY_NUM}"
echo "  Start Hour: ${HOUR_START}"
echo "  Evaluation Directory: ${EVAL_DIR}"
echo "  Database Path: ${DB_PATH}"
echo "  Operator URL: ${OPERATOR_URL}"
echo ""

# Ensure evaluation directory exists
mkdir -p "${EVAL_DIR}"

echo "Data Collector service started. Waiting for first collection cycle..."
echo "Press Ctrl+C to stop"

while true; do
    echo ""
    echo "=================================="
    echo "Starting collection: Day ${DAY_NUM}, Hour ${HOUR_START}"
    TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
    DAY_DIR="${EVAL_DIR}/day-${DAY_NUM}"
    mkdir -p "${DAY_DIR}"

    echo "Timestamp: ${TIMESTAMP}"
    echo "=================================="

    # Fetch operator metrics file
    echo "Fetching metrics from operator..."
    METRICS_FILE="${DAY_DIR}/operator-metrics-${TIMESTAMP}.txt"
    if curl -s "${OPERATOR_URL}/metrics" > "${METRICS_FILE}" 2>/dev/null; then
        echo "✅ Saved metrics file: $(basename ${METRICS_FILE})"
    else
        echo "⚠️  Failed to fetch metrics from ${OPERATOR_URL}/metrics"
    fi

    # Fetch health status file
    echo "Fetching health status from operator..."
    HEALTH_FILE="${DAY_DIR}/health-status-${TIMESTAMP}.json"
    if curl -s "${OPERATOR_URL}/api/v1/health" > "${HEALTH_FILE}" 2>/dev/null; then
        echo "✅ Saved health status: $(basename ${HEALTH_FILE})"
    else
        echo "⚠️  Failed to fetch health status from ${OPERATOR_URL}/api/v1/health"
    fi

    # Process metrics with existing script
    echo "Processing metrics for Day ${DAY_NUM}, Hour ${HOUR_START}..."

    if python3 /app/process-evaluation-metrics.py \
        --day "${DAY_NUM}" \
        --hour "${HOUR_START}" \
        --metrics-dir "${DAY_DIR}" \
        --database "${DB_PATH}" \
        --timestamp "${TIMESTAMP}"; then
        echo "✅ Collection completed successfully"

        # Verify output files were created
        if [ -d "${DAY_DIR}" ]; then
            FILE_COUNT=$(find "${DAY_DIR}" -type f | wc -l)
            echo "   Created ${FILE_COUNT} files in ${DAY_DIR}"
        fi
    else
        echo "❌ Collection failed (will retry next hour)"
        echo "   Error occurred during metrics processing"
    fi

    echo "Collection cycle complete. Next run in 1 hour..."
    echo "Sleeping until: $(date -d '+1 hour' -u +"%Y-%m-%dT%H:%M:%SZ" 2>/dev/null || date -v+1H -u +"%Y-%m-%dT%H:%M:%SZ")"

    # Sleep for 1 hour (3600 seconds)
    sleep 3600

    # Increment hour counter
    HOUR_START=$((HOUR_START + 1))

    # Handle day rollover
    if [ $HOUR_START -ge 24 ]; then
        HOUR_START=0
        DAY_NUM=$((DAY_NUM + 1))
        echo "Day rolled over! Now starting Day ${DAY_NUM}"
    fi
done