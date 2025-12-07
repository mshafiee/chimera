# Reconciliation Monitoring

## Overview

Reconciliation monitoring tracks discrepancies between database state and on-chain state, providing alerts and metrics for operational visibility.

## Prometheus Metrics

### Metrics Exposed

1. **`chimera_reconciliation_checked_total`** (Counter)
   - Total number of positions checked during reconciliation
   - Incremented for each position verified

2. **`chimera_reconciliation_discrepancies_total`** (Counter)
   - Total number of discrepancies found
   - Incremented when a discrepancy is detected

3. **`chimera_reconciliation_unresolved_total`** (Gauge)
   - Current number of unresolved discrepancies
   - Updated after each reconciliation run

### Metrics Integration

The metrics are defined in `operator/src/metrics.rs` and exposed via `/metrics` endpoint.

**To update metrics from reconciliation script:**

Option 1: Use Prometheus Pushgateway
```bash
# Install pushgateway
# Update metrics via pushgateway
echo "chimera_reconciliation_checked_total $total_checked" | \
  curl --data-binary @- http://localhost:9091/metrics/job/reconciliation
```

Option 2: Add API endpoint to update metrics
```rust
// In operator/src/handlers/api.rs
POST /api/v1/metrics/reconciliation
{
  "checked": 100,
  "discrepancies": 5,
  "unresolved": 2
}
```

Option 3: Direct database query (Prometheus SQLite exporter)
- Use prometheus-sqlite-exporter to query reconciliation_log table
- Expose metrics via SQL queries

## Prometheus Alerts

### Alert Rules

**Location**: `ops/prometheus/alerts.yml`

1. **ReconciliationDiscrepancies**
   - **Condition**: `chimera_reconciliation_unresolved_total > 5`
   - **Duration**: 5 minutes
   - **Severity**: Warning
   - **Action**: Manual review required

2. **ReconciliationDiscrepancySpike**
   - **Condition**: `rate(chimera_reconciliation_discrepancies_total[1h]) > 10`
   - **Duration**: 10 minutes
   - **Severity**: Critical
   - **Action**: Investigate system integrity

3. **HighDiscrepancyRate**
   - **Condition**: `rate(chimera_reconciliation_discrepancies_total[24h]) > 20`
   - **Duration**: 1 hour
   - **Severity**: Warning
   - **Action**: Review reconciliation process

### Alert Routing

Alerts are routed via Alertmanager to:
- **Critical**: Telegram/Discord notifications
- **Warning**: Email or Slack (if configured)

**Configuration**: `ops/alertmanager/config.yml`

## Dashboard Queries

### Grafana Queries

**Reconciliation Check Rate:**
```promql
rate(chimera_reconciliation_checked_total[5m])
```

**Discrepancy Rate:**
```promql
rate(chimera_reconciliation_discrepancies_total[5m])
```

**Unresolved Discrepancies:**
```promql
chimera_reconciliation_unresolved_total
```

**Discrepancy Ratio:**
```promql
rate(chimera_reconciliation_discrepancies_total[24h]) / 
rate(chimera_reconciliation_checked_total[24h])
```

## Manual Monitoring

### Check Unresolved Discrepancies

```bash
sqlite3 /opt/chimera/data/chimera.db "
SELECT 
    COUNT(*) as unresolved,
    discrepancy,
    COUNT(*) as count
FROM reconciliation_log
WHERE resolved_at IS NULL
AND created_at > datetime('now', '-24 hours')
GROUP BY discrepancy;
"
```

### Check Reconciliation Run Status

```bash
sqlite3 /opt/chimera/data/chimera.db "
SELECT 
    key,
    new_value,
    changed_at,
    change_reason
FROM config_audit
WHERE key = 'reconciliation_run'
ORDER BY changed_at DESC
LIMIT 5;
"
```

## Integration with Notification System

The reconciliation script (`ops/reconcile.sh`) already sends alerts when unresolved discrepancies are found:

```bash
# Send alert if there are unresolved discrepancies
if [[ "$unresolved_count" -gt 0 ]]; then
    notify "WARNING" "Found $unresolved_count unresolved discrepancies in the last 24h. Manual review required."
fi
```

This integrates with the notification system (Telegram/Discord) configured in the operator.

## Next Steps

1. ✅ Metrics defined in `operator/src/metrics.rs`
2. ✅ Alert rules added to `ops/prometheus/alerts.yml`
3. ✅ Metrics updates integrated from reconciliation script (`ops/reconcile.sh` calls `POST /api/v1/metrics/reconciliation`)
4. ✅ Daily metrics update job implemented (`ops/update-metrics.sh` with cron schedule)

## Testing

### Test Metrics Exposure

```bash
# Check metrics endpoint
curl http://localhost:8080/metrics | grep reconciliation
```

### Test Alert Triggering

```bash
# Manually insert test discrepancy
sqlite3 chimera.db "
INSERT INTO reconciliation_log (trade_uuid, discrepancy, expected_state, actual_on_chain)
VALUES ('test-alert', 'MISSING_TRANSACTION', 'ACTIVE', 'NOT_FOUND');
"

# Wait for alert (5 minutes)
# Check Alertmanager for alert
```
