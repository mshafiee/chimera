#!/bin/bash
set -e

# Chimera Report Generator Entry Point
# Implements daily report generation with database initialization
# Converts one-time script into long-running scheduled service

echo "==================================="
echo "Chimera Report Generator Service"
echo "==================================="

# Configuration from environment variables
REPORT_TIME=${REPORT_GENERATION_TIME:-02:00}
EVAL_DIR=${EVAL_DIR:-/evaluation}
DB_PATH=${EVAL_DB_PATH:-/evaluation/evaluation.db}
OUTPUT_DIR=${OUTPUT_DIR:-/evaluation/reports}

echo "Configuration:"
echo "  Report Generation Time: ${REPORT_TIME}"
echo "  Evaluation Directory: ${EVAL_DIR}"
echo "  Database Path: ${DB_PATH}"
echo "  Output Directory: ${OUTPUT_DIR}"
echo ""

# Ensure directories exist
mkdir -p "${EVAL_DIR}"
mkdir -p "${OUTPUT_DIR}"

# Ensure database exists and initialize if needed
if [ ! -f "${DB_PATH}" ]; then
    echo "Database not found. Initializing evaluation database..."
    if [ -f "/app/evaluation_schema.sql" ]; then
        sqlite3 "${DB_PATH}" < /app/evaluation_schema.sql
        echo "✅ Database initialized successfully"
    else
        echo "❌ Error: Schema file not found at /app/evaluation_schema.sql"
        echo "Creating minimal database structure..."
        sqlite3 "${DB_PATH}" << 'EOF'
-- Minimal database structure for report generation
CREATE TABLE IF NOT EXISTS evaluation_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    snapshot_time TEXT,
    day_number INTEGER,
    hour_number INTEGER,
    metrics_data TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS evaluation_anomalies (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    anomaly_time TEXT,
    severity TEXT,
    metric_name TEXT,
    metric_value REAL,
    threshold_value REAL,
    resolved INTEGER DEFAULT 0,
    detected_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS daily_evaluation_summaries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    evaluation_date TEXT,
    total_trades INTEGER,
    success_rate REAL,
    net_pnl_sol REAL,
    overall_grade TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
EOF
        echo "✅ Minimal database structure created"
    fi
else
    echo "✅ Database exists: ${DB_PATH}"
fi

echo ""
echo "Report Generator service started."
echo "Waiting for scheduled report generation at ${REPORT_TIME}..."
echo "Press Ctrl+C to stop"

# Main service loop
while true; do
    # Get current time
    CURRENT_HOUR=$(date +%H)
    CURRENT_MINUTE=$(date +%M)
    CURRENT_TIME="${CURRENT_HOUR}:${CURRENT_MINUTE}"

    echo "Current time: ${CURRENT_TIME} (next report at: ${REPORT_TIME})"

    # Check if it's time to generate report
    if [ "${CURRENT_TIME}" = "${REPORT_TIME}" ]; then
        echo ""
        echo "==================================="
        echo "Starting Daily Report Generation"
        echo "==================================="
        echo "Timestamp: $(date -u +"%Y-%m-%dT%H:%M:%SZ")"

        # Run report generation
        if python3 /app/generate-evaluation-report.py \
            --evaluation-dir "${EVAL_DIR}" \
            --database "${DB_PATH}" \
            --output "${OUTPUT_DIR}/daily-report-$(date +%Y%m%d).html"; then
            echo "✅ Report generated successfully"

            # Verify output file was created
            LATEST_REPORT=$(ls -t "${OUTPUT_DIR}"/daily-report-*.html 2>/dev/null | head -1)
            if [ -n "${LATEST_REPORT}" ]; then
                FILE_SIZE=$(stat -f%z "${LATEST_REPORT}" 2>/dev/null || stat -c%s "${LATEST_REPORT}" 2>/dev/null)
                echo "   Report file: ${LATEST_REPORT}"
                echo "   Size: ${FILE_SIZE} bytes"
            fi
        else
            echo "❌ Report generation failed"
            echo "   Check logs above for error details"
        fi

        echo "Report generation complete."
        echo "Next report will be generated tomorrow at ${REPORT_TIME}"

        # Calculate sleep time until next day (24 hours - current time in seconds)
        CURRENT_SECONDS=$(( $(date +%H) * 3600 + $(date +%M) * 60 + $(date +%S) ))
        TARGET_SECONDS=$(( $(echo "${REPORT_TIME}" | cut -d: -f1) * 3600 + $(echo "${REPORT_TIME}" | cut -d: -f2) * 60 ))
        if [ $TARGET_SECONDS -le $CURRENT_SECONDS ]; then
            # Target time is tomorrow
            SLEEP_SECONDS=$(( ((24 * 3600) - CURRENT_SECONDS) + TARGET_SECONDS ))
        else
            # Target time is today (shouldn't happen if we just ran)
            SLEEP_SECONDS=$(( TARGET_SECONDS - CURRENT_SECONDS ))
        fi

        echo "Sleeping for ${SLEEP_SECONDS} seconds until next report..."
        echo "Next report time: $(date -d "+${SLEEP_SECONDS} seconds" -u +"%Y-%m-%dT%H:%M:%SZ" 2>/dev/null || date -v+${SLEEP_SECONDS}S -u +"%Y-%m-%dT%H:%M:%SZ")"
        sleep "${SLEEP_SECONDS}"
    else
        # Sleep for 1 minute and check again
        sleep 60
    fi
done