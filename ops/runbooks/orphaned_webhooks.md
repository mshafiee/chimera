# Orphaned Webhooks Procedures

## Overview

- **Purpose:** Manage orphaned Helius webhooks and optimize webhook costs
- **Scope:** Webhook lifecycle management, cost optimization, cleanup procedures
- **Audience:** Platform Team, Operators
- **Frequency:** Weekly automated checks, monthly manual reviews

Orphaned webhooks are Helius webhooks that exist in the Helius system but are no longer tracked in the Chimera database. They typically occur when wallets are removed from monitoring, but the corresponding Helius webhook isn't deleted. This guide covers detection, cleanup, and prevention.

## What Are Orphaned Webhooks?

### Definition

An **orphaned webhook** is a Helius webhook subscription that:
- Exists in the Helius system (active webhook subscription)
- Is NOT tracked in Chimera's `wallet_monitoring` table
- Continues to generate costs and traffic
- Provides no value since Chimera no longer processes the signals

### How Webhooks Become Orphaned

**Common Causes:**
1. **Wallet Removal:** Wallet removed from roster without webhook cleanup
2. **Manual Deletions:** Direct database deletion without webhook deregistration
3. **Failed Cleanup:** Network issues during webhook deletion process
4. **System Failures:** Partial transaction rollbacks during wallet removal
5. **Configuration Changes:** Webhook URL changes leaving old webhooks active

**Cost Impact:**
- Helius charges per active webhook subscription
- Each orphaned webhook continues receiving transaction notifications
- Unnecessary traffic and processing costs
- Potential rate limit impact on legitimate webhooks

## Detection Mechanisms

### Automated Detection

**Daily Startup Check:**
- Runs automatically when Operator starts
- Compares Helius webhooks vs database entries
- Logs orphaned webhook count to system logs
- Does not automatically delete without configuration

**Cron-Based Detection:**
- Can be scheduled via cron jobs
- Regular monitoring and alerting
- Integrates with monitoring systems

### Manual Detection

**Via Web Dashboard:**
1. Navigate to **Monitoring → Webhooks**
2. View webhook statistics panel
3. Check **"Orphaned Webhooks"** count
4. Review webhook audit log for discrepancies

**Via API:**
```bash
# Get webhook statistics
curl http://localhost:3000/api/v1/webhooks/stats

# Check for orphaned webhooks in response
{
  "total_webhooks": 45,
  "active_webhooks": 42,
  "orphaned_webhooks": 3,
  "orphaned_cost_estimate": "$15.00/month"
}
```

**Via Database Query:**
```sql
-- Manually check for potential orphans
SELECT COUNT(*) FROM wallet_monitoring 
WHERE monitoring_enabled = 0 AND helius_webhook_id IS NOT NULL;
```

### Helius Portal Verification

1. Log into Helius Dashboard
2. Navigate to **Webhooks** section
3. Export webhook list
4. Compare against Chimera database entries
5. Identify discrepancies

## Configuration Options

### Helius Deletion Behavior

**Configuration Parameter:** `helius_delete_orphaned`
- **Location:** `operator/config/config.yaml`
- **Default:** `true` (safe deletion enabled)
- **Type:** Boolean

**Behavior:**
- `true`: Automatically delete orphaned webhooks during reconciliation
- `false`: Dry-run mode (report only, no deletion)

**Example Configuration:**
```yaml
# operator/config/config.yaml
helius:
  delete_orphaned: true  # Enable automatic cleanup
  # delete_orphaned: false  # Dry-run mode for testing
```

### Webhook Reconciliation Settings

**Reconciliation Frequency:**
- **Startup:** Automatic check on Operator startup
- **Manual:** On-demand via API or web interface
- **Scheduled:** Optional cron job configuration

**Safety Limits:**
- Maximum deletion per reconciliation: 100 webhooks
- Requires operator role authentication
- Logs all deletions for audit trail

## Reconciliation Procedures

### Automatic Reconciliation

**When It Runs:**
- Operator startup (`run_startup_webhook_check`)
- Background task (if configured)
- After wallet removal operations

**Process:**
1. Fetch all webhooks from Helius API
2. Fetch all monitored wallets from database
3. Identify orphaned webhooks (Helius but not in DB)
4. Identify missing webhooks (in DB but not in Helius)
5. Clean up orphans (if `delete_orphaned: true`)
6. Register missing webhooks
7. Update webhook URLs if changed
8. Log reconciliation results

### Manual Reconciliation

**Via Web Dashboard (Recommended):**

1. Navigate to **Monitoring → Webhooks**
2. Review current webhook statistics
3. Click **"Reconcile Webhooks"** button
4. Confirm action (if prompted)
5. Monitor progress indicator
6. Review reconciliation results

**Via API:**
```bash
# Manual reconciliation endpoint
POST /api/v1/webhooks/reconcile

# Requires operator role authentication
curl -X POST http://localhost:3000/api/v1/webhooks/reconcile \
  -H "Authorization: Bearer <operator-token>"

# Expected response
{
  "success": true,
  "message": "Reconciliation completed: 2 registered, 3 orphaned, 1 updated",
  "data": {
    "registered": 2,
    "orphaned": 3,
    "updated": 1,
    "failed": 0,
    "duration_ms": 1234
  }
}
```

**Via Direct Script:**
```bash
# Run reconciliation script
cd /opt/chimera
python ops/scripts/webhook_reconciliation.py
```

## Cleanup Strategies

### Conservative Approach (Recommended)

**Use When:** Uncertain about webhook status, testing procedures

**Steps:**
1. Set `helius_delete_orphaned: false` (dry-run mode)
2. Run manual reconciliation
3. Review identified orphans carefully
4. Verify wallets should be removed
5. Re-run with deletion enabled if confirmed

**Benefits:**
- Safe testing of reconciliation process
- Opportunity to verify webhook status
- Prevents accidental deletion of active webhooks

### Standard Approach

**Use When:** Regular maintenance, confident in webhook status

**Steps:**
1. Ensure `helius_delete_orphaned: true` in config
2. Run manual reconciliation via dashboard
3. Review results and confirm orphaned count
4. Verify cost reduction in next billing cycle
5. Monitor for any unintended side effects

**Benefits:**
- Automated cleanup process
- Immediate cost reduction
- Maintains system hygiene

### Aggressive Approach

**Use When:** Large-scale cleanup, known orphan issues

**Steps:**
1. Verify backup of database and configuration
2. Set `helius_delete_orphaned: true`
3. Run reconciliation multiple times if needed
4. Monitor system performance closely
5. Verify all legitimate webhooks still functional

**Benefits:**
- Maximum cleanup efficiency
- Immediate cost optimization
- Reduced webhook management overhead

## Verification Procedures

### Post-Reconciliation Checks

**1. Verify Legitimate Webhooks:**
```bash
# Check active monitoring in database
SELECT wallet_address, helius_webhook_id, monitoring_enabled 
FROM wallet_monitoring 
WHERE monitoring_enabled = 1 AND helius_webhook_id IS NOT NULL;
```

**2. Verify Helius Portal:**
- Log into Helius Dashboard
- Confirm webhook count matches expected
- Verify webhook URLs are correct
- Check recent webhook activity

**3. Test Webhook Functionality:**
```bash
# Trigger test transaction for monitored wallet
# Verify webhook receives notification
# Check Operator logs for webhook processing
```

**4. Cost Verification:**
- Monitor next Helius billing statement
- Verify cost reduction matches orphaned count
- Track ongoing webhook costs

### System Health Checks

**Database Integrity:**
```bash
# Verify no inconsistencies remain
SELECT COUNT(*) FROM wallet_monitoring 
WHERE monitoring_enabled = 1 AND helius_webhook_id IS NULL;
```

**Webhook Health:**
```bash
# Run webhook health check
curl http://localhost:3000/api/v1/webhooks/health
```

**Monitoring Alerts:**
- Check for webhook-related errors in logs
- Verify no increase in failed webhook deliveries
- Monitor transaction processing latency

## Prevention Measures

### Wallet Removal Procedures

**Standard Process:**
1. Stop monitoring for wallet (set `monitoring_enabled = 0`)
2. Delete Helius webhook via API
3. Remove wallet from database
4. Verify webhook deletion success
5. Log removal for audit trail

**Safe Removal Script:**
```bash
# Use safe wallet removal script
cd /opt/chimera
python ops/scripts/remove_wallet_safely.py <wallet_address>
```

### Configuration Management

**Webhook URL Changes:**
1. Update webhook URL in configuration
2. Run full reconciliation to update all webhooks
3. Verify old webhooks are cleaned up
4. Test new webhook URL functionality

**System Maintenance:**
- Regular reconciliation scheduling
- Automated monitoring and alerting
- Periodic audits of webhook inventory
- Documentation updates

### Operational Best Practices

**Daily:**
- Monitor webhook delivery success rates
- Check for webhook-related errors in logs
- Verify new webhook registrations

**Weekly:**
- Run automated webhook health checks
- Review webhook statistics in dashboard
- Monitor costs and identify anomalies

**Monthly:**
- Manual reconciliation verification
- Full webhook inventory audit
- Cost optimization review
- Update documentation and procedures

## Troubleshooting

### Issue: Orphaned Webhooks Persist After Reconciliation

**Possible Causes:**
- `helius_delete_orphaned: false` in configuration
- API authentication failures
- Network connectivity issues
- Helius API rate limiting

**Resolution:**
1. Check configuration setting
2. Verify authentication credentials
3. Review Operator logs for errors
4. Re-run reconciliation with verbose logging
5. Manually delete via Helius API if needed

### Issue: Legitimate Webhooks Deleted

**Possible Causes:**
- Database sync issues
- Incorrect webhook identification
- Race conditions during wallet removal

**Resolution:**
1. Check database for removed wallet entries
2. Review webhook audit trail
3. Re-register webhooks for active wallets
4. Investigate root cause of deletion
5. Update procedures to prevent recurrence

### Issue: High Orphan Count After System Changes

**Possible Causes:**
- Bulk wallet removal without cleanup
- System rollback after webhook registration
- Configuration changes

**Resolution:**
1. Identify time of system change
2. Review deployment logs
3. Run reconciliation with dry-run first
4. Verify webhook deletion necessity
5. Proceed with cleanup carefully

### Issue: Reconciliation API Returns Errors

**Possible Causes:**
- Insufficient permissions (requires operator role)
- Helius API authentication failure
- Network connectivity problems

**Resolution:**
1. Verify authentication token is valid
2. Check user has operator role
3. Test Helius API connectivity
4. Review rate limiting status
5. Check system logs for detailed errors

## Emergency Procedures

### Webhook Service Outage

**Immediate Actions:**
1. Pause webhook reconciliation operations
2. Verify Helius service status
3. Check Operator webhook processing
4. Monitor for transaction processing delays

**Recovery:**
1. Wait for Helius service restoration
2. Run full reconciliation when stable
3. Verify all active webhooks functional
4. Monitor for missed transactions

### Accidental Webhook Deletion

**Immediate Actions:**
1. Identify deleted webhooks from logs
2. Re-register webhooks for active wallets
3. Verify webhook functionality with test transactions
4. Monitor for processing delays

**Prevention:**
1. Enable dry-run mode before bulk operations
2. Maintain webhook registry backup
3. Implement approval workflows for deletions
4. Enhance monitoring and alerting

### Cost Alert Response

**When webhook costs exceed expectations:**

1. **Immediate Investigation:**
   - Run manual reconciliation
   - Check for orphaned webhooks
   - Verify Helius billing details

2. **Emergency Cleanup:**
   - Set `helius_delete_orphaned: true`
   - Run reconciliation immediately
   - Monitor cost reduction

3. **Root Cause Analysis:**
   - Review recent wallet removal operations
   - Check for system failures
   - Update prevention procedures

## Monitoring and Alerting

### Key Metrics

**Webhook Counts:**
- Total webhooks (Helius)
- Active monitoring (database)
- Orphaned webhooks
- Cost per webhook

**Reconciliation Metrics:**
- Registered webhooks (new)
- Orphaned webhooks (cleaned up)
- Updated webhooks (URL changes)
- Failed operations

**System Metrics:**
- Webhook delivery success rate
- API latency for webhook operations
- Error rates for webhook management

### Alert Configuration

**Warning Alerts:**
- Orphaned webhook count > 5
- Reconciliation failure rate > 10%
- Webhook cost increase > 20%

**Critical Alerts:**
- Orphaned webhook count > 20
- All reconciliation operations failing
- Webhook delivery success rate < 90%

### Prometheus Metrics

```bash
# Webhook metrics
curl http://localhost:3000/metrics | grep chimera_webhook

# Example metrics:
# chimera_webhook_orphans_total 3
# chimera_webhook_registered_total 42
# chimera_webhook_reconciliation_duration_seconds 1.234
```

## Configuration Reference

### Environment Variables

```bash
# Helius Configuration
HELIUS_API_KEY=<your-helius-api-key>
HELIUS_WEBHOOK_URL=https://your-domain.com/api/v1/webhook

# Webhook Behavior
CHIMERA_HELIUS_DELETE_ORPHANED=true
CHIMERA_HELIUS_DRY_RUN=false
```

### Configuration File

```yaml
# operator/config/config.yaml
helius:
  api_key: "${HELIUS_API_KEY}"
  webhook_url: "${HELIUS_WEBHOOK_URL}"
  delete_orphaned: true
  dry_run: false
  rate_limit: 45  # requests per second
```

### Database Schema

**Table: wallet_monitoring**
```sql
CREATE TABLE wallet_monitoring (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  wallet_address TEXT NOT NULL,
  helius_webhook_id TEXT,
  monitoring_enabled INTEGER DEFAULT 1,
  created_at TEXT DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT DEFAULT CURRENT_TIMESTAMP
);
```

## Related Documentation

- **Architecture:** Webhook lifecycle management (`docs/architecture.md`)
- **API Reference:** Webhook endpoints (`docs/core/api.md`)
- **Monitoring:** Webhook health monitoring (`ops/monitoring/`)
- **Runbooks:** Webhook procedures (`ops/runbooks/webhook_lifecycle.md`)

## Quick Reference

### Common Commands

```bash
# Check webhook statistics
curl http://localhost:3000/api/v1/webhooks/stats

# Manual reconciliation
curl -X POST http://localhost:3000/api/v1/webhooks/reconcile

# Webhook health check
curl http://localhost:3000/api/v1/webhooks/health

# View webhook audit log
curl http://localhost:3000/api/v1/webhooks/audit
```

### Configuration Checklist

- [ ] `helius_delete_orphaned: true` for production
- [ ] Webhook URL correctly configured
- [ ] Helius API key valid and active
- [ ] Rate limiting appropriate for plan
- [ ] Monitoring and alerting configured

### Troubleshooting Checklist

**Orphaned webhooks persist:**
- [ ] Check `helius_delete_orphaned` setting
- [ ] Verify API authentication
- [ ] Review system logs for errors
- [ ] Test Helius API connectivity
- [ ] Re-run reconciliation with verbose logging

**Costs increasing:**
- [ ] Run manual reconciliation
- [ ] Check Helius billing portal
- [ ] Verify webhook count accuracy
- [ ] Review recent wallet additions
- [ ] Audit webhook inventory

### Decision Tree

```
Orphaned webhooks detected?
├─ Yes → Check helius_delete_orphaned setting
│         ├─ true → Run reconciliation
│         │           └─ Success → Verify cost reduction
│         │           └─ Failed → Check logs, retry
│         └─ false → Enable deletion or investigate
└─ No → Monitor webhook count regularly
```

## Cost Optimization

### Webhook Cost Calculation

**Helius Pricing (Example):**
- Cost per webhook: $5/month
- Orphaned webhooks: 3
- Monthly waste: $15
- Annual waste: $180

**ROI Analysis:**
- Reconciliation time: 5 minutes
- Cost savings per cleanup: $15/month
- Annual savings: $180
- Time investment: Minimal

### Optimization Strategy

1. **Baseline:** Establish current webhook count and costs
2. **Monitoring:** Track webhook trends monthly
3. **Automation:** Schedule regular reconciliation
4. **Verification:** Confirm cost reduction in billing
5. **Continuous:** Maintain webhook hygiene practices