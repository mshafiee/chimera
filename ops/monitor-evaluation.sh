#!/bin/bash
# monitor-evaluation.sh - Real-time monitoring during 10-day evaluation
# Provides continuous status updates and alerts during evaluation period

set -euo pipefail

# ===================================================================
# CONFIGURATION
# ===================================================================
EVAL_DIR="${EVAL_DIR:-/opt/chimera/evaluation}"
MONITOR_INTERVAL="${MONITOR_INTERVAL:-60}"  # Check every 60 seconds
ALERT_THRESHOLD_CPU="${ALERT_THRESHOLD_CPU:-90}"
ALERT_THRESHOLD_MEMORY="${ALERT_THRESHOLD_MEMORY:-85}"
ALERT_THRESHOLD_QUEUE="${ALERT_THRESHOLD_QUEUE:-800}"

# Colors for terminal output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# ===================================================================
# SIGNAL HANDLING
# ===================================================================
cleanup() {
    echo ""
    echo -e "${YELLOW}Monitoring stopped by user${NC}"
    exit 0
}

trap cleanup SIGINT SIGTERM

# ===================================================================
# MONITORING FUNCTIONS
# ===================================================================

check_service_health() {
    local service_name=$1
    local health_url=$2

    if curl -sf "${health_url}" > /dev/null 2>&1; then
        echo -e "${GREEN}✓${NC} ${service_name}"
        return 0
    else
        echo -e "${RED}✗${NC} ${service_name}"
        return 1
    fi
}

get_system_metrics() {
    local metrics_data=""
    local health_check=0

    # Get metrics from operator
    if curl -sf "http://localhost:8080/metrics" > /tmp/operator-metrics.txt 2>/dev/null; then
        # Extract key metrics
        local cpu_usage=$(grep "chimera_cpu_usage_percent" /tmp/operator-metrics.txt | cut -d' ' -f2 || echo "0")
        local memory_usage=$(grep "chimera_memory_usage_percent" /tmp/operator-metrics.txt | cut -d' ' -f2 || echo "0")
        local queue_depth=$(grep "chimera_queue_depth" /tmp/operator-metrics.txt | cut -d' ' -f2 || echo "0")
        local active_positions=$(grep "chimera_active_positions" /tmp/operator-metrics.txt | cut -d' ' -f2 || echo "0")
        local trades_total=$(grep "chimera_trades_total" /tmp/operator-metrics.txt | cut -d' ' -f2 || echo "0")
        local latency_avg=$(grep "chimera_trade_latency_avg_ms" /tmp/operator-metrics.txt | cut -d' ' -f2 || echo "0")
        local circuit_breaker=$(grep "chimera_circuit_breaker_state" /tmp/operator-metrics.txt | cut -d' ' -f2 || echo "0")

        health_check=1
    else
        cpu_usage="N/A"
        memory_usage="N/A"
        queue_depth="N/A"
        active_positions="N/A"
        trades_total="N/A"
        latency_avg="N/A"
        circuit_breaker="N/A"
    fi

    # Get Docker stats
    local docker_stats=$(docker stats --no-stream --format "table {{.Container}}\t{{.CPUPerc}}\t{{.MemUsage}}" 2>/dev/null || echo "Docker stats unavailable")

    # Build metrics data
    metrics_data=$(cat <<EOF
CPU Usage: ${cpu_usage}%
Memory Usage: ${memory_usage}%
Queue Depth: ${queue_depth}
Active Positions: ${active_positions}
Total Trades: ${trades_total}
Avg Latency: ${latency_avg}ms
Circuit Breaker: ${circuit_breaker}

Docker Stats:
${docker_stats}
EOF
)

    echo "${metrics_data}"
    return ${health_check}
}

check_anomalies() {
    local anomaly_count=0
    local critical_count=0

    # Check evaluation database for recent anomalies
    if [ -f "${EVAL_DIR}/evaluation.db" ]; then
        local last_hour=$(date -u -d '1 hour ago' +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u -v-1H +%Y-%m-%dT%H:%M:%SZ)

        anomaly_count=$(sqlite3 "${EVAL_DIR}/evaluation.db" "SELECT COUNT(*) FROM evaluation_anomalies WHERE resolved = 0;" 2>/dev/null || echo "0")
        critical_count=$(sqlite3 "${EVAL_DIR}/evaluation.db" "SELECT COUNT(*) FROM evaluation_anomalies WHERE resolved = 0 AND severity = 'CRITICAL';" 2>/dev/null || echo "0")
    fi

    echo "${anomaly_count}|${critical_count}"
}

check_disk_space() {
    local eval_dir_size=$(du -sh "${EVAL_DIR}" 2>/dev/null | cut -f1 || echo "N/A")
    local disk_usage=$(df -h "${EVAL_DIR}" 2>/dev/null | tail -1 | awk '{print $5}' || echo "N/A")

    echo "${eval_dir_size}|${disk_usage}"
}

send_alert() {
    local alert_type=$1
    local alert_message=$2

    # Send Telegram alert
    if [ -n "${TELEGRAM_BOT_TOKEN:-}" ] && [ -n "${TELEGRAM_CHAT_ID:-}" ]; then
        local emoji="⚠️"
        [ "${alert_type}" = "CRITICAL" ] && emoji="🔴"

        local telegram_message="${emoji} Chimera Evaluation Alert

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Type: ${alert_type}
Time: $(date -u +%Y-%m-%dT%H:%M:%SZ)

${alert_message}
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

        curl -s "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/sendMessage" \
            -d "chat_id=${TELEGRAM_CHAT_ID}" \
            -d "text=${telegram_message}" > /dev/null 2>&1
    fi

    # Send Discord alert
    if [ -n "${DISCORD_WEBHOOK_URL:-}" ]; then
        local color=16776960  # Yellow for warning
        [ "${alert_type}" = "CRITICAL" ] && color=16711680  # Red for critical

        local discord_payload=$(cat <<EOF
{
  "embeds": [{
    "title": "Chimera Evaluation Alert",
    "description": "${alert_message}",
    "color": ${color},
    "fields": [
      {"name": "Type", "value": "${alert_type}"},
      {"name": "Time", "value": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"}
    ]
  }]
}
EOF
)

        curl -s "${DISCORD_WEBHOOK_URL}" \
            -H "Content-Type: application/json" \
            -d "${discord_payload}" > /dev/null 2>&1
    fi
}

# ===================================================================
# MAIN MONITORING LOOP
# ===================================================================
main() {
    echo "=========================================="
    echo "Chimera Evaluation Monitoring"
    echo "=========================================="
    echo "Evaluation Directory: ${EVAL_DIR}"
    echo "Monitor Interval: ${MONITOR_INTERVAL}s"
    echo ""
    echo "Press Ctrl+C to stop monitoring"
    echo ""

    local iteration=0
    local start_time=$(date +%s)

    while true; do
        iteration=$((iteration + 1))
        local current_time=$(date +%s)
        local elapsed=$((current_time - start_time))
        local elapsed_hours=$((elapsed / 3600))
        local elapsed_minutes=$(((elapsed % 3600) / 60))

        # Clear screen (optional, can be disabled for cleaner output)
        # clear

        echo "=========================================="
        echo "Monitoring Cycle #${iteration}"
        echo "=========================================="
        echo "Elapsed Time: ${elapsed_hours}h ${elapsed_minutes}m"
        echo "Current Time: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
        echo ""

        # Service Health Check
        echo -e "${BLUE}[Service Health]${NC}"
        check_service_health "Operator" "http://localhost:8080/api/v1/health"
        check_service_health "Scout" "http://localhost:8081/health"
        check_service_health "Prometheus Eval" "http://localhost:9091/-/healthy"
        check_service_health "Fluentd" "http://localhost:24224/api/json" 2>/dev/null || echo -e "${YELLOW}?${NC} Fluentd"
        echo ""

        # System Metrics
        echo -e "${BLUE}[System Metrics]${NC}"
        local metrics_output=$(get_system_metrics)
        local metrics_health=$?

        echo "${metrics_output}"
        echo ""

        # Check for metric-based alerts
        local cpu_usage=$(echo "${metrics_output}" | grep "CPU Usage" | awk '{print $3}' | sed 's/%//')
        local memory_usage=$(echo "${metrics_output}" | grep "Memory Usage" | awk '{print $3}' | sed 's/%//')
        local queue_depth=$(echo "${metrics_output}" | grep "Queue Depth" | awk '{print $3}')

        if [ "${metrics_health}" -eq 1 ]; then
            # CPU Alert
            if [ "${cpu_usage}" != "N/A" ] && [ "${cpu_usage}" -gt "${ALERT_THRESHOLD_CPU}" ]; then
                echo -e "${RED}⚠️ CPU usage alert: ${cpu_usage}% exceeds ${ALERT_THRESHOLD_CPU}%${NC}"
                send_alert "WARNING" "CPU usage (${cpu_usage}%) exceeds threshold (${ALERT_THRESHOLD_CPU}%)"
            fi

            # Memory Alert
            if [ "${memory_usage}" != "N/A" ] && [ "${memory_usage}" -gt "${ALERT_THRESHOLD_MEMORY}" ]; then
                echo -e "${RED}⚠️ Memory usage alert: ${memory_usage}% exceeds ${ALERT_THRESHOLD_MEMORY}%${NC}"
                send_alert "WARNING" "Memory usage (${memory_usage}%) exceeds threshold (${ALERT_THRESHOLD_MEMORY}%)"
            fi

            # Queue Depth Alert
            if [ "${queue_depth}" != "N/A" ] && [ "${queue_depth}" -gt "${ALERT_THRESHOLD_QUEUE}" ]; then
                echo -e "${RED}⚠️ Queue depth alert: ${queue_depth} exceeds ${ALERT_THRESHOLD_QUEUE}${NC}"
                send_alert "WARNING" "Queue depth (${queue_depth}) exceeds threshold (${ALERT_THRESHOLD_QUEUE})"
            fi
        else
            echo -e "${YELLOW}⚠️ Unable to fetch metrics from operator${NC}"
        fi

        echo ""

        # Anomaly Check
        echo -e "${BLUE}[Anomaly Status]${NC}"
        local anomaly_data=$(check_anomalies)
        local total_anomalies=$(echo "${anomaly_data}" | cut -d'|' -f1)
        local critical_anomalies=$(echo "${anomaly_data}" | cut -d'|' -f2)

        echo "Total Active Anomalies: ${total_anomalies}"
        echo "Critical Anomalies: ${critical_anomalies}"

        if [ "${critical_anomalies}" -gt 0 ]; then
            echo -e "${RED}⚠️ ${critical_anomalies} critical anomalies detected!${NC}"
            send_alert "CRITICAL" "${critical_anomalies} critical anomalies detected in evaluation system"
        fi

        echo ""

        # Disk Space Check
        echo -e "${BLUE}[Storage Status]${NC}"
        local disk_data=$(check_disk_space)
        local eval_size=$(echo "${disk_data}" | cut -d'|' -f1)
        local disk_usage=$(echo "${disk_data}" | cut -d'|' -f2)

        echo "Evaluation Directory Size: ${eval_size}"
        echo "Disk Usage: ${disk_usage}"

        # Disk space alert
        local disk_percent=$(echo "${disk_usage}" | sed 's/%//')
        if [ "${disk_percent}" -gt 90 ]; then
            echo -e "${RED}⚠️ Disk usage critical: ${disk_usage}${NC}"
            send_alert "CRITICAL" "Disk usage (${disk_usage}) exceeds 90% - evaluation at risk"
        elif [ "${disk_percent}" -gt 80 ]; then
            echo -e "${YELLOW}⚠️ Disk usage warning: ${disk_usage}${NC}"
        fi

        echo ""

        # Data Collection Status
        echo -e "${BLUE}[Data Collection]${NC}"
        local last_collection_file=$(find "${EVAL_DIR}" -name "collection-summary-*.json" -mmin -120 2>/dev/null | head -1)

        if [ -n "${last_collection_file}" ]; then
            local last_collection_time=$(stat -c %Y "${last_collection_file}" 2>/dev/null || stat -f %m "${last_collection_file}")
            local current_timestamp=$(date +%s)
            local minutes_since_collection=$(((current_timestamp - last_collection_time) / 60))

            echo "Last Data Collection: ${minutes_since_collection} minutes ago"

            if [ ${minutes_since_collection} -gt 120 ]; then
                echo -e "${YELLOW}⚠️ Data collection delay: ${minutes_since_collection} minutes${NC}"
            else
                echo -e "${GREEN}✓ Data collection up to date${NC}"
            fi
        else
            echo -e "${YELLOW}⚠️ No recent data collection found${NC}"
        fi

        echo ""

        # Service Status Summary
        echo -e "${BLUE}[Container Status]${NC}"
        docker ps --filter "name=chimera" --format "table {{.Names}}\t{{.Status}}\t{{.Ports}}" 2>/dev/null || echo "Unable to get container status"

        echo ""
        echo "=========================================="
        echo "Monitoring cycle complete"
        echo "Next check in ${MONITOR_INTERVAL} seconds..."
        echo "=========================================="

        # Wait for next cycle
        sleep ${MONITOR_INTERVAL}
    done
}

# ===================================================================
# ENTRY POINT
# ===================================================================

# Show usage if requested
if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    echo "Usage: $0 [options]"
    echo ""
    echo "Options:"
    echo "  --help, -h     Show this help message"
    echo ""
    echo "Environment Variables:"
    echo "  EVAL_DIR              Evaluation directory (default: /opt/chimera/evaluation)"
    echo "  MONITOR_INTERVAL     Check interval in seconds (default: 60)"
    echo "  ALERT_THRESHOLD_CPU  CPU alert threshold % (default: 90)"
    echo "  ALERT_THRESHOLD_MEMORY Memory alert threshold % (default: 85)"
    echo "  ALERT_THRESHOLD_QUEUE Queue alert threshold (default: 800)"
    echo "  TELEGRAM_BOT_TOKEN   Telegram bot token for alerts"
    echo "  TELEGRAM_CHAT_ID     Telegram chat ID for alerts"
    echo "  DISCORD_WEBHOOK_URL  Discord webhook URL for alerts"
    echo ""
    echo "Examples:"
    echo "  $0                              # Start with default settings"
    echo "  EVAL_DIR=/data/eval $0         # Use custom evaluation directory"
    echo "  MONITOR_INTERVAL=30 $0         # Check every 30 seconds"
    echo ""
    exit 0
fi

# Start monitoring
main