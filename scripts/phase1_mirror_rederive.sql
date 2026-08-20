-- Phase 1: re-derive the shadow-mirror gate thresholds on DEDUPED shadow data.
-- Replicates production get_token_mirror_avg_pnl semantics exactly (one exit per
-- (wallet, hour) bucket, 'mirror_main' strategy, 'no_price' exits excluded)
-- but computed per-token so we can see how many tokens clear each candidate
-- min_samples / min_avg_pct across the current 48h window and proposed 72h/168h.

WITH windows AS (
    SELECT 48 AS wh UNION ALL SELECT 72 UNION ALL SELECT 168
),
-- Exactly one row per (token, wallet, hour-bucket, window): the earliest exit's pnl.
bucketed AS (
    SELECT DISTINCT ON (sp.token_address, sp.wallet_address, w.wh, date_trunc('hour', sp.opened_at))
           sp.token_address AS token,
           w.wh AS wh,
           se.pnl_pct
    FROM shadow_exits se
    JOIN shadow_positions sp ON sp.shadow_id = se.shadow_id
    CROSS JOIN windows w
    WHERE se.exit_strategy = 'mirror_main'
      AND se.exit_reason IS DISTINCT FROM 'no_price'
      AND sp.opened_at > NOW() - make_interval(hours => w.wh)
    ORDER BY sp.token_address, sp.wallet_address, w.wh, date_trunc('hour', sp.opened_at), sp.opened_at
),
per_token AS (
    SELECT token, wh,
           count(*) AS deduped_samples,
           avg(pnl_pct) AS avg_pnl
    FROM bucketed
    GROUP BY token, wh
)
SELECT wh,
       count(*) AS tokens_with_any_data,
       count(*) FILTER (WHERE deduped_samples >= 3) AS tokens_ge3,
       count(*) FILTER (WHERE deduped_samples >= 5) AS tokens_ge5,
       count(*) FILTER (WHERE deduped_samples >= 10) AS tokens_ge10,
       count(*) FILTER (WHERE deduped_samples >= 3 AND avg_pnl >= 1.4) AS ge3_avg_ge1_4,
       count(*) FILTER (WHERE deduped_samples >= 5 AND avg_pnl >= 1.4) AS ge5_avg_ge1_4,
       round(avg(avg_pnl) FILTER (WHERE deduped_samples >= 3)::numeric, 3) AS mean_avg_ge3,
       round(percentile_cont(0.25) WITHIN GROUP (ORDER BY avg_pnl) FILTER (WHERE deduped_samples >= 3)::numeric, 3) AS p25_avg_ge3,
       round(percentile_cont(0.50) WITHIN GROUP (ORDER BY avg_pnl) FILTER (WHERE deduped_samples >= 3)::numeric, 3) AS p50_avg_ge3
FROM per_token
GROUP BY wh
ORDER BY wh;
