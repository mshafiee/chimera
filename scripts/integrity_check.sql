-- Integrity monitor: missing-outcome rate by decision day.
-- Mirrors the count_missing_outcomes semantics (terminal-dead only, in-flight
-- excluded). Run on the production server:
--   docker exec -i chimera-postgres psql -U chimera -d chimera < scripts/integrity_check.sql
--
-- The verdict's integrity gate is meant to track NEW missing outcomes. The
-- historical Aug-2026 batch (admitted decisions with no trade row) rolls off
-- the 30-day window on its own; watch the right-hand column (new missing per
-- day) staying near zero as the health signal.

\echo '=== Missing-outcome rate by decision day (terminal-dead only, 30d window) ==='
SELECT date_trunc('day', dr.decided_at)::date AS day,
       COUNT(*) FILTER (WHERE dr.admitted) AS admitted_buys,
       -- terminal-dead WITHOUT a valid closed BUY (mirrors count_missing_outcomes)
       COUNT(*) FILTER (
           WHERE dr.admitted
             AND NOT EXISTS (
                 SELECT 1 FROM trades t
                 WHERE t.trade_uuid = dr.trade_uuid
                   AND t.status = 'CLOSED' AND t.pnl_data_valid = TRUE AND t.side = 'BUY'
             )
             AND (
                 (dr.trade_uuid IS NULL AND dr.decided_at < NOW() - MAKE_INTERVAL(days => 3))
                 OR EXISTS (
                     SELECT 1 FROM trades t
                     WHERE t.trade_uuid = dr.trade_uuid
                       AND (t.status IN ('DEAD_LETTER','REJECTED')
                            OR (t.status = 'FAILED' AND t.updated_at < NOW() - MAKE_INTERVAL(days => 3)))
                 )
             )
       ) AS missing_outcomes
FROM decision_records dr
WHERE dr.decided_at > NOW() - INTERVAL '30 days'
GROUP BY 1
ORDER BY 1 DESC;

\echo ''
\echo '=== In-flight admissions (should NOT count as missing) ==='
SELECT COALESCE(t.status, 'NO_TRADE_ROW') AS status, COUNT(*)
FROM decision_records dr LEFT JOIN trades t ON t.trade_uuid = dr.trade_uuid
WHERE dr.admitted = TRUE AND dr.action = 'BUY'
  AND dr.decided_at > NOW() - INTERVAL '30 days'
GROUP BY 1 ORDER BY 2 DESC;
