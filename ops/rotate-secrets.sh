#!/bin/bash
# Chimera Secret Rotation Script
#
# Automatically rotates webhook HMAC secrets according to PDD schedule:
# - Webhook HMAC Key: Every 30 days
# - RPC API Keys: Every 90 days
#
# Features:
# - Generates cryptographically secure secrets
# - Updates encrypted config with grace period
# - Sends notification on rotation
# - Logs rotation to config_audit table
#
# Usage: ./rotate-secrets.sh [--force] [--type=webhook|rpc]

set -euo pipefail

# Configuration
CHIMERA_HOME="${CHIMERA_HOME:-/opt/chimera}"
DB_PATH="${CHIMERA_HOME}/data/chimera.db"
CONFIG_FILE="${CHIMERA_HOME}/config/.env"
LOG_FILE="/var/log/chimera/secret-rotation.log"
GRACE_PERIOD_HOURS=24

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log() {
    local level="$1"
    shift
    echo "[$(date -u '+%Y-%m-%dT%H:%M:%SZ')] [$level] $*" | tee -a "$LOG_FILE"
}

send_notification() {
    local message="$1"
    
    if [[ -n "${TELEGRAM_BOT_TOKEN:-}" && -n "${TELEGRAM_CHAT_ID:-}" ]]; then
        curl -s -X POST "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/sendMessage" \
            -d "chat_id=${TELEGRAM_CHAT_ID}" \
            -d "text=🔐 Chimera Secret Rotation: ${message}" \
            -d "parse_mode=HTML" > /dev/null 2>&1 || true
    fi
}

# Generate cryptographically secure secret
generate_secret() {
    openssl rand -hex 32
}

# Check if rotation is due
check_rotation_due() {
    local secret_type="$1"
    local rotation_days="$2"
    
    # Get last rotation date from config_audit
    local last_rotation
    last_rotation=$(sqlite3 "$DB_PATH" "
        SELECT changed_at 
        FROM config_audit 
        WHERE key LIKE 'secret_rotation.${secret_type}%'
        ORDER BY changed_at DESC 
        LIMIT 1
    " 2>/dev/null || echo "")
    
    if [[ -z "$last_rotation" ]]; then
        log "INFO" "No previous rotation found for $secret_type - rotation due"
        return 0
    fi
    
    # Calculate days since last rotation
    local days_since
    days_since=$(sqlite3 "$DB_PATH" "
        SELECT CAST(julianday('now') - julianday('$last_rotation') AS INTEGER)
    " 2>/dev/null || echo "0")
    
    if [[ $days_since -ge $rotation_days ]]; then
        log "INFO" "Rotation due for $secret_type (last rotated $days_since days ago)"
        return 0
    else
        log "INFO" "Rotation not due for $secret_type (rotated $days_since days ago, need $rotation_days)"
        return 1
    fi
}

# Rotate webhook HMAC secret
rotate_webhook_secret() {
    log "INFO" "Starting webhook HMAC secret rotation"
    
    # Read current secret
    local current_secret
    current_secret=$(grep "^CHIMERA_SECURITY__WEBHOOK_SECRET=" "$CONFIG_FILE" 2>/dev/null | cut -d= -f2 || echo "")
    
    if [[ -z "$current_secret" ]]; then
        log "ERROR" "Current webhook secret not found in config"
        return 1
    fi
    
    # Generate new secret
    local new_secret
    new_secret=$(generate_secret)
    
    log "INFO" "Generated new webhook secret (length: ${#new_secret})"
    
    # Backup current config
    cp "$CONFIG_FILE" "${CONFIG_FILE}.backup.$(date +%Y%m%d_%H%M%S)"
    
    # Update config: set previous secret and new secret
    if grep -q "^CHIMERA_SECURITY__WEBHOOK_SECRET_PREVIOUS=" "$CONFIG_FILE"; then
        # Update existing previous secret line
        sed -i.bak "s|^CHIMERA_SECURITY__WEBHOOK_SECRET_PREVIOUS=.*|CHIMERA_SECURITY__WEBHOOK_SECRET_PREVIOUS=${current_secret}|" "$CONFIG_FILE"
    else
        # Add previous secret line
        echo "CHIMERA_SECURITY__WEBHOOK_SECRET_PREVIOUS=${current_secret}" >> "$CONFIG_FILE"
    fi
    
    # Update current secret
    sed -i.bak "s|^CHIMERA_SECURITY__WEBHOOK_SECRET=.*|CHIMERA_SECURITY__WEBHOOK_SECRET=${new_secret}|" "$CONFIG_FILE"
    rm -f "${CONFIG_FILE}.bak"
    
    # Log to database
    sqlite3 "$DB_PATH" "
        INSERT INTO config_audit (key, old_value, new_value, changed_by, change_reason)
        VALUES (
            'secret_rotation.webhook_hmac',
            '[REDACTED]',
            '[REDACTED]',
            'SYSTEM_ROTATION',
            'Automated secret rotation (grace period: ${GRACE_PERIOD_HOURS}h)'
        );
    "
    
    log "INFO" "Webhook secret rotated successfully"
    log "INFO" "Grace period active: old and new secrets accepted for ${GRACE_PERIOD_HOURS} hours"
    
    # Update metrics via API
    if [[ -n "${API_URL:-}" ]] && [[ -n "${API_KEY:-}" ]]; then
        local current_timestamp
        current_timestamp=$(date +%s)
        local days_until_due=30  # Next rotation in 30 days
        
        log "INFO" "Updating secret rotation metrics via API..."
        local metrics_response
        metrics_response=$(curl -s -X POST "${API_URL}/api/v1/metrics/secret-rotation" \
            -H "Content-Type: application/json" \
            -H "Authorization: Bearer ${API_KEY}" \
            -d "{
                \"last_success_timestamp\": ${current_timestamp},
                \"days_until_due\": ${days_until_due}
            }" 2>&1)
        
        if echo "$metrics_response" | grep -q '"status":"updated"'; then
            log "INFO" "Metrics updated successfully"
        else
            log "WARN" "Failed to update metrics: $metrics_response"
        fi
    else
        log "INFO" "Skipping metrics update (API_URL or API_KEY not set)"
    fi
    
    send_notification "Webhook HMAC secret rotated. Grace period: ${GRACE_PERIOD_HOURS}h"
    
    return 0
}

# Rotate RPC API key
rotate_rpc_key() {
    local key_type="$1"  # primary or fallback
    
    log "INFO" "Starting RPC API key rotation for: $key_type"
    
    # Check if key exists
    local env_var
    if [[ "$key_type" == "primary" ]]; then
        env_var="CHIMERA_RPC__PRIMARY_URL"
    else
        env_var="CHIMERA_RPC__FALLBACK_URL"
    fi
    
    local current_url
    current_url=$(grep "^${env_var}=" "$CONFIG_FILE" 2>/dev/null | cut -d= -f2- || echo "")
    
    if [[ -z "$current_url" ]]; then
        log "WARN" "RPC URL not found for $key_type - skipping rotation"
        return 0
    fi
    
    # Extract API key from URL (if present)
    if echo "$current_url" | grep -q "api-key="; then
        log "INFO" "API key found in URL - manual rotation required"
        log "INFO" "Update ${env_var} in config file with new API key"
        
        sqlite3 "$DB_PATH" "
            INSERT INTO config_audit (key, old_value, new_value, changed_by, change_reason)
            VALUES (
                'secret_rotation.rpc_${key_type}',
                'MANUAL_ROTATION_REQUIRED',
                'MANUAL_ROTATION_REQUIRED',
                'SYSTEM_ROTATION',
                'RPC API key rotation requires manual update'
            );
        "
        
        send_notification "RPC ${key_type} key rotation reminder - manual update required"
    else
        log "INFO" "No API key in URL - rotation not applicable"
    fi
    
    return 0
}

# Main rotation logic
main() {
    local force_rotation="${FORCE_ROTATION:-false}"
    local secret_type="${SECRET_TYPE:-all}"
    
    # Ensure log directory exists
    mkdir -p "$(dirname "$LOG_FILE")"
    
    log "INFO" "Starting secret rotation check"
    
    # Check if database exists
    if [[ ! -f "$DB_PATH" ]]; then
        log "ERROR" "Database not found at $DB_PATH"
        exit 1
    fi
    
    # Check if config file exists
    if [[ ! -f "$CONFIG_FILE" ]]; then
        log "ERROR" "Config file not found at $CONFIG_FILE"
        exit 1
    fi
    
    # Rotate webhook secret if due or forced
    if [[ "$secret_type" == "all" || "$secret_type" == "webhook" ]]; then
        if [[ "$force_rotation" == "true" ]] || check_rotation_due "webhook" 30; then
            rotate_webhook_secret || log "ERROR" "Webhook secret rotation failed"
        fi
    fi
    
    # Rotate RPC keys if due or forced
    if [[ "$secret_type" == "all" || "$secret_type" == "rpc" ]]; then
        if [[ "$force_rotation" == "true" ]] || check_rotation_due "rpc_primary" 90; then
            rotate_rpc_key "primary"
        fi
        
        if [[ "$force_rotation" == "true" ]] || check_rotation_due "rpc_fallback" 90; then
            rotate_rpc_key "fallback"
        fi
    fi
    
    log "INFO" "Secret rotation check complete"
    
    # Reload service to pick up new secrets (if service is running)
    if systemctl is-active --quiet chimera 2>/dev/null; then
        log "INFO" "Reloading service to pick up new secrets"
        systemctl reload chimera || systemctl restart chimera
    fi
}

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --force)
            FORCE_ROTATION=true
            shift
            ;;
        --type=*)
            SECRET_TYPE="${1#*=}"
            shift
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [--force] [--type=webhook|rpc|all]"
            exit 1
            ;;
    esac
done

main
