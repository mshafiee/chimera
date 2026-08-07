#!/bin/bash
# Monitor whether quality wallets produce admitting signals.
# Polls decision_records + positions every N seconds and prints a line when
# anything new is admitted or a position opens.
#
# Usage: bash scripts/monitor_admissions.sh [interval_seconds]
# Run on the production server (or locally with ssh).

INTERVAL="${1:-60}"
STATE_FILE="${CHIMERA_MONITOR_STATE:-/tmp/chimera_admissions_last_check}"

if [[ ! "$INTERVAL" =~ ^[0-9]+$ ]] || [ "$INTERVAL" -lt 1 ]; then
    echo "Invalid interval '$INTERVAL': must be a positive integer" >&2
    exit 1
fi

# Load the checkpoint from a previous run so events that happened while the
# monitor was stopped are not silently missed.
if [ -f "$STATE_FILE" ]; then
    LAST_CHECK="$(cat "$STATE_FILE" 2>/dev/null || true)"
fi
LAST_CHECK="${LAST_CHECK:-$(date -u +%Y-%m-%dT%H:%M:%SZ)}"

echo "Monitoring admissions every ${INTERVAL}s. Ctrl-C to stop."
echo "Baseline last_check: ${LAST_CHECK}"
echo "----------------------------------------"

query_positions() {
    docker exec chimera-postgres psql -U chimera -d chimera -t -A -F'|' -c "
      SELECT 'POSITION', opened_at, token_symbol, entry_amount_sol
      FROM positions WHERE opened_at > '$LAST_CHECK'::timestamptz
      ORDER BY opened_at;" 2>&1
}

query_admitted() {
    docker exec chimera-postgres psql -U chimera -d chimera -t -A -F'|' -c "
      SELECT 'ADMITTED', decided_at, wallet_address, token_address, wqs
      FROM decision_records WHERE admitted AND decided_at > '$LAST_CHECK'::timestamptz
      ORDER BY decided_at;" 2>&1
}

query_rejections() {
    docker exec chimera-postgres psql -U chimera -d chimera -t -A -F'|' -c "
      SELECT 'REJECT', rejection_code, COUNT(*)
      FROM decision_records WHERE NOT admitted AND decided_at > '$LAST_CHECK'::timestamptz
      GROUP BY rejection_code ORDER BY 3 DESC LIMIT 3;" 2>&1
}

while true; do
    sleep "$INTERVAL"
    NEW_CHECK="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    MAX_OBSERVED="$NEW_CHECK"

    POSITIONS_OUTPUT=$(query_positions)
    if [ $? -ne 0 ] || echo "$POSITIONS_OUTPUT" | grep -qi "error"; then
        echo "[ERROR] DB query failed for positions: $(echo "$POSITIONS_OUTPUT" | head -1)"
    else
        echo "$POSITIONS_OUTPUT" | grep -v '^$' | while IFS='|' read -r tag ts sym amt; do
            echo "[$(date -u +%H:%M:%S)] POSITION OPENED: $sym amount=$amt SOL at $ts"
        done
        # Advance the cursor past the newest row observed so nothing is
        # reported twice across polls
        OBSERVED_TS=$(echo "$POSITIONS_OUTPUT" | grep -v '^$' | tail -1 | cut -d'|' -f2)
        [ -n "$OBSERVED_TS" ] && [ "$OBSERVED_TS" \> "$MAX_OBSERVED" ] && MAX_OBSERVED="$OBSERVED_TS"
    fi

    ADMITTED_OUTPUT=$(query_admitted)
    if [ $? -ne 0 ] || echo "$ADMITTED_OUTPUT" | grep -qi "error"; then
        echo "[ERROR] DB query failed for decision_records: $(echo "$ADMITTED_OUTPUT" | head -1)"
    else
        echo "$ADMITTED_OUTPUT" | grep -v '^$' | while IFS='|' read -r tag ts wal tok wqs; do
            echo "[$(date -u +%H:%M:%S)] BUY ADMITTED: wallet=${wal:0:8} wqs=$wqs token=${tok:0:12} at $ts"
        done
        OBSERVED_TS=$(echo "$ADMITTED_OUTPUT" | grep -v '^$' | tail -1 | cut -d'|' -f2)
        [ -n "$OBSERVED_TS" ] && [ "$OBSERVED_TS" \> "$MAX_OBSERVED" ] && MAX_OBSERVED="$OBSERVED_TS"
    fi

    REJECT_OUTPUT=$(query_rejections)
    if [ $? -ne 0 ] || echo "$REJECT_OUTPUT" | grep -qi "error"; then
        echo "[ERROR] DB query failed for rejections: $(echo "$REJECT_OUTPUT" | head -1)"
    else
        echo "$REJECT_OUTPUT" | grep -v '^$' | while IFS='|' read -r tag code n; do
            echo "[$(date -u +%H:%M:%S)] rejections: $code x$n"
        done
        OBSERVED_TS=$(echo "$REJECT_OUTPUT" | grep -v '^$' | tail -1 | cut -d'|' -f2)
        [ -n "$OBSERVED_TS" ] && [ "$OBSERVED_TS" \> "$MAX_OBSERVED" ] && MAX_OBSERVED="$OBSERVED_TS"
    fi

    # Persist the checkpoint so a restart does not miss events
    LAST_CHECK="$MAX_OBSERVED"
    echo "$LAST_CHECK" > "$STATE_FILE"
done
