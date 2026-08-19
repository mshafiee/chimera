-- Rejection funnel: which gate rejects the most, and was it right?
-- Uses the shadow trader's counterfactual data (migration 0015_shadow_trader.sql),
-- which records a mirror_main PnL for EVERY signal the main system rejected.
--
-- Run on the production server:
--   docker exec -i chimera-postgres psql -U chimera -d chimera < scripts/rejection_funnel.sql

\echo '=== Per-gate signal volume + counterfactual PnL (mirror_main) ==='
SELECT gate,
       signal_count,
       winners,
       losers,
       ROUND(winners::NUMERIC / NULLIF(signal_count, 0) * 100, 1) AS win_pct,
       ROUND(avg_pnl_pct, 3)  AS avg_pnl_pct,
       ROUND(total_pnl_sol::NUMERIC, 4) AS total_pnl_sol
FROM   shadow_summary_by_gate
WHERE  exit_strategy = 'mirror_main'
ORDER  BY signal_count DESC;

\echo ''
\echo '=== Lost profit vs correct rejection per gate (what we gave up / dodged) ==='
SELECT main_rejection_code                            AS gate,
       COUNT(*)                                       AS signals,
       COUNT(*) FILTER (WHERE classification = 'lost_profit')     AS lost_profit,
       COUNT(*) FILTER (WHERE classification = 'correct_rejection') AS correct_rej,
       ROUND(SUM(pnl_sol)::NUMERIC, 4)               AS net_pnl_sol_if_admitted
FROM   shadow_comparison
WHERE  exit_strategy = 'mirror_main'
  AND  main_admitted = FALSE
GROUP  BY main_rejection_code
ORDER  BY net_pnl_sol_if_admitted DESC;

\echo ''
\echo '=== WINSORIZED counterfactual (outlier-capped ±100%, per-signal) ==='
\echo '-- Do NOT loosen gates on raw total_pnl_sol. This view caps each exit at ±100%'
\echo '-- and normalizes by entry size, so moon-shot outliers (e.g. +3,852%) and any'
\echo '-- non-standard sizing cannot inflate an edge. A gate here is only worth'
\echo '-- loosening if winsorized avg_pnl_pct is clearly positive at volume. --'
WITH wins AS (
    SELECT main_rejection_code AS gate,
           entry_amount_sol AS size_sol,
           LEAST(GREATEST(pnl_pct, -100.0), 100.0) AS pnl_pct_w,
           CASE WHEN pnl_pct BETWEEN -100 AND 100 THEN pnl_sol ELSE 0 END AS pnl_sol_in_band,
           (pnl_pct > 100) AS is_moonshot,
           (pnl_pct < -50) AS is_big_loss
    FROM shadow_comparison
    WHERE exit_strategy = 'mirror_main' AND main_admitted = FALSE
)
SELECT gate,
       COUNT(*)                                  AS signals,
       COUNT(*) FILTER (WHERE pnl_sol_in_band > 0) AS wins_w,
       COUNT(*) FILTER (WHERE is_moonshot)        AS moonshots,
       COUNT(*) FILTER (WHERE is_big_loss)        AS big_losses,
       ROUND(100.0 * COUNT(*) FILTER (WHERE pnl_sol_in_band > 0) / NULLIF(COUNT(*), 0), 1) AS win_pct_w,
       ROUND(AVG(pnl_pct_w)::NUMERIC, 2)          AS avg_pnl_pct_winsorized,
       ROUND(SUM(pnl_sol_in_band)::NUMERIC, 4)    AS total_pnl_sol_winsorized,
       ROUND(AVG(size_sol)::NUMERIC, 4)           AS avg_size_sol
FROM wins
GROUP BY gate
ORDER BY total_pnl_sol_winsorized DESC;
