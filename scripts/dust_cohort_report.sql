-- Hydra Day-14 dust-cohort promotion report (Phase C).
--
-- Judges the DUST_LIVE lane against the pre-registered promotion thresholds
-- (docs/profitability-gates.md Hydra appendix). A passing cohort still needs
-- the full 8-gate GO (n>=60) before full-live sizing; a failing cohort parks
-- the strategy (no scale-up).
--
-- Scope notes (follow the code, not the doc):
--   * Lane predicate is dr.trade_mode / t.trade_mode = 'DUST_LIVE' (0027).
--     Pre-0027 rows default to 'PAPER' and never leak into this cohort.
--   * Side convention is t.side = 'BUY' (profitability.rs, conversion_funnel).
--   * Outcome sample mirrors fetch_outcomes: admitted BUY + CLOSED + valid PnL
--     + non-null net/size. Window is 14 days on closed_at (dust window), not
--     the 30-day decided_at verdict window.
--
-- Run on the production server:
--   docker exec -i chimera-postgres psql -U chimera -d chimera < scripts/dust_cohort_report.sql

\echo '=== Dust cohort outcomes (14d, DUST_LIVE only) ==='
WITH sample AS (
    SELECT (t.net_pnl_sol / NULLIF(dr.size_sol, 0)) AS ret
    FROM decision_records dr
    JOIN trades t ON t.trade_uuid = dr.trade_uuid
    WHERE dr.trade_mode = 'DUST_LIVE'
      AND dr.admitted = TRUE
      AND dr.action = 'BUY'
      AND t.trade_mode = 'DUST_LIVE'
      AND t.status = 'CLOSED'
      AND t.pnl_data_valid = TRUE
      AND t.side = 'BUY'
      AND t.net_pnl_sol IS NOT NULL
      AND dr.size_sol IS NOT NULL
      AND dr.size_sol <> 0
      AND t.closed_at > NOW() - INTERVAL '14 days'
),
agg AS (
    SELECT COUNT(*) AS n,
           AVG(ret) AS mean_ret,
           STDDEV_SAMP(ret) AS sd_ret
    FROM sample
)
SELECT n AS sample_n,
       ROUND(mean_ret::NUMERIC, 5) AS mean_net_per_trade,
       ROUND((mean_ret - 1.96 * sd_ret / SQRT(GREATEST(n, 1)))::NUMERIC, 5) AS lower_95ci,
       ROUND((mean_ret + 1.96 * sd_ret / SQRT(GREATEST(n, 1)))::NUMERIC, 5) AS upper_95ci,
       CASE WHEN n >= 30 THEN 'PASS' ELSE 'FAIL' END AS n_ge_30,
       CASE WHEN mean_ret > 0.05 THEN 'PASS' ELSE 'FAIL' END AS mean_gt_5pct,
       CASE WHEN (mean_ret - 1.96 * sd_ret / SQRT(GREATEST(n, 1))) > 0 THEN 'PASS' ELSE 'FAIL' END AS lower_ci_gt_0
FROM agg;

\echo ''
\echo '=== Dust funnel: admitted -> closed (14d decided_at) ==='
SELECT COUNT(*) FILTER (WHERE dr.admitted) AS admitted_buys,
       COUNT(*) FILTER (WHERE dr.admitted AND dr.trade_uuid IS NOT NULL AND EXISTS (
           SELECT 1 FROM trades t WHERE t.trade_uuid = dr.trade_uuid AND t.status = 'CLOSED'
             AND t.pnl_data_valid = TRUE AND t.side = 'BUY'
       )) AS closed_with_pnl,
       ROUND(100.0 *
         COUNT(*) FILTER (WHERE dr.admitted AND dr.trade_uuid IS NOT NULL AND EXISTS (
             SELECT 1 FROM trades t WHERE t.trade_uuid = dr.trade_uuid AND t.status = 'CLOSED'
               AND t.pnl_data_valid = TRUE AND t.side = 'BUY'
         )) / NULLIF(COUNT(*) FILTER (WHERE dr.admitted), 0), 1) AS fill_rate_pct,
       CASE WHEN ROUND(100.0 *
         COUNT(*) FILTER (WHERE dr.admitted AND dr.trade_uuid IS NOT NULL AND EXISTS (
             SELECT 1 FROM trades t WHERE t.trade_uuid = dr.trade_uuid AND t.status = 'CLOSED'
               AND t.pnl_data_valid = TRUE AND t.side = 'BUY'
         )) / NULLIF(COUNT(*) FILTER (WHERE dr.admitted), 0), 1) >= 80.0 THEN 'PASS' ELSE 'FAIL' END AS fill_ge_80pct
FROM decision_records dr
WHERE dr.trade_mode = 'DUST_LIVE'
  AND dr.action = 'BUY'
  AND dr.decided_at > NOW() - INTERVAL '14 days';

\echo ''
\echo '=== Dust integrity: missing / invalid / unpriced (must all be 0) ==='
SELECT
    (SELECT COUNT(*)
     FROM decision_records dr
     WHERE dr.trade_mode = 'DUST_LIVE'
       AND dr.admitted = TRUE
       AND dr.action = 'BUY'
       AND dr.decided_at > NOW() - INTERVAL '14 days'
       AND NOT EXISTS (
           SELECT 1 FROM trades t
           WHERE t.trade_uuid = dr.trade_uuid
             AND t.status = 'CLOSED'
             AND t.pnl_data_valid = TRUE
             AND t.side = 'BUY'
       )
       AND (
           (dr.trade_uuid IS NULL AND dr.decided_at < NOW() - INTERVAL '3 days')
           OR EXISTS (
               SELECT 1 FROM trades t
               WHERE t.trade_uuid = dr.trade_uuid
                 AND (t.status IN ('DEAD_LETTER', 'REJECTED')
                      OR (t.status = 'FAILED' AND t.updated_at < NOW() - INTERVAL '3 days'))
           )
       )) AS missing_outcomes,
    (SELECT COUNT(*)
     FROM decision_records dr
     JOIN trades t ON t.trade_uuid = dr.trade_uuid
     WHERE dr.trade_mode = 'DUST_LIVE'
       AND dr.admitted = TRUE
       AND dr.action = 'BUY'
       AND t.pnl_data_valid = FALSE
       AND t.closed_at > NOW() - INTERVAL '14 days') AS invalid_pnl,
    (SELECT COUNT(*)
     FROM trades t
     WHERE t.trade_mode = 'DUST_LIVE'
       AND t.status = 'CLOSED'
       AND t.side = 'BUY'
       AND t.net_pnl_sol IS NULL
       AND t.closed_at > NOW() - INTERVAL '14 days') AS unpriced_exits;

\echo ''
\echo '=== PROMOTION VERDICT (all five must read PROMOTE) ==='
WITH sample AS (
    SELECT (t.net_pnl_sol / NULLIF(dr.size_sol, 0)) AS ret
    FROM decision_records dr
    JOIN trades t ON t.trade_uuid = dr.trade_uuid
    WHERE dr.trade_mode = 'DUST_LIVE'
      AND dr.admitted = TRUE
      AND dr.action = 'BUY'
      AND t.trade_mode = 'DUST_LIVE'
      AND t.status = 'CLOSED'
      AND t.pnl_data_valid = TRUE
      AND t.side = 'BUY'
      AND t.net_pnl_sol IS NOT NULL
      AND dr.size_sol IS NOT NULL
      AND dr.size_sol <> 0
      AND t.closed_at > NOW() - INTERVAL '14 days'
),
agg AS (
    SELECT COUNT(*) AS n,
           AVG(ret) AS mean_ret,
           STDDEV_SAMP(ret) AS sd_ret
    FROM sample
),
funnel AS (
    SELECT COUNT(*) FILTER (WHERE dr.admitted) AS admitted,
           COUNT(*) FILTER (WHERE dr.admitted AND dr.trade_uuid IS NOT NULL AND EXISTS (
               SELECT 1 FROM trades t WHERE t.trade_uuid = dr.trade_uuid AND t.status = 'CLOSED'
                 AND t.pnl_data_valid = TRUE AND t.side = 'BUY'
           )) AS closed
    FROM decision_records dr
    WHERE dr.trade_mode = 'DUST_LIVE'
      AND dr.action = 'BUY'
      AND dr.decided_at > NOW() - INTERVAL '14 days'
),
integrity AS (
    SELECT
        (SELECT COUNT(*)
         FROM decision_records dr
         WHERE dr.trade_mode = 'DUST_LIVE'
           AND dr.admitted = TRUE
           AND dr.action = 'BUY'
           AND dr.decided_at > NOW() - INTERVAL '14 days'
           AND NOT EXISTS (
               SELECT 1 FROM trades t
               WHERE t.trade_uuid = dr.trade_uuid
                 AND t.status = 'CLOSED'
                 AND t.pnl_data_valid = TRUE
                 AND t.side = 'BUY'
           )
           AND (
               (dr.trade_uuid IS NULL AND dr.decided_at < NOW() - INTERVAL '3 days')
               OR EXISTS (
                   SELECT 1 FROM trades t
                   WHERE t.trade_uuid = dr.trade_uuid
                     AND (t.status IN ('DEAD_LETTER', 'REJECTED')
                          OR (t.status = 'FAILED' AND t.updated_at < NOW() - INTERVAL '3 days'))
               )
           )) AS missing,
        (SELECT COUNT(*)
         FROM decision_records dr
         JOIN trades t ON t.trade_uuid = dr.trade_uuid
         WHERE dr.trade_mode = 'DUST_LIVE'
           AND dr.admitted = TRUE
           AND dr.action = 'BUY'
           AND t.pnl_data_valid = FALSE
           AND t.closed_at > NOW() - INTERVAL '14 days') AS invalid,
        (SELECT COUNT(*)
         FROM trades t
         WHERE t.trade_mode = 'DUST_LIVE'
           AND t.status = 'CLOSED'
           AND t.side = 'BUY'
           AND t.net_pnl_sol IS NULL
           AND t.closed_at > NOW() - INTERVAL '14 days') AS unpriced
)
SELECT CASE WHEN (SELECT n FROM agg) >= 30 THEN 'PROMOTE' ELSE 'PARK' END AS sample_n_ge_30,
       CASE WHEN (SELECT mean_ret FROM agg) > 0.05 THEN 'PROMOTE' ELSE 'PARK' END AS mean_gt_5pct,
       CASE WHEN (SELECT mean_ret - 1.96 * sd_ret / SQRT(GREATEST(n, 1)) FROM agg) > 0
            THEN 'PROMOTE' ELSE 'PARK' END AS lower_ci_gt_0,
       CASE WHEN ROUND(100.0 * (SELECT closed FROM funnel)
                       / NULLIF((SELECT admitted FROM funnel), 0), 1) >= 80.0
            THEN 'PROMOTE' ELSE 'PARK' END AS fill_ge_80pct,
       CASE WHEN (SELECT missing + invalid + unpriced FROM integrity) = 0
            THEN 'PROMOTE' ELSE 'PARK' END AS zero_missing_invalid_unpriced;
