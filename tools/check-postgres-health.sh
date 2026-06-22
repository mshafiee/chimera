#!/bin/bash
# PostgreSQL health check for monitoring
set -e

POSTGRES_HOST="${POSTGRES_HOST:-postgres}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-chimera}"
POSTGRES_DB="${POSTGRES_DB:-chimera}"

echo "=== PostgreSQL Health Check ==="
echo "Host: $POSTGRES_HOST:$POSTGRES_PORT"
echo "Database: $POSTGRES_DB"
echo ""

# Check if PostgreSQL container is running
if ! docker-compose ps | grep -q "chimera-postgres.*Up"; then
    echo "❌ PostgreSQL container is not running"
    exit 1
fi

# Check connection
echo "Checking connection..."
if docker-compose exec postgres pg_isready -h "$POSTGRES_HOST" -p "$POSTGRES_PORT" -U "$POSTGRES_USER" -d "$POSTGRES_DB" > /dev/null 2>&1; then
    echo "✓ PostgreSQL is ready"
else
    echo "❌ PostgreSQL is not ready"
    exit 1
fi

# Check database responsiveness
echo "Checking database responsiveness..."
if docker-compose exec postgres psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -c "SELECT 1;" > /dev/null 2>&1; then
    echo "✓ Database is responsive"
else
    echo "❌ Database is not responsive"
    exit 1
fi

# Check connection count
echo "Checking connection statistics..."
CONNECTION_COUNT=$(docker-compose exec postgres psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -t -c "SELECT count(*) FROM pg_stat_activity;" 2>/dev/null | tr -d ' ')
if [ -n "$CONNECTION_COUNT" ]; then
    echo "✓ Active connections: $CONNECTION_COUNT"
else
    echo "⚠ Could not retrieve connection count"
fi

# Check max connections
MAX_CONNECTIONS=$(docker-compose exec postgres psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -t -c "SELECT setting::int FROM pg_settings WHERE name = 'max_connections';" 2>/dev/null | tr -d ' ')
if [ -n "$MAX_CONNECTIONS" ]; then
    CONNECTION_PERCENT=$((CONNECTION_COUNT * 100 / MAX_CONNECTIONS))
    echo "  Max connections: $MAX_CONNECTIONS (${CONNECTION_PERCENT}% utilized)"
fi

# Check database size
echo "Checking database size..."
DB_SIZE=$(docker-compose exec postgres psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -t -c "SELECT pg_size_pretty(pg_database_size('$POSTGRES_DB'));" 2>/dev/null | tr -d ' ')
if [ -n "$DB_SIZE" ]; then
    echo "✓ Database size: $DB_SIZE"
else
    echo "⚠ Could not retrieve database size"
fi

# Check table counts
echo "Checking database tables..."
TABLE_COUNT=$(docker-compose exec postgres psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -t -c "SELECT count(*) FROM information_schema.tables WHERE table_schema = 'public';" 2>/dev/null | tr -d ' ')
if [ -n "$TABLE_COUNT" ]; then
    echo "✓ Total tables: $TABLE_COUNT"
else
    echo "⚠ Could not retrieve table count"
fi

# Check for long-running queries
echo "Checking for long-running queries..."
LONG_RUNNING=$(docker-compose exec postgres psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -t -c "SELECT count(*) FROM pg_stat_activity WHERE state != 'idle' AND query_start < now() - interval '5 minutes';" 2>/dev/null | tr -d ' ')
if [ -n "$LONG_RUNNING" ]; then
    if [ "$LONG_RUNNING" -gt 0 ]; then
        echo "⚠ Long-running queries: $LONG_RUNNING"
    else
        echo "✓ No long-running queries"
    fi
else
    echo "⚠ Could not check for long-running queries"
fi

# Check replication status (if applicable)
echo "Checking replication status..."
REPLICATION_ENABLED=$(docker-compose exec postgres psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -t -c "SELECT setting FROM pg_settings WHERE name = 'max_wal_senders';" 2>/dev/null | tr -d ' ')
if [ "$REPLICATION_ENABLED" -gt 0 ]; then
    echo "✓ Replication configured (max_wal_senders: $REPLICATION_ENABLED)"
else
    echo "ℹ Replication not configured"
fi

echo ""
echo "=== PostgreSQL Health Check Passed ==="
echo "All critical checks completed successfully."
