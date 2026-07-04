-- =============================================================================
-- PostgreSQL Optimization Index Verification Script
-- =============================================================================
-- This script tests and verifies that the optimization indexes are working
-- correctly by running sample queries and checking execution plans.
--
-- Usage: psql chimera < test_postgres_optimization.sql
-- =============================================================================

-- Start timing
\timing on

-- Display verbose execution plans
SET client_encoding = 'UTF8';
SET TimeZone = 'UTC';

-- =============================================================================
-- INDEX VERIFICATION
-- =============================================================================

\echo '============================================================================'
\echo 'INDEX VERIFICATION - Checking if optimization indexes exist'
\echo '============================================================================'

-- Check if key optimization indexes exist
SELECT
    schemaname,
    tablename,
    indexname,
    indexdef
FROM pg_indexes
WHERE indexname LIKE 'idx_%_pnl%'
   OR indexname LIKE 'idx_%_strategy%'
   OR indexname LIKE 'idx_%_wallets%'
   OR indexname LIKE 'idx_%_positions%'
ORDER BY tablename, indexname;

\echo ''
\echo 'Expected: Should see indexes like idx_trades_pnl_percent, idx_trades_strategy_pnl, etc.'
\echo ''

-- =============================================================================
-- SAMPLE QUERIES WITH EXECUTION PLANS
-- =============================================================================

\echo '============================================================================'
\echo 'QUERY 1: PnL Percentage Calculation (should use idx_trades_pnl_percent)'
\echo '============================================================================'

EXPLAIN (ANALYZE, BUFFERS, VERBOSE)
SELECT
    trade_uuid,
    (net_pnl_sol - COALESCE(total_cost_sol, 0)) / amount_sol * 100.0 as roi_percent,
    net_pnl_sol,
    amount_sol
FROM trades
WHERE net_pnl_sol IS NOT NULL
  AND amount_sol > 0
  AND (net_pnl_sol - COALESCE(total_cost_sol, 0)) / amount_sol * 100.0 > 10.0
ORDER BY roi_percent DESC
LIMIT 10;

\echo 'Expected: Should show "Index Scan using idx_trades_pnl_percent"'
\echo ''

-- =============================================================================
-- QUERY 2: Strategy Performance Aggregation
-- =============================================================================

\echo '============================================================================'
\echo 'QUERY 2: Strategy Performance (should use idx_trades_strategy_pnl)'
\echo '============================================================================'

EXPLAIN (ANALYZE, BUFFERS, VERBOSE)
SELECT
    strategy,
    COUNT(*) as total_trades,
    SUM(net_pnl_sol) as total_pnl,
    AVG((net_pnl_sol - COALESCE(total_cost_sol, 0)) / amount_sol * 100.0) as avg_roi
FROM trades
WHERE status = 'CLOSED'
  AND net_pnl_sol IS NOT NULL
  AND created_at >= CURRENT_DATE - INTERVAL '7 days'
GROUP BY strategy
ORDER BY total_pnl DESC;

\echo 'Expected: Should show "Index Scan using idx_trades_strategy_pnl"'
\echo ''

-- =============================================================================
-- QUERY 3: Wallet ROI Ranking
-- =============================================================================

\echo '============================================================================'
\echo 'QUERY 3: Wallet ROI Ranking (should use idx_wallets_roi_percent)'
\echo '============================================================================'

EXPLAIN (ANALYZE, BUFFERS, VERBOSE)
SELECT
    address,
    realized_pnl_30d_sol / avg_trade_size_sol * 100.0 as roi_percent,
    wqs_score
FROM wallets
WHERE status = 'ACTIVE'
  AND avg_trade_size_sol > 0
  AND realized_pnl_30d_sol IS NOT NULL
ORDER BY roi_percent DESC
LIMIT 20;

\echo 'Expected: Should show "Index Scan using idx_wallets_roi_percent"'
\echo ''

-- =============================================================================
-- QUERY 4: Active Positions with Unrealized PnL
-- =============================================================================

\echo '============================================================================'
\echo 'QUERY 4: Active Positions (should use idx_positions_unrealized_pnl_percent)'
\echo '============================================================================'

EXPLAIN (ANALYZE, BUFFERS, VERBOSE)
SELECT
    trade_uuid,
    wallet_address,
    unrealized_pnl_sol / entry_amount_sol * 100.0 as unrealized_roi_percent,
    entry_amount_sol,
    unrealized_pnl_sol
FROM positions
WHERE state IN ('ACTIVE', 'EXITING')
  AND unrealized_pnl_sol IS NOT NULL
  AND entry_amount_sol > 0
  AND unrealized_pnl_sol / entry_amount_sol * 100.0 > 20.0
ORDER BY unrealized_roi_percent DESC
LIMIT 10;

\echo 'Expected: Should show "Index Scan using idx_positions_unrealized_pnl_percent"'
\echo ''

-- =============================================================================
-- QUERY 5: High-Value Trades
-- =============================================================================

\echo '============================================================================'
\echo 'QUERY 5: High-Value Trades (should use idx_trades_high_value)'
\echo '============================================================================'

EXPLAIN (ANALYZE, BUFFERS, VERBOSE)
SELECT
    trade_uuid,
    wallet_address,
    amount_sol,
    net_pnl_sol,
    created_at
FROM trades
WHERE amount_sol >= 1.0
  AND created_at >= CURRENT_DATE - INTERVAL '24 hours'
ORDER BY amount_sol DESC, created_at DESC
LIMIT 20;

\echo 'Expected: Should show "Index Scan using idx_trades_high_value"'
\echo ''

-- =============================================================================
-- INDEX USAGE STATISTICS
-- =============================================================================

\echo '============================================================================'
\echo 'INDEX USAGE STATISTICS'
\echo '============================================================================'

SELECT
    schemaname,
    tablename,
    indexname,
    idx_scan as index_scans,
    idx_tup_read as tuples_read,
    idx_tup_fetch as tuples_fetched,
    pg_size_pretty(pg_relation_size(indexrelid)) as index_size
FROM pg_stat_user_indexes
WHERE schemaname = 'public'
  AND (
    indexname LIKE 'idx_%_pnl%'
    OR indexname LIKE 'idx_%_strategy%'
    OR indexname LIKE 'idx_%_wallets%'
    OR indexname LIKE 'idx_%_positions%'
    OR indexname LIKE 'idx_%_high_value%'
  )
ORDER BY idx_scan DESC;

\echo ''
\echo 'Note: Initially index_scans may be 0 if queries haven\'t been run yet'
\echo ''

-- =============================================================================
-- TABLE AND INDEX SIZE SUMMARY
-- =============================================================================

\echo '============================================================================'
\echo 'STORAGE SUMMARY'
\echo '============================================================================'

SELECT
    tablename,
    pg_size_pretty(pg_total_relation_size(tablename::regclass)) as total_size,
    pg_size_pretty(pg_relation_size(tablename::regclass)) as table_size,
    pg_size_pretty(pg_indexes_size(tablename::regclass)) as indexes_size,
    ROUND(
        100.0 * pg_indexes_size(tablename::regclass) /
        NULLIF(pg_total_relation_size(tablename::regclass), 0), 2
    ) as index_percentage
FROM pg_tables
WHERE schemaname = 'public'
  AND tablename IN ('trades', 'positions', 'wallets')
ORDER BY pg_total_relation_size(tablename::regclass) DESC;

\echo ''
\echo 'Note: Index percentage should be roughly 15-25% with optimizations'
\echo ''

-- =============================================================================
-- PERFORMANCE COMPARISON (Sequential vs Index)
-- =============================================================================

\echo '============================================================================'
\echo 'PERFORMANCE COMPARISON'
\echo '============================================================================'

\echo 'Testing with indexes enabled (default):'
SET enable_seqscan = on;
EXPLAIN (ANALYZE, BUFFERS)
SELECT
    strategy,
    SUM(net_pnl_sol) as total_pnl
FROM trades
WHERE status = 'CLOSED'
  AND net_pnl_sol IS NOT NULL
GROUP BY strategy;

\echo ''
\echo 'Testing with indexes disabled (sequential scan):'
SET enable_seqscan = off;
EXPLAIN (ANALYZE, BUFFERS)
SELECT
    strategy,
    SUM(net_pnl_sol) as total_pnl
FROM trades
WHERE status = 'CLOSED'
  AND net_pnl_sol IS NOT NULL
GROUP BY strategy;

-- Reset to default
SET enable_seqscan = on;

\echo ''
\echo 'Compare execution times and buffer usage between the two approaches'
\echo ''

-- =============================================================================
-- INDEX MAINTENANCE RECOMMENDATIONS
-- =============================================================================

\echo '============================================================================'
\echo 'MAINTENANCE RECOMMENDATIONS'
\echo '============================================================================'

-- Check for potentially unused indexes
SELECT
    schemaname,
    tablename,
    indexname,
    pg_size_pretty(pg_relation_size(indexrelid)) as index_size,
    idx_scan as scans
FROM pg_stat_user_indexes
WHERE schemaname = 'public'
  AND idx_scan = 0
  AND indexname LIKE 'idx_%'
ORDER BY pg_relation_size(indexrelid) DESC
LIMIT 10;

\echo ''
\echo 'If any indexes show 0 scans after normal usage, they may be candidates for removal'
\echo ''

-- Check for index bloat indicators
SELECT
    schemaname,
    tablename,
    indexname,
    pg_size_pretty(pg_relation_size(indexrelid)) as size,
    idx_scan as scans,
    idx_tup_read as tuples_read,
    idx_tup_fetch as tuples_fetched,
    CASE
        WHEN idx_scan > 0 THEN
            ROUND(100.0 * idx_tup_fetch / idx_tup_read, 2)
        ELSE 0
    END as fetch_ratio_percent
FROM pg_stat_user_indexes
WHERE schemaname = 'public'
  AND indexname LIKE 'idx_%'
  AND idx_scan > 0
ORDER BY idx_tup_fetch DESC
LIMIT 10;

\echo ''
\echo 'High fetch ratios (>50%) typically indicate efficient index usage'
\echo ''

-- =============================================================================
-- SUMMARY
-- =============================================================================

\echo '============================================================================'
\echo 'OPTIMIZATION VERIFICATION COMPLETE'
\echo '============================================================================'
\echo ''
\echo 'Key points to review:'
\echo '1. Check that optimization indexes exist in first section'
\echo '2. Verify execution plans show "Index Scan" instead of "Seq Scan"'
\echo '3. Compare execution times between indexed and sequential scans'
\echo '4. Monitor index usage statistics over time'
\echo '5. Check storage impact is within expected 15-25% range'
\echo ''
\echo 'For detailed performance analysis, review the EXPLAIN output above.'
\echo 'For ongoing monitoring, query pg_stat_user_indexes regularly.'
\echo ''

-- Stop timing
\timing off

\echo '============================================================================'
\echo 'End of verification script'
\echo '============================================================================'