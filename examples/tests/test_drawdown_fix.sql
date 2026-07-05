-- Test script to verify 24-hour drawdown window fix
-- This demonstrates that the drawdown calculation now only considers recent positions

-- Setup: Create test database schema
CREATE TABLE IF NOT EXISTS positions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    trade_uuid TEXT NOT NULL,
    wallet_address TEXT NOT NULL,
    token_address TEXT NOT NULL,
    strategy TEXT NOT NULL,
    entry_amount_sol REAL NOT NULL,
    entry_price REAL NOT NULL,
    entry_tx_signature TEXT NOT NULL,
    state TEXT NOT NULL,
    realized_pnl_sol REAL,
    unrealized_pnl_sol REAL,
    closed_at TIMESTAMP
);

-- Test Scenario 1: Historical positions (should be excluded)
INSERT INTO positions (trade_uuid, wallet_address, token_address, strategy, entry_amount_sol, entry_price, entry_tx_signature, state, realized_pnl_sol, closed_at)
VALUES
    ('hist-1', 'wallet1', 'token1', 'SHIELD', 1.0, 1.0, 'sig1', 'CLOSED', 100.0, '2026-01-01 00:00:00'),
    ('hist-2', 'wallet1', 'token2', 'SHIELD', 1.0, 1.0, 'sig2', 'CLOSED', 100.0, '2026-01-01 00:01:00'),
    ('hist-3', 'wallet1', 'token3', 'SHIELD', 1.0, 1.0, 'sig3', 'CLOSED', 100.0, '2026-01-01 00:02:00');

-- Test Scenario 2: Recent positions (should be included - within 24 hours)
INSERT INTO positions (trade_uuid, wallet_address, token_address, strategy, entry_amount_sol, entry_price, entry_tx_signature, state, realized_pnl_sol, closed_at)
VALUES
    ('recent-1', 'wallet1', 'token4', 'SHIELD', 1.0, 1.0, 'sig4', 'CLOSED', -50.0, datetime('now', '-12 hours')),
    ('recent-2', 'wallet1', 'token5', 'SHIELD', 1.0, 1.0, 'sig5', 'CLOSED', -30.0, datetime('now', '-6 hours')),
    ('recent-3', 'wallet1', 'token6', 'SHIELD', 1.0, 1.0, 'sig6', 'CLOSED', -20.0, datetime('now', '-1 hour'));

-- Test Scenario 3: Active positions (should be included)
INSERT INTO positions (trade_uuid, wallet_address, token_address, strategy, entry_amount_sol, entry_price, entry_tx_signature, state, unrealized_pnl_sol)
VALUES
    ('active-1', 'wallet1', 'token7', 'SHIELD', 1.0, 1.0, 'sig7', 'ACTIVE', 10.0),
    ('active-2', 'wallet1', 'token8', 'SHIELD', 1.0, 1.0, 'sig8', 'EXITING', 5.0);

-- Verification: Show which positions are included/excluded
SELECT '=== Position Classification ===' as info;
SELECT
    trade_uuid,
    realized_pnl_sol,
    closed_at,
    CASE
        WHEN closed_at >= datetime('now', '-24 hours') THEN 'INCLUDED (Recent)'
        ELSE 'EXCLUDED (Historical)'
    END as status
FROM positions
WHERE state = 'CLOSED'
ORDER BY closed_at DESC;

-- Compare OLD vs NEW logic
SELECT '=== OLD Logic (All-Time Peak) ===' as info;
SELECT
    COUNT(*) as total_positions,
    SUM(realized_pnl_sol) as total_pnl
FROM positions
WHERE state = 'CLOSED';

SELECT '=== NEW Logic (24-Hour Peak) ===' as info;
SELECT
    COUNT(*) as total_positions,
    SUM(realized_pnl_sol) as total_pnl
FROM positions
WHERE state = 'CLOSED' AND closed_at >= datetime('now', '-24 hours');

-- Demonstrate the fix impact
SELECT '=== Impact Analysis ===' as info;
SELECT
    'OLD logic would use' as approach,
    300.0 as peak_pnl,
    'This includes historical gains from January'
UNION ALL
SELECT
    'NEW logic uses' as approach,
    -100.0 as peak_pnl,
    'This only includes recent losses from last 24 hours';

-- Cleanup
DROP TABLE positions;
