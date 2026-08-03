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
    local temp_db
    local test_restore_db=""

    temp_db=$(mktemp "/tmp/verify_XXXXXX.db") || return 1

    log "INFO" "Verifying backup: $backup_name"

    # Check file exists and is readable
    if [[ ! -r "$backup" ]]; then
        log "ERROR" "Backup file is not readable: $backup"
        rm -f "$temp_db"
        return 1
    fi

    # Get file size
    local size=$(du -h "$backup" | cut -f1)
    log "INFO" "  Size: $size"

    # Get file age (portable stat + date arithmetic)
    local age_days="unknown"
    if [[ "$(uname)" == "Darwin" ]]; then
        local mtime
        mtime=$(stat -f %m "$backup" 2>/dev/null || echo "")
        [ -n "$mtime" ] && age_days=$(( ($(date +%s) - mtime) / 86400 ))
    else
        local mtime_gnu
        mtime_gnu=$(stat -c %Y "$backup" 2>/dev/null || echo "")
        [ -n "$mtime_gnu" ] && age_days=$(( ($(date +%s) - mtime_gnu) / 86400 ))
    fi
    log "INFO" "  Age: ${age_days} days"

    # Decompress if needed (to a unique temp file)
    if [[ "$backup" == *.gz ]]; then
        # Test decompression
        if ! gunzip -t "$backup" 2>/dev/null; then
            log "ERROR" "  Backup file is corrupted (gzip test failed)"
            rm -f "$temp_db"
            return 1
        fi

        # Decompress to temp location
        gunzip -c "$backup" > "$temp_db" || {
            log "ERROR" "  Failed to decompress backup"
            rm -f "$temp_db"
            return 1
        }
    else
        cp "$backup" "$temp_db"
    fi

    # Verify SQLite integrity
    local integrity_check
    integrity_check=$(sqlite3 "$temp_db" "PRAGMA integrity_check;" 2>&1 || true)

    if echo "$integrity_check" | grep -q "ok"; then
        log "SUCCESS" "  ✓ Integrity check passed"
    else
        log "ERROR" "  ✗ Integrity check failed: $integrity_check"
        rm -f "$temp_db"
        return 1
    fi

    # Check schema version
    local schema_version
    schema_version=$(sqlite3 "$temp_db" "
    SELECT value FROM schema_version WHERE key = 'version';
    " 2>/dev/null || echo "unknown")
    log "INFO" "  Schema version: $schema_version"

    # Check table counts
    local position_count trade_count wallet_count
    position_count=$(sqlite3 "$temp_db" "SELECT COUNT(*) FROM positions;" 2>/dev/null || echo "0")
    trade_count=$(sqlite3 "$temp_db" "SELECT COUNT(*) FROM trades;" 2>/dev/null || echo "0")
    wallet_count=$(sqlite3 "$temp_db" "SELECT COUNT(*) FROM wallets;" 2>/dev/null || echo "0")

    log "INFO" "  Positions: $position_count"
    log "INFO" "  Trades: $trade_count"
    log "INFO" "  Wallets: $wallet_count"

    # Test restore procedure if requested
    if [[ "$TEST_RESTORE" == "true" ]]; then
        log "INFO" "  Testing restore procedure..."

        test_restore_db=$(mktemp "/tmp/verify_restore_XXXXXX.db") || {
            rm -f "$temp_db"
            return 1
        }

        # Backups are raw SQLite database files: restore means copying the
        # bytes into place, NOT piping them through the SQL engine.
        cp "$temp_db" "$test_restore_db"

        # Verify restored database
        if sqlite3 "$test_restore_db" "PRAGMA integrity_check;" | grep -q "ok"; then
            log "SUCCESS" "  ✓ Restore test passed"
        else
            log "ERROR" "  ✗ Restored database integrity check failed"
            rm -f "$temp_db" "$test_restore_db"
            return 1
        fi

        # Compare record counts (any mismatch is a failure)
        local restored_positions restored_trades restored_wallets
        restored_positions=$(sqlite3 "$test_restore_db" "SELECT COUNT(*) FROM positions;" 2>/dev/null || echo "0")
        restored_trades=$(sqlite3 "$test_restore_db" "SELECT COUNT(*) FROM trades;" 2>/dev/null || echo "0")
        restored_wallets=$(sqlite3 "$test_restore_db" "SELECT COUNT(*) FROM wallets;" 2>/dev/null || echo "0")

        local counts_match=1
        [[ "$restored_positions" != "$position_count" ]] && counts_match=0
        [[ "$restored_trades" != "$trade_count" ]] && counts_match=0
        [[ "$restored_wallets" != "$wallet_count" ]] && counts_match=0

        if [[ "$counts_match" -eq 1 ]]; then
            log "SUCCESS" "  ✓ Restored counts match (positions=$restored_positions, trades=$restored_trades, wallets=$restored_wallets)"
        else
            log "ERROR" "  ✗ Count mismatch: positions $restored_positions/$position_count, trades $restored_trades/$trade_count, wallets $restored_wallets/$wallet_count"
            rm -f "$temp_db" "$test_restore_db"
            return 1
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

    backup_count=0
    success_count=0
    fail_count=0

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
