#!/bin/bash
# Automated SQLite → PostgreSQL migration for Docker environment
set -e
set -o pipefail

echo "=== Chimera Database Migration: SQLite → PostgreSQL ==="

# Configuration
SQLITE_PATH="${SQLITE_PATH:-/app/data/chimera.db}"
POSTGRES_URL="${DATABASE_URL:-postgresql://chimera:changeme@postgres:5432/chimera}"
DRY_RUN="${DRY_RUN:-false}"

# Normalize DRY_RUN so TRUE/True/1/yes all mean dry run
DRY_RUN="$(echo "$DRY_RUN" | tr '[:upper:]' '[:lower:]')"
case "$DRY_RUN" in
    true|1|yes) DRY_RUN="true" ;;
    *) DRY_RUN="false" ;;
esac

echo "SQLite Path: $SQLITE_PATH"
# Mask the password when logging the connection URL
MASKED_URL=$(echo "$POSTGRES_URL" | sed -E 's#://[^:]+:([^@]+)@#://***:***@#')
echo "PostgreSQL URL: $MASKED_URL"
echo "Dry Run: $DRY_RUN"

# Ensure PostgreSQL is up and reachable before doing anything destructive
docker-compose up -d postgres
echo "Waiting for PostgreSQL to become ready..."
PG_ATTEMPT=0
until docker-compose exec -T postgres pg_isready -U chimera -d chimera > /dev/null 2>&1; do
    PG_ATTEMPT=$((PG_ATTEMPT + 1))
    if [ $PG_ATTEMPT -ge 60 ]; then
        echo "❌ PostgreSQL did not become ready in time" >&2
        exit 1
    fi
    sleep 2
done
echo "✓ PostgreSQL is ready"

# Stop the writer services so the snapshot is consistent (restore afterwards)
if [ "$DRY_RUN" != "true" ]; then
    echo "Stopping operator/scout for a consistent snapshot..."
    docker-compose stop operator scout 2>/dev/null || true
    # Always restart them when this script exits
    trap 'docker-compose start operator scout > /dev/null 2>&1 || true' EXIT
fi

# Step 1: Backup existing databases (only when actually migrating)
if [ "$DRY_RUN" != "true" ]; then
    echo "Step 1: Creating backups..."
    if [ -f "$SQLITE_PATH" ]; then
        # SQLite-aware backup: consistent even with concurrent writers
        sqlite3 "$SQLITE_PATH" ".backup '${SQLITE_PATH}.backup.$(date +%Y%m%d_%H%M%S)'"
        echo "✓ SQLite backup created"
    else
        echo "⚠ SQLite database not found at $SQLITE_PATH (fresh install)"
    fi

    # Try to backup PostgreSQL if it exists
    if docker-compose exec -T postgres pg_dump "$POSTGRES_URL" > "postgres_backup.$(date +%Y%m%d_%H%M%S).sql" 2>/dev/null; then
        echo "✓ PostgreSQL backup created"
    else
        echo "⚠ PostgreSQL backup skipped (database may not exist yet)"
    fi
else
    echo "Step 1: [DRY RUN] Would create SQLite and PostgreSQL backups"
fi

# Step 2: Check if scout container is running
if [ -z "$(docker-compose ps -q scout 2>/dev/null)" ]; then
    if [ "$DRY_RUN" = "true" ]; then
        echo "⚠ [DRY RUN] Scout container is not running; would start it"
    else
        echo "⚠ Scout container is not running. Starting it..."
        docker-compose up -d scout
        for _ in $(seq 1 30); do
            [ -n "$(docker-compose ps -q scout 2>/dev/null)" ] && break
            sleep 1
        done
    fi
fi

# Step 3: Run migration script
if [ "$DRY_RUN" = "true" ]; then
    echo "Step 2: [DRY RUN] Running migration in dry-run mode..."
    docker-compose exec -T scout python3 /app/tools/migrate_sqlite_to_postgres.py \
        --sqlite-path "$SQLITE_PATH" \
        --postgres-url "$POSTGRES_URL" \
        --dry-run
else
    echo "Step 2: Running migration..."
    docker-compose exec -T scout python3 /app/tools/migrate_sqlite_to_postgres.py \
        --sqlite-path "$SQLITE_PATH" \
        --postgres-url "$POSTGRES_URL"
fi

# Step 4: Verify migration
echo "Step 3: Verifying migration..."
docker-compose exec -T scout python3 /app/tools/migrate_sqlite_to_postgres.py \
    --sqlite-path "$SQLITE_PATH" \
    --postgres-url "$POSTGRES_URL" \
    --verify-only

# Step 5: Update application mode (if not dry run)
if [ "$DRY_RUN" != "true" ]; then
    echo "Step 4: MANUAL ACTION REQUIRED — dual-write is not applied automatically."
    echo "   The migration is complete; to enable dual-write mode you must:"
    echo ""
    echo "   1. Update your environment file:"
    echo "      CHIMERA_DB_MODE=dual-write"
    echo ""
    echo "   2. Restart services:"
    echo "      docker-compose up -d operator scout"
    echo ""
    echo "   3. Monitor for 24-48 hours before cutover to PostgreSQL:"
    echo "      CHIMERA_DB_MODE=postgres"
    echo "      docker-compose up -d operator scout"
else
    echo "DRY RUN COMPLETE. No changes made."
fi

echo ""
echo "Migration process completed successfully!"
echo "Next steps:"
echo "  1. Verify data integrity in PostgreSQL"
echo "  2. Test application with dual-write mode (manual step above)"
echo "  3. Monitor for 24-48 hours"
echo "  4. Cutover to PostgreSQL read-write"
