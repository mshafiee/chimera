-- Backfill decision_records.trade_uuid where an unambiguous link to trades exists.
--
-- WHY: a batch of admitted BUY decisions (concentrated ~2026-08-01..08-05) never got
-- their trade_uuid recorded. They currently inflate count_missing_outcomes and are
-- invisible to the profitability outcome sample. This links them back ONLY when the
-- match is unambiguous, so we never fabricate a link.
--
-- SAFETY:
--   * Match on (wallet_address, token_address, action='BUY', strategy) with a tight
--     time window (abs(decided_at - created_at) < 120s).
--   * Only link when EXACTLY ONE trade candidate exists for the decision.
--   * Never reassign a trade that a different decision_record already owns.
--   * This is OPTIONAL. The measurement funnel (conversion_funnel.sql) is the
--     required piece; backfill only repairs history so the sample is representative.
--
-- Run on the production server (dry-run first!):
--   docker exec -i chimera-postgres psql -U chimera -d chimera < scripts/backfill_decision_trade_links.sql   (dry-run)
--   docker exec -i chimera-postgres psql -U chimera -d chimera -v DO_LINK=1 < scripts/backfill_decision_trade_links.sql

-- How many rows would be touched (dry-run) / how many are being updated.
\echo '=== Candidates with an unambiguous single trade match (admitted BUY, no trade_uuid) ==='
SELECT COUNT(*) AS linkable
FROM decision_records dr
WHERE dr.admitted = TRUE
  AND dr.action = 'BUY'
  AND dr.trade_uuid IS NULL
  AND dr.decided_at > NOW() - INTERVAL '30 days'
  AND EXISTS (
      SELECT 1 FROM trades t
      WHERE t.wallet_address = dr.wallet_address
        AND t.token_address = dr.token_address
        AND t.side = 'BUY'
        AND t.strategy = COALESCE(dr.strategy, t.strategy)
        AND abs(EXTRACT(EPOCH FROM (t.created_at - dr.decided_at))) < 120
        -- exactly one candidate for THIS decision (wallet/token/time)
        AND 1 = (
            SELECT COUNT(*) FROM trades t2
            WHERE t2.wallet_address = dr.wallet_address
              AND t2.token_address = dr.token_address
              AND t2.side = 'BUY'
              AND t2.strategy = COALESCE(dr.strategy, t2.strategy)
              AND abs(EXTRACT(EPOCH FROM (t2.created_at - dr.decided_at))) < 120
        )
        AND NOT EXISTS ( -- trade not already claimed by another decision_record
            SELECT 1 FROM decision_records dr2
            WHERE dr2.trade_uuid = t.trade_uuid::text
              AND dr2.decision_id <> dr.decision_id
        )
  );

\echo ''
\echo '=== Sample of linkable candidates ==='
SELECT left(dr.decision_id,8) AS decision, left(dr.wallet_address,6) AS w,
       left(t.trade_uuid,8) AS trade, t.status AS trade_status,
       dr.decided_at, t.created_at,
       round(EXTRACT(EPOCH FROM (t.created_at - dr.decided_at))::numeric,1) AS dt_secs
FROM decision_records dr
JOIN trades t
  ON t.wallet_address = dr.wallet_address
 AND t.token_address = dr.token_address
 AND t.side = 'BUY'
 AND t.strategy = COALESCE(dr.strategy, t.strategy)
 AND abs(EXTRACT(EPOCH FROM (t.created_at - dr.decided_at))) < 120
 AND 1 = (
     SELECT COUNT(*) FROM trades t2
     WHERE t2.wallet_address = dr.wallet_address
       AND t2.token_address = dr.token_address
       AND t2.side = 'BUY'
       AND t2.strategy = COALESCE(dr.strategy, t2.strategy)
       AND abs(EXTRACT(EPOCH FROM (t2.created_at - dr.decided_at))) < 120
 )
 AND NOT EXISTS (
     SELECT 1 FROM decision_records dr2
     WHERE dr2.trade_uuid = t.trade_uuid::text AND dr2.decision_id <> dr.decision_id
 )
WHERE dr.admitted = TRUE AND dr.action = 'BUY' AND dr.trade_uuid IS NULL
  AND dr.decided_at > NOW() - INTERVAL '30 days'
ORDER BY dr.decided_at DESC LIMIT 10;

\echo ''
\echo '=== Apply the backfill (define DO_LINK to execute, e.g. -v DO_LINK=1) ==='
\if :{?DO_LINK}
    BEGIN;
    UPDATE decision_records dr
    SET trade_uuid = (
        SELECT t.trade_uuid::text
        FROM trades t
        WHERE t.wallet_address = dr.wallet_address
          AND t.token_address = dr.token_address
          AND t.side = 'BUY'
          AND t.strategy = COALESCE(dr.strategy, t.strategy)
          AND abs(EXTRACT(EPOCH FROM (t.created_at - dr.decided_at))) < 120
          AND 1 = (
              SELECT COUNT(*) FROM trades t2
              WHERE t2.wallet_address = dr.wallet_address
                AND t2.token_address = dr.token_address
                AND t2.side = 'BUY'
                AND t2.strategy = COALESCE(dr.strategy, t2.strategy)
                AND abs(EXTRACT(EPOCH FROM (t2.created_at - dr.decided_at))) < 120
          )
          AND NOT EXISTS (
              SELECT 1 FROM decision_records dr2
              WHERE dr2.trade_uuid = t.trade_uuid::text AND dr2.decision_id <> dr.decision_id
          )
        LIMIT 1
    )
    WHERE dr.admitted = TRUE AND dr.action = 'BUY' AND dr.trade_uuid IS NULL
      AND dr.decided_at > NOW() - INTERVAL '30 days'
      AND EXISTS (
          SELECT 1 FROM trades t
          WHERE t.wallet_address = dr.wallet_address
            AND t.token_address = dr.token_address
            AND t.side = 'BUY'
            AND t.strategy = COALESCE(dr.strategy, t.strategy)
            AND abs(EXTRACT(EPOCH FROM (t.created_at - dr.decided_at))) < 120
      );
    COMMIT;
    \echo 'Backfill applied.'
\else
    \echo 'Dry-run only. Re-run with -v DO_LINK=1 to apply.'
\endif
