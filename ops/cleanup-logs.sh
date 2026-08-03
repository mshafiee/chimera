#!/bin/bash
# Retention for operator/scout daily-rotated log files.
#
# The app rotates logs daily (operator.log.YYYY-MM-DD, ~250MB/day) but never
# deletes old files, so they accumulate unbounded. This keeps the last
# KEEP_DAYS of rotated logs and deletes older ones. It never touches the
# current un-rotated operator.log / scout.log (no date suffix).
#
# Logs live on the host-mounted ./data volume (/opt/chimera/data/logs).
# Cron: daily (see ops/ cron setup / server crontab).
set -e
set -o pipefail

LOG_DIR="${LOG_DIR:-/opt/chimera/data/logs}"
KEEP_DAYS="${LOG_RETENTION_DAYS:-14}"

echo "=== Log cleanup $(date -u +%FT%TZ) === (keep ${KEEP_DAYS}d)"

if [ ! -d "$LOG_DIR" ]; then
  echo "log dir not found: $LOG_DIR — nothing to do"
  exit 0
fi

# Count + delete rotated daily logs older than KEEP_DAYS.
DELETED=$(find "$LOG_DIR" -type f \( -name "operator.log.*" -o -name "scout.log.*" \) -mtime +"${KEEP_DAYS}" -print -delete | wc -l)
echo "deleted ${DELETED} rotated log files older than ${KEEP_DAYS} days"
du -sh "$LOG_DIR" 2>/dev/null | awk '{print "log dir now: " $1}'
