#!/bin/bash
# Redis health check for monitoring
set -e

REDIS_HOST="${REDIS_HOST:-redis}"
REDIS_PORT="${REDIS_PORT:-6379}"
COMPOSE_CMD="${COMPOSE_CMD:-docker-compose}"

echo "=== Redis Health Check ==="
echo "Host: $REDIS_HOST:$REDIS_PORT"
echo ""

# Check if Redis service is running (compose-aware, no container-name parsing)
if [ -z "$($COMPOSE_CMD ps -q redis 2>/dev/null)" ]; then
    echo "❌ Redis service is not running"
    exit 1
fi

# All redis-cli calls are bounded with `timeout` so a hung container cannot
# block the monitor indefinitely.
REDIS_CLI="timeout 10 $COMPOSE_CMD exec -T redis redis-cli -h $REDIS_HOST -p $REDIS_PORT --connect-timeout 5"

# Check connection
echo "Checking connection..."
if $REDIS_CLI ping | grep -q PONG; then
    echo "✓ Redis is responding"
else
    echo "❌ Redis is not responding"
    exit 1
fi

# Fetch all INFO sections in a single exec (fewer container spawns, one
# consistent snapshot)
INFO_ALL=$($REDIS_CLI INFO all 2>/dev/null || true)

# Get memory usage
echo "Checking memory usage..."
if [ -n "$INFO_ALL" ]; then
    USED_MEMORY=$(echo "$INFO_ALL" | grep "^used_memory_human:" | cut -d: -f2 | tr -d '\r')
    PEAK_MEMORY=$(echo "$INFO_ALL" | grep "^used_memory_peak_human:" | cut -d: -f2 | tr -d '\r')
    MAX_MEMORY=$(echo "$INFO_ALL" | grep "^maxmemory:" | cut -d: -f2 | tr -d '\r')

    echo "✓ Used memory: $USED_MEMORY"
    echo "  Peak memory: $PEAK_MEMORY"
    if [ -n "$MAX_MEMORY" ] && [ "$MAX_MEMORY" != "0" ]; then
        echo "  Max memory: $MAX_MEMORY"
    fi
else
    echo "⚠ Could not retrieve memory info"
fi

# Check key count
echo "Checking key count..."
KEY_COUNT=$($REDIS_CLI DBSIZE 2>/dev/null | tr -d '\r' || true)
if [ -n "$KEY_COUNT" ]; then
    echo "✓ Total keys: $KEY_COUNT"
else
    echo "⚠ Could not retrieve key count"
fi

# Check Redis version
echo "Checking Redis version..."
REDIS_VERSION=$(echo "$INFO_ALL" | grep "^redis_version:" | cut -d: -f2 | tr -d '\r' || true)
if [ -n "$REDIS_VERSION" ]; then
    echo "✓ Redis version: $REDIS_VERSION"
else
    echo "⚠ Could not retrieve Redis version"
fi

# Check uptime
echo "Checking uptime..."
UPTIME_DAYS=$(echo "$INFO_ALL" | grep "^uptime_in_days:" | cut -d: -f2 | tr -d '\r' || true)
if [ -n "$UPTIME_DAYS" ]; then
    echo "✓ Uptime: $UPTIME_DAYS days"
else
    echo "⚠ Could not retrieve uptime"
fi

# Check hit rate
echo "Checking cache performance..."
if [ -n "$INFO_ALL" ]; then
    HITS=$(echo "$INFO_ALL" | grep "^keyspace_hits:" | cut -d: -f2 | tr -d '\r')
    MISSES=$(echo "$INFO_ALL" | grep "^keyspace_misses:" | cut -d: -f2 | tr -d '\r')

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
if [ -n "$INFO_ALL" ]; then
    AOF_ENABLED=$(echo "$INFO_ALL" | grep "^aof_enabled:" | cut -d: -f2 | tr -d '\r')
    SAVING=$(echo "$INFO_ALL" | grep "^rdb_bgsave_in_progress:" | cut -d: -f2 | tr -d '\r')

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
if [ -n "$INFO_ALL" ]; then
    CONNECTED_CLIENTS=$(echo "$INFO_ALL" | grep "^connected_clients:" | cut -d: -f2 | tr -d '\r')
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
