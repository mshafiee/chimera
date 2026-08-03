#!/bin/bash
# generate-daily-report.sh - Daily evaluation health report generator
# Generates comprehensive daily HTML reports for the Chimera 10-day evaluation

set -euo pipefail

# ===================================================================
# CONFIGURATION
# ===================================================================
DAY_NUM="${1:-1}"
EVAL_DIR="${EVAL_DIR:-/opt/chimera/evaluation}"
DB_PATH="${EVAL_DIR}/evaluation.db"
OUTPUT_DIR="${EVAL_DIR}/day-${DAY_NUM}"

# DAY_NUM is used in file paths and passed to Python; require a positive integer
if [[ ! "${DAY_NUM}" =~ ^[0-9]+$ ]] || [ "${DAY_NUM}" -lt 1 ]; then
    echo -e "${RED}Error: DAY_NUM must be a positive integer (got '${DAY_NUM}')${NC}" >&2
    exit 1
fi

# Colors for terminal output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo "=========================================="
echo "Daily Evaluation Report Generator"
echo "=========================================="
echo "Day: ${DAY_NUM}"
echo "Output Directory: ${OUTPUT_DIR}"
echo ""

# ===================================================================
# VALIDATION
# ===================================================================
if [ ! -d "${OUTPUT_DIR}" ]; then
    echo -e "${RED}Error: Output directory does not exist: ${OUTPUT_DIR}${NC}"
    exit 1
fi

if [ ! -f "${DB_PATH}" ]; then
    echo -e "${RED}Error: Evaluation database not found at ${DB_PATH}${NC}"
    exit 1
fi

# Create reports subdirectory
mkdir -p "${OUTPUT_DIR}/reports"

# ===================================================================
# DATA COLLECTION FUNCTIONS
# ===================================================================

collect_summary_stats() {
    local day=$1
    echo "Collecting summary statistics for day ${day}..."

    DB_PATH="$DB_PATH" DAY="$day" python3 <<'PYCOLLECT'
import sqlite3
import json
import os

try:
    conn = sqlite3.connect(os.environ['DB_PATH'])
    cursor = conn.cursor()

    # Get daily summary if available (explicit columns, no positional indexes)
    cursor.execute('''
        SELECT
            day_number, total_trades, successful_trades, failed_trades,
            success_rate_percent, total_pnl_sol, total_costs_sol,
            avg_trade_latency_ms, max_drawdown_percent, total_anomalies
        FROM daily_evaluation_summaries
        WHERE day_number = ?
    ''', (int(os.environ['DAY']),))

    row = cursor.fetchone()

    if row:
        print(json.dumps({
            'success': True,
            'data': {
                'total_trades': row[1],
                'successful_trades': row[2],
                'failed_trades': row[3],
                'success_rate': row[4],
                'total_pnl_sol': row[5],
                'total_costs_sol': row[6],
                'avg_latency_ms': row[7],
                'max_drawdown_percent': row[8],
                'total_anomalies': row[9]
            }
        }))
    else:
        # Calculate from hourly snapshots
        cursor.execute('''
            SELECT
                COUNT(*) as snapshots,
                SUM(total_trades_today) as trades,
                SUM(successful_trades_today) as successful,
                AVG(avg_trade_latency_ms) as avg_latency,
                SUM(total_pnl_sol) as total_pnl,
                SUM(total_costs_sol) as total_costs,
                MAX(max_drawdown_percent) as max_drawdown
            FROM evaluation_snapshots
            WHERE day_number = ?
        ''', (int(os.environ['DAY']),))

        result = cursor.fetchone()
        if result and result[0] > 0:
            print(json.dumps({
                'success': True,
                'data': {
                    'snapshots_count': result[0],
                    'total_trades': result[1] or 0,
                    'successful_trades': result[2] or 0,
                    'failed_trades': (result[1] or 0) - (result[2] or 0),
                    'success_rate': ((result[2] or 0) / result[1] * 100) if result[1] > 0 else 0,
                    'total_pnl_sol': result[4] or 0.0,
                    'total_costs_sol': result[5] or 0.0,
                    'avg_latency_ms': result[3] or 0.0,
                    'max_drawdown_percent': result[6] or 0.0
                }
            }))
        else:
            print(json.dumps({'success': False, 'error': 'No data found'}))

    conn.close()
except Exception as e:
    print(json.dumps({'success': False, 'error': str(e)}))
PYCOLLECT
}
collect_anomalies_summary() {
    local day=$1
    echo "Collecting anomalies summary for day ${day}..."

    DB_PATH="$DB_PATH" DAY="$day" python3 <<'PYCOLLECT'
import sqlite3
import json
import os

try:
    conn = sqlite3.connect(os.environ['DB_PATH'])
    cursor = conn.cursor()

    cursor.execute('''
        SELECT
            COUNT(*) as total_anomalies,
            SUM(CASE WHEN severity = 'CRITICAL' THEN 1 ELSE 0 END) as critical,
            SUM(CASE WHEN severity = 'WARNING' THEN 1 ELSE 0 END) as warnings,
            COUNT(DISTINCT metric_name) as affected_metrics,
            AVG(deviation_percent) as avg_deviation
        FROM evaluation_anomalies
        WHERE day_number = ?
    ''', (int(os.environ['DAY']),))

    result = cursor.fetchone()

    if result and result[0] > 0:
        # Get top 5 anomalies by deviation
        cursor.execute('''
            SELECT metric_name, severity, deviation_percent, description
            FROM evaluation_anomalies
            WHERE day_number = ?
            ORDER BY ABS(deviation_percent) DESC
            LIMIT 5
        ''', (int(os.environ['DAY']),))

        top_anomalies = []
        for row in cursor.fetchall():
            top_anomalies.append({
                'metric': row[0],
                'severity': row[1],
                'deviation': row[2],
                'description': row[3]
            })

        print(json.dumps({
            'success': True,
            'data': {
                'total_anomalies': result[0],
                'critical_count': result[1],
                'warning_count': result[2],
                'affected_metrics': result[3],
                'avg_deviation_percent': result[4],
                'top_anomalies': top_anomalies
            }
        }))
    else:
        print(json.dumps({
            'success': True,
            'data': {
                'total_anomalies': 0,
                'critical_count': 0,
                'warning_count': 0,
                'affected_metrics': 0,
                'avg_deviation_percent': 0,
                'top_anomalies': []
            }
        }))

    conn.close()
except Exception as e:
    print(json.dumps({'success': False, 'error': str(e)}))
PYCOLLECT
}
collect_performance_metrics() {
    local day=$1
    echo "Collecting performance metrics for day ${day}..."

    DB_PATH="$DB_PATH" DAY="$day" python3 <<'PYCOLLECT'
import sqlite3
import json
import os

try:
    conn = sqlite3.connect(os.environ['DB_PATH'])
    cursor = conn.cursor()

    # Get hourly performance data
    cursor.execute('''
        SELECT
            hour_number,
            avg_trade_latency_ms,
            p95_trade_latency_ms,
            p99_trade_latency_ms,
            rpc_latency_avg_ms,
            total_trades_today,
            successful_trades_today
        FROM evaluation_snapshots
        WHERE day_number = ?
        ORDER BY hour_number
    ''', (int(os.environ['DAY']),))

    hourly_data = []
    for row in cursor.fetchall():
        hourly_data.append({
            'hour': row[0],
            'avg_latency': row[1] or 0,
            'p95_latency': row[2] or 0,
            'p99_latency': row[3] or 0,
            'rpc_latency': row[4] or 0,
            'trades': row[5] or 0,
            'successful_trades': row[6] or 0
        })

    # Calculate statistics
    if hourly_data:
        avg_latency = sum(h['avg_latency'] for h in hourly_data) / len(hourly_data)
        total_trades = sum(h['trades'] for h in hourly_data)
        success_rate = sum(h['successful_trades'] for h in hourly_data) / total_trades * 100 if total_trades > 0 else 0
    else:
        avg_latency = 0
        total_trades = 0
        success_rate = 0

    print(json.dumps({
        'success': True,
        'data': {
            'hourly_data': hourly_data,
            'avg_latency_ms': avg_latency,
            'total_trades': total_trades,
            'success_rate_percent': success_rate
        }
    }))

    conn.close()
except Exception as e:
    print(json.dumps({'success': False, 'error': str(e)}))
PYCOLLECT
}
generate_html_report() {
    local day=$1
    local output_file="${OUTPUT_DIR}/reports/daily-report-day-${day}.html"

    echo "Generating HTML report for day ${day}..."

    # Collect all data
    SUMMARY_STATS=$(collect_summary_stats ${day})
    ANOMALIES_SUMMARY=$(collect_anomalies_summary ${day})
    PERFORMANCE_METRICS=$(collect_performance_metrics ${day})

    # Escape for safe embedding inside a <script> block (prevents stored XSS
    # if any DB value contains "</script>")
    ESCAPED_SUMMARY=$(echo "$SUMMARY_STATS" | sed 's|</|<\\/|g')
    ESCAPED_ANOMALIES=$(echo "$ANOMALIES_SUMMARY" | sed 's|</|<\\/|g')
    ESCAPED_PERFORMANCE=$(echo "$PERFORMANCE_METRICS" | sed 's|</|<\\/|g')

    # Get current timestamp
    TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

    # Create HTML report
    cat > "${output_file}" <<EOF
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Chimera Evaluation Report - Day ${day}</title>
    <style>
        body {
            font-family: 'Segoe UI', Tahoma, Geneva, Verdana, sans-serif;
            line-height: 1.6;
            color: #333;
            max-width: 1200px;
            margin: 0 auto;
            padding: 20px;
            background-color: #f5f5f5;
        }
        .header {
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            color: white;
            padding: 30px;
            border-radius: 10px;
            margin-bottom: 30px;
            box-shadow: 0 4px 6px rgba(0,0,0,0.1);
        }
        .header h1 {
            margin: 0 0 10px 0;
            font-size: 2.5em;
        }
        .header .subtitle {
            font-size: 1.2em;
            opacity: 0.9;
        }
        .header .timestamp {
            font-size: 0.9em;
            opacity: 0.7;
            margin-top: 10px;
        }
        .summary-cards {
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
            gap: 20px;
            margin-bottom: 30px;
        }
        .card {
            background: white;
            padding: 20px;
            border-radius: 8px;
            box-shadow: 0 2px 4px rgba(0,0,0,0.1);
            border-left: 4px solid #667eea;
        }
        .card h3 {
            margin: 0 0 10px 0;
            color: #667eea;
            font-size: 1.1em;
        }
        .card .value {
            font-size: 2em;
            font-weight: bold;
            color: #333;
        }
        .card .subtitle {
            color: #666;
            font-size: 0.9em;
        }
        .section {
            background: white;
            padding: 25px;
            border-radius: 8px;
            margin-bottom: 30px;
            box-shadow: 0 2px 4px rgba(0,0,0,0.1);
        }
        .section h2 {
            margin: 0 0 20px 0;
            color: #667eea;
            border-bottom: 2px solid #667eea;
            padding-bottom: 10px;
        }
        .status-good {
            color: #28a745;
            font-weight: bold;
        }
        .status-warning {
            color: #ffc107;
            font-weight: bold;
        }
        .status-critical {
            color: #dc3545;
            font-weight: bold;
        }
        table {
            width: 100%;
            border-collapse: collapse;
            margin-top: 15px;
        }
        th, td {
            padding: 12px;
            text-align: left;
            border-bottom: 1px solid #ddd;
        }
        th {
            background-color: #f8f9fa;
            font-weight: bold;
            color: #667eea;
        }
        tr:hover {
            background-color: #f5f5f5;
        }
        .metric-chart {
            height: 200px;
            background: linear-gradient(to right, #f0f0f0 1%, #f0f0f0 1%);
            background-size: 20px 100%;
            border: 1px solid #ddd;
            border-radius: 4px;
            position: relative;
            margin: 20px 0;
        }
        .alert-box {
            padding: 15px;
            border-radius: 5px;
            margin: 15px 0;
        }
        .alert-critical {
            background-color: #f8d7da;
            border-left: 4px solid #dc3545;
            color: #721c24;
        }
        .alert-warning {
            background-color: #fff3cd;
            border-left: 4px solid #ffc107;
            color: #856404;
        }
        .alert-success {
            background-color: #d4edda;
            border-left: 4px solid #28a745;
            color: #155724;
        }
        .footer {
            text-align: center;
            color: #666;
            margin-top: 30px;
            padding: 20px;
            border-top: 1px solid #ddd;
        }
    </style>
</head>
<body>
    <div class="header">
        <h1>🔬 Chimera Evaluation Report</h1>
        <div class="subtitle">Day ${day} - Comprehensive System Analysis</div>
        <div class="timestamp">Generated: ${TIMESTAMP}</div>
    </div>

    <div class="summary-cards">
        <div class="card">
            <h3>Day Status</h3>
            <div class="value" id="day-status">Loading...</div>
            <div class="subtitle">Completion Status</div>
        </div>
        <div class="card">
            <h3>Total Trades</h3>
            <div class="value" id="total-trades">Loading...</div>
            <div class="subtitle">Trading Activity</div>
        </div>
        <div class="card">
            <h3>Success Rate</h3>
            <div class="value" id="success-rate">Loading...</div>
            <div class="subtitle">Trade Success Percentage</div>
        </div>
        <div class="card">
            <h3>Anomalies</h3>
            <div class="value" id="anomalies-count">Loading...</div>
            <div class="subtitle">Issues Detected</div>
        </div>
    </div>

    <div class="section">
        <h2>📊 Performance Overview</h2>
        <div id="performance-content">Loading performance data...</div>
    </div>

    <div class="section">
        <h2>⚠️ Anomalies & Issues</h2>
        <div id="anomalies-content">Loading anomaly data...</div>
    </div>

    <div class="section">
        <h2>💰 Financial Performance</h2>
        <div id="financial-content">Loading financial data...</div>
    </div>

    <div class="section">
        <h2>🔧 System Health</h2>
        <div id="health-content">Loading health data...</div>
    </div>

    <div class="footer">
        <p>Chimera Evaluation System - Automated Report Generation</p>
        <p>Report ID: $(uuidgen 2>/dev/null || echo "unknown")</p>
    </div>

    <script>
        // Parse the collected data
        const summaryData = ${ESCAPED_SUMMARY};
        const anomaliesData = ${ESCAPED_ANOMALIES};
        const performanceData = ${ESCAPED_PERFORMANCE};

        // Escape any value before inserting into innerHTML
        function escapeHtml(value) {
            return String(value).replace(/[&<>"']/g, function (c) {
                return {'&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;'}[c];
            });
        }

        // Update summary cards
        function updateSummaryCards() {
            const statusEl = document.getElementById('day-status');
            if (summaryData.success && summaryData.data) {
                const trades = summaryData.data.total_trades || 0;
                const success = summaryData.data.success_rate || 0;
                const anomalies = (summaryData.data.total_anomalies || 0);
                if (trades > 0 && success >= 95 && anomalies === 0) {
                    statusEl.textContent = 'Good';
                    statusEl.className = 'value status-good';
                } else if (anomalies > 0) {
                    statusEl.textContent = 'Review needed';
                    statusEl.className = 'value status-critical';
                } else {
                    statusEl.textContent = 'Monitor';
                    statusEl.className = 'value status-warning';
                }
            } else {
                statusEl.textContent = 'N/A';
            }
            if (summaryData.success && summaryData.data) {
                document.getElementById('total-trades').textContent = summaryData.data.total_trades || 0;
                document.getElementById('success-rate').textContent =
                    (summaryData.data.success_rate || 0).toFixed(1) + '%';
            }

            if (anomaliesData.success && anomaliesData.data) {
                document.getElementById('anomalies-count').textContent =
                    anomaliesData.data.total_anomalies || 0;
            }
        }

        // Render performance section
        function renderPerformance() {
            const container = document.getElementById('performance-content');
            if (!performanceData.success || !performanceData.data) {
                container.innerHTML = '<p class="alert-warning">Performance data not available</p>';
                return;
            }

            const data = performanceData.data;
            let html = '<table><tr><th>Metric</th><th>Value</th><th>Status</th></tr>';
            const e = escapeHtml;

            html += '<tr><td>Average Trade Latency</td><td>' + (data.avg_latency_ms || 0).toFixed(2) + ' ms</td>';
            html += '<td class="status-good">Good</td></tr>';

            html += '<tr><td>Total Trades</td><td>' + (data.total_trades || 0) + '</td>';
            html += '<td class="status-good">Active</td></tr>';

            html += '<tr><td>Success Rate</td><td>' + (data.success_rate_percent || 0).toFixed(1) + '%</td>';
            html += '<td class="' + (data.success_rate_percent >= 95 ? 'status-good' : 'status-warning') + '">';
            html += (data.success_rate_percent >= 95 ? 'Good' : 'Review') + '</td></tr>';

            html += '</table>';
            container.innerHTML = html;
        }

        // Render anomalies section
        function renderAnomalies() {
            const container = document.getElementById('anomalies-content');
            if (!anomaliesData.success || !anomaliesData.data) {
                container.innerHTML = '<p class="alert-success">No anomalies detected</p>';
                return;
            }

            const data = anomaliesData.data;
            let html = '';

            if (data.total_anomalies === 0) {
                html = '<div class="alert alert-success">✅ No anomalies detected during this period</div>';
            } else {
                html = '<div class="alert alert-critical">⚠️ ' + data.total_anomalies + ' anomalies detected</div>';
                html += '<p><strong>' + data.critical_count + '</strong> critical, <strong>' + data.warning_count + '</strong> warnings</p>';

                if (data.top_anomalies && data.top_anomalies.length > 0) {
                    html += '<h3>Top Anomalies:</h3><table><tr><th>Metric</th><th>Severity</th><th>Deviation</th><th>Description</th></tr>';
                    data.top_anomalies.forEach(anomaly => {
                        const sev = anomaly.severity || '';
                        html += '<tr><td>' + e(anomaly.metric) + '</td>';
                        html += '<td class="status-' + (sev === 'CRITICAL' ? 'critical' : 'warning') + '">';
                        html += e(sev) + '</td>';
                        html += '<td>' + (anomaly.deviation || 0).toFixed(1) + '%</td>';
                        html += '<td>' + e(anomaly.description || 'N/A') + '</td></tr>';
                    });
                    html += '</table>';
                }
            }

            container.innerHTML = html;
        }

        // Render financial section
        function renderFinancial() {
            const container = document.getElementById('financial-content');
            if (!summaryData.success || !summaryData.data) {
                container.innerHTML = '<p class="alert-warning">Financial data not available</p>';
                return;
            }

            const data = summaryData.data;
            let html = '<table><tr><th>Metric</th><th>Value</th></tr>';

            html += '<tr><td>Total PnL (SOL)</td><td>' + (data.total_pnl_sol || 0).toFixed(4) + ' SOL</td></tr>';
            html += '<tr><td>Total Costs (SOL)</td><td>' + (data.total_costs_sol || 0).toFixed(4) + ' SOL</td></tr>';
            html += '<tr><td>Net PnL</td><td>' + ((data.total_pnl_sol || 0) - (data.total_costs_sol || 0)).toFixed(4) + ' SOL</td></tr>';

            html += '</table>';
            container.innerHTML = html;
        }

        // Render health section
        function renderHealth() {
            const container = document.getElementById('health-content');
            container.innerHTML = '<div class="alert alert-success">✅ System operating normally</div>';
            container.innerHTML += '<p>All system components are within normal operating parameters.</p>';
        }

        // Initialize report
        document.addEventListener('DOMContentLoaded', function() {
            updateSummaryCards();
            renderPerformance();
            renderAnomalies();
            renderFinancial();
            renderHealth();
        });
    </script>
</body>
</html>
EOF

    if [ -s "${output_file}" ]; then
        echo -e "${GREEN}✓ HTML report generated: ${output_file}${NC}"
        echo "  Report size: $(wc -c < "${output_file}") bytes"
    else
        echo -e "${RED}✗ Failed to generate HTML report${NC}"
        return 1
    fi
}

# ===================================================================
# REPORT GENERATION
# ===================================================================

echo "Starting daily report generation for day ${DAY_NUM}..."

# Generate HTML report
# Treat collector failures (success: false) as a hard error so a report is
# never silently rendered with zero values.
if ! echo "$(collect_summary_stats ${DAY_NUM})" | jq -e '.success == true' > /dev/null 2>&1; then
    echo -e "${RED}✗ Summary statistics collection failed; aborting report generation${NC}"
    exit 1
fi

if generate_html_report ${DAY_NUM}; then
    REPORT_FILE="${OUTPUT_DIR}/reports/daily-report-day-${DAY_NUM}.html"

    # Generate summary for notifications
    SUMMARY=$(collect_summary_stats ${DAY_NUM})

    if echo "${SUMMARY}" | jq -e '.success == true' > /dev/null 2>&1; then
        TOTAL_TRADES=$(echo "${SUMMARY}" | jq -r '.data.total_trades // 0')
        SUCCESS_RATE=$(echo "${SUMMARY}" | jq -r '.data.success_rate // 0')
        TOTAL_ANOMALIES=$(collect_anomalies_summary ${DAY_NUM} | jq -r '.data.total_anomalies // 0')

        echo ""
        echo "=========================================="
        echo "Daily Report Summary"
        echo "=========================================="
        echo "Day: ${DAY_NUM}"
        echo "Total Trades: ${TOTAL_TRADES}"
        echo "Success Rate: ${SUCCESS_RATE}%"
        echo "Anomalies: ${TOTAL_ANOMALIES}"
        echo "Report: ${REPORT_FILE}"
        echo ""

        # Send notification if Telegram is configured
        if [ -n "${TELEGRAM_BOT_TOKEN:-}" ] && [ -n "${TELEGRAM_CHAT_ID:-}" ]; then
            TELEGRAM_MESSAGE="📊 Chimera Daily Report - Day ${DAY_NUM}
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
📈 Total Trades: ${TOTAL_TRADES}
✅ Success Rate: ${SUCCESS_RATE}%
⚠️ Anomalies: ${TOTAL_ANOMALIES}
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Report: ${REPORT_FILE}"

            if curl -s --max-time 10 "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/sendMessage" \
                --data-urlencode "chat_id=${TELEGRAM_CHAT_ID}" \
                --data-urlencode "text=${TELEGRAM_MESSAGE}" > /dev/null 2>&1; then
                echo -e "${GREEN}✓ Telegram notification sent${NC}"
            else
                echo -e "${RED}✗ Telegram notification failed${NC}"
            fi
        fi

        # Send notification if Discord is configured
        if [ -n "${DISCORD_WEBHOOK_URL:-}" ]; then
            DISCORD_PAYLOAD=$(cat <<EOF
{
  "embeds": [{
    "title": "📊 Chimera Daily Report - Day ${DAY_NUM}",
    "color": $([ ${TOTAL_ANOMALIES} -eq 0 ] && echo "5763729" || echo "16711680"),
    "fields": [
      {"name": "Total Trades", "value": "${TOTAL_TRADES}", "inline": true},
      {"name": "Success Rate", "value": "${SUCCESS_RATE}%", "inline": true},
      {"name": "Anomalies", "value": "${TOTAL_ANOMALIES}", "inline": true}
    ],
    "footer": {"text": "Chimera Evaluation System"}
  }]
}
EOF
)

            if curl -s --max-time 10 "${DISCORD_WEBHOOK_URL}" \
                -H "Content-Type: application/json" \
                -d "${DISCORD_PAYLOAD}" > /dev/null 2>&1; then
                echo -e "${GREEN}✓ Discord notification sent${NC}"
            else
                echo -e "${RED}✗ Discord notification failed${NC}"
            fi
        fi
    else
        echo -e "${YELLOW}⚠ Failed to collect summary statistics${NC}"
    fi
else
    echo -e "${RED}✗ Report generation failed${NC}"
    exit 1
fi

echo ""
echo "=========================================="
echo "Daily Report Generation Complete"
echo "=========================================="
echo "Report: ${REPORT_FILE}"
echo "Day: ${DAY_NUM}"
echo "Timestamp: ${TIMESTAMP}"
echo ""

exit 0