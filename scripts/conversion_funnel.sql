-- Conversion funnel: how many admitted BUY decisions actually reach a profitable close.
--
-- This is the health metric for the "2% admission-to-close" problem. Run on the
-- production server:
--   docker exec -i chimera-postgres psql -U chimera -d chimera < scripts/conversion_funnel.sql
--
-- The funnel exposes:
--   * admitted BUYs  -> linked-to-trade -> terminal outcome (CLOSED/DEAD_LETTER/REJECTED/FAILED)
--   * real-only win rate + net PnL (the sample the profitability verdict is computed on)
-- Every metric is a deterministic count, so the funnel can be diffed run-over-run.

\echo '=== Conversion funnel by day (admitted BUYs -> terminal outcome) ==='
SELECT date_trunc('day', dr.decided_at)::date AS day,
       COUNT(*) FILTER (WHERE dr.admitted) AS admitted_buys,
       COUNT(*) FILTER (WHERE dr.admitted AND dr.trade_uuid IS NOT NULL) AS linked_to_trade,
       COUNT(*) FILTER (
           WHERE dr.admitted AND dr.trade_uuid IS NOT NULL AND EXISTS (
               SELECT 1 FROM trades t WHERE t.trade_uuid = dr.trade_uuid AND t.status = 'CLOSED'
                 AND t.pnl_data_valid = TRUE AND t.side = 'BUY'
           )
       ) AS closed_with_pnl,
       COUNT(*) FILTER (
           WHERE dr.admitted AND dr.trade_uuid IS NOT NULL AND EXISTS (
               SELECT 1 FROM trades t WHERE t.trade_uuid = dr.trade_uuid AND t.status IN ('DEAD_LETTER','REJECTED')
           )
       ) AS dead_rejected,
       COUNT(*) FILTER (
           WHERE dr.admitted AND dr.trade_uuid IS NOT NULL AND EXISTS (
               SELECT 1 FROM trades t WHERE t.trade_uuid = dr.trade_uuid AND t.status = 'FAILED'
           )
       ) AS failed,
       COUNT(*) FILTER (
           WHERE dr.admitted AND dr.trade_uuid IS NOT NULL AND EXISTS (
               SELECT 1 FROM trades t WHERE t.trade_uuid = dr.trade_uuid
                 AND t.status IN ('PENDING','QUEUED','EXECUTING','ACTIVE','EXITING','RETRY')
           )
       ) AS in_flight,
       ROUND(100.0 *
         COUNT(*) FILTER (WHERE dr.admitted AND dr.trade_uuid IS NOT NULL AND EXISTS (
             SELECT 1 FROM trades t WHERE t.trade_uuid = dr.trade_uuid AND t.status = 'CLOSED'
               AND t.pnl_data_valid = TRUE AND t.side = 'BUY'
         )) / NULLIF(COUNT(*) FILTER (WHERE dr.admitted), 0), 2) AS close_rate_pct
FROM decision_records dr
WHERE dr.decided_at > NOW() - INTERVAL '30 days'
GROUP BY 1
ORDER BY 1 DESC;

\echo ''
\echo '=== 30-day rollup ==='
SELECT COUNT(*) FILTER (WHERE dr.admitted) AS admitted_buys,
       COUNT(*) FILTER (WHERE dr.admitted AND dr.trade_uuid IS NOT NULL) AS linked,
       COUNT(*) FILTER (WHERE dr.admitted AND dr.trade_uuid IS NOT NULL AND EXISTS (
           SELECT 1 FROM trades t WHERE t.trade_uuid = dr.trade_uuid AND t.status = 'CLOSED'
             AND t.pnl_data_valid = TRUE AND t.side = 'BUY'
       )) AS closed_with_pnl,
       ROUND(100.0 *
         COUNT(*) FILTER (WHERE dr.admitted AND dr.trade_uuid IS NOT NULL AND EXISTS (
             SELECT 1 FROM trades t WHERE t.trade_uuid = dr.trade_uuid AND t.status = 'CLOSED'
               AND t.pnl_data_valid = TRUE AND t.side = 'BUY'
         )) / NULLIF(COUNT(*) FILTER (WHERE dr.admitted), 0), 2) AS close_rate_pct
FROM decision_records dr
WHERE dr.decided_at > NOW() - INTERVAL '30 days';

\echo ''
\echo '=== Real closed trades: win rate + net PnL (same sample as the verdict) ==='
SELECT strategy,
       COUNT(*) AS n,
       ROUND(100.0 * COUNT(*) FILTER (WHERE pnl_sol > 0) / NULLIF(COUNT(*), 0), 1) AS win_pct,
       ROUND(SUM(pnl_sol)::NUMERIC, 4) AS gross_pnl_sol,
       ROUND(SUM(net_pnl_sol)::NUMERIC, 4) AS net_pnl_sol,
       ROUND(AVG(net_pnl_sol)::NUMERIC, 5) AS avg_net_pnl_per_trade
FROM trades
WHERE status = 'CLOSED' AND pnl_data_valid = TRUE AND side = 'BUY'
  AND closed_at > NOW() - INTERVAL '30 days'
GROUP BY strategy
ORDER BY net_pnl_sol DESC;

\echo ''
\echo '=== DEAD_LETTER diagnosability (NULL error_message should approach 0) ==='
SELECT COUNT(*) AS dead_letter_total,
       COUNT(*) FILTER (WHERE error_message IS NULL OR error_message = '') AS null_reason,
       ROUND(100.0 * COUNT(*) FILTER (WHERE error_message IS NULL OR error_message = '')
             / NULLIF(COUNT(*), 0), 1) AS null_reason_pct,
       COUNT(*) FILTER (WHERE retry_count > 0) AS retried
FROM trades
WHERE status = 'DEAD_LETTER' AND created_at > NOW() - INTERVAL '30 days';
