#!/bin/bash
# Chimera Secret Rotation Script
#
# Automatically rotates webhook HMAC secrets on a schedule (every 30 days).
# Implements grace period where both old and new secrets are accepted.
#
# Usage: ./rotate-secrets.sh [--force]
#
# Features:
# - Generates cryptographically secure new secret
# - Updates configuration with grace period support
# - Sends notification on rotation
# - Logs rotation to config_audit table

set -euo pipefail

# Configuration
CHIMERA_HOME="${CHIMERA_HOME:-/opt/chimera}"
CONFIG_FILE="${CHIMERA_HOME}/config/.env"
CONFIG_YAML="${CHIMERA_HOME}/config/config.yaml"
DB_PATH="${CHIMERA_HOME}/data/chimera.db"
ROTATION_INTERVAL_DAYS=30
GRACE_PERIOD_HOURS=24
LOG_FILE="/var/log/chimera/secret-rotation.log"
NOTIFY_ON_ROTATION="${NOTIFY_ON_ROTATION:-true}"

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

log() {
    local level="$1"
    shift
    echo "[$(date -u '+%Y-%m-%dT%H:%M:%SZ')] [$level] $*" | tee -a "$LOG_FILE"
}

log_info() { log "INFO" "$@"; }
log_warn() { log "WARN" "$@"; }
log_error() { log "ERROR" "$@"; }

# Send notification
notify() {
    local message="$1"
    
    if [[ "$NOTIFY_ON_ROTATION" == "true" ]]; then
        if [[ -n "${TELEGRAM_BOT_TOKEN:-}" && -n "${TELEGRAM_CHAT_ID:-}" ]]; then
            curl -s -X POST "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/sendMessage" \
                -d "chat_id=${TELEGRAM_CHAT_ID}" \
                -d "text=🔐 Chimera Secret Rotation: ${message}" \
                -d "parse_mode=HTML" > /dev/null 2>&1 || true
        fi
    fi
}

# Generate cryptographically secure secret
generate_secret() {
    # Use openssl if available, otherwise fallback to /dev/urandom
    if command -v openssl &> /dev/null; then
        openssl rand -hex 32
    else
        head -c 32 /dev/urandom | xxd -p -c 32 | tr -d '\n'
    fi
}

# Get current secret from config
get_current_secret() {
    if [[ -f "$CONFIG_FILE" ]]; then
        grep "^CHIMERA_SECURITY__WEBHOOK_SECRET=" "$CONFIG_FILE" | cut -d= -f2- | tr -d '"' || echo ""
    else
        echo ""
    fi
}

# Get previous secret from config
get_previous_secret() {
    if [[ -f "$CONFIG_FILE" ]]; then
        grep "^CHIMERA_SECURITY__WEBHOOK_SECRET_PREVIOUS=" "$CONFIG_FILE" | cut -d= -f2- | tr -d '"' || echo ""
    else
        echo ""
    fi
}

# Get last rotation date from database
get_last_rotation_date() {
    sqlite3 "$DB_PATH" "
        SELECT changed_at 
        FROM config_audit 
        WHERE key = 'webhook_secret_rotation' 
        ORDER BY changed_at DESC 
        LIMIT 1
    " 2>/dev/null || echo ""
}

# Check if rotation is due
is_rotation_due() {
    local last_rotation
    last_rotation=$(get_last_rotation_date)
    
    if [[ -z "$last_rotation" ]]; then
        log_info "No previous rotation found - rotation due"
        return 0
    fi
    
    # Calculate days since last rotation
    local last_epoch
    last_epoch=$(date -d "$last_rotation" +%s 2>/dev/null || date -j -f "%Y-%m-%d %H:%M:%S" "$last_rotation" +%s 2>/dev/null || echo "0")
    local current_epoch
    current_epoch=$(date +%s)
    local days_since
    days_since=$(( (current_epoch - last_epoch) / 86400 ))
    
    if [[ $days_since -ge $ROTATION_INTERVAL_DAYS ]]; then
        log_info "Rotation due: ${days_since} days since last rotation (threshold: ${ROTATION_INTERVAL_DAYS} days)"
        return 0
    else
        log_info "Rotation not due: ${days_since} days since last rotation (threshold: ${ROTATION_INTERVAL_DAYS} days)"
        return 1
    fi
}

# Rotate secret
rotate_secret() {
    log_info "Starting secret rotation..."
    
    local current_secret
    current_secret=$(get_current_secret)
    local new_secret
    new_secret=$(generate_secret)
    
    if [[ -z "$current_secret" ]]; then
        log_error "Current secret not found in config"
        return 1
    fi
    
    log_info "Generated new secret (length: ${#new_secret} chars)"
    
    # Backup current config
    if [[ -f "$CONFIG_FILE" ]]; then
        cp "$CONFIG_FILE" "${CONFIG_FILE}.backup.$(date +%Y%m%d_%H%M%S)"
    fi
    
    # Update config file
    if [[ -f "$CONFIG_FILE" ]]; then
        # Update current secret to previous
        sed -i.bak "s|^CHIMERA_SECURITY__WEBHOOK_SECRET_PREVIOUS=.*|CHIMERA_SECURITY__WEBHOOK_SECRET_PREVIOUS=\"${current_secret}\"|" "$CONFIG_FILE" || \
            echo "CHIMERA_SECURITY__WEBHOOK_SECRET_PREVIOUS=\"${current_secret}\"" >> "$CONFIG_FILE"
        
        # Update current secret to new
        sed -i.bak "s|^CHIMERA_SECURITY__WEBHOOK_SECRET=.*|CHIMERA_SECURITY__WEBHOOK_SECRET=\"${new_secret}\"|" "$CONFIG_FILE" || \
            echo "CHIMERA_SECURITY__WEBHOOK_SECRET=\"${new_secret}\"" >> "$CONFIG_FILE"
        
        # Remove backup files
        rm -f "${CONFIG_FILE}.bak"
    else
        log_error "Config file not found: $CONFIG_FILE"
        return 1
    fi
    
    # Log to database
    if [[ -f "$DB_PATH" ]]; then
        sqlite3 "$DB_PATH" "
            INSERT INTO config_audit (key, old_value, new_value, changed_by, change_reason)
            VALUES (
                'webhook_secret_rotation',
                '${current_secret:0:8}...',
                '${new_secret:0:8}...',
                'SYSTEM_ROTATION',
                'Automated secret rotation (grace period: ${GRACE_PERIOD_HOURS}h)'
            );
        " 2>/dev/null || log_warn "Failed to log rotation to database"
    fi
    
    # Reload service to pick up new secret (grace period allows both)
    if systemctl is-active --quiet chimera 2>/dev/null; then
        log_info "Reloading service to apply new secret..."
        systemctl reload chimera 2>/dev/null || systemctl restart chimera
        log_info "Service reloaded (grace period active for ${GRACE_PERIOD_HOURS} hours)"
    else
        log_warn "Service not running - new secret will be active on next start"
    fi
    
    # Send notification
    notify "Webhook secret rotated. Grace period: ${GRACE_PERIOD_HOURS}h. Old secret expires at $(date -d "+${GRACE_PERIOD_HOURS} hours" -u '+%Y-%m-%d %H:%M:%S UTC')."
    
    log_info "Secret rotation complete"
    log_info "Grace period: ${GRACE_PERIOD_HOURS} hours (both secrets accepted)"
    log_info "Old secret expires: $(date -d "+${GRACE_PERIOD_HOURS} hours" -u '+%Y-%m-%d %H:%M:%S UTC')"
    
    return 0
}

# Clean up expired previous secret (after grace period)
cleanup_expired_secret() {
    local last_rotation
    last_rotation=$(get_last_rotation_date)
    
    if [[ -z "$last_rotation" ]]; then
        return 0
    fi
    
    # Calculate hours since last rotation
    local last_epoch
    last_epoch=$(date -d "$last_rotation" +%s 2>/dev/null || date -j -f "%Y-%m-%d %H:%M:%S" "$last_rotation" +%s 2>/dev/null || echo "0")
    local current_epoch
    current_epoch=$(date +%s)
    local hours_since
    hours_since=$(( (current_epoch - last_epoch) / 3600 ))
    
    if [[ $hours_since -gt $GRACE_PERIOD_HOURS ]]; then
        log_info "Grace period expired (${hours_since}h > ${GRACE_PERIOD_HOURS}h) - removing previous secret"
        
        # Remove previous secret from config
        if [[ -f "$CONFIG_FILE" ]]; then
            sed -i.bak '/^CHIMERA_SECURITY__WEBHOOK_SECRET_PREVIOUS=/d' "$CONFIG_FILE"
            rm -f "${CONFIG_FILE}.bak"
            
            # Reload service
            if systemctl is-active --quiet chimera 2>/dev/null; then
                systemctl reload chimera 2>/dev/null || systemctl restart chimera
            fi
            
            log_info "Previous secret removed"
        fi
    fi
}

# Main
main() {
    local force_rotation=false
    
    # Parse arguments
    while [[ $# -gt 0 ]]; do
        case $1 in
            --force|-f)
                force_rotation=true
                shift
                ;;
            *)
                echo "Unknown option: $1"
                echo "Usage: $0 [--force]"
                exit 1
                ;;
        esac
    done
    
    # Ensure log directory exists
    mkdir -p "$(dirname "$LOG_FILE")"
    
    log_info "Secret rotation check started"
    
    # Clean up expired secrets first
    cleanup_expired_secret
    
    # Check if rotation is due or forced
    if [[ "$force_rotation" == "true" ]] || is_rotation_due; then
        rotate_secret
        exit $?
    else
        log_info "Rotation not due yet"
        exit 0
    fi
}

main "$@"

