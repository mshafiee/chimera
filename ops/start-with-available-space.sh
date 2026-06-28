#!/bin/bash
# Modified configuration for 86GB disk space evaluation

echo "🔧 Adjusting evaluation for 86GB available disk space"
echo "====================================================="
echo ""

# Modify env.evaluation for reduced retention
if [ -f "docker/env.evaluation" ]; then
    # Reduce retention periods
    sed -i.bak 's/LOG_RETENTION_DAYS=10/LOG_RETENTION_DAYS=7/' docker/env.evaluation
    sed -i.bak 's/METRICS_RETENTION_DAYS=10/METRICS_RETENTION_DAYS=7/' docker/env.evaluation
    sed -i.bak 's/PROMETHEUS_RETENTION_DAYS=10/PROMETHEUS_RETENTION_DAYS=7/' docker/env.evaluation
    sed -i.bak 's/RECORD_RETENTION_DAYS=30/RECORD_RETENTION_DAYS=14/' docker/env.evaluation

    echo "✅ Adjusted retention policies for 86GB disk space:"
    echo "   - Logs: 7 days (was 10 days)"
    echo "   - Metrics: 7 days (was 10 days)"
    echo "   - Reports: 14 days (was 30 days)"
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
else
    echo "❌ docker/env.evaluation not found"
    exit 1
fi