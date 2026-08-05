#!/bin/bash
# Chimera Daily Shadow & Trading Report
# Run daily (e.g. 08:00 UTC) to review wallet quality, trade outcomes,
# blacklist state, and Dune cohort performance.
#
# Usage: ops/daily_report.sh
# Requires: docker access to the chimera-postgres container.

set -uo pipefail

PSQL() {
  docker exec chimera-postgres psql -U chimera -d chimera -t -A -c "$1" 2>/dev/null
}
PSQL_HEAD() {
  docker exec chimera-postgres psql -U chimera -d chimera -c "$1" 2>/dev/null
}

echo "================================================================"
echo " CHIMERA DAILY REPORT — $(date -u '+%Y-%m-%d %H:%M UTC')"
echo "================================================================"

echo ""
echo "== TRADES (last 24h) =="
PSQL_HEAD "
SELECT status, count(*) AS trades,
       round(100.0 * sum(CASE WHEN pnl_sol > 0 THEN 1 ELSE 0 END) / NULLIF(count(*), 0), 1) AS win_pct,
       round(sum(pnl_sol), 4) AS total_pnl_sol
FROM trades
WHERE created_at > NOW() - INTERVAL '24 hours'
GROUP BY status ORDER BY trades DESC;"

echo ""
echo "== CLOSED TRADES DETAIL (last 24h) =="
PSQL_HEAD "
SELECT left(t.wallet_address, 10) AS wallet, substr(t.token_address, 1, 10) AS token,
       t.strategy,
       round((t.pnl_sol / NULLIF(t.amount_sol, 0) * 100), 1) AS pnl_pct,
       round(EXTRACT(EPOCH FROM (COALESCE(t.closed_at, NOW()) - t.created_at)) / 60, 0) AS hold_min,
       to_char(t.created_at, 'MM-DD HH24:MI') AS opened
FROM trades t
WHERE t.status = 'CLOSED' AND t.created_at > NOW() - INTERVAL '24 hours'
ORDER BY t.created_at DESC LIMIT 15;"

echo ""
echo "== SHADOW LEADERBOARD (7d, admitted DEX signals, top 15 by total PnL) =="
PSQL_HEAD "
SELECT sp.wallet_address AS wallet, se.exit_strategy,
       count(*) AS exits,
       round(100.0 * sum(CASE WHEN se.pnl_sol > 0 THEN 1 ELSE 0 END) / count(*), 1) AS win_pct,
       round(avg(se.pnl_pct), 2) AS avg_pnl_pct,
       round(sum(se.pnl_sol), 3) AS total_pnl_sol
FROM shadow_exits se
JOIN shadow_positions sp ON sp.shadow_id = se.shadow_id
WHERE sp.opened_at > NOW() - INTERVAL '7 days'
  AND sp.token_address NOT LIKE '%pump'
  AND sp.main_admitted = true
GROUP BY 1, 2
HAVING count(*) >= 5
ORDER BY total_pnl_sol DESC LIMIT 15;"

echo ""
echo "== WALLET QUALITY: stop-loss rate & best strategy (48h, admitted DEX) =="
PSQL_HEAD "
SELECT left(sp.wallet_address, 10) AS wallet, count(*) AS n,
       round(100.0 * sum(CASE WHEN se.exit_reason = 'stop_loss' OR se.exit_reason = 'recovery_gate' THEN 1 ELSE 0 END) / count(*), 0) AS loss_exit_pct,
       round(avg(se.pnl_pct), 2) AS avg_pnl_pct
FROM shadow_exits se
JOIN shadow_positions sp ON sp.shadow_id = se.shadow_id
WHERE sp.opened_at > NOW() - INTERVAL '48 hours'
  AND sp.token_address NOT LIKE '%pump'
  AND se.exit_strategy = 'mirror_main'
  AND sp.main_admitted = true
GROUP BY 1 HAVING count(*) >= 5
ORDER BY avg_pnl_pct DESC LIMIT 10;"

echo ""
echo "== TOKEN SHADOW BLACKLIST (currently banned) =="
PSQL_HEAD "
SELECT substr(sp.token_address, 1, 14) AS token, count(*) AS exits,
       round(avg(se.pnl_pct), 1) AS avg_pnl_pct
FROM shadow_exits se
JOIN shadow_positions sp ON sp.shadow_id = se.shadow_id
WHERE se.exit_strategy = 'mirror_main'
  AND sp.token_address NOT LIKE '%pump'
  AND sp.opened_at > NOW() - INTERVAL '48 hours'
GROUP BY 1
HAVING count(*) >= 10 AND avg(se.pnl_pct) < -1.5
ORDER BY avg_pnl_pct ASC LIMIT 10;"

echo ""
echo "== DUNE COHORT (ACTIVE wallets by shadow PnL, 7d) =="
PSQL_HEAD "
SELECT left(w.address, 10) AS wallet, w.status,
       round(COALESCE(w.wqs_score, 0), 1) AS wqs,
       count(dr.decision_id) FILTER (WHERE dr.decided_at > NOW() - INTERVAL '24 hours') AS decisions_24h
FROM wallets w
LEFT JOIN decision_records dr ON dr.wallet_address = w.address
WHERE w.status = 'ACTIVE'
GROUP BY 1, 2, 3
ORDER BY decisions_24h DESC NULLS LAST LIMIT 12;"

echo ""
echo "== SYSTEM HEALTH =="
PSQL_HEAD "
SELECT 'active_wallets' AS metric, count(*) FROM wallets WHERE status = 'ACTIVE'
UNION ALL
SELECT 'active_with_webhook', count(*) FROM wallet_monitoring WHERE helius_webhook_id IS NOT NULL
UNION ALL
SELECT 'muted_wallets', count(*) FROM muted_wallets WHERE is_muted AND muted_until > NOW()
UNION ALL
SELECT 'postgres_crash_notices_24h', count(*) FROM (SELECT 1) x WHERE FALSE;"

echo ""
echo "== LAST DUNE PROMOTIONS/DEMOTIONS (operator log) =="
docker logs chimera-operator --since 24h 2>/dev/null | grep -E "Dune promotion|Shadow quality" | tail -5 || \
docker exec chimera-operator grep -E "Dune promotion|Shadow quality" /app/data/logs/operator.log.$(date -u +%Y-%m-%d) 2>/dev/null | tail -5

echo ""
echo "================================================================"
echo " Report complete"
echo "================================================================"
