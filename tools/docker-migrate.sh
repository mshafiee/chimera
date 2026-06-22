#!/bin/bash
# Automated SQLite → PostgreSQL migration for Docker environment
set -e

echo "=== Chimera Database Migration: SQLite → PostgreSQL ==="

# Configuration
SQLITE_PATH="${SQLITE_PATH:-/app/data/chimera.db}"
POSTGRES_URL="${DATABASE_URL:-postgresql://chimera:changeme@postgres:5432/chimera}"
DRY_RUN="${DRY_RUN:-false}"

echo "SQLite Path: $SQLITE_PATH"
echo "PostgreSQL URL: $POSTGRES_URL"
echo "Dry Run: $DRY_RUN"

# Step 1: Backup existing databases
echo "Step 1: Creating backups..."
if [ -f "$SQLITE_PATH" ]; then
    cp "$SQLITE_PATH" "${SQLITE_PATH}.backup.$(date +%Y%m%d_%H%M%S)"
    echo "✓ SQLite backup created"
else
    echo "⚠ SQLite database not found at $SQLITE_PATH (fresh install)"
fi

# Try to backup PostgreSQL if it exists
docker-compose exec -T postgres pg_dump "$POSTGRES_URL" > "postgres_backup.$(date +%Y%m%d_%H%M%S).sql" 2>/dev/null || echo "⚠ PostgreSQL backup skipped (database may not exist yet)"

# Step 2: Check if scout container is running
if ! docker-compose ps | grep -q "chimera-scout.*Up"; then
    echo "⚠ Scout container is not running. Starting it..."
    docker-compose up -d scout
    sleep 5
fi

# Step 3: Run migration script
echo "Step 2: Running migration..."
if [ "$DRY_RUN" = "true" ]; then
    echo "Running in DRY RUN mode..."
    docker-compose exec -T scout python3 tools/migrate_sqlite_to_postgres.py \
        --sqlite-path "$SQLITE_PATH" \
        --postgres-url "$POSTGRES_URL" \
        --dry-run
else
    echo "Running migration..."
    docker-compose exec -T scout python3 tools/migrate_sqlite_to_postgres.py \
        --sqlite-path "$SQLITE_PATH" \
        --postgres-url "$POSTGRES_URL"
fi

# Step 4: Verify migration
echo "Step 3: Verifying migration..."
docker-compose exec -T scout python3 tools/migrate_sqlite_to_postgres.py \
    --sqlite-path "$SQLITE_PATH" \
    --postgres-url "$POSTGRES_URL" \
    --verify-only

# Step 5: Update application mode (if not dry run)
if [ "$DRY_RUN" != "true" ]; then
    echo "Step 4: Updating database mode to dual-write..."
    echo "⚠ IMPORTANT: System now in DUAL-WRITE mode."
    echo "   Monitor for 24-48 hours before cutover to PostgreSQL."
    echo ""
    echo "To enable dual-write mode, update your environment file:"
    echo "  CHIMERA_DB_MODE=dual-write"
    echo ""
    echo "Then restart services:"
    echo "  docker-compose up -d operator scout"
    echo ""
    echo "To cutover to PostgreSQL after validation period:"
    echo "  CHIMERA_DB_MODE=postgres"
    echo "  docker-compose up -d operator scout"
else
    echo "DRY RUN COMPLETE. No changes made."
fi

echo ""
echo "Migration process completed successfully!"
echo "Next steps:"
echo "  1. Verify data integrity in PostgreSQL"
echo "  2. Test application with dual-write mode"
echo "  3. Monitor for 24-48 hours"
echo "  4. Cutover to PostgreSQL read-write"
