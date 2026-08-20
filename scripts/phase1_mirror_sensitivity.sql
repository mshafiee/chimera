-- Phase 1 sensitivity: how many tokens clear candidate min_avg_pct thresholds
-- at recommended (min_samples, window) combos, on deduped shadow data.
WITH windows AS (
    SELECT 48 AS wh UNION ALL SELECT 72 UNION ALL SELECT 168
),
bucketed AS (
    SELECT DISTINCT ON (sp.token_address, sp.wallet_address, w.wh, date_trunc('hour', sp.opened_at))
           sp.token_address AS token, w.wh AS wh, se.pnl_pct
    FROM shadow_exits se
    JOIN shadow_positions sp ON sp.shadow_id = se.shadow_id
    CROSS JOIN windows w
    WHERE se.exit_strategy = 'mirror_main'
      AND se.exit_reason IS DISTINCT FROM 'no_price'
      AND sp.opened_at > NOW() - make_interval(hours => w.wh)
    ORDER BY sp.token_address, sp.wallet_address, w.wh, date_trunc('hour', sp.opened_at), sp.opened_at
),
per_token AS (
    SELECT token, wh, count(*) AS n, avg(pnl_pct) AS avg_pnl
    FROM bucketed GROUP BY token, wh
)
SELECT wh,
  count(*) FILTER (WHERE n >= 3 AND avg_pnl >= 1.5) AS s3_a15,
  count(*) FILTER (WHERE n >= 3 AND avg_pnl >= 1.4) AS s3_a14,
  count(*) FILTER (WHERE n >= 5 AND avg_pnl >= 1.5) AS s5_a15,
  count(*) FILTER (WHERE n >= 5 AND avg_pnl >= 1.4) AS s5_a14,
  count(*) FILTER (WHERE n >= 3 AND avg_pnl >= 0.0) AS s3_nonneg
FROM per_token GROUP BY wh ORDER BY wh;
