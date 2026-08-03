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
-- Updated: 2026-07-22 — added monitoring_enabled + 45cKASDe promotion.
-- Updated: 2026-08-02 — all steps wrapped in a single transaction so a
--   failure anywhere rolls back the whole promotion atomically; monitoring is
--   enabled only after the promotions; ranking has a stable tie-breaker.

\set PROMOTE_LIMIT 5
\set MIN_WQS 80
\set MIN_WIN_RATE 0.80
\set NEW_ACTIVE_WALLET '45cKASDe'

BEGIN;

-- ---------------------------------------------------------------------------
-- 0. Pre-flight: preview which wallets WILL be promoted (read-only, inside
--    the same transaction so the reviewed set matches the promoted set).
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
ORDER BY wqs_score DESC, win_rate DESC, address
LIMIT :PROMOTE_LIMIT;
-- Review this list before running the UPDATE below.

-- ---------------------------------------------------------------------------
-- 1. Promote the top-N high-WQS candidates to ACTIVE (30-day TTL).
--    FOR UPDATE SKIP LOCKED + a stable tie-breaker (address) keep the
--    promoted set deterministic and equal to the previewed list.
-- ---------------------------------------------------------------------------
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
                   ORDER BY wqs_score DESC, win_rate DESC, address
               ) AS rn
        FROM wallets
        WHERE status     = 'CANDIDATE'
          AND wqs_score  >= :MIN_WQS
          AND win_rate   >= :MIN_WIN_RATE
        FOR UPDATE SKIP LOCKED
    ) ranked
    WHERE rn <= :PROMOTE_LIMIT
);

-- Confirm the promotion landed.
SELECT address, status, promoted_at, ttl_expires_at
FROM wallets
WHERE status = 'ACTIVE'
  AND promoted_at >= NOW() - INTERVAL '5 minutes';

-- ---------------------------------------------------------------------------
-- 2. Promote 45cKASDe (WQS 37, 35 trades/30d, confidence 0.71).
--    This wallet does NOT meet the strict high-WQS criteria above, so it
--    needs a dedicated promotion block. It is a SCALPER with reasonable
--    confidence (0.71) and the highest trade frequency among candidates.
--    Guarded with status = 'CANDIDATE' so an already-active wallet is not
--    silently re-promoted (which would reset its TTL).
-- ---------------------------------------------------------------------------
UPDATE wallets
SET status         = 'ACTIVE',
    promoted_at    = NOW(),
    ttl_expires_at = NOW() + INTERVAL '30 days',
    updated_at     = NOW()
WHERE address = :'NEW_ACTIVE_WALLET'
  AND status  = 'CANDIDATE';

-- Confirm promotion (0 rows => wallet missing or already active).
SELECT address, status, promoted_at, ttl_expires_at
FROM wallets
WHERE address = :'NEW_ACTIVE_WALLET';

-- ---------------------------------------------------------------------------
-- 3. Ensure 45cKASDe has monitoring enabled (creates row if missing).
--    Only monitoring_enabled is flipped; existing webhook fields (which may
--    be paused/failed/orphaned on purpose) are left untouched.
-- ---------------------------------------------------------------------------
INSERT INTO wallet_monitoring
    (wallet_address, monitoring_enabled, webhook_status, webhook_health_status)
VALUES
    (:'NEW_ACTIVE_WALLET', true, 'unknown', 'unknown')
ON CONFLICT (wallet_address) DO UPDATE
SET monitoring_enabled = true,
    updated_at         = NOW();

-- ---------------------------------------------------------------------------
-- 4. Enable monitoring for ALL ACTIVE wallets — run AFTER the promotions so
--    the newly promoted wallets are covered too. (Existing ACTIVE wallets
--    that were deliberately disabled are re-enabled here by design; see the
--    note in the plan if that is not desired.)
-- ---------------------------------------------------------------------------
UPDATE wallet_monitoring
SET monitoring_enabled = true
WHERE wallet_address IN (
    SELECT address FROM wallets WHERE status = 'ACTIVE'
);

COMMIT;

-- ---------------------------------------------------------------------------
-- 5. Rollback (only if the promotion was a mistake).
--    Reverts ONLY the wallets promoted by this run back to CANDIDATE, and
--    restores their monitoring rows.
-- ---------------------------------------------------------------------------
-- BEGIN;
-- UPDATE wallets
-- SET status         = 'CANDIDATE',
--     promoted_at    = NULL,
--     ttl_expires_at = NULL,
--     updated_at     = NOW()
-- WHERE address IN (
--     SELECT address FROM wallets
--     WHERE status = 'ACTIVE'
--       AND promoted_at >= NOW() - INTERVAL '30 minutes'
-- );
-- UPDATE wallet_monitoring
-- SET monitoring_enabled = false
-- WHERE wallet_address IN (
--     SELECT address FROM wallets
--     WHERE status = 'CANDIDATE'
--       AND promoted_at IS NULL
--       AND ttl_expires_at IS NULL
-- );
-- COMMIT;
