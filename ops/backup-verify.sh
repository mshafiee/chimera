#!/bin/bash
# Chimera Backup Verification Script
#
# Verifies backup integrity and tests restore procedure:
# - Checks backup file integrity
# - Tests restore to temporary location
# - Validates restored database
# - Reports backup age and size
#
# Usage: ./backup-verify.sh [--backup=FILE] [--test-restore]

set -euo pipefail

# Configuration
CHIMERA_HOME="${CHIMERA_HOME:-/opt/chimera}"
DB_PATH="${CHIMERA_HOME}/data/chimera.db"
BACKUP_DIR="${CHIMERA_HOME}/backups"
LOG_FILE="/var/log/chimera/backup_verify.log"
TEST_RESTORE="${TEST_RESTORE:-false}"

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
        --test-restore)
            TEST_RESTORE=true
            shift
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [--backup=FILE] [--test-restore]"
            exit 1
            ;;
    esac
done

# Ensure log directory exists
mkdir -p "$(dirname "$LOG_FILE")"

log "INFO" "Starting backup verification"

# Function to verify a single backup
verify_backup() {
    local backup="$1"
    local backup_name=$(basename "$backup")
    
    log "INFO" "Verifying backup: $backup_name"
    
    # Check file exists and is readable
    if [[ ! -r "$backup" ]]; then
        log "ERROR" "Backup file is not readable: $backup"
        return 1
    fi
    
    # Get file size
    local size=$(du -h "$backup" | cut -f1)
    log "INFO" "  Size: $size"
    
    # Get file age
    local age_days=$(find "$backup" -printf "%T@\n" 2>/dev/null | awk "{print int((systime() - \$1) / 86400)}" || echo "unknown")
    log "INFO" "  Age: ${age_days} days"
    
    # Decompress if needed
    local temp_db="/tmp/verify_${backup_name%.gz}.db"
    
    if [[ "$backup" == *.gz ]]; then
        # Test decompression
        if ! gunzip -t "$backup" 2>/dev/null; then
            log "ERROR" "  Backup file is corrupted (gzip test failed)"
            return 1
        fi
        
        # Decompress to temp location
        gunzip -c "$backup" > "$temp_db" || {
            log "ERROR" "  Failed to decompress backup"
            return 1
        }
    else
        cp "$backup" "$temp_db"
    fi
    
    # Verify SQLite integrity
    local integrity_check=$(sqlite3 "$temp_db" "PRAGMA integrity_check;" 2>&1)
    
    if echo "$integrity_check" | grep -q "ok"; then
        log "SUCCESS" "  ✓ Integrity check passed"
    else
        log "ERROR" "  ✗ Integrity check failed: $integrity_check"
        rm -f "$temp_db"
        return 1
    fi
    
    # Check schema version
    local schema_version=$(sqlite3 "$temp_db" "
    SELECT value FROM schema_version WHERE key = 'version';
    " 2>/dev/null || echo "unknown")
    log "INFO" "  Schema version: $schema_version"
    
    # Check table counts
    local position_count=$(sqlite3 "$temp_db" "SELECT COUNT(*) FROM positions;" 2>/dev/null || echo "0")
    local trade_count=$(sqlite3 "$temp_db" "SELECT COUNT(*) FROM trades;" 2>/dev/null || echo "0")
    local wallet_count=$(sqlite3 "$temp_db" "SELECT COUNT(*) FROM wallets;" 2>/dev/null || echo "0")
    
    log "INFO" "  Positions: $position_count"
    log "INFO" "  Trades: $trade_count"
    log "INFO" "  Wallets: $wallet_count"
    
    # Test restore procedure if requested
    if [[ "$TEST_RESTORE" == "true" ]]; then
        log "INFO" "  Testing restore procedure..."
        
        local test_restore_db="/tmp/test_restore_$(date +%s).db"
        
        # Simulate restore
        if [[ "$backup" == *.gz ]]; then
            gunzip -c "$backup" | sqlite3 "$test_restore_db" || {
                log "ERROR" "  ✗ Restore test failed"
                rm -f "$temp_db" "$test_restore_db"
                return 1
            }
        else
            sqlite3 "$test_restore_db" < "$backup" || {
                log "ERROR" "  ✗ Restore test failed"
                rm -f "$temp_db" "$test_restore_db"
                return 1
            }
        fi
        
        # Verify restored database
        if sqlite3 "$test_restore_db" "PRAGMA integrity_check;" | grep -q "ok"; then
            log "SUCCESS" "  ✓ Restore test passed"
        else
            log "ERROR" "  ✗ Restored database integrity check failed"
            rm -f "$temp_db" "$test_restore_db"
            return 1
        fi
        
        # Compare record counts
        local restored_positions=$(sqlite3 "$test_restore_db" "SELECT COUNT(*) FROM positions;" 2>/dev/null || echo "0")
        if [[ "$restored_positions" != "$position_count" ]]; then
            log "WARNING" "  Position count mismatch: original=$position_count, restored=$restored_positions"
        fi
        
        rm -f "$test_restore_db"
    fi
    
    # Clean up
    rm -f "$temp_db"
    
    return 0
}

# Main verification
if [[ -n "$BACKUP_FILE" ]]; then
    # Verify specific backup
    if [[ ! -f "$BACKUP_FILE" ]]; then
        error_exit "Backup file not found: $BACKUP_FILE"
    fi
    
    if verify_backup "$BACKUP_FILE"; then
        log "SUCCESS" "Backup verification completed successfully"
        exit 0
    else
        error_exit "Backup verification failed"
    fi
else
    # Verify all backups
    log "INFO" "Verifying all backups in $BACKUP_DIR"
    
    if [[ ! -d "$BACKUP_DIR" ]]; then
        error_exit "Backup directory not found: $BACKUP_DIR"
    fi
    
    local backup_count=0
    local success_count=0
    local fail_count=0
    
    # Find all backup files
    while IFS= read -r backup; do
        backup_count=$((backup_count + 1))
        echo ""
        if verify_backup "$backup"; then
            success_count=$((success_count + 1))
        else
            fail_count=$((fail_count + 1))
        fi
    done < <(find "$BACKUP_DIR" -name "chimera_*.db*" -type f | sort -r)
    
    # Summary
    echo ""
    log "INFO" "=== Verification Summary ==="
    log "INFO" "Total backups: $backup_count"
    log "INFO" "Successful: $success_count"
    log "INFO" "Failed: $fail_count"
    
    if [[ $fail_count -gt 0 ]]; then
        log "WARNING" "Some backups failed verification - review and fix"
        exit 1
    elif [[ $backup_count -eq 0 ]]; then
        log "WARNING" "No backups found in $BACKUP_DIR"
        exit 1
    else
        log "SUCCESS" "All backups verified successfully"
        exit 0
    fi
fi
