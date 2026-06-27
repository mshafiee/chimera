# Secret Rotation Monitoring

## Overview

Secret rotation monitoring tracks the status of automated secret rotations and alerts when rotations are overdue or fail.

## Secret Rotation Schedule

According to PDD v7.1:

| Secret Type | Rotation Frequency | Grace Period |
|-------------|-------------------|--------------|
| Webhook HMAC Key | Every 30 days | 24 hours |
| RPC API Keys | Every 90 days | Immediate |
| Database Encryption Key | Annually | Manual migration |

## Prometheus Metrics

### Metrics Exposed

1. **`chimera_secret_rotation_last_success_timestamp`** (Gauge)
   - Unix timestamp of last successful secret rotation
   - Updated after each successful rotation
   - Used to detect rotation failures

2. **`chimera_secret_rotation_days_until_due`** (Gauge)
   - Number of days until next secret rotation is due
   - Negative values indicate overdue
   - Updated daily

### Metrics Integration

The metrics are defined in `operator/src/metrics.rs` and exposed via `/metrics` endpoint.

**To update metrics from rotation script:**

The rotation script (`ops/rotate-secrets.sh`) should update metrics after successful rotation:

```bash
# After successful rotation, update metrics via API or pushgateway
curl -X POST http://localhost:8080/api/v1/metrics/secret-rotation \
  -H "Content-Type: application/json" \
  -d '{
    "last_success_timestamp": '$(date +%s)',
    "days_until_due": 30
  }'
```

**Alternative: Direct Database Query**

Metrics can be calculated from `config_audit` table:

```sql
-- Get last rotation timestamp
SELECT 
    MAX(CAST(changed_at AS INTEGER)) as last_success
FROM config_audit
WHERE key LIKE 'secret_rotation.%'
AND changed_by = 'SYSTEM_ROTATION';

-- Calculate days until due
SELECT 
    CAST(30 - (julianday('now') - julianday(MAX(changed_at))) AS INTEGER) as days_until_due
FROM config_audit
WHERE key = 'secret_rotation.webhook_hmac'
AND changed_by = 'SYSTEM_ROTATION';
```

## Prometheus Alerts

### Alert Rules

**Location**: `ops/prometheus/alerts.yml`

1. **SecretRotationOverdue**
   - **Condition**: `chimera_secret_rotation_days_until_due < 0`
   - **Duration**: 1 hour
   - **Severity**: Warning
   - **Action**: Rotate secret immediately

2. **SecretRotationDueSoon**
   - **Condition**: `chimera_secret_rotation_days_until_due > 0 AND <= 3`
   - **Duration**: 24 hours
   - **Severity**: Info
   - **Action**: Prepare for rotation

3. **SecretRotationFailure**
   - **Condition**: `(time() - chimera_secret_rotation_last_success_timestamp) > 2592000` (30 days)
   - **Duration**: 1 hour
   - **Severity**: Critical
   - **Action**: Investigate rotation script

### Alert Routing

Alerts are routed via Alertmanager to:
- **Critical**: Telegram/Discord notifications
- **Warning**: Email or Slack
- **Info**: Daily summary (optional)

## Database Tracking

### Secret Rotation History

Secret rotations are logged to `config_audit` table:

```sql
SELECT 
    key,
    changed_at,
    changed_by,
    change_reason
FROM config_audit
WHERE key LIKE 'secret_rotation.%'
ORDER BY changed_at DESC;
```

### Rotation Status Query

```sql
-- Check rotation status for each secret type
SELECT 
    'webhook_hmac' as secret_type,
    MAX(changed_at) as last_rotation,
    CAST(30 - (julianday('now') - julianday(MAX(changed_at))) AS INTEGER) as days_until_due
FROM config_audit
WHERE key = 'secret_rotation.webhook_hmac'
UNION ALL
SELECT 
    'rpc_primary' as secret_type,
    MAX(changed_at) as last_rotation,
    CAST(90 - (julianday('now') - julianday(MAX(changed_at))) AS INTEGER) as days_until_due
FROM config_audit
WHERE key = 'secret_rotation.rpc_primary';
```

## Integration with Notification System

The rotation script already sends notifications on rotation:

```bash
send_notification "Webhook HMAC secret rotated. Grace period: ${GRACE_PERIOD_HOURS}h"
```

This integrates with Telegram/Discord notifications.

### Notification Events

1. **Rotation Success**: Sent when rotation completes
2. **Rotation Failure**: Sent if rotation script fails
3. **Rotation Overdue**: Sent via Prometheus alert

## Cron Schedule

Secret rotation is scheduled via cron:

```bash
# Webhook secret: Every 30 days at 2 AM UTC
0 2 */30 * * /opt/chimera/ops/rotate-secrets.sh --type=webhook

# RPC keys: Every 90 days at 3 AM UTC
0 3 */90 * * /opt/chimera/ops/rotate-secrets.sh --type=rpc
```

**Installation**: `ops/install-crons.sh`

## Manual Rotation

### Force Rotation

```bash
# Force immediate rotation
./rotate-secrets.sh --force --type=webhook

# Rotate specific secret type
./rotate-secrets.sh --type=rpc
```

### Verify Rotation

```bash
# Check rotation in database
sqlite3 chimera.db "
SELECT * FROM config_audit 
WHERE key LIKE 'secret_rotation.%' 
ORDER BY changed_at DESC 
LIMIT 5;
"

# Check metrics
curl http://localhost:8080/metrics | grep secret_rotation
```

## Grace Period Management

### Webhook HMAC Grace Period

During the 24-hour grace period:
- Both old and new secrets are accepted
- System logs which secret was used
- After grace period, only new secret accepted

**Grace Period End:**
```bash
# After 24 hours, remove old secret
sed -i '/CHIMERA_SECURITY__WEBHOOK_SECRET_PREVIOUS=/d' /opt/chimera/config/.env
systemctl reload chimera
```

## Troubleshooting

### Rotation Script Fails

1. Check script logs: `tail -f /var/log/chimera/secret-rotation.log`
2. Verify database access
3. Verify config file permissions
4. Check for openssl (required for secret generation)

### Metrics Not Updating

1. Verify metrics endpoint: `curl http://localhost:8080/metrics`
2. Check if rotation script updates metrics
3. Verify Prometheus can scrape metrics

### Alerts Not Firing

1. Check Prometheus alert rules: `ops/prometheus/alerts.yml`
2. Verify Alertmanager configuration
3. Check alert routing in Alertmanager

## Next Steps

1. ✅ Metrics defined in `operator/src/metrics.rs`
2. ✅ Alert rules added to `ops/prometheus/alerts.yml`
3. ✅ Rotation script exists with notification support
4. ✅ API endpoint for metrics updates implemented (`POST /api/v1/metrics/secret-rotation`)
5. ✅ Daily metrics update job implemented (`ops/update-metrics.sh` with cron schedule)

## Testing

### Test Rotation

```bash
# Force rotation
./rotate-secrets.sh --force --type=webhook

# Verify in database
sqlite3 chimera.db "SELECT * FROM config_audit WHERE key = 'secret_rotation.webhook_hmac' ORDER BY changed_at DESC LIMIT 1;"

# Verify notification sent
# Check Telegram/Discord for notification
```

### Test Overdue Alert

```bash
# Manually set last rotation to 35 days ago
sqlite3 chimera.db "
UPDATE config_audit 
SET changed_at = datetime('now', '-35 days')
WHERE key = 'secret_rotation.webhook_hmac'
ORDER BY changed_at DESC
LIMIT 1;
"

# Wait for alert (1 hour)
# Check Alertmanager for alert
```
