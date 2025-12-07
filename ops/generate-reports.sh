#!/bin/bash
# Chimera Compliance Report Generator
#
# Generates compliance reports for audit/tax purposes:
# - Trade history (CSV)
# - PnL summary (PDF/CSV)
# - Wallet roster changes (CSV)
# - Configuration audit trail (CSV)
#
# Usage: ./generate-reports.sh [--format=csv|pdf] [--period=30d|90d|all]

set -euo pipefail

# Configuration
CHIMERA_HOME="${CHIMERA_HOME:-/opt/chimera}"
DB_PATH="${CHIMERA_HOME}/data/chimera.db"
REPORTS_DIR="${CHIMERA_HOME}/reports"
PERIOD="${PERIOD:-30d}"
FORMAT="${FORMAT:-csv}"

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log() {
    echo -e "${GREEN}[INFO]${NC} $*"
}

# Calculate date range
get_date_range() {
    local period="$1"
    local end_date
    end_date=$(date -u '+%Y-%m-%d')
    
    case "$period" in
        30d)
            local start_date
            start_date=$(date -u -d '30 days ago' '+%Y-%m-%d' 2>/dev/null || date -u -v-30d '+%Y-%m-%d')
            echo "$start_date|$end_date"
            ;;
        90d)
            local start_date
            start_date=$(date -u -d '90 days ago' '+%Y-%m-%d' 2>/dev/null || date -u -v-90d '+%Y-%m-%d')
            echo "$start_date|$end_date"
            ;;
        all)
            echo "all|$end_date"
            ;;
        *)
            echo "all|$end_date"
            ;;
    esac
}

# Generate trade history report
generate_trade_history() {
    local output_file="$1"
    local date_range="$2"
    
    log "Generating trade history report..."
    
    local start_date
    local end_date
    IFS='|' read -r start_date end_date <<< "$date_range"
    
    if [[ "$start_date" == "all" ]]; then
        sqlite3 -header -csv "$DB_PATH" "
            SELECT 
                trade_uuid,
                wallet_address,
                token_address,
                token_symbol,
                strategy,
                side,
                amount_sol,
                price_at_signal,
                tx_signature,
                status,
                pnl_sol,
                pnl_usd,
                created_at,
                updated_at
            FROM trades
            ORDER BY created_at DESC
        " > "$output_file"
    else
        sqlite3 -header -csv "$DB_PATH" "
            SELECT 
                trade_uuid,
                wallet_address,
                token_address,
                token_symbol,
                strategy,
                side,
                amount_sol,
                price_at_signal,
                tx_signature,
                status,
                pnl_sol,
                pnl_usd,
                created_at,
                updated_at
            FROM trades
            WHERE date(created_at) >= date('$start_date')
            AND date(created_at) <= date('$end_date')
            ORDER BY created_at DESC
        " > "$output_file"
    fi
    
    log "Trade history saved to: $output_file"
}

# Generate PnL summary report
generate_pnl_summary() {
    local output_file="$1"
    local date_range="$2"
    
    log "Generating PnL summary report..."
    
    local start_date
    local end_date
    IFS='|' read -r start_date end_date <<< "$date_range"
    
    if [[ "$start_date" == "all" ]]; then
        sqlite3 -header -csv "$DB_PATH" "
            SELECT 
                strategy,
                COUNT(*) as trade_count,
                SUM(CASE WHEN pnl_usd > 0 THEN 1 ELSE 0 END) as winning_trades,
                SUM(CASE WHEN pnl_usd < 0 THEN 1 ELSE 0 END) as losing_trades,
                SUM(pnl_usd) as total_pnl_usd,
                AVG(pnl_usd) as avg_pnl_usd,
                MIN(pnl_usd) as min_pnl_usd,
                MAX(pnl_usd) as max_pnl_usd,
                SUM(amount_sol) as total_volume_sol
            FROM trades
            WHERE status = 'CLOSED' AND pnl_usd IS NOT NULL
            GROUP BY strategy
        " > "$output_file"
    else
        sqlite3 -header -csv "$DB_PATH" "
            SELECT 
                strategy,
                COUNT(*) as trade_count,
                SUM(CASE WHEN pnl_usd > 0 THEN 1 ELSE 0 END) as winning_trades,
                SUM(CASE WHEN pnl_usd < 0 THEN 1 ELSE 0 END) as losing_trades,
                SUM(pnl_usd) as total_pnl_usd,
                AVG(pnl_usd) as avg_pnl_usd,
                MIN(pnl_usd) as min_pnl_usd,
                MAX(pnl_usd) as max_pnl_usd,
                SUM(amount_sol) as total_volume_sol
            FROM trades
            WHERE status = 'CLOSED' 
            AND pnl_usd IS NOT NULL
            AND date(created_at) >= date('$start_date')
            AND date(created_at) <= date('$end_date')
            GROUP BY strategy
        " > "$output_file"
    fi
    
    log "PnL summary saved to: $output_file"
}

# Generate wallet roster changes report
generate_wallet_changes() {
    local output_file="$1"
    local date_range="$2"
    
    log "Generating wallet roster changes report..."
    
    local start_date
    local end_date
    IFS='|' read -r start_date end_date <<< "$date_range"
    
    if [[ "$start_date" == "all" ]]; then
        sqlite3 -header -csv "$DB_PATH" "
            SELECT 
                key,
                old_value,
                new_value,
                changed_by,
                change_reason,
                changed_at
            FROM config_audit
            WHERE key LIKE 'wallet:%'
            ORDER BY changed_at DESC
        " > "$output_file"
    else
        sqlite3 -header -csv "$DB_PATH" "
            SELECT 
                key,
                old_value,
                new_value,
                changed_by,
                change_reason,
                changed_at
            FROM config_audit
            WHERE key LIKE 'wallet:%'
            AND date(changed_at) >= date('$start_date')
            AND date(changed_at) <= date('$end_date')
            ORDER BY changed_at DESC
        " > "$output_file"
    fi
    
    log "Wallet changes saved to: $output_file"
}

# Generate configuration audit trail
generate_config_audit() {
    local output_file="$1"
    local date_range="$2"
    
    log "Generating configuration audit trail..."
    
    local start_date
    local end_date
    IFS='|' read -r start_date end_date <<< "$date_range"
    
    if [[ "$start_date" == "all" ]]; then
        sqlite3 -header -csv "$DB_PATH" "
            SELECT 
                id,
                key,
                old_value,
                new_value,
                changed_by,
                change_reason,
                changed_at
            FROM config_audit
            ORDER BY changed_at DESC
        " > "$output_file"
    else
        sqlite3 -header -csv "$DB_PATH" "
            SELECT 
                id,
                key,
                old_value,
                new_value,
                changed_by,
                change_reason,
                changed_at
            FROM config_audit
            WHERE date(changed_at) >= date('$start_date')
            AND date(changed_at) <= date('$end_date')
            ORDER BY changed_at DESC
        " > "$output_file"
    fi
    
    log "Config audit trail saved to: $output_file"
}

# Generate reconciliation discrepancies report
generate_reconciliation_report() {
    local output_file="$1"
    
    log "Generating reconciliation discrepancies report..."
    
    sqlite3 -header -csv "$DB_PATH" "
        SELECT 
            id,
            trade_uuid,
            expected_state,
            actual_on_chain,
            discrepancy,
            on_chain_tx_signature,
            on_chain_amount_sol,
            expected_amount_sol,
            resolved_at,
            resolved_by,
            notes,
            created_at
        FROM reconciliation_log
        WHERE resolved_at IS NULL
        ORDER BY created_at DESC
    " > "$output_file"
    
    log "Reconciliation report saved to: $output_file"
}

# Main report generation
main() {
    local date_range
    date_range=$(get_date_range "$PERIOD")
    
    local timestamp
    timestamp=$(date -u '+%Y%m%d_%H%M%S')
    
    mkdir -p "$REPORTS_DIR"
    
    log "Generating compliance reports (period: $PERIOD, format: $FORMAT)"
    
    # Generate all reports
    generate_trade_history "${REPORTS_DIR}/trade_history_${PERIOD}_${timestamp}.csv" "$date_range"
    generate_pnl_summary "${REPORTS_DIR}/pnl_summary_${PERIOD}_${timestamp}.csv" "$date_range"
    generate_wallet_changes "${REPORTS_DIR}/wallet_changes_${PERIOD}_${timestamp}.csv" "$date_range"
    generate_config_audit "${REPORTS_DIR}/config_audit_${PERIOD}_${timestamp}.csv" "$date_range"
    generate_reconciliation_report "${REPORTS_DIR}/reconciliation_discrepancies_${timestamp}.csv"
    
    log "All reports generated in: $REPORTS_DIR"
    
    # Create summary
    local summary_file="${REPORTS_DIR}/report_summary_${timestamp}.txt"
    cat > "$summary_file" << EOF
Chimera Compliance Report Summary
Generated: $(date -u '+%Y-%m-%d %H:%M:%S UTC')
Period: $PERIOD

Reports Generated:
- Trade History: trade_history_${PERIOD}_${timestamp}.csv
- PnL Summary: pnl_summary_${PERIOD}_${timestamp}.csv
- Wallet Changes: wallet_changes_${PERIOD}_${timestamp}.csv
- Config Audit: config_audit_${PERIOD}_${timestamp}.csv
- Reconciliation: reconciliation_discrepancies_${timestamp}.csv

EOF
    
    log "Summary saved to: $summary_file"
}

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --period=*)
            PERIOD="${1#*=}"
            shift
            ;;
        --format=*)
            FORMAT="${1#*=}"
            shift
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [--period=30d|90d|all] [--format=csv|pdf]"
            exit 1
            ;;
    esac
done

main
