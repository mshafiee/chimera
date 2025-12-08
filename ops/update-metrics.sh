#!/bin/bash
# Chimera Daily Metrics Update Script
#
# Calculates metrics from database and updates via API endpoint.
# Runs daily via cron to ensure metrics are current even if scripts don't call API.
#
# Usage: ./update-metrics.sh [--api-url=URL] [--api-key=KEY]

set -euo pipefail

# Configuration
CHIMERA_HOME="${CHIMERA_HOME:-/opt/chimera}"
DB_PATH="${CHIMERA_HOME}/data/chimera.db"
API_URL="${API_URL:-http://localhost:8080}"
API_KEY="${API_KEY:-}"
LOG_FILE="/var/log/chimera/metrics-update.log"

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

log() {
    local level="$1"
    shift
    local color="${GREEN}"
    [[ "$level" == "WARN" ]] && color="${YELLOW}"
    [[ "$level" == "ERROR" ]] && color="${RED}"
    echo -e "${color}[$(date -u '+%Y-%m-%dT%H:%M:%SZ')] [$level]${NC} $*" | tee -a "$LOG_FILE"
}

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --api-url=*)
            API_URL="${1#*=}"
            shift
            ;;
        --api-key=*)
            API_KEY="${1#*=}"
            shift
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [--api-url=URL] [--api-key=KEY]"
            exit 1
            ;;
    esac
done

# Ensure log directory exists
mkdir -p "$(dirname "$LOG_FILE")"

# Check dependencies
if ! command -v curl &> /dev/null; then
    log "ERROR" "curl is required but not installed"
    exit 1
fi

if [[ ! -f "$DB_PATH" ]]; then
    log "ERROR" "Database not found at $DB_PATH"
    exit 1
fi

# Update reconciliation metrics
update_reconciliation_metrics() {
    log "INFO" "Calculating reconciliation metrics from database..."
    
    # Get total checked (from reconciliation_log table)
    local total_checked
    total_checked=$(sqlite3 "$DB_PATH" "
        SELECT COUNT(DISTINCT trade_uuid) 
        FROM reconciliation_log
        WHERE created_at > datetime('now', '-24 hours')
    " 2>/dev/null || echo "0")
    
    # Get total discrepancies found in last 24h
    local total_discrepancies
    total_discrepancies=$(sqlite3 "$DB_PATH" "
        SELECT COUNT(*) 
        FROM reconciliation_log
        WHERE discrepancy != 'NONE'
        AND created_at > datetime('now', '-24 hours')
    " 2>/dev/null || echo "0")
    
    # Get current unresolved count
    local unresolved
    unresolved=$(sqlite3 "$DB_PATH" "
        SELECT COUNT(*) 
        FROM reconciliation_log
        WHERE resolved_at IS NULL
    " 2>/dev/null || echo "0")
    
    log "INFO" "Reconciliation metrics: checked=$total_checked, discrepancies=$total_discrepancies, unresolved=$unresolved"
    
    # Update via API
    local response
    response=$(curl -s -X POST "${API_URL}/api/v1/metrics/reconciliation" \
        -H "Content-Type: application/json" \
        -H "Authorization: Bearer ${API_KEY}" \
        -d "{
            \"checked\": ${total_checked},
            \"discrepancies\": ${total_discrepancies},
            \"unresolved\": ${unresolved}
        }" 2>&1)
    
    if echo "$response" | grep -q '"status":"updated"'; then
        log "INFO" "Reconciliation metrics updated successfully"
    else
        log "WARN" "Failed to update reconciliation metrics: $response"
        return 1
    fi
}

# Update secret rotation metrics
update_secret_rotation_metrics() {
    log "INFO" "Calculating secret rotation metrics from database..."
    
    # Get last webhook HMAC rotation timestamp
    local last_rotation
    last_rotation=$(sqlite3 "$DB_PATH" "
        SELECT CAST(strftime('%s', changed_at) AS INTEGER)
        FROM config_audit
        WHERE key LIKE 'secret_rotation.webhook_hmac%'
        ORDER BY changed_at DESC
        LIMIT 1
    " 2>/dev/null || echo "")
    
    if [[ -z "$last_rotation" ]]; then
        log "WARN" "No secret rotation history found"
        return 0
    fi
    
    # Calculate days until due (webhook rotates every 30 days)
    local days_until_due
    days_until_due=$(sqlite3 "$DB_PATH" "
        SELECT CAST(30 - (julianday('now') - julianday(MAX(changed_at))) AS INTEGER)
        FROM config_audit
        WHERE key LIKE 'secret_rotation.webhook_hmac%'
    " 2>/dev/null || echo "0")
    
    log "INFO" "Secret rotation metrics: last_success=$last_rotation, days_until_due=$days_until_due"
    
    # Update via API
    local response
    response=$(curl -s -X POST "${API_URL}/api/v1/metrics/secret-rotation" \
        -H "Content-Type: application/json" \
        -H "Authorization: Bearer ${API_KEY}" \
        -d "{
            \"last_success_timestamp\": ${last_rotation},
            \"days_until_due\": ${days_until_due}
        }" 2>&1)
    
    if echo "$response" | grep -q '"status":"updated"'; then
        log "INFO" "Secret rotation metrics updated successfully"
    else
        log "WARN" "Failed to update secret rotation metrics: $response"
        return 1
    fi
}

# Main execution
main() {
    log "INFO" "Starting daily metrics update"
    
    local errors=0
    
    # Update reconciliation metrics
    if ! update_reconciliation_metrics; then
        ((errors++))
    fi
    
    # Update secret rotation metrics
    if ! update_secret_rotation_metrics; then
        ((errors++))
    fi
    
    if [[ $errors -eq 0 ]]; then
        log "INFO" "All metrics updated successfully"
        exit 0
    else
        log "WARN" "Some metrics updates failed (errors: $errors)"
        exit 1
    fi
}

main

