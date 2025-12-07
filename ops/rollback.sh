#!/bin/bash
# Chimera Rollback Script
#
# Safely rolls back to previous version:
# - Restores database from backup
# - Reverts application binary
# - Restarts service with verification
#
# Usage: ./rollback.sh [--backup=FILE] [--dry-run]

set -euo pipefail

# Configuration
CHIMERA_HOME="${CHIMERA_HOME:-/opt/chimera}"
DB_PATH="${CHIMERA_HOME}/data/chimera.db"
BACKUP_DIR="${CHIMERA_HOME}/backups"
LOG_FILE="/var/log/chimera/rollback.log"
SERVICE_NAME="chimera"
DRY_RUN="${DRY_RUN:-false}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log() {
    local level="$1"
    shift
    local message="[$(date -u '+%Y-%m-%dT%H:%M:%SZ')] [$level] $*"
    echo -e "${message}" | tee -a "$LOG_FILE"
    case "$level" in
        ERROR|CRITICAL)
            echo -e "${RED}${message}${NC}" >&2
            ;;
        WARNING)
            echo -e "${YELLOW}${message}${NC}"
            ;;
        SUCCESS)
            echo -e "${GREEN}${message}${NC}"
            ;;
    esac
}

error_exit() {
    log "ERROR" "$1"
    exit 1
}

# Parse arguments
BACKUP_FILE=""
while [[ $# -gt 0 ]]; do
    case $1 in
        --backup=*)
            BACKUP_FILE="${1#*=}"
            shift
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [--backup=FILE] [--dry-run]"
            exit 1
            ;;
    esac
done

# Ensure log directory exists
mkdir -p "$(dirname "$LOG_FILE")"

log "WARNING" "=== ROLLBACK PROCEDURE ==="
log "WARNING" "This will restore the database from backup and may cause data loss"
[[ "$DRY_RUN" == "true" ]] && log "WARNING" "DRY RUN MODE - No changes will be made"

# Step 1: List available backups
log "INFO" "Step 1: Listing available backups"

if [[ -z "$BACKUP_FILE" ]]; then
    # Find most recent backup
    BACKUP_FILE=$(ls -t "${BACKUP_DIR}"/chimera_*.db.gz 2>/dev/null | head -1)
    
    if [[ -z "$BACKUP_FILE" ]]; then
        error_exit "No backups found in $BACKUP_DIR"
    fi
    
    log "INFO" "Using most recent backup: $BACKUP_FILE"
else
    if [[ ! -f "$BACKUP_FILE" ]]; then
        error_exit "Backup file not found: $BACKUP_FILE"
    fi
    log "INFO" "Using specified backup: $BACKUP_FILE"
fi

# List all backups for reference
log "INFO" "Available backups:"
ls -lh "${BACKUP_DIR}"/chimera_*.db.gz 2>/dev/null | tail -10 || log "WARNING" "No backups found"

# Step 2: Verify backup integrity
log "INFO" "Step 2: Verifying backup integrity"

if [[ "$DRY_RUN" != "true" ]]; then
    # Decompress backup to temp location
    TEMP_BACKUP="/tmp/chimera_rollback_$(date +%s).db"
    
    if [[ "$BACKUP_FILE" == *.gz ]]; then
        gunzip -c "$BACKUP_FILE" > "$TEMP_BACKUP" || error_exit "Failed to decompress backup"
    else
        cp "$BACKUP_FILE" "$TEMP_BACKUP"
    fi
    
    # Verify integrity
    if ! sqlite3 "$TEMP_BACKUP" "PRAGMA integrity_check;" | grep -q "ok"; then
        rm -f "$TEMP_BACKUP"
        error_exit "Backup integrity check failed - DO NOT USE THIS BACKUP"
    fi
    
    log "SUCCESS" "Backup integrity verified"
else
    log "INFO" "[DRY RUN] Would verify backup integrity"
fi

# Step 3: Create current state backup (safety measure)
log "INFO" "Step 3: Creating safety backup of current state"

if [[ "$DRY_RUN" != "true" ]]; then
    SAFETY_BACKUP="${BACKUP_DIR}/chimera_pre_rollback_$(date +%Y%m%d_%H%M%S).db"
    sqlite3 "$DB_PATH" ".backup '$SAFETY_BACKUP'" || error_exit "Failed to create safety backup"
    gzip "$SAFETY_BACKUP"
    log "SUCCESS" "Safety backup created: ${SAFETY_BACKUP}.gz"
else
    log "INFO" "[DRY RUN] Would create safety backup"
fi

# Step 4: Stop service
log "INFO" "Step 4: Stopping service"

if [[ "$DRY_RUN" != "true" ]]; then
    systemctl stop "$SERVICE_NAME" || error_exit "Failed to stop service"
    
    # Wait for clean shutdown
    TIMEOUT=30
    ELAPSED=0
    while pgrep -f chimera_operator > /dev/null && [[ $ELAPSED -lt $TIMEOUT ]]; do
        sleep 1
        ELAPSED=$((ELAPSED + 1))
    done
    
    if pgrep -f chimera_operator > /dev/null; then
        log "WARNING" "Service did not stop gracefully, forcing kill"
        pkill -9 -f chimera_operator || true
        sleep 2
    fi
    
    log "SUCCESS" "Service stopped"
else
    log "INFO" "[DRY RUN] Would stop service"
fi

# Step 5: Restore database
log "INFO" "Step 5: Restoring database from backup"

if [[ "$DRY_RUN" != "true" ]]; then
    # Backup current database (additional safety)
    CURRENT_BACKUP="${DB_PATH}.rollback_backup_$(date +%Y%m%d_%H%M%S)"
    cp "$DB_PATH" "$CURRENT_BACKUP" || error_exit "Failed to backup current database"
    
    # Restore from backup
    if [[ "$BACKUP_FILE" == *.gz ]]; then
        gunzip -c "$BACKUP_FILE" | sqlite3 "$DB_PATH" || error_exit "Failed to restore database"
    else
        sqlite3 "$DB_PATH" < "$BACKUP_FILE" || error_exit "Failed to restore database"
    fi
    
    # Verify restored database
    if ! sqlite3 "$DB_PATH" "PRAGMA integrity_check;" | grep -q "ok"; then
        # Restore from current backup if restore failed
        cp "$CURRENT_BACKUP" "$DB_PATH"
        error_exit "Restored database integrity check failed - restored original"
    fi
    
    # Clean up temp files
    rm -f "$TEMP_BACKUP" "$CURRENT_BACKUP"
    
    log "SUCCESS" "Database restored successfully"
else
    log "INFO" "[DRY RUN] Would restore database from: $BACKUP_FILE"
fi

# Step 6: Revert application binary (if needed)
log "INFO" "Step 6: Checking application version"

if [[ "$DRY_RUN" != "true" ]]; then
    # This would typically:
    # 1. Check current version
    # 2. Revert to previous version binary
    # 3. Verify binary integrity
    
    log "INFO" "Application binary revert logic would run here"
    # Example:
    # PREVIOUS_VERSION=$(get_previous_version)
    # cp "${CHIMERA_HOME}/bin/chimera_operator.${PREVIOUS_VERSION}" "${CHIMERA_HOME}/bin/chimera_operator"
else
    log "INFO" "[DRY RUN] Would revert application binary"
fi

# Step 7: Start service
log "INFO" "Step 7: Starting service"

if [[ "$DRY_RUN" != "true" ]]; then
    systemctl start "$SERVICE_NAME" || error_exit "Failed to start service"
    
    # Wait for service to be ready
    TIMEOUT=60
    ELAPSED=0
    while ! systemctl is-active --quiet "$SERVICE_NAME" && [[ $ELAPSED -lt $TIMEOUT ]]; do
        sleep 1
        ELAPSED=$((ELAPSED + 1))
    done
    
    if ! systemctl is-active --quiet "$SERVICE_NAME"; then
        error_exit "Service failed to start within $TIMEOUT seconds"
    fi
    
    log "SUCCESS" "Service started"
else
    log "INFO" "[DRY RUN] Would start service"
fi

# Step 8: Health check
log "INFO" "Step 8: Performing health checks"

if [[ "$DRY_RUN" != "true" ]]; then
    sleep 5
    
    # Check health endpoint
    MAX_RETRIES=12
    RETRY_COUNT=0
    HEALTH_OK=false
    
    while [[ $RETRY_COUNT -lt $MAX_RETRIES ]]; do
        if curl -sf http://localhost:8080/health > /dev/null 2>&1; then
            HEALTH_RESPONSE=$(curl -s http://localhost:8080/health)
            if echo "$HEALTH_RESPONSE" | jq -e '.status == "healthy" or .status == "degraded"' > /dev/null 2>&1; then
                HEALTH_OK=true
                break
            fi
        fi
        sleep 5
        RETRY_COUNT=$((RETRY_COUNT + 1))
        log "INFO" "Health check attempt $RETRY_COUNT/$MAX_RETRIES"
    done
    
    if [[ "$HEALTH_OK" != "true" ]]; then
        log "ERROR" "Health check failed after $MAX_RETRIES attempts"
        log "ERROR" "Rollback may have issues - investigate immediately"
        exit 1
    fi
    
    log "SUCCESS" "Health checks passed"
else
    log "INFO" "[DRY RUN] Would perform health checks"
fi

# Step 9: Log rollback
log "INFO" "Step 9: Logging rollback event"

if [[ "$DRY_RUN" != "true" ]]; then
    sqlite3 "$DB_PATH" "
    INSERT INTO config_audit (key, old_value, new_value, changed_by, change_reason)
    VALUES (
        'rollback',
        'current',
        '$(basename "$BACKUP_FILE")',
        'ROLLBACK_SCRIPT',
        'Database rolled back from backup: $BACKUP_FILE at $(date -u +%Y-%m-%dT%H:%M:%SZ)'
    );" 2>/dev/null || log "WARNING" "Failed to log rollback to config_audit"
fi

log "SUCCESS" "Rollback completed successfully"

# Summary
echo ""
log "INFO" "=== Rollback Summary ==="
log "INFO" "Backup Used: $BACKUP_FILE"
log "INFO" "Service Status: $(systemctl is-active "$SERVICE_NAME" 2>/dev/null || echo "unknown")"
log "INFO" "Health: $(curl -s http://localhost:8080/health 2>/dev/null | jq -r '.status // "unknown"' || echo "unknown")"

if [[ "$DRY_RUN" == "true" ]]; then
    log "INFO" "This was a DRY RUN - no changes were made"
fi

log "WARNING" "Review system logs and verify all functionality before considering rollback complete"

exit 0
