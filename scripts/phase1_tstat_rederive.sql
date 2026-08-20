-- Phase 1 (item 2): re-derive the wallet t-stat gate on DEDUPED shadow data for
-- the ACTIVE roster. Replicates production get_wallet_pnl_statistics semantics
-- (one exit per (token, strategy, hour), 'mirror_main'+'dune_wallet', no_price
-- excluded, 30d window) and computes each wallet's t-statistic to see how many
-- clear the 1.645 threshold at candidate min_samples 3/5/10.

WITH dedup AS (
    SELECT DISTINCT ON (sp.token_address, se.exit_strategy, date_trunc('hour', sp.opened_at))
           sp.wallet_address AS wallet,
           se.pnl_pct
    FROM shadow_exits se
    JOIN shadow_positions sp ON sp.shadow_id = se.shadow_id
    WHERE sp.wallet_address IN (SELECT address FROM wallets WHERE status = 'ACTIVE')
      AND se.exit_strategy IN ('mirror_main', 'dune_wallet')
      AND se.exit_reason IS DISTINCT FROM 'no_price'
      AND sp.opened_at > NOW() - interval '30 days'
    ORDER BY sp.token_address, se.exit_strategy, date_trunc('hour', sp.opened_at), sp.opened_at
),
stats AS (
    SELECT wallet,
           count(*) AS n,
           avg(pnl_pct) AS mean,
           stddev(pnl_pct) AS sd
    FROM dedup
    GROUP BY wallet
),
tstat AS (
    SELECT wallet, n, mean, sd,
           CASE WHEN sd > 0 AND n > 0 THEN mean / (sd / sqrt(n::numeric)) ELSE NULL END AS t
    FROM stats
)
SELECT
  count(*) FILTER (WHERE n >= 3 AND t > 1.645) AS ge3_tgt1645,
  count(*) FILTER (WHERE n >= 5 AND t > 1.645) AS ge5_tgt1645,
  count(*) FILTER (WHERE n >= 10 AND t > 1.645) AS ge10_tgt1645,
  count(*) FILTER (WHERE n >= 3) AS wallets_ge3,
  count(*) FILTER (WHERE n >= 5) AS wallets_ge5,
  count(*) FILTER (WHERE n >= 10) AS wallets_ge10
FROM tstat;

\echo '=== active wallet count ==='
SELECT count(*) AS active_wallets FROM wallets WHERE status = 'ACTIVE';
