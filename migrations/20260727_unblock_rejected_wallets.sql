-- Migration: Unblock REJECTED wallets with high WQS scores
-- Purpose: Promote wallets back to CANDIDATE status so they can be re-validated
--            after removing the SCALPER forbidden archetype filter
--
-- Scoped to the SCALPER archetype only: wallets rejected for other reasons
-- (manual review, fraud, other archetypes) must NOT be promoted.
--
-- Atomicity: the whole file is wrapped in a single transaction so the row-count
-- assertion and the UPDATE either commit together or roll back together. The
-- expected count is asserted BEFORE mutating (counting after the UPDATE was
-- both non-atomic across statements and could be polluted by unrelated rows).
-- A re-run in an already-migrated DB finds 0 matches and aborts loudly rather
-- than silently doing nothing — intended for a one-off prod data fix.

BEGIN;

DO $$
DECLARE
    matched_count INTEGER;
    expected_count CONSTANT INTEGER := 2;  -- both SCALPER wallets, WQS=108.8
BEGIN
    SELECT COUNT(*) INTO matched_count
    FROM wallets
    WHERE status = 'REJECTED'
      AND archetype = 'SCALPER'
      AND wqs_score >= 70;

    IF matched_count <> expected_count THEN
        RAISE EXCEPTION
            'Aborting unblock migration: expected % matching REJECTED SCALPER wallets (WQS >= 70), found %. Transaction rolled back.',
            expected_count, matched_count;
    END IF;
END $$;

UPDATE wallets
SET status = 'CANDIDATE',
    updated_at = CURRENT_TIMESTAMP,
    notes = COALESCE(notes, '') || ' | Re-promoted from REJECTED after SCALPER filter removed'
WHERE status = 'REJECTED'
  AND archetype = 'SCALPER'
  AND wqs_score >= 70;

COMMIT;
