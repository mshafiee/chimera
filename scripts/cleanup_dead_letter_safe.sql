-- Hydra Phase 0.2: safe DEAD_LETTER cleanup (CASCADE-aware).
-- The naive `DELETE FROM trades WHERE status='DEAD_LETTER'` silently wipes
-- `positions` (ON DELETE CASCADE) and orphans `dead_letter_queue` rows (no FK).
-- Run inside a transaction; prefer the full-reset script for paper.
-- Usage: psql $DATABASE_URL -f scripts/cleanup_dead_letter_safe.sql

BEGIN;

-- 1. Purge orphan DLQ rows for stale dead-letter trades (>48h).
DELETE FROM dead_letter_queue
WHERE trade_uuid IN (
    SELECT trade_uuid FROM trades
    WHERE status = 'DEAD_LETTER'
      AND created_at < NOW() - INTERVAL '48 HOURS'
);

-- 2. Delete child positions first (explicit, auditable) for the same cohort.
DELETE FROM positions
WHERE trade_uuid IN (
    SELECT trade_uuid FROM trades
    WHERE status = 'DEAD_LETTER'
      AND created_at < NOW() - INTERVAL '48 HOURS'
);

-- 3. Delete exit_targets for the same cohort (if table exists).
DO $$
BEGIN
    IF to_regclass('public.exit_targets') IS NOT NULL THEN
        DELETE FROM exit_targets
        WHERE trade_uuid IN (
            SELECT trade_uuid FROM trades
            WHERE status = 'DEAD_LETTER'
              AND created_at < NOW() - INTERVAL '48 HOURS'
        );
    END IF;
END $$;

-- 4. Finally delete the stale dead-letter trades.
DELETE FROM trades
WHERE status = 'DEAD_LETTER'
  AND created_at < NOW() - INTERVAL '48 HOURS';

COMMIT;
