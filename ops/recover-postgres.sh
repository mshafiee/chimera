#!/bin/bash
# PostgreSQL recovery script
set -e

# Configuration
BACKUP_FILE="${1:-}"
BACKUP_DIR="${BACKUP_DIR:-/backups/postgres}"
POSTGRES_HOST="${POSTGRES_HOST:-postgres}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-chimera}"
POSTGRES_DB="${POSTGRES_DB:-chimera}"

echo "=== PostgreSQL Recovery ==="

# Check if backup file is provided
if [ -z "$BACKUP_FILE" ]; then
    # Find the latest backup if not specified
    BACKUP_FILE=$(ls -t "$BACKUP_DIR"/chimera_*.sql.gz 2>/dev/null | head -1)
    if [ -z "$BACKUP_FILE" ]; then
        echo "❌ No backup file found in $BACKUP_DIR"
        echo "Usage: $0 [backup_file.sql.gz]"
        echo "Example: $0 /backups/postgres/chimera_20230101_120000.sql.gz"
        exit 1
    fi
    echo "Using latest backup: $BACKUP_FILE"
else
    if [ ! -f "$BACKUP_FILE" ]; then
        echo "❌ Backup file not found: $BACKUP_FILE"
        exit 1
    fi
fi

echo "Backup file: $BACKUP_FILE"
echo "Target database: $POSTGRES_DB"

# Confirm recovery
echo ""
echo "⚠ WARNING: This will completely replace the existing database!"
echo "All current data will be lost."
echo ""
read -p "Are you sure you want to continue? (yes/no): " CONFIRM

if [ "$CONFIRM" != "yes" ]; then
    echo "Recovery cancelled"
    exit 0
fi

# Check if PostgreSQL container is running
if ! docker-compose ps | grep -q "chimera-postgres.*Up"; then
    echo "❌ PostgreSQL container is not running"
    echo "Starting PostgreSQL container..."
    docker-compose up -d postgres
    sleep 10
fi

# Create a backup of current database before recovery
echo ""
echo "Creating safety backup of current database..."
SAFETY_BACKUP="$BACKUP_DIR/chimera_before_recovery_$(date +%Y%m%d_%H%M%S).sql.gz"
if docker-compose exec postgres pg_dump -U "$POSTGRES_USER" "$POSTGRES_DB" | gzip > "$SAFETY_BACKUP"; then
    echo "✓ Safety backup created: $SAFETY_BACKUP"
else
    echo "⚠ Could not create safety backup, continuing anyway..."
fi

# Stop application services to prevent conflicts
echo ""
echo "Stopping application services..."
docker-compose stop operator scout

# Drop existing database
echo ""
echo "Step 1: Dropping existing database..."
if docker-compose exec postgres psql -U "$POSTGRES_USER" -c "DROP DATABASE IF EXISTS $POSTGRES_DB;" > /dev/null 2>&1; then
    echo "✓ Database dropped"
else
    echo "⚠ Warning: Could not drop database (may not exist)"
fi

# Create fresh database
echo "Step 2: Creating fresh database..."
if docker-compose exec postgres psql -U "$POSTGRES_USER" -c "CREATE DATABASE $POSTGRES_DB;" > /dev/null 2>&1; then
    echo "✓ Database created"
else
    echo "❌ Failed to create database"
    exit 1
fi

# Restore backup
echo "Step 3: Restoring backup..."
if gunzip -c "$BACKUP_FILE" | docker-compose exec -T postgres psql -U "$POSTGRES_USER" "$POSTGRES_DB" > /dev/null 2>&1; then
    echo "✓ Backup restored successfully"
else
    echo "❌ Failed to restore backup"
    echo "Attempting to restore from safety backup..."
    if [ -f "$SAFETY_BACKUP" ]; then
        gunzip -c "$SAFETY_BACKUP" | docker-compose exec -T postgres psql -U "$POSTGRES_USER" "$POSTGRES_DB" > /dev/null 2>&1
    fi
    exit 1
fi

# Verify restoration
echo "Step 4: Verifying restoration..."
TABLE_COUNT=$(docker-compose exec postgres psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -t -c "SELECT count(*) FROM information_schema.tables WHERE table_schema = 'public';" 2>/dev/null | tr -d ' ')
if [ -n "$TABLE_COUNT" ] && [ "$TABLE_COUNT" -gt 0 ]; then
    echo "✓ Database verified ($TABLE_COUNT tables found)"
else
    echo "⚠ Warning: Could not verify database restoration"
fi

# Restart application services
echo ""
echo "Restarting application services..."
docker-compose start operator scout

echo ""
echo "=== Recovery completed successfully ==="
echo "Safety backup saved at: $SAFETY_BACKUP"
echo ""
echo "Next steps:"
echo "  1. Verify application functionality"
echo "  2. Check data integrity"
echo "  3. Monitor application logs"
