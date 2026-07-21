-- Wallet promotion for the profitability overhaul (commits 305b7bc + 4534614).
--
-- Run manually against the production PostgreSQL DB after the operator deploy.
-- This is a one-off operational DML, NOT a schema migration — do NOT add it to
-- the auto-applied migration directories.
--
-- WHY ORDER BY wqs_score DESC (not profit_factor):
--   The target wallets typically have profit_factor IS NULL (logged as
--   "backtest: No trades"). Under ORDER BY profit_factor DESC NULLS LAST they
--   rank last and are never selected. wqs_score is always populated, so it is
--   the reliable ranking key here.
--
-- Context: .kilo/plans/1784670196734-profitability-remediation.md (Task 3).

\set PROMOTE_LIMIT 5
\set MIN_WQS 80
\set MIN_WIN_RATE 0.80

-- ---------------------------------------------------------------------------
-- 0. Pre-flight: preview which wallets WILL be promoted (read-only).
-- ---------------------------------------------------------------------------
SELECT address,
       wqs_score,
       win_rate,
       profit_factor,
       status
FROM wallets
WHERE status = 'CANDIDATE'
  AND wqs_score >= :MIN_WQS
  AND win_rate  >= :MIN_WIN_RATE
ORDER BY wqs_score DESC, win_rate DESC
LIMIT :PROMOTE_LIMIT;
-- Review this list before running the UPDATE below.

-- ---------------------------------------------------------------------------
-- 1. Promote the top-N high-WQS candidates to ACTIVE (30-day TTL).
-- ---------------------------------------------------------------------------
BEGIN;

UPDATE wallets
SET status         = 'ACTIVE',
    promoted_at    = NOW(),
    ttl_expires_at = NOW() + INTERVAL '30 days',
    updated_at     = NOW()
WHERE address IN (
    SELECT address
    FROM (
        SELECT address,
               ROW_NUMBER() OVER (
                   ORDER BY wqs_score DESC, win_rate DESC
               ) AS rn
        FROM wallets
        WHERE status     = 'CANDIDATE'
          AND wqs_score  >= :MIN_WQS
          AND win_rate   >= :MIN_WIN_RATE
    ) ranked
    WHERE rn <= :PROMOTE_LIMIT
);

-- Confirm the promotion landed.
SELECT address, status, promoted_at, ttl_expires_at
FROM wallets
WHERE status = 'ACTIVE'
  AND promoted_at >= NOW() - INTERVAL '5 minutes';

COMMIT;

-- ---------------------------------------------------------------------------
-- 2. Rollback (only if the promotion was a mistake).
--    Reverts the wallets promoted in this run back to CANDIDATE.
-- ---------------------------------------------------------------------------
-- BEGIN;
-- UPDATE wallets
-- SET status         = 'CANDIDATE',
--     promoted_at    = NULL,
--     ttl_expires_at = NULL,
--     updated_at     = NOW()
-- WHERE status      = 'ACTIVE'
--   AND promoted_at >= NOW() - INTERVAL '30 minutes';
-- COMMIT;
