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
        curl -s --max-time 10 -X POST "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/sendMessage" \
            -d "chat_id=${TELEGRAM_CHAT_ID}" \
            --data-urlencode "text=🔐 Chimera Secret Rotation: ${message}" \
            -d "parse_mode=HTML" > /dev/null 2>&1 || log "ERROR" "Failed to send Telegram notification"
    fi
}

# Generate cryptographically secure secret
generate_secret() {
    openssl rand -hex 32
}

# Rewrite the two webhook secret lines in the env file safely (no sed/echo
# interpolation of values that may contain $, &, |, \, or '=').
update_webhook_secrets_in_config() {
    local current_secret="$1"
    local new_secret="$2"
    python3 - "$CONFIG_FILE" "$current_secret" "$new_secret" <<'PY'
import sys

path, current, new = sys.argv[1:4]
with open(path, 'r') as f:
    lines = f.read().split('\n')

out = []
prev_written = False
cur_written = False
for line in lines:
    if line.startswith('CHIMERA_SECURITY__WEBHOOK_SECRET_PREVIOUS='):
        out.append('CHIMERA_SECURITY__WEBHOOK_SECRET_PREVIOUS=' + current)
        prev_written = True
    elif line.startswith('CHIMERA_SECURITY__WEBHOOK_SECRET='):
        out.append('CHIMERA_SECURITY__WEBHOOK_SECRET=' + new)
        cur_written = True
    else:
        out.append(line)

if not prev_written:
    out.append('CHIMERA_SECURITY__WEBHOOK_SECRET_PREVIOUS=' + current)
if not cur_written:
    out.append('CHIMERA_SECURITY__WEBHOOK_SECRET=' + new)

with open(path, 'w') as f:
    f.write('\n'.join(out))
PY
}

# Check if rotation is due
check_rotation_due() {
    local secret_type="$1"
    local rotation_days="$2"

    # Get last rotation date from config_audit
    local last_rotation
    if ! last_rotation=$(sqlite3 "$DB_PATH" "
        SELECT changed_at
        FROM config_audit
        WHERE key LIKE 'secret_rotation.${secret_type}%'
        ORDER BY changed_at DESC
        LIMIT 1
    " 2>&1); then
        log "ERROR" "Failed to query rotation history for $secret_type: $last_rotation"
        return 1
    fi

    if [[ -z "$last_rotation" ]]; then
        log "INFO" "No previous rotation found for $secret_type - rotation due"
        return 0
    fi

    # Calculate days since last rotation
    local days_since
    if ! days_since=$(sqlite3 "$DB_PATH" "
        SELECT CAST(julianday('now') - julianday('$last_rotation') AS INTEGER)
    " 2>&1); then
        log "ERROR" "Failed to compute days since rotation for $secret_type: $days_since"
        return 1
    fi

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

    # Read current secret (full value after the first '=')
    local current_secret
    current_secret=$(grep "^CHIMERA_SECURITY__WEBHOOK_SECRET=" "$CONFIG_FILE" 2>/dev/null | sed 's/^[^=]*=//' || true)

    if [[ -z "$current_secret" ]]; then
        log "ERROR" "Current webhook secret not found in config"
        return 1
    fi

    # Generate new secret
    local new_secret
    new_secret=$(generate_secret)

    log "INFO" "Generated new webhook secret (length: ${#new_secret})"

    # Backup current config (for rollback if the audit insert fails)
    local backup_file
    backup_file="${CONFIG_FILE}.backup.$(date +%Y%m%d_%H%M%S)"
    cp "$CONFIG_FILE" "$backup_file"

    # Update config: set previous secret and new secret
    if ! update_webhook_secrets_in_config "$current_secret" "$new_secret"; then
        log "ERROR" "Failed to update config file; restoring backup"
        cp "$backup_file" "$CONFIG_FILE"
        return 1
    fi

    # Log to database; if this fails, roll the config back so the new secret
    # is never left active without an audit record
    if ! sqlite3 "$DB_PATH" "
        INSERT INTO config_audit (key, old_value, new_value, changed_by, change_reason)
        VALUES (
            'secret_rotation.webhook_hmac',
            '[REDACTED]',
            '[REDACTED]',
            'SYSTEM_ROTATION',
            'Automated secret rotation (grace period: ${GRACE_PERIOD_HOURS}h)'
        );
    "; then
        log "ERROR" "Failed to record rotation in database; restoring previous config"
        cp "$backup_file" "$CONFIG_FILE"
        return 1
    fi

    log "INFO" "Webhook secret rotated successfully"
    log "INFO" "Grace period active: old and new secrets accepted for ${GRACE_PERIOD_HOURS} hours"

    # Update metrics via API
    if [[ -n "${API_URL:-}" ]] && [[ -n "${API_KEY:-}" ]]; then
        local current_timestamp
        current_timestamp=$(date +%s)
        local days_until_due=30  # Next rotation in 30 days

        log "INFO" "Updating secret rotation metrics via API..."
        local metrics_response
        metrics_response=$(curl -s --max-time 15 -X POST "${API_URL}/api/v1/metrics/secret-rotation" \
            -H "Content-Type: application/json" \
            -H "Authorization: Bearer ${API_KEY}" \
            -d "{
                \"last_success_timestamp\": ${current_timestamp},
                \"days_until_due\": ${days_until_due}
            }" 2>&1 || true)

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
    current_url=$(grep "^${env_var}=" "$CONFIG_FILE" 2>/dev/null | sed 's/^[^=]*=//' || true)

    if [[ -z "$current_url" ]]; then
        log "WARN" "RPC URL not found for $key_type - skipping rotation"
        return 0
    fi

    # Extract API key from URL (if present)
    if echo "$current_url" | grep -q "api-key="; then
        log "INFO" "API key found in URL - manual rotation required"
        log "INFO" "Update ${env_var} in config file with new API key"

        # Do NOT record a rotation date here: the key was not rotated, so the
        # daily reminder must keep firing until the operator actually rotates it.
        send_notification "RPC ${key_type} key rotation reminder - manual update required"
    else
        log "INFO" "No API key in URL - rotation not applicable"
    fi

    return 0
}

# Main rotation logic
main() {
    # Exclusive lock so concurrent cron runs cannot race on the same secret
    exec 9>"${CHIMERA_HOME}/.rotate-secrets.lock"
    flock -n 9 || { log "ERROR" "Another rotation is already in progress"; exit 1; }

    local force_rotation="${FORCE_ROTATION:-false}"
    local secret_type="${SECRET_TYPE:-all}"
    local rotated=0

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
            if ! rotate_webhook_secret; then
                log "ERROR" "Webhook secret rotation failed"
                exit 1
            fi
            rotated=1
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

    # Only reload the service when a rotation actually modified the config
    if [[ "$rotated" -eq 1 ]] && systemctl is-active --quiet chimera 2>/dev/null; then
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
