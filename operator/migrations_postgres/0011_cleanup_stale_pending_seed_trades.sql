-- Cleanup stuck PENDING seed-trade-init trades
-- These are initialization artifacts (SOL-to-SOL trades) that were never executed.
-- Run on production after deploying the stale trade reaper or manually before.
-- After verifying no ACTIVE positions reference these trades, they should be CANCELLED.

-- Verify no ACTIVE positions reference these trades first:
-- SELECT COUNT(*) FROM positions
-- WHERE trade_uuid IN (
--     SELECT trade_uuid FROM trades WHERE tx_signature = 'seed-trade-init'
-- ) AND state = 'ACTIVE';

UPDATE trades
SET status = 'CANCELLED',
    updated_at = NOW()
WHERE status = 'PENDING'
  AND tx_signature = 'seed-trade-init';