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
