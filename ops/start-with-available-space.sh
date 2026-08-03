#!/bin/bash
# Modified configuration for 86GB disk space evaluation

set -euo pipefail

ENV_FILE="docker/env.evaluation"

echo "🔧 Adjusting evaluation for 86GB available disk space"
echo "====================================================="
echo ""

if [ ! -f "$ENV_FILE" ]; then
    echo "❌ $ENV_FILE not found (run from repo root)" >&2
    exit 1
fi

# Reduce retention periods in a single pass so the .bak preserves the
# original file, and verify each key took effect afterwards.
sed -i.bak -e 's/^\(LOG_RETENTION_DAYS\)[[:space:]]*=.*/\1=7/' \
           -e 's/^\(METRICS_RETENTION_DAYS\)[[:space:]]*=.*/\1=7/' \
           -e 's/^\(PROMETHEUS_RETENTION_DAYS\)[[:space:]]*=.*/\1=7/' \
           -e 's/^\(REPORT_RETENTION_DAYS\)[[:space:]]*=.*/\1=14/' \
           "$ENV_FILE"

for key in LOG_RETENTION_DAYS METRICS_RETENTION_DAYS PROMETHEUS_RETENTION_DAYS REPORT_RETENTION_DAYS; do
    grep -q "^${key}[[:space:]]*=" "$ENV_FILE" || { echo "❌ ${key} not updated" >&2; exit 1; }
done

# Align the Prometheus container flag with the env value (the compose file
# hardcodes 10d, which would otherwise defeat the retention adjustment).
if grep -q 'storage.tsdb.retention.time=10d' docker-compose.evaluation.yml; then
    sed -i.bak 's/storage.tsdb.retention.time=10d/storage.tsdb.retention.time=7d/' docker-compose.evaluation.yml
    rm -f docker-compose.evaluation.yml.bak
fi

echo "✅ Adjusted retention policies for 86GB disk space:"
echo "   - Logs: 7 days"
echo "   - Metrics: 7 days"
echo "   - Prometheus: 7 days (compose flag aligned)"
echo "   - Reports: 14 days"
echo ""
echo "📊 Estimated space usage with adjustments:"
echo "   - Hourly snapshots: ~5GB (240 files × 7 days)"
echo "   - Compressed logs: ~25GB (7 days)"
echo "   - Prometheus metrics: ~15GB (7 days)"
echo "   - Database backups: ~8GB (7 days)"
echo "   - Reports & analysis: ~3GB"
echo "   - Total: ~56GB (within 86GB available)"
echo ""
echo "✅ Ready to start evaluation with adjusted configuration!"
echo ""
echo "Next step:"
echo "1. Configure Helius API key in docker/env.evaluation.local"
echo "2. Run: sudo ./ops/start-evaluation.sh evaluation"
