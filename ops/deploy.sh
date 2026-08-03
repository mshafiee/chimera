#!/bin/bash
# Chimera Deployment Script
#
# Handles safe deployment of new versions:
# - Database migration verification
# - Backup creation and verification
# - Service restart with health checks
# - Rollback capability
#
# Usage: ./deploy.sh [--version=VERSION] [--skip-backup] [--dry-run]

set -euo pipefail

# Configuration
CHIMERA_HOME="${CHIMERA_HOME:-/opt/chimera}"
DB_PATH="${CHIMERA_HOME}/data/chimera.db"
BACKUP_DIR="${CHIMERA_HOME}/backups"
LOG_FILE="/var/log/chimera/deploy.log"
SERVICE_NAME="chimera"
DRY_RUN="${DRY_RUN:-false}"
SKIP_BACKUP="${SKIP_BACKUP:-false}"

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
VERSION=""
while [[ $# -gt 0 ]]; do
    case $1 in
        --version=*)
            VERSION="${1#*=}"
            if [[ ! "$VERSION" =~ ^[0-9A-Za-z._-]+$ ]]; then
                echo "Error: invalid --version value: $VERSION" >&2
                exit 1
            fi
            shift
            ;;
        --skip-backup)
            SKIP_BACKUP=true
            shift
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [--version=VERSION] [--skip-backup] [--dry-run]"
            exit 1
            ;;
    esac
done

# Ensure log directory exists
mkdir -p "$(dirname "$LOG_FILE")"
mkdir -p "$BACKUP_DIR"

# Mutual exclusion: never run two deployments concurrently
exec 9>"${CHIMERA_HOME}/deploy.lock"
flock -n 9 || error_exit "Another deployment is already in progress"

log "INFO" "Starting deployment process"
[[ "$DRY_RUN" == "true" ]] && log "WARNING" "DRY RUN MODE - No changes will be made"

# Step 1: Pre-deployment checks
log "INFO" "Step 1: Pre-deployment checks"

# Check if service is running
if systemctl is-active --quiet "$SERVICE_NAME" 2>/dev/null; then
    log "INFO" "Service is currently running"
    SERVICE_WAS_RUNNING=true
else
    log "WARNING" "Service is not running"
    SERVICE_WAS_RUNNING=false
fi

# Check database exists
if [[ ! -f "$DB_PATH" ]]; then
    error_exit "Database not found at $DB_PATH"
fi

# Check database integrity
log "INFO" "Checking database integrity"
if ! sqlite3 "$DB_PATH" "PRAGMA integrity_check;" | grep -q "ok"; then
    error_exit "Database integrity check failed - DO NOT DEPLOY"
fi

# Check disk space (need at least 2x database size for backup)
DB_SIZE=$(du -b "$DB_PATH" | cut -f1)
AVAILABLE_SPACE=$(df -B1 "$(dirname "$DB_PATH")" | tail -1 | awk '{print $4}')
REQUIRED_SPACE=$((DB_SIZE * 2))

if [[ $AVAILABLE_SPACE -lt $REQUIRED_SPACE ]]; then
    error_exit "Insufficient disk space for backup (need $((REQUIRED_SPACE / 1024 / 1024))MB, have $((AVAILABLE_SPACE / 1024 / 1024))MB)"
fi

# Step 2: Create backup
if [[ "$SKIP_BACKUP" != "true" ]]; then
    log "INFO" "Step 2: Creating database backup"
    
    BACKUP_FILE="${BACKUP_DIR}/chimera_$(date +%Y%m%d_%H%M%S).db"
    
    if [[ "$DRY_RUN" != "true" ]]; then
        # Create backup
        sqlite3 "$DB_PATH" ".backup '$BACKUP_FILE'" || error_exit "Backup creation failed"
        
        # Verify backup integrity
        if ! sqlite3 "$BACKUP_FILE" "PRAGMA integrity_check;" | grep -q "ok"; then
            error_exit "Backup integrity check failed"
        fi
        
        # Compress backup
        gzip "$BACKUP_FILE" || error_exit "Backup compression failed"
        BACKUP_FILE="${BACKUP_FILE}.gz"
        
        log "SUCCESS" "Backup created: $BACKUP_FILE"
        
        # Keep only last 7 backups
        ls -t "${BACKUP_DIR}"/chimera_*.db.gz 2>/dev/null | tail -n +8 | xargs rm -f 2>/dev/null || true
    else
        log "INFO" "[DRY RUN] Would create backup: $BACKUP_FILE"
    fi
else
    log "WARNING" "Skipping backup (--skip-backup flag set)"
fi

# Step 3: Check for pending migrations
log "INFO" "Step 3: Checking for database migrations"

# Check if schema version table exists
SCHEMA_VERSION=$(sqlite3 "$DB_PATH" "
SELECT value FROM schema_version WHERE key = 'version';
" 2>/dev/null || echo "unknown")

log "INFO" "Current schema version: $SCHEMA_VERSION"

# Check for pending migrations via the SQLx migrations table (applied at
# startup by the operator); mtime heuristics are unreliable.
if sqlite3 "$DB_PATH" "SELECT 1 FROM _sqlx_migrations LIMIT 1;" > /dev/null 2>&1; then
    PENDING_MIGRATIONS=$(sqlite3 "$DB_PATH" "SELECT COUNT(*) FROM _sqlx_migrations WHERE success = 0;" 2>/dev/null || echo "0")
    if [[ $PENDING_MIGRATIONS -gt 0 ]]; then
        log "WARNING" "Found $PENDING_MIGRATIONS failed migration record(s)"
    else
        log "INFO" "No pending migrations"
    fi
else
    log "INFO" "No _sqlx_migrations table found (migrations applied on first startup)"
fi

# Step 4: Stop service gracefully
if [[ "$SERVICE_WAS_RUNNING" == "true" ]]; then
    log "INFO" "Step 4: Stopping service gracefully"
    
    if [[ "$DRY_RUN" != "true" ]]; then
        systemctl stop "$SERVICE_NAME" || error_exit "Failed to stop service"
        
        # Wait for clean shutdown (max 30 seconds)
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
fi

# Step 5: Update application (if version specified)
if [[ -n "$VERSION" ]]; then
    log "INFO" "Step 5: Updating application to version $VERSION"
    
    if [[ "$DRY_RUN" != "true" ]]; then
        # Application update is not implemented yet; refusing to silently
        # report success while the old binary keeps running.
        error_exit "Version updates are not implemented yet (remove --version or use --dry-run)"
    else
        log "INFO" "[DRY RUN] Would update application to version $VERSION"
    fi
fi

# Step 6: Start service
log "INFO" "Step 6: Starting service"

if [[ "$DRY_RUN" != "true" ]]; then
    systemctl start "$SERVICE_NAME" || error_exit "Failed to start service"
    
    # Wait for service to be ready (max 60 seconds)
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

# Step 7: Health check
log "INFO" "Step 7: Performing health checks"

if [[ "$DRY_RUN" != "true" ]]; then
    # Wait for service to be ready
    sleep 5
    
    # Check health endpoint
    MAX_RETRIES=12
    RETRY_COUNT=0
    HEALTH_OK=false
    
    while [[ $RETRY_COUNT -lt $MAX_RETRIES ]]; do
        if curl -sf http://localhost:8080/health > /dev/null 2>&1; then
            HEALTH_RESPONSE=$(curl -s http://localhost:8080/health)
            if echo "$HEALTH_RESPONSE" | jq -e '.status == "healthy"' > /dev/null 2>&1; then
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
        log "ERROR" "Service may not be functioning correctly"
        log "WARNING" "Consider rolling back: ./rollback.sh"
        exit 1
    fi
    
    # Verify database connectivity
    if ! sqlite3 "$DB_PATH" "SELECT 1;" > /dev/null 2>&1; then
        error_exit "Database connectivity check failed"
    fi
    
    # Check for critical errors in logs
    sleep 5
    if journalctl -u "$SERVICE_NAME" --since "1 minute ago" --no-pager | grep -i "error\|panic\|fatal" > /dev/null; then
        log "WARNING" "Errors detected in service logs - review immediately"
        journalctl -u "$SERVICE_NAME" --since "1 minute ago" --no-pager | grep -i "error\|panic\|fatal" | tail -10
    fi
    
    log "SUCCESS" "Health checks passed"
else
    log "INFO" "[DRY RUN] Would perform health checks"
fi

# Step 8: Post-deployment verification
log "INFO" "Step 8: Post-deployment verification"

if [[ "$DRY_RUN" != "true" ]]; then
    # Check service status
    systemctl status "$SERVICE_NAME" --no-pager -l | head -20
    
    # Check API endpoints
    log "INFO" "Testing API endpoints"
    curl -sf http://localhost:8080/api/v1/health > /dev/null && log "INFO" "✓ Health endpoint OK" || log "WARNING" "✗ Health endpoint failed"
    curl -sf http://localhost:8080/api/v1/config > /dev/null && log "INFO" "✓ Config endpoint OK" || log "WARNING" "✗ Config endpoint failed"
    
    # Log deployment to config audit (values escaped for SQL)
    OLD_VERSION=$(systemctl show -p Version "$SERVICE_NAME" 2>/dev/null || echo "unknown")
    NEW_VERSION="${VERSION:-$OLD_VERSION}"
    sqlite3 "$DB_PATH" "
    INSERT INTO config_audit (key, old_value, new_value, changed_by, change_reason)
    VALUES (
        'deployment',
        '${OLD_VERSION//'/''}',
        '${NEW_VERSION//'/''}',
        'DEPLOY_SCRIPT',
        'Deployment completed at $(date -u +%Y-%m-%dT%H:%M:%SZ)'
    );" 2>/dev/null || log "WARNING" "Failed to log deployment to config_audit"
fi

log "SUCCESS" "Deployment completed successfully"

# Summary
echo ""
log "INFO" "=== Deployment Summary ==="
log "INFO" "Backup: ${BACKUP_FILE:-N/A}"
log "INFO" "Schema Version: $SCHEMA_VERSION"
log "INFO" "Service Status: $(systemctl is-active "$SERVICE_NAME" 2>/dev/null || echo "unknown")"
log "INFO" "Health: $(curl -s http://localhost:8080/health 2>/dev/null | jq -r '.status // "unknown"' || echo "unknown")"

if [[ "$DRY_RUN" == "true" ]]; then
    log "INFO" "This was a DRY RUN - no changes were made"
fi

exit 0
