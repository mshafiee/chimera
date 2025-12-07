#!/bin/bash
# Chimera Daily Reconciliation Script
# Runs daily via cron at 4 AM
#
# Purpose:
# - Compare DB state (positions) vs on-chain state
# - Detect discrepancies (missing transactions, amount mismatches)
# - Log findings to reconciliation_log table
# - Send alerts for unresolved issues
#
# Requires:
# - solana CLI installed and configured
# - jq for JSON parsing
# - Access to Helius or other RPC endpoint

set -euo pipefail

# Configuration
CHIMERA_HOME="${CHIMERA_HOME:-/opt/chimera}"
DB_PATH="${CHIMERA_HOME}/data/chimera.db"
LOG_FILE="/var/log/chimera/reconcile.log"
RPC_URL="${HELIUS_RPC_URL:-https://api.mainnet-beta.solana.com}"
EPSILON="0.0001"  # Dust tolerance for amount comparisons (0.01%)
NOTIFY_ON_DISCREPANCY="${NOTIFY_ON_DISCREPANCY:-true}"

# Logging function
log() {
    local level="$1"
    shift
    echo "[$(date -u '+%Y-%m-%dT%H:%M:%SZ')] [$level] $*" | tee -a "$LOG_FILE"
}

# Send notification
notify() {
    local level="$1"
    local message="$2"
    
    if [[ "$NOTIFY_ON_DISCREPANCY" == "true" ]]; then
        if [[ -n "${TELEGRAM_BOT_TOKEN:-}" && -n "${TELEGRAM_CHAT_ID:-}" ]]; then
            local emoji="📊"
            [[ "$level" == "CRITICAL" ]] && emoji="🚨"
            [[ "$level" == "WARNING" ]] && emoji="⚠️"
            
            curl -s -X POST "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/sendMessage" \
                -d "chat_id=${TELEGRAM_CHAT_ID}" \
                -d "text=${emoji} Chimera Reconciliation: ${message}" \
                -d "parse_mode=HTML" > /dev/null 2>&1 || true
        fi
    fi
}

# Query on-chain transaction status
check_transaction() {
    local signature="$1"
    local result
    
    result=$(curl -s -X POST "$RPC_URL" \
        -H "Content-Type: application/json" \
        -d '{
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getTransaction",
            "params": ["'"$signature"'", {"encoding": "json", "maxSupportedTransactionVersion": 0}]
        }' 2>/dev/null)
    
    if echo "$result" | jq -e '.result != null' > /dev/null 2>&1; then
        echo "FOUND"
    elif echo "$result" | jq -e '.error != null' > /dev/null 2>&1; then
        echo "ERROR"
    else
        echo "MISSING"
    fi
}

# Main reconciliation logic
reconcile() {
    log "INFO" "Starting daily reconciliation"
    
    local total_checked=0
    local discrepancies_found=0
    local auto_resolved=0
    
    # Query all ACTIVE and EXITING positions
    local positions
    positions=$(sqlite3 -json "$DB_PATH" "
        SELECT 
            p.trade_uuid,
            p.token_address,
            p.entry_tx_signature,
            p.exit_tx_signature,
            p.entry_amount_sol,
            p.state,
            t.status as trade_status
        FROM positions p
        JOIN trades t ON p.trade_uuid = t.trade_uuid
        WHERE p.state IN ('ACTIVE', 'EXITING')
        ORDER BY p.opened_at DESC
        LIMIT 100
    " 2>/dev/null || echo "[]")
    
    # Handle case where no positions exist
    if [[ "$positions" == "[]" || -z "$positions" ]]; then
        log "INFO" "No active positions to reconcile"
        return 0
    fi
    
    # Process each position
    echo "$positions" | jq -c '.[]' | while read -r position; do
        local trade_uuid=$(echo "$position" | jq -r '.trade_uuid')
        local entry_sig=$(echo "$position" | jq -r '.entry_tx_signature')
        local exit_sig=$(echo "$position" | jq -r '.exit_tx_signature // empty')
        local expected_amount=$(echo "$position" | jq -r '.entry_amount_sol')
        local state=$(echo "$position" | jq -r '.state')
        
        ((total_checked++))
        
        log "DEBUG" "Checking position: $trade_uuid (state: $state)"
        
        # Check entry transaction
        if [[ -n "$entry_sig" && "$entry_sig" != "null" ]]; then
            local entry_status=$(check_transaction "$entry_sig")
            
            if [[ "$entry_status" == "MISSING" ]]; then
                log "WARNING" "Entry TX missing for $trade_uuid: $entry_sig"
                ((discrepancies_found++))
                
                # Log to reconciliation table
                sqlite3 "$DB_PATH" "
                    INSERT INTO reconciliation_log 
                    (trade_uuid, expected_state, actual_on_chain, discrepancy, 
                     on_chain_tx_signature, expected_amount_sol, notes)
                    VALUES 
                    ('$trade_uuid', '$state', 'MISSING', 'MISSING_TX',
                     '$entry_sig', $expected_amount, 'Entry transaction not found on-chain');
                "
            fi
        fi
        
        # Check exit transaction for EXITING positions
        if [[ "$state" == "EXITING" && -n "$exit_sig" && "$exit_sig" != "null" ]]; then
            local exit_status=$(check_transaction "$exit_sig")
            
            if [[ "$exit_status" == "FOUND" ]]; then
                # Transaction confirmed but DB still shows EXITING - auto-resolve
                log "INFO" "Auto-resolving: $trade_uuid exit confirmed on-chain"
                ((auto_resolved++))
                
                sqlite3 "$DB_PATH" "
                    UPDATE positions SET state = 'CLOSED', closed_at = datetime('now') 
                    WHERE trade_uuid = '$trade_uuid';
                    
                    UPDATE trades SET status = 'CLOSED', updated_at = datetime('now')
                    WHERE trade_uuid = '$trade_uuid';
                    
                    INSERT INTO reconciliation_log 
                    (trade_uuid, expected_state, actual_on_chain, discrepancy, 
                     on_chain_tx_signature, resolved_at, resolved_by, notes)
                    VALUES 
                    ('$trade_uuid', 'EXITING', 'FOUND', 'STATE_MISMATCH',
                     '$exit_sig', datetime('now'), 'AUTO', 'Auto-resolved: exit confirmed on-chain');
                "
            elif [[ "$exit_status" == "MISSING" ]]; then
                log "WARNING" "Exit TX missing for $trade_uuid: $exit_sig"
                ((discrepancies_found++))
                
                sqlite3 "$DB_PATH" "
                    INSERT INTO reconciliation_log 
                    (trade_uuid, expected_state, actual_on_chain, discrepancy, 
                     on_chain_tx_signature, expected_amount_sol, notes)
                    VALUES 
                    ('$trade_uuid', 'EXITING', 'MISSING', 'MISSING_TX',
                     '$exit_sig', $expected_amount, 'Exit transaction not found on-chain');
                "
            fi
        fi
        
        # Rate limit to avoid RPC throttling
        sleep 0.2
    done
    
    # Check for unresolved discrepancies
    local unresolved_count
    unresolved_count=$(sqlite3 "$DB_PATH" "
        SELECT COUNT(*) FROM reconciliation_log 
        WHERE resolved_at IS NULL 
        AND created_at > datetime('now', '-24 hours')
    " 2>/dev/null || echo "0")
    
    # Summary
    log "INFO" "Reconciliation complete: checked=$total_checked, discrepancies=$discrepancies_found, auto_resolved=$auto_resolved, unresolved=$unresolved_count"
    
    # Update metrics via API
    if [[ -n "${API_URL:-}" ]] && [[ -n "${API_KEY:-}" ]]; then
        log "INFO" "Updating reconciliation metrics via API..."
        local metrics_response
        metrics_response=$(curl -s -X POST "${API_URL}/api/v1/metrics/reconciliation" \
            -H "Content-Type: application/json" \
            -H "Authorization: Bearer ${API_KEY}" \
            -d "{
                \"checked\": ${total_checked},
                \"discrepancies\": ${discrepancies_found},
                \"unresolved\": ${unresolved_count}
            }" 2>&1)
        
        if echo "$metrics_response" | grep -q '"status":"updated"'; then
            log "INFO" "Metrics updated successfully"
        else
            log "WARN" "Failed to update metrics: $metrics_response"
        fi
    else
        log "INFO" "Skipping metrics update (API_URL or API_KEY not set)"
    fi
    
    # Send alert if there are unresolved discrepancies
    if [[ "$unresolved_count" -gt 0 ]]; then
        notify "WARNING" "Found $unresolved_count unresolved discrepancies in the last 24h. Manual review required."
    fi
    
    # Record successful reconciliation run
    sqlite3 "$DB_PATH" "
        INSERT INTO config_audit (key, new_value, changed_by, change_reason)
        VALUES ('reconciliation_run', datetime('now'), 'SYSTEM_RECONCILE', 
                'Checked: $total_checked, Discrepancies: $discrepancies_found, Auto-resolved: $auto_resolved');
    "
}

# Ensure log directory exists
mkdir -p "$(dirname "$LOG_FILE")"

# Check dependencies
if ! command -v jq &> /dev/null; then
    log "ERROR" "jq is required but not installed"
    exit 1
fi

if ! command -v curl &> /dev/null; then
    log "ERROR" "curl is required but not installed"
    exit 1
fi

if [[ ! -f "$DB_PATH" ]]; then
    log "ERROR" "Database not found at $DB_PATH"
    exit 1
fi

# Run reconciliation
reconcile

exit 0
