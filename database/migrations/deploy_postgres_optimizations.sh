#!/bin/bash
# =============================================================================
# PostgreSQL Optimization Index Deployment Script
# =============================================================================
# This script deploys the query optimization indexes to the Chimera PostgreSQL
# database and verifies their installation.
#
# Usage: ./deploy_postgres_optimizations.sh [postgres_url]
# Example: ./deploy_postgres_optimizations.sh "postgresql://user:pass@localhost:5432/chimera"
# =============================================================================

set -e  # Exit on error

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OPTIMIZATION_SQL="${SCRIPT_DIR}/postgres_optimization_indexes.sql"
TEST_SQL="${SCRIPT_DIR}/test_postgres_optimization.sql"

# Default PostgreSQL URL (can be overridden by argument)
DEFAULT_URL="postgresql://chimera:chimera@localhost:5432/chimera"
POSTGRES_URL="${1:-$DEFAULT_URL}"

echo -e "${BLUE}============================================================================${NC}"
echo -e "${BLUE}Chimera PostgreSQL Query Optimization Deployment${NC}"
echo -e "${BLUE}============================================================================${NC}"
echo ""

# Check if psql is available
if ! command -v psql &> /dev/null; then
    echo -e "${RED}Error: psql client not found. Please install PostgreSQL client tools.${NC}"
    exit 1
fi

# Check if optimization SQL file exists
if [ ! -f "$OPTIMIZATION_SQL" ]; then
    echo -e "${RED}Error: Optimization SQL file not found at $OPTIMIZATION_SQL${NC}"
    exit 1
fi

# Function to check PostgreSQL connection
check_connection() {
    echo -e "${YELLOW}Testing PostgreSQL connection...${NC}"
    if psql "$POSTGRES_URL" -c "SELECT 1;" &> /dev/null; then
        echo -e "${GREEN}✓ PostgreSQL connection successful${NC}"
        return 0
    else
        echo -e "${RED}✗ PostgreSQL connection failed${NC}"
        echo -e "${YELLOW}Please check your connection string and database status${NC}"
        return 1
    fi
}

# Function to create backup
create_backup() {
    echo -e "${YELLOW}Creating database backup...${NC}"
    BACKUP_DIR="${SCRIPT_DIR}/backups"
    mkdir -p "$BACKUP_DIR"
    BACKUP_FILE="${BACKUP_DIR}/chimera_backup_$(date +%Y%m%d_%H%M%S).sql"

    if pg_dump "$POSTGRES_URL" > "$BACKUP_FILE" 2>/dev/null; then
        echo -e "${GREEN}✓ Backup created: $BACKUP_FILE${NC}"
        return 0
    else
        echo -e "${RED}✗ Backup creation failed${NC}"
        return 1
    fi
}

# Function to deploy optimization indexes
deploy_optimizations() {
    echo -e "${YELLOW}Deploying optimization indexes...${NC}"

    if psql "$POSTGRES_URL" -f "$OPTIMIZATION_SQL"; then
        echo -e "${GREEN}✓ Optimization indexes deployed successfully${NC}"
        return 0
    else
        echo -e "${RED}✗ Optimization deployment failed${NC}"
        return 1
    fi
}

# Function to verify indexes
verify_indexes() {
    echo -e "${YELLOW}Verifying optimization indexes...${NC}"

    # Check if key indexes exist
    INDEX_COUNT=$(psql "$POSTGRES_URL" -t -c "
        SELECT COUNT(*)
        FROM pg_indexes
        WHERE schemaname = 'public'
          AND (
            indexname LIKE 'idx_trades_pnl_percent'
            OR indexname LIKE 'idx_trades_strategy_pnl'
            OR indexname LIKE 'idx_wallets_roi_percent'
            OR indexname LIKE 'idx_positions_unrealized_pnl_percent'
          );
    ")

    if [ "$INDEX_COUNT" -ge 4 ]; then
        echo -e "${GREEN}✓ Key optimization indexes verified ($INDEX_COUNT indexes found)${NC}"
        return 0
    else
        echo -e "${YELLOW}⚠ Only $INDEX_COUNT key indexes found (expected at least 4)${NC}"
        return 1
    fi
}

# Function to run performance test
run_performance_test() {
    echo -e "${YELLOW}Running performance verification...${NC}"

    if [ -f "$TEST_SQL" ]; then
        psql "$POSTGRES_URL" -f "$TEST_SQL"
        echo -e "${GREEN}✓ Performance test completed${NC}"
        return 0
    else
        echo -e "${YELLOW}⚠ Test SQL file not found, skipping performance test${NC}"
        return 1
    fi
}

# Function to show index statistics
show_statistics() {
    echo -e "${YELLOW}Index Statistics:${NC}"
    echo ""

    psql "$POSTGRES_URL" -c "
    SELECT
        tablename,
        indexname,
        pg_size_pretty(pg_relation_size(indexrelid)) as size,
        idx_scan as scans
    FROM pg_stat_user_indexes
    WHERE schemaname = 'public'
      AND indexname LIKE 'idx_%'
    ORDER BY tablename, indexname;
    "

    echo ""
}

# Main deployment flow
main() {
    echo -e "${BLUE}Starting optimization deployment...${NC}"
    echo ""

    # Step 1: Check connection
    if ! check_connection; then
        exit 1
    fi
    echo ""

    # Step 2: Create backup
    if ! create_backup; then
        echo -e "${RED}Backup failed. Aborting deployment for safety.${NC}"
        exit 1
    fi
    echo ""

    # Step 3: Deploy optimizations
    if ! deploy_optimizations; then
        echo -e "${RED}Deployment failed. You can restore from backup if needed.${NC}"
        exit 1
    fi
    echo ""

    # Step 4: Verify indexes
    verify_indexes
    echo ""

    # Step 5: Show statistics
    show_statistics
    echo ""

    # Step 6: Run performance test (optional)
    echo -e "${YELLOW}Run performance test? (y/n)${NC}"
    read -r response
    if [[ "$response" =~ ^[Yy]$ ]]; then
        run_performance_test
    else
        echo -e "${YELLOW}Skipping performance test. Run manually:${NC}"
        echo -e "${BLUE}psql \"$POSTGRES_URL\" < \"$TEST_SQL\"${NC}"
    fi
    echo ""

    echo -e "${GREEN}============================================================================${NC}"
    echo -e "${GREEN}Optimization deployment completed successfully!${NC}"
    echo -e "${GREEN}============================================================================${NC}"
    echo ""
    echo -e "${BLUE}Next steps:${NC}"
    echo -e "1. Monitor query performance improvements"
    echo -e "2. Review index usage statistics over time"
    echo -e "3. Check storage impact: ~15-25% increase expected"
    echo -e "4. Run ANALYZE regularly to update statistics"
    echo ""
    echo -e "${YELLOW}For detailed usage guide, see: postgres_optimization_guide.md${NC}"
    echo ""
}

# Script usage
usage() {
    echo "Usage: $0 [postgres_url]"
    echo ""
    echo "Arguments:"
    echo "  postgres_url    PostgreSQL connection string (default: postgresql://chimera:chimera@localhost:5432/chimera)"
    echo ""
    echo "Examples:"
    echo "  $0"
    echo "  $0 \"postgresql://user:pass@localhost:5432/chimera\""
    echo ""
}

# Handle script arguments
case "${1:-}" in
    -h|--help)
        usage
        exit 0
        ;;
    *)
        main
        ;;
esac