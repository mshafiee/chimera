#!/bin/bash
# Redis health check for monitoring
set -e

REDIS_HOST="${REDIS_HOST:-redis}"
REDIS_PORT="${REDIS_PORT:-6379}"

echo "=== Redis Health Check ==="
echo "Host: $REDIS_HOST:$REDIS_PORT"
echo ""

# Check if Redis container is running
if ! docker-compose ps | grep -q "chimera-redis.*Up"; then
    echo "❌ Redis container is not running"
    exit 1
fi

# Check connection
echo "Checking connection..."
if docker-compose exec redis redis-cli -h "$REDIS_HOST" -p "$REDIS_PORT" ping | grep -q PONG; then
    echo "✓ Redis is responding"
else
    echo "❌ Redis is not responding"
    exit 1
fi

# Get memory usage
echo "Checking memory usage..."
MEMORY_INFO=$(docker-compose exec redis redis-cli -h "$REDIS_HOST" -p "$REDIS_PORT" INFO memory 2>/dev/null)
if [ -n "$MEMORY_INFO" ]; then
    USED_MEMORY=$(echo "$MEMORY_INFO" | grep "used_memory_human:" | cut -d: -f2 | tr -d '\r')
    PEAK_MEMORY=$(echo "$MEMORY_INFO" | grep "used_memory_peak_human:" | cut -d: -f2 | tr -d '\r')
    MAX_MEMORY=$(echo "$MEMORY_INFO" | grep "maxmemory:" | cut -d: -f2 | tr -d '\r')

    echo "✓ Used memory: $USED_MEMORY"
    echo "  Peak memory: $PEAK_MEMORY"
    if [ "$MAX_MEMORY" != "0" ]; then
        echo "  Max memory: $MAX_MEMORY"
    fi
else
    echo "⚠ Could not retrieve memory info"
fi

# Check key count
echo "Checking key count..."
KEY_COUNT=$(docker-compose exec redis redis-cli -h "$REDIS_HOST" -p "$REDIS_PORT" DBSIZE 2>/dev/null | tr -d '\r')
if [ -n "$KEY_COUNT" ]; then
    echo "✓ Total keys: $KEY_COUNT"
else
    echo "⚠ Could not retrieve key count"
fi

# Check Redis version
echo "Checking Redis version..."
REDIS_VERSION=$(docker-compose exec redis redis-cli -h "$REDIS_HOST" -p "$REDIS_PORT" INFO server 2>/dev/null | grep "redis_version" | cut -d: -f2 | tr -d '\r')
if [ -n "$REDIS_VERSION" ]; then
    echo "✓ Redis version: $REDIS_VERSION"
else
    echo "⚠ Could not retrieve Redis version"
fi

# Check uptime
echo "Checking uptime..."
UPTIME_DAYS=$(docker-compose exec redis redis-cli -h "$REDIS_HOST" -p "$REDIS_PORT" INFO server 2>/dev/null | grep "uptime_in_days" | cut -d: -f2 | tr -d '\r')
if [ -n "$UPTIME_DAYS" ]; then
    echo "✓ Uptime: $UPTIME_DAYS days"
else
    echo "⚠ Could not retrieve uptime"
fi

# Check hit rate
echo "Checking cache performance..."
STATS_INFO=$(docker-compose exec redis redis-cli -h "$REDIS_HOST" -p "$REDIS_PORT" INFO stats 2>/dev/null)
if [ -n "$STATS_INFO" ]; then
    HITS=$(echo "$STATS_INFO" | grep "keyspace_hits:" | cut -d: -f2 | tr -d '\r')
    MISSES=$(echo "$STATS_INFO" | grep "keyspace_misses:" | cut -d: -f2 | tr -d '\r')

    if [ -n "$HITS" ] && [ -n "$MISSES" ]; then
        TOTAL=$((HITS + MISSES))
        if [ $TOTAL -gt 0 ]; then
            HIT_RATE=$((HITS * 100 / TOTAL))
            echo "✓ Cache hit rate: ${HIT_RATE}% (hits: $HITS, misses: $MISSES)"
        else
            echo "ℹ No cache activity yet"
        fi
    else
        echo "⚠ Could not retrieve cache statistics"
    fi
else
    echo "⚠ Could not retrieve cache statistics"
fi

# Check persistence
echo "Checking persistence..."
PERSISTENCE_INFO=$(docker-compose exec redis redis-cli -h "$REDIS_HOST" -p "$REDIS_PORT" INFO persistence 2>/dev/null)
if [ -n "$PERSISTENCE_INFO" ]; then
    AOF_ENABLED=$(echo "$PERSISTENCE_INFO" | grep "aof_enabled:" | cut -d: -f2 | tr -d '\r')
    SAVING=$(echo "$PERSISTENCE_INFO" | grep "rdb_bgsave_in_progress:" | cut -d: -f2 | tr -d '\r')

    if [ "$AOF_ENABLED" = "1" ]; then
        echo "✓ AOF persistence enabled"
    else
        echo "ℹ AOF persistence disabled"
    fi

    if [ "$SAVING" = "1" ]; then
        echo "⚠ Background save in progress"
    else
        echo "✓ No background save in progress"
    fi
else
    echo "⚠ Could not retrieve persistence info"
fi

# Check connected clients
echo "Checking connected clients..."
CLIENT_INFO=$(docker-compose exec redis redis-cli -h "$REDIS_HOST" -p "$REDIS_PORT" INFO clients 2>/dev/null)
if [ -n "$CLIENT_INFO" ]; then
    CONNECTED_CLIENTS=$(echo "$CLIENT_INFO" | grep "connected_clients:" | cut -d: -f2 | tr -d '\r')
    if [ -n "$CONNECTED_CLIENTS" ]; then
        echo "✓ Connected clients: $CONNECTED_CLIENTS"
    else
        echo "⚠ Could not retrieve client count"
    fi
else
    echo "⚠ Could not retrieve client info"
fi

echo ""
echo "=== Redis Health Check Passed ==="
echo "All critical checks completed successfully."
