-- Migration: Unblock REJECTED wallets with high WQS scores
-- Purpose: Promote wallets back to CANDIDATE status so they can be re-validated
--            after removing the SCALPER forbidden archetype filter

UPDATE wallets
SET status = 'CANDIDATE',
    notes = notes || ' | Re-promoted from REJECTED after SCALPER filter removed'
WHERE status = 'REJECTED'
  AND wqs_score >= 70;

-- Verify the migration affected 2 rows (as expected)
-- Expected: 2 wallets (both SCALPER, WQS=108.8)
SELECT COUNT(*) as affected_rows
FROM wallets
WHERE status = 'CANDIDATE'
  AND notes LIKE '%Re-promoted from REJECTED%';
