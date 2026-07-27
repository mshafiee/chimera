#!/bin/bash
# Chimera Profitability Check
# Run on production server to check profitability over last 12 hours

set -e

# Portable date handling: BSD (macOS) vs GNU (Linux)
if date -v-1d +%Y-%m-%d >/dev/null 2>&1; then
    SINCE=$(date -v-12H -u +%Y-%m-%d\ %H:%M:%S)
else
    SINCE=$(date -u -d '12 hours ago' +%Y-%m-%d\ %H:%M:%S)
fi

echo "=========================================="
echo "Chimera Profitability Analysis"
echo "Last 12 hours (up to $SINCE)"
echo "=========================================="
echo ""

# 1. Total trades in last 12 hours
echo "1. Total Trades (Last 12 Hours)..."
echo "----------------------------------------"
TOTAL_TRADES=$(docker compose -f docker-compose.yml -f docker-compose-haproxy.yml exec -T postgres psql -U chimera -d chimera_db -t -c "SELECT COUNT(*) FROM trade_history WHERE created_at > NOW() - INTERVAL '12 hours';" 2>&1 | tr -d ' ')
echo "Total trades: $TOTAL_TRADES"
echo ""

# 2. PnL by strategy (SHIELD vs SPEAR)
echo "2. PnL by Strategy..."
echo "----------------------------------------"
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml exec -T postgres psql -U chimera -d chimera_db -c "
SELECT
    strategy,
    COALESCE(SUM(CASE WHEN action = 'BUY' THEN amount_sol ELSE -amount_sol END), 0) as net_pnl_sol,
    COUNT(*) as trade_count,
    AVG(CASE WHEN action = 'BUY' THEN amount_sol ELSE -amount_sol END) as avg_pnl_per_trade
FROM trade_history
WHERE created_at > NOW() - INTERVAL '12 hours'
GROUP BY strategy
ORDER BY net_pnl_sol DESC;
" 2>&1 || echo "Query failed"
echo ""

# 3. Top performing wallets
echo "3. Top Performing Wallets (Last 12 Hours)..."
echo "----------------------------------------"
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml exec -T postgres psql -U chimera -d chimera_db -c "
SELECT
    wr.address,
    wr.archetype,
    wr.wqs_score,
    wr.roi_7d,
    COUNT(th.trade_uuid) as trade_count,
    COALESCE(SUM(CASE WHEN th.action = 'BUY' THEN th.amount_sol ELSE -th.amount_sol END), 0) as wallet_pnl_sol,
    MAX(th.created_at) as last_trade
FROM wallet_roster wr
LEFT JOIN trade_history th ON wr.address = th.wallet_address
WHERE th.created_at > NOW() - INTERVAL '12 hours'
GROUP BY wr.address, wr.archetype, wr.wqs_score, wr.roi_7d
ORDER BY wallet_pnl_sol DESC
LIMIT 10;
" 2>&1 || echo "Query failed"
echo ""

# 4. Trade status breakdown
echo "4. Trade Status Breakdown (Last 12 Hours)..."
echo "----------------------------------------"
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml exec -T postgres psql -U chimera -d chimera_db -c "
SELECT
    status,
    COUNT(*) as count,
    ROUND(100.0 * COUNT(*) / SUM(COUNT(*)) OVER (), 2) as percentage
FROM trade_history
WHERE created_at > NOW() - INTERVAL '12 hours'
GROUP BY status
ORDER BY count DESC;
" 2>&1 || echo "Query failed"
echo ""

# 5. Wallet copy performance
echo "5. Wallet Copy Performance (Last 12 Hours)..."
echo "----------------------------------------"
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml exec -T postgres psql -U chimera -d chimera_db -c "
SELECT
    source_wallet,
    COALESCE(SUM(CASE WHEN trade_action = 'BUY' THEN amount_sol ELSE -amount_sol END), 0) as total_pnl_sol,
    COUNT(*) as copied_trades,
    COUNT(DISTINCT trade_uuid) as unique_trades,
    MAX(created_at) as last_copy
FROM wallet_copy_performance
WHERE created_at > NOW() - INTERVAL '12 hours'
GROUP BY source_wallet
ORDER BY total_pnl_sol DESC
LIMIT 10;
" 2>&1 || echo "Query failed"
echo ""

# 6. Exit strategy performance
echo "6. Exit Strategy Performance (Last 12 Hours)..."
echo "----------------------------------------"
docker compose -f docker-compose.yml -f docker-compose-haproxy.yml exec -T postgres psql -U chimera -d chimera_db -c "
SELECT
    CASE
        WHEN exit_reason = 'STOP_LOSS' THEN 'Stop Loss'
        WHEN exit_reason = 'PROFIT_TARGET' THEN 'Profit Target'
        WHEN exit_reason = 'MOMENTUM' THEN 'Momentum'
        WHEN exit_reason = 'TIME_EXIT' THEN 'Time Exit'
        ELSE 'Other'
    END as exit_strategy,
    COUNT(*) as trade_count,
    ROUND(AVG(CASE WHEN exit_reason = 'PROFIT_TARGET' THEN pnl_sol ELSE 0 END), 4) as avg_profit_target_pnl,
    ROUND(AVG(CASE WHEN exit_reason = 'STOP_LOSS' THEN pnl_sol ELSE 0 END), 4) as avg_stop_loss_pnl
FROM trade_history
WHERE created_at > NOW() - INTERVAL '12 hours' AND exit_reason IS NOT NULL
GROUP BY exit_strategy
ORDER BY trade_count DESC;
" 2>&1 || echo "Query failed"
echo ""

# 7. Total portfolio PnL
echo "7. Total Portfolio PnL (Last 12 Hours)..."
echo "----------------------------------------"
TOTAL_PNL=$(docker compose -f docker-compose.yml -f docker-compose-haproxy.yml exec -T postgres psql -U chimera -d chimera_db -t -c "SELECT COALESCE(SUM(CASE WHEN action = 'BUY' THEN amount_sol ELSE -amount_sol END), 0) FROM trade_history WHERE created_at > NOW() - INTERVAL '12 hours';" 2>&1 | tr -d ' ')
echo "Total PnL: $TOTAL_PNL SOL"
echo ""

# 8. Profitability metrics
echo "8. Profitability Metrics..."
echo "----------------------------------------"
WALLET_COUNT=$(docker compose -f docker-compose.yml -f docker-compose-haproxy.yml exec -T postgres psql -U chimera -d chimera_db -t -c "SELECT COUNT(*) FROM wallet_roster WHERE status = 'ACTIVE';" 2>&1 | tr -d ' ')
if [ "$WALLET_COUNT" -gt 0 ]; then
    AVG_PER_WALLET=$(docker compose -f docker-compose.yml -f docker-compose-haproxy.yml exec -T postgres psql -U chimera -d chimera_db -t -c "SELECT COALESCE(AVG(CASE WHEN action = 'BUY' THEN amount_sol ELSE -amount_sol END), 0) FROM trade_history WHERE created_at > NOW() - INTERVAL '12 hours';" 2>&1 | tr -d ' ')
    echo "Active wallets: $WALLET_COUNT"
    echo "Average PnL per wallet: $AVG_PER_WALLET SOL"
    echo "Trades per active wallet: $(echo "scale=2; $TOTAL_TRADES / $WALLET_COUNT" | bc)"
else
    echo "Active wallets: $WALLET_COUNT"
fi
echo ""

echo "=========================================="
echo "Analysis Complete"
echo "=========================================="
echo ""
echo "Summary:"
echo "- Total PnL: $TOTAL_PNL SOL"
echo "- Total trades: $TOTAL_TRADES"
echo ""

if [ "$TOTAL_PNL" -gt 0 ]; then
    echo "✅ PROFITABLE - System is generating positive returns"
elif [ "$TOTAL_PNL" -lt 0 ]; then
    echo "❌ LOSING - System is losing money"
else
    echo "⚠️  BREAK-EVEN - No profit or loss"
fi
echo ""
