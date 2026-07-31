#!/bin/bash
# Monitor whether quality wallets produce admitting signals.
# Polls decision_records + positions every N seconds and prints a line when
# anything new is admitted or a position opens.
#
# Usage: bash scripts/monitor_admissions.sh [interval_seconds]
# Run on the production server (or locally with ssh).

INTERVAL="${1:-60}"
LAST_CHECK="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

echo "Monitoring admissions every ${INTERVAL}s. Ctrl-C to stop."
echo "Baseline last_check: ${LAST_CHECK}"
echo "----------------------------------------"

while true; do
    sleep "$INTERVAL"
    NEW_CHECK="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    docker exec chimera-postgres psql -U chimera -d chimera -t -A -F'|' -c "
      SELECT 'POSITION', opened_at, token_symbol, entry_amount_sol
      FROM positions WHERE opened_at > '$LAST_CHECK'::timestamptz
      ORDER BY opened_at;" 2>/dev/null | grep -v '^$' | while IFS='|' read -r tag ts sym amt; do
        echo "[$(date -u +%H:%M:%S)] POSITION OPENED: $sym amount=$amt SOL at $ts"
    done
    docker exec chimera-postgres psql -U chimera -d chimera -t -A -F'|' -c "
      SELECT 'ADMITTED', decided_at, wallet_address, token_address, wqs_score
      FROM decision_records WHERE admitted AND decided_at > '$LAST_CHECK'::timestamptz
      ORDER BY decided_at;" 2>/dev/null | grep -v '^$' | while IFS='|' read -r tag ts wal tok wqs; do
        echo "[$(date -u +%H:%M:%S)] BUY ADMITTED: wallet=${wal:0:8} wqs=$wqs token=${tok:0:12} at $ts"
    done
    docker exec chimera-postgres psql -U chimera -d chimera -t -A -F'|' -c "
      SELECT 'REJECT', rejection_code, COUNT(*)
      FROM decision_records WHERE NOT admitted AND decided_at > '$LAST_CHECK'::timestamptz
      GROUP BY rejection_code ORDER BY 3 DESC LIMIT 3;" 2>/dev/null | grep -v '^$' | while IFS='|' read -r tag code n; do
        echo "[$(date -u +%H:%M:%S)] rejections: $code x$n"
    done
    LAST_CHECK="$NEW_CHECK"
done
