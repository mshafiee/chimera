#!/bin/bash
# Sample pg_stat_activity every few seconds into a rolling log so a postgres
# backend SIGKILL (observed ~17x/day on 2026-08-07, source unknown) can be
# correlated with the connections that were alive right before the kill.
#
# Usage: nohup bash ops/pg-activity-sampler.sh >> /var/log/pg-activity-sampler.log 2>&1 &
# The auditd rule (sigkill9) captures the kill syscall with the killer PID;
# this sampler captures the DB-side state at 5s granularity.

INTERVAL="${PG_SAMPLER_INTERVAL:-5}"
MAX_BYTES=52428800  # 50MB rolling cap

echo "pg-activity-sampler started at $(date -u +%FT%TZ) interval=${INTERVAL}s"

while true; do
    SIZE=$(stat -c%s /var/log/pg-activity-sampler.log 2>/dev/null || echo 0)
    if [ "$SIZE" -gt "$MAX_BYTES" ]; then
        mv /var/log/pg-activity-sampler.log /var/log/pg-activity-sampler.log.1 2>/dev/null
    fi

    docker exec chimera-postgres psql -U chimera -d chimera -t -A -F'|' -c "
      SELECT NOW(), pid, usename, COALESCE(client_addr::text,'local'), state,
             COALESCE(LEFT(query, 80), '')
      FROM pg_stat_activity
      ORDER BY pid;" 2>/dev/null | while IFS='|' read -r ts pid user client state q; do
        echo "$ts|pid=$pid|user=$user|client=$client|state=$state|$q"
    done
    echo "--- sampler tick $(date -u +%FT%TZ) ---"

    sleep "$INTERVAL"
done
