#!/bin/bash
# Chimera Database Backup Script
# Runs daily via cron at 3 AM
# 
# Features:
# - SQLite VACUUM INTO for consistent backup
# - 7-day retention with automatic cleanup
# - SHA256 checksum verification
# - Records backup in database backups table
# - Sends notification on failure

set -euo pipefail

# Configuration
CHIMERA_HOME="${CHIMERA_HOME:-/opt/chimera}"
DB_PATH="${CHIMERA_HOME}/data/chimera.db"
BACKUP_DIR="${CHIMERA_HOME}/backups"
RETENTION_DAYS=7
LOG_FILE="/var/log/chimera/backup.log"
NOTIFY_ON_FAILURE="${NOTIFY_ON_FAILURE:-true}"

# Logging function
log() {
    local level="$1"
    shift
    echo "[$(date -u '+%Y-%m-%dT%H:%M:%SZ')] [$level] $*" | tee -a "$LOG_FILE"
}

# Send notification (uses Telegram if configured)
notify_failure() {
    local message="$1"
    log "ERROR" "$message"
    
    if [[ "$NOTIFY_ON_FAILURE" == "true" ]]; then
        if [[ -n "${TELEGRAM_BOT_TOKEN:-}" && -n "${TELEGRAM_CHAT_ID:-}" ]]; then
            curl -s -X POST "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/sendMessage" \
                -d "chat_id=${TELEGRAM_CHAT_ID}" \
                -d "text=🚨 Chimera Backup Failed: ${message}" \
                -d "parse_mode=HTML" > /dev/null 2>&1 || true
        fi
    fi
}

# Ensure directories exist
mkdir -p "$BACKUP_DIR"
mkdir -p "$(dirname "$LOG_FILE")"

log "INFO" "Starting database backup"

# Check if database exists
if [[ ! -f "$DB_PATH" ]]; then
    notify_failure "Database not found at $DB_PATH"
    exit 1
fi

# Generate backup filename with timestamp
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
BACKUP_FILE="${BACKUP_DIR}/chimera_${TIMESTAMP}.db"

# Perform VACUUM INTO (creates consistent backup without locking main DB)
log "INFO" "Running VACUUM INTO ${BACKUP_FILE}"
if ! sqlite3 "$DB_PATH" "VACUUM INTO '${BACKUP_FILE}';" 2>> "$LOG_FILE"; then
    notify_failure "VACUUM INTO failed"
    exit 1
fi

# Verify backup file exists and has content
if [[ ! -f "$BACKUP_FILE" || ! -s "$BACKUP_FILE" ]]; then
    notify_failure "Backup file is empty or missing"
    exit 1
fi

# Generate checksum
CHECKSUM=$(sha256sum "$BACKUP_FILE" | cut -d' ' -f1)
BACKUP_SIZE=$(stat -f%z "$BACKUP_FILE" 2>/dev/null || stat -c%s "$BACKUP_FILE" 2>/dev/null)

log "INFO" "Backup created: ${BACKUP_FILE} (${BACKUP_SIZE} bytes, SHA256: ${CHECKSUM:0:16}...)"

# Verify backup integrity
log "INFO" "Verifying backup integrity"
INTEGRITY_CHECK=$(sqlite3 "$BACKUP_FILE" "PRAGMA integrity_check;" 2>&1)
if [[ "$INTEGRITY_CHECK" != "ok" ]]; then
    notify_failure "Backup integrity check failed: $INTEGRITY_CHECK"
    rm -f "$BACKUP_FILE"
    exit 1
fi

# Record backup in database
log "INFO" "Recording backup in database"
sqlite3 "$DB_PATH" <<EOF
INSERT INTO backups (path, size_bytes, checksum, backup_type, created_at)
VALUES ('${BACKUP_FILE}', ${BACKUP_SIZE}, '${CHECKSUM}', 'SCHEDULED', datetime('now'));
EOF

# Cleanup old backups (older than RETENTION_DAYS)
log "INFO" "Cleaning up backups older than ${RETENTION_DAYS} days"
DELETED_COUNT=0
while IFS= read -r old_backup; do
    if [[ -n "$old_backup" && -f "$old_backup" ]]; then
        rm -f "$old_backup"
        ((DELETED_COUNT++))
        log "INFO" "Deleted old backup: $old_backup"
    fi
done < <(find "$BACKUP_DIR" -name "chimera_*.db" -type f -mtime +${RETENTION_DAYS} 2>/dev/null)

# Clean up old entries from backups table
sqlite3 "$DB_PATH" "DELETE FROM backups WHERE created_at < datetime('now', '-${RETENTION_DAYS} days');"

log "INFO" "Backup completed successfully. Deleted ${DELETED_COUNT} old backups."

# Optional: Compress backup if larger than 100MB
if [[ $BACKUP_SIZE -gt 104857600 ]]; then
    log "INFO" "Compressing large backup"
    gzip -9 "$BACKUP_FILE"
    BACKUP_FILE="${BACKUP_FILE}.gz"
    log "INFO" "Compressed to ${BACKUP_FILE}"
fi

exit 0
