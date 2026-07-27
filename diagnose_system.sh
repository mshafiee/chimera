#!/bin/bash
# Chimera System Diagnostics Script
# Run on production server to diagnose profitability issues

set -e

echo "=========================================="
echo "Chimera System Diagnostics"
echo "=========================================="
echo ""

# 1. Check git status and commits
echo "1. Checking Git Status..."
echo "----------------------------------------"
git fetch origin main 2>&1 > /dev/null
LOCAL=$(git rev-parse HEAD)
REMOTE=$(git rev-parse origin/main)

if [ "$LOCAL" != "$REMOTE" ]; then
    echo "❌ MISMATCH: Local and remote commits differ"
    echo "Local:   $(git rev-parse --short HEAD)"
    echo "Remote:  $(git rev-parse --short origin/main)"
else
    echo "✅ SYNCED: Local and remote are in sync"
    echo "Current: $(git rev-parse --short HEAD)"
    echo "Latest:  $(git log --oneline -1 origin/main)"
fi
echo ""

# 2. Check container status
echo "2. Checking Container Status..."
echo "----------------------------------------"
if docker compose -f docker-compose.yml -f docker-compose-haproxy.yml ps &> /dev/null; then
    docker compose -f docker-compose.yml -f docker-compose-haproxy.yml ps
else
    echo "❌ Docker not accessible"
fi
echo ""

# 3. Check operator logs (last 100 lines)
echo "3. Checking Operator Logs (Last 100 lines)..."
echo "----------------------------------------"
if docker compose -f docker-compose.yml -f docker-compose-haproxy.yml logs operator --tail=100 2>&1 | tail -50
then
    echo "✅ Operator logs retrieved"
else
    echo "❌ Could not retrieve operator logs"
fi
echo ""

# 4. Check scout logs (last 100 lines)
echo "4. Checking Scout Logs (Last 100 lines)..."
echo "----------------------------------------"
if docker compose -f docker-compose.yml -f docker-compose-haproxy.yml logs scout --tail=100 2>&1 | tail -50
then
    echo "✅ Scout logs retrieved"
else
    echo "❌ Could not retrieve scout logs"
fi
echo ""

# 5. Check environment variables
echo "5. Checking Environment Variables..."
echo "----------------------------------------"
echo "Operator WQS Minimum: $(docker compose -f docker-compose.yml -f docker-compose-haproxy.yml config | grep CHIMERA_SELECTION__MIN_WQS_SCORE)"
echo "Scout Close Ratio SCALPER: $(docker compose -f docker-compose.yml -f docker-compose-haproxy.yml config | grep SCOUT_MIN_CLOSE_RATIO_SCALPER)"
echo "Scout Close Ratio SWING: $(docker compose -f docker-compose.yml -f docker-compose-haproxy.yml config | grep SCOUT_MIN_CLOSE_RATIO_SWING)"
echo "Scout Close Ratio WHALE: $(docker compose -f docker-compose.yml -f docker-compose-haproxy.yml config | grep SCOUT_MIN_CLOSE_RATIO_WHALE)"
echo ""

# 6. Check active wallets
echo "6. Checking Active Wallets..."
echo "----------------------------------------"
if [ -f /opt/chimera/operator/target/debug/chimera_operator ]; then
    echo "Checking database for active wallets..."
    docker compose -f docker-compose.yml -f docker-compose-haproxy.yml exec -T postgres psql -U chimera -d chimera_db -c "SELECT address, archetype, wqs_score, roi_7d, last_polled_at FROM wallet_roster WHERE status = 'ACTIVE' ORDER BY wqs_score DESC;" 2>&1 || echo "Database query failed"
else
    echo "❌ Operator binary not found"
fi
echo ""

# 7. Check trade history (last 12 hours)
echo "7. Checking Trade History (Last 12 Hours)..."
echo "----------------------------------------"
echo "Trade table:"
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml exec -T postgres psql -U chimera -d chimera_db -c "
SELECT
    trade_uuid,
    wallet_address,
    token_address,
    action,
    amount_sol,
    status,
    created_at,
    updated_at
FROM trade_history
WHERE created_at > NOW() - INTERVAL '12 hours'
ORDER BY created_at DESC
LIMIT 20;
" 2>&1 || echo "Database query failed"
echo ""

# 8. Check PnL summary
echo "8. Checking PnL Summary (Last 12 Hours)..."
echo "----------------------------------------"
echo "PnL by wallet:"
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml exec -T postgres psql -U chimera -d chimera_db -c "
SELECT
    wallet_address,
    COALESCE(SUM(CASE WHEN action = 'BUY' THEN amount_sol ELSE -amount_sol END), 0) as total_buy_sell_sol,
    COUNT(*) as trade_count,
    MAX(created_at) as last_trade
FROM trade_history
WHERE created_at > NOW() - INTERVAL '12 hours'
GROUP BY wallet_address
ORDER BY total_buy_sell_sol DESC
LIMIT 10;
" 2>&1 || echo "Database query failed"
echo ""

# 9. Check operator configuration
echo "9. Checking Operator Configuration..."
echo "----------------------------------------"
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml exec -T postgres psql -U chimera -d chimera_db -c "
SELECT
    config_key,
    config_value,
    last_updated
FROM system_config
WHERE config_key LIKE '%WQS%' OR config_key LIKE '%close%';
" 2>&1 || echo "Database query failed"
echo ""

# 10. Check for recent errors
echo "10. Checking for Recent Errors..."
echo "----------------------------------------"
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml logs operator --tail=200 2>&1 | grep -i "error\|reject\|fail" | tail -20 || echo "No recent errors found in operator logs"
echo ""

echo "=========================================="
echo "Diagnostics Complete"
echo "=========================================="
echo ""
echo "Next Steps:"
echo "1. Review the output above for any errors or issues"
echo "2. Check if operator is detecting BUY/SELL signals"
echo "3. Verify that trades are being executed"
echo "4. Review profitability metrics"
