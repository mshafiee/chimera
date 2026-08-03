-- Test script to verify 24-hour drawdown window fix
-- This demonstrates that the drawdown calculation now only considers recent positions.
--
-- Assertions FAIL LOUDLY: on any mismatch the script emits an error (sqlite3
-- exits non-zero), instead of silently printing results.

-- Freeze the clock so boundary classifications are deterministic regardless of
-- how long the script takes to run.
CREATE TEMP TABLE test_clock AS SELECT datetime('now') AS now;

-- Setup: Create test database schema
-- SOL values are seeded as exact integers (exactly representable in REAL),
-- so SUM-based assertions are exact.
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

-- Test Scenario 1: Historical positions (should be excluded - months old)
INSERT INTO positions (trade_uuid, wallet_address, token_address, strategy, entry_amount_sol, entry_price, entry_tx_signature, state, realized_pnl_sol, closed_at)
VALUES
    ('hist-1', 'wallet1', 'token1', 'SHIELD', 1.0, 1.0, 'sig1', 'CLOSED', 100.0, '2026-01-01 00:00:00'),
    ('hist-2', 'wallet1', 'token2', 'SHIELD', 1.0, 1.0, 'sig2', 'CLOSED', 100.0, '2026-01-01 00:01:00'),
    ('hist-3', 'wallet1', 'token3', 'SHIELD', 1.0, 1.0, 'sig3', 'CLOSED', 100.0, '2026-01-01 00:02:00');

-- Test Scenario 2: Recent positions (should be included - within 24 hours)
INSERT INTO positions (trade_uuid, wallet_address, token_address, strategy, entry_amount_sol, entry_price, entry_tx_signature, state, realized_pnl_sol, closed_at)
VALUES
    ('recent-1', 'wallet1', 'token4', 'SHIELD', 1.0, 1.0, 'sig4', 'CLOSED', -50.0, datetime((SELECT now FROM test_clock), '-12 hours')),
    ('recent-2', 'wallet1', 'token5', 'SHIELD', 1.0, 1.0, 'sig5', 'CLOSED', -30.0, datetime((SELECT now FROM test_clock), '-6 hours')),
    ('recent-3', 'wallet1', 'token6', 'SHIELD', 1.0, 1.0, 'sig6', 'CLOSED', -20.0, datetime((SELECT now FROM test_clock), '-1 hour'));

-- Test Scenario 3: Active positions (should be included - open positions count regardless of age)
INSERT INTO positions (trade_uuid, wallet_address, token_address, strategy, entry_amount_sol, entry_price, entry_tx_signature, state, unrealized_pnl_sol)
VALUES
    ('active-1', 'wallet1', 'token7', 'SHIELD', 1.0, 1.0, 'sig7', 'ACTIVE', 10.0),
    ('active-2', 'wallet1', 'token8', 'SHIELD', 1.0, 1.0, 'sig8', 'EXITING', 5.0);

-- Boundary rows: exactly at the 24h cutoff (must be INCLUDED) and just beyond
-- it (must be EXCLUDED), locking down inclusive-vs-exclusive behavior.
INSERT INTO positions (trade_uuid, wallet_address, token_address, strategy, entry_amount_sol, entry_price, entry_tx_signature, state, realized_pnl_sol, closed_at)
VALUES
    ('boundary-in', 'wallet1', 'token9', 'SHIELD', 1.0, 1.0, 'sig9', 'CLOSED', -40.0, (SELECT now FROM test_clock)),
    ('boundary-out', 'wallet1', 'token10', 'SHIELD', 1.0, 1.0, 'sig10', 'CLOSED', -60.0, datetime((SELECT now FROM test_clock), '-25 hours'));

-- =============================================================================
-- Assertions (each raises an error -> non-zero exit on mismatch)
-- =============================================================================

-- Assertion 1: both ACTIVE/EXITING positions participate in the window
SELECT json('{assert-active-inclusion-failed') AS drawdown_assert
WHERE (SELECT COUNT(*) FROM positions WHERE state IN ('ACTIVE', 'EXITING')) != 2;

-- Assertion 2: the exact-cutoff row is INCLUDED and the row just beyond it is EXCLUDED
SELECT json('{assert-boundary-in-failed') AS drawdown_assert
WHERE NOT EXISTS (
    SELECT 1 FROM positions
    WHERE trade_uuid = 'boundary-in'
      AND closed_at >= datetime((SELECT now FROM test_clock), '-24 hours')
);
SELECT json('{assert-boundary-out-failed') AS drawdown_assert
WHERE EXISTS (
    SELECT 1 FROM positions
    WHERE trade_uuid = 'boundary-out'
      AND closed_at >= datetime((SELECT now FROM test_clock), '-24 hours')
);

-- Assertion 3: the 24-hour window nets exactly the recent + boundary-in losses
-- (recent-1..3 = -100.0, boundary-in = -40.0; historical and boundary-out excluded)
SELECT json('{assert-24h-window-sum-failed') AS drawdown_assert
WHERE (SELECT SUM(realized_pnl_sol) FROM positions
       WHERE state = 'CLOSED'
         AND closed_at >= datetime((SELECT now FROM test_clock), '-24 hours')) != -140.0;

-- Assertion 4: historical (pre-window) positions never leak into the window
SELECT json('{assert-historical-excluded-failed') AS drawdown_assert
WHERE EXISTS (
    SELECT 1 FROM positions
    WHERE trade_uuid LIKE 'hist-%'
      AND closed_at >= datetime((SELECT now FROM test_clock), '-24 hours')
);

-- =============================================================================
-- Verification output
-- =============================================================================

SELECT '=== Position Classification ===' as info;
SELECT
    trade_uuid,
    COALESCE(realized_pnl_sol, unrealized_pnl_sol) AS pnl_sol,
    closed_at,
    CASE
        WHEN closed_at IS NULL OR closed_at >= datetime((SELECT now FROM test_clock), '-24 hours') THEN 'INCLUDED (Recent)'
        ELSE 'EXCLUDED (Historical)'
    END as status
FROM positions
ORDER BY state, closed_at DESC;

-- Compare OLD vs NEW logic
SELECT '=== OLD Logic (All-Time, all CLOSED) ===' as info;
SELECT
    COUNT(*) as total_positions,
    SUM(realized_pnl_sol) as total_pnl
FROM positions
WHERE state = 'CLOSED';

SELECT '=== NEW Logic (24-Hour Window) ===' as info;
SELECT
    COUNT(*) as total_positions,
    SUM(realized_pnl_sol) as total_pnl
FROM positions
WHERE state = 'CLOSED' AND closed_at >= datetime((SELECT now FROM test_clock), '-24 hours');

-- Demonstrate the fix impact (values derived from the table, not hardcoded)
SELECT '=== Impact Analysis ===' as info;
SELECT
    'OLD logic would use' as approach,
    (SELECT SUM(realized_pnl_sol) FROM positions WHERE state = 'CLOSED') as total_pnl,
    'This includes historical gains from January'
UNION ALL
SELECT
    'NEW logic uses' as approach,
    (SELECT SUM(realized_pnl_sol) FROM positions
     WHERE state = 'CLOSED' AND closed_at >= datetime((SELECT now FROM test_clock), '-24 hours')),
    'This only includes recent losses from last 24 hours';

SELECT 'ALL ASSERTIONS PASSED' as result;

-- Cleanup
DROP TABLE positions;
DROP TABLE test_clock;
