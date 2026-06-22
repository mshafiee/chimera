#!/bin/bash
# PostgreSQL automated backup script
set -e

# Configuration
BACKUP_DIR="${BACKUP_DIR:-/backups/postgres}"
POSTGRES_HOST="${POSTGRES_HOST:-postgres}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-chimera}"
POSTGRES_DB="${POSTGRES_DB:-chimera}"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
BACKUP_FILE="$BACKUP_DIR/chimera_${TIMESTAMP}.sql.gz"

echo "=== PostgreSQL Backup ==="
echo "Backup directory: $BACKUP_DIR"
echo "Database: $POSTGRES_DB"
echo "Timestamp: $TIMESTAMP"

# Create backup directory if it doesn't exist
mkdir -p "$BACKUP_DIR"

# Check if PostgreSQL container is running
if ! docker-compose ps | grep -q "chimera-postgres.*Up"; then
    echo "❌ PostgreSQL container is not running"
    exit 1
fi

# Create backup
echo "Creating PostgreSQL backup..."
if docker-compose exec postgres pg_dump -U "$POSTGRES_USER" "$POSTGRES_DB" | gzip > "$BACKUP_FILE"; then
    echo "✓ Backup created successfully"
else
    echo "❌ Backup failed"
    exit 1
fi

# Verify backup
if [ -f "$BACKUP_FILE" ] && [ -s "$BACKUP_FILE" ]; then
    BACKUP_SIZE=$(du -h "$BACKUP_FILE" | cut -f1)
    BACKUP_SIZE_BYTES=$(stat -f%z "$BACKUP_FILE" 2>/dev/null || stat -c%s "$BACKUP_FILE" 2>/dev/null)

    echo "✓ Backup file: $BACKUP_FILE"
    echo "✓ Backup size: $BACKUP_SIZE"
    echo "✓ Backup size (bytes): $BACKUP_SIZE_BYTES"

    # Clean old backups (keep last 7 days)
    echo "Cleaning old backups (keeping last 7 days)..."
    DELETED=$(find "$BACKUP_DIR" -name "chimera_*.sql.gz" -mtime +7 -delete -print | wc -l)
    echo "✓ Deleted $DELETED old backup(s)"

    # List current backups
    echo ""
    echo "Current backups:"
    ls -lh "$BACKUP_DIR"/chimera_*.sql.gz 2>/dev/null || echo "No backups found in $BACKUP_DIR"

    echo ""
    echo "=== Backup completed successfully ==="
else
    echo "❌ Backup verification failed"
    rm -f "$BACKUP_FILE"
    exit 1
fi

# Optional: Upload to remote storage (uncomment if needed)
# if [ -n "$BACKUP_REMOTE_URL" ]; then
#     echo "Uploading to remote storage..."
#     curl -X PUT "$BACKUP_REMOTE_URL/chimera_${TIMESTAMP}.sql.gz" --upload-file "$BACKUP_FILE"
# fi
