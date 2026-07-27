#!/usr/bin/env bash
# reset-trading-state.sh — Fresh DB Cleanup & Paper Trading Restart
# Usage: ./scripts/reset-trading-state.sh [--force] [--restart]
#   --force    Skip confirmation prompt
#   --restart  Restart operator after cleanup (default: skip restart)

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Parse arguments
FORCE=false
RESTART_OPERATOR=false
while [[ $# -gt 0 ]]; do
    case $1 in
        --force)
            FORCE=true
            shift
            ;;
        --restart)
            RESTART_OPERATOR=true
            shift
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [--force] [--restart]"
            exit 1
            ;;
    esac
done

# Confirmation prompt
if [ "$FORCE" = false ]; then
    echo -e "${RED}WARNING: This will truncate all trading history tables!${NC}"
    echo "The following will be done:"
    echo "  - Stop operator"
    echo "  - Backup preserved tables"
    echo "  - Truncate all history tables (trades, positions, dlq, etc.)"
    echo "  - Reset circuit_breaker_state and kill_switch_state"
    echo "  - Drop orphaned test databases"
    echo "  - Flush Redis cache"
    echo "  - Clean stale operator data files"
    if [ "$RESTART_OPERATOR" = true ]; then
        echo "  - Restart operator"
    fi
    echo ""
    read -p "Are you sure? Type 'yes' to continue: " confirmation
    if [ "$confirmation" != "yes" ]; then
        echo "Aborted."
        exit 0
    fi
fi

echo -e "${BLUE}Starting DB cleanup...${NC}"

# Step 1 — Stop Operator
echo -e "${GREEN}[1/8]${NC} Stopping operator..."
COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml stop operator

# Step 2 — Backup Preserved Tables
echo -e "${GREEN}[2/8]${NC} Backing up preserved tables..."
mkdir -p backups
TIMESTAMP=$(date -u +%Y%m%d_%H%M%S)
BACKUP_FILE="backups/preserved_data_${TIMESTAMP}.sql"

for t in admin_wallets wallets wallet_monitoring webhook_configuration toxic_wallets circuit_breaker_state kill_switch_state; do
    echo "-- Table: $t" >> "$BACKUP_FILE"
    docker compose -f docker-compose.yml -f docker-compose-haproxy.yml exec -T postgres psql -U chimera -d chimera -c "COPY $t TO STDOUT" 2>/dev/null >> "$BACKUP_FILE" || true
done
echo "Backup saved to: $BACKUP_FILE"

# Step 3 — Truncate History Tables
echo -e "${GREEN}[3/8]${NC} Truncating history tables..."
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml exec -T postgres psql -U chimera -d chimera <<'EOF'
TRUNCATE TABLE trades RESTART IDENTITY CASCADE;
TRUNCATE TABLE dead_letter_queue RESTART IDENTITY;
TRUNCATE TABLE decision_records RESTART IDENTITY;
TRUNCATE TABLE promotion_episodes RESTART IDENTITY;
TRUNCATE TABLE webhook_lifecycle_audit RESTART IDENTITY;
TRUNCATE TABLE signal_aggregation RESTART IDENTITY;
TRUNCATE TABLE wallet_copy_performance RESTART IDENTITY;
TRUNCATE TABLE jito_tip_history RESTART IDENTITY;
TRUNCATE TABLE rate_limit_metrics RESTART IDENTITY;
TRUNCATE TABLE reconciliation_log RESTART IDENTITY;
TRUNCATE TABLE health_checks RESTART IDENTITY;
TRUNCATE TABLE ml_predictions RESTART IDENTITY;
TRUNCATE TABLE alerts RESTART IDENTITY;
TRUNCATE TABLE config_audit RESTART IDENTITY;
TRUNCATE TABLE wallet_performance_history RESTART IDENTITY;
TRUNCATE TABLE wqs_pnl_correlation RESTART IDENTITY;
TRUNCATE TABLE multi_timeframe_discovery_stats RESTART IDENTITY;
TRUNCATE TABLE credit_history RESTART IDENTITY;
TRUNCATE TABLE roi_metrics RESTART IDENTITY;
TRUNCATE TABLE metrics RESTART IDENTITY;
TRUNCATE TABLE historical_liquidity RESTART IDENTITY;
TRUNCATE TABLE backups RESTART IDENTITY;
UPDATE circuit_breaker_state SET state = 'ACTIVE', tripped_at = NULL, trip_reason = NULL WHERE id = 1;
UPDATE kill_switch_state SET state = 'INACTIVE', changed_at = NOW(), changed_by = 'SYSTEM', reason = 'Fresh DB reset for paper trading' WHERE id = 1;
EOF

# Step 4 — Drop Orphaned Test Databases
echo -e "${GREEN}[4/8]${NC} Dropping orphaned test databases..."
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml exec -T postgres bash -c \
    "psql -U chimera -c \"DROP DATABASE IF EXISTS test_ab2d5b72_fb3d_419a_a46f_c2cf44765f83\" && \
     psql -U chimera -c \"DROP DATABASE IF EXISTS test_1f1de278_24e0_45bd_a6b2_e6270b4639b3\" && \
     psql -U chimera -c \"DROP DATABASE IF EXISTS test_f8e777eb_306a_447f_bf85_2d8dd6823220\" && \
     psql -U chimera -c \"DROP DATABASE IF EXISTS test_90ba7e53_ff77_4999_986f_a5a5a5a401bd\" && \
     psql -U chimera -c \"DROP DATABASE IF EXISTS test_7fd913a7_d01f_441c_9f6f_d41bad46d294\"" 2>&1

# Step 5 — Flush Redis Cache
echo -e "${GREEN}[5/8]${NC} Flushing Redis cache..."
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml exec -T redis redis-cli FLUSHDB > /dev/null

# Step 6 — Clean Stale Operator Data
echo -e "${GREEN}[6/8]${NC} Cleaning stale operator data files..."
rm -f data/logs/operator.log.* data/logs/scout.log.* data/logs/operator.log data/logs/scout.log 2>/dev/null || true
rm -rf data/parse_failures/* 2>/dev/null || true
mkdir -p data/parse_failures
rm -f data/chimera.db 2>/dev/null || true

# Step 7 — Restart Operator (optional)
if [ "$RESTART_OPERATOR" = true ]; then
    echo -e "${GREEN}[7/8]${NC} Restarting operator..."
    COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml up -d --force-recreate operator
    echo "Waiting for operator to start..."
    sleep 15
else
    echo -e "${YELLOW}[7/8]${NC} Skipping operator restart (use --restart to enable)"
fi

# Step 8 — Verification Summary
echo -e "${GREEN}[8/8]${NC} Verification summary:"
echo ""

# Check table counts
echo "Table counts:"
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml exec -T postgres psql -U chimera -d chimera -c \
    "SELECT 'trades' AS table_name, COUNT(*) FROM trades
     UNION ALL SELECT 'positions', COUNT(*) FROM positions
     UNION ALL SELECT 'dead_letter_queue', COUNT(*) FROM dead_letter_queue
     UNION ALL SELECT 'wallets_active', COUNT(*) FROM wallets WHERE status = 'ACTIVE'
     UNION ALL SELECT 'wallet_monitoring', COUNT(*) FROM wallet_monitoring
     UNION ALL SELECT 'admin_wallets', COUNT(*) FROM admin_wallets;" 2>/dev/null

echo ""
echo "Circuit breaker state:"
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml exec -T postgres psql -U chimera -d chimera -c \
    "SELECT * FROM circuit_breaker_state;" 2>/dev/null

echo ""
echo "Kill switch state:"
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml exec -T postgres psql -U chimera -d chimera -c \
    "SELECT * FROM kill_switch_state;" 2>/dev/null

echo ""
echo -e "${GREEN}✓ DB cleanup completed successfully!${NC}"
echo "Backup file: $BACKUP_FILE"
echo ""
echo "To restart the operator manually:"
echo "  COMPOSE_PROFILE=mainnet-prod docker compose -f docker-compose.yml -f docker-compose-haproxy.yml up -d --force-recreate operator"
echo ""
echo "To verify after restart:"
echo "  curl -s http://localhost:8080/health"
echo "  curl -s http://localhost:8080/metrics | grep chimera_"