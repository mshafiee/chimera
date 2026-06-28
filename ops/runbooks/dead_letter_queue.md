# Dead Letter Queue Management

## Overview

- **Trigger:** Failed trades accumulating in dead letter queue, manual retry operations
- **Severity:** MEDIUM to HIGH (depending on volume and reason)
- **SLA:** Investigate within 1 hour, resolve within 4 hours
- **On-Call:** Platform Team

The Dead Letter Queue (DLQ) captures trades that failed during processing and couldn't be completed automatically. This runbook covers manual retry operations and DLQ management.

## When to Use Manual Retry vs Automatic Processing

### Automatic Processing (Background Worker)
The system automatically processes retryable DLQ items when:
- Circuit breaker is active
- Wallet is still in ACTIVE status
- Retry count hasn't exceeded limits (default: 3)
- The failure reason is temporary (network issues, RPC timeouts)

### Manual Retry (Operator Intervention)
Use manual retry when:
- **Immediate action needed:** High-priority wallet signals
- **Bulk operations:** Multiple similar failures need reprocessing
- **Testing:** Verify that a fix resolves recurring issues
- **Special cases:** Override retry limits for critical trades
- **Investigation:** Test if underlying issue has been resolved

## Identify the Issue

### Symptoms
- **High DLQ volume:** More than 20 items in queue
- **Specific failure patterns:** Same error reason repeated
- **Stuck trades:** Critical trades failing repeatedly
- **Performance impact:** DLQ processing affecting system performance

### Verification Steps

1. **Check current DLQ status:**
   ```bash
   # Via API
   curl http://localhost:3000/api/v1/incidents/dead-letter
   
   # Via web dashboard
   # Navigate to Incidents → Dead Letter Queue tab
   ```

2. **Analyze failure patterns:**
   - Group by `reason` field to identify common issues
   - Check `retry_count` to see repeated failures
   - Examine `error_details` for specific error messages

3. **Review system health:**
   - Circuit breaker status: `/api/v1/health`
   - RPC health: Check primary and fallback RPC endpoints
   - Database performance: `/api/v1/metrics/database-performance`

## Common Failure Reasons and Resolution

### 1. VALIDATION_FAILURE
**Cause:** Token safety checks failed (honeypot, freeze authority, liquidity)
**Resolution:**
- Verify token safety manually using external tools
- If safe to proceed, retry the trade
- Update token safety configuration if needed

### 2. WALLET_INACTIVE
**Cause:** Tracked wallet status changed to INACTIVE or REJECTED
**Resolution:**
- Check wallet status via roster management
- Update wallet status if it should be active
- Do not retry if wallet is legitimately inactive

### 3. PORTFOLIO_LIMITS
**Cause:** Portfolio heat limits or position count limits exceeded
**Resolution:**
- Wait for existing positions to exit
- Increase portfolio limits if appropriate
- Retry trade when capacity is available

### 4. CIRCUIT_BREAKER_TRIPPED
**Cause:** Circuit breaker is tripped, blocking new trades
**Resolution:**
- Investigate circuit breaker trigger reason
- Resolve underlying issue (loss limits, etc.)
- Reset circuit breaker via API or SIGHUP
- Retry trades when system is healthy

### 5. RPC_FAILURE
**Cause:** RPC endpoint timeout or failure
**Resolution:**
- Check RPC endpoint status
- System should automatically retry with fallback RPC
- Manual retry only if RPC issues are resolved

### 6. QUEUE_FULL
**Cause:** Internal queue depth exceeded, trade was shed
**Resolution:**
- Wait for queue depth to decrease
- Check consumer lag and processing speed
- Retry when system capacity is available

### 7. PARSE_ERROR
**Cause:** Invalid webhook payload format
**Resolution:**
- Review original webhook payload
- Fix payload format at source
- Manual retry with corrected payload

## Immediate Actions

### 1. Triage the DLQ
```bash
# Group items by failure reason
curl http://localhost:3000/api/v1/incidents/dead-letter | jq '.items | group_by(.reason)'

# Check retry count distribution
curl http://localhost:3000/api/v1/incidents/dead-letter | jq '.items | map(.retry_count) | group_by(.)'
```

### 2. Stabilize the System
- Resolve any underlying system issues (circuit breaker, RPC problems)
- Ensure adequate system capacity
- Verify wallet statuses are correct

### 3. Prioritize Items
- **High priority:** Recent signals from alpha wallets
- **Medium priority:** General retriable failures
- **Low priority:** Old signals or low-priority wallets

## Manual Retry Operations

### Via Web Dashboard (Recommended)

1. Navigate to **Incidents → Dead Letter Queue**
2. Filter by severity or reason
3. Review failed trade details
4. Click **Retry** button for specific items
5. Monitor success/error feedback
6. Verify trade processes successfully

### Via API

```bash
# Retry a specific trade
curl -X POST http://localhost:3000/api/v1/incidents/dead-letter/{trade_uuid}/retry

# Expected response
{
  "success": true,
  "message": "Trade abc-123 queued for retry (attempt 1/3)",
  "trade_uuid": "abc-123",
  "retry_attempt": 1
}
```

### Bulk Operations

For multiple similar failures, consider:
1. Export DLQ data via API
2. Filter items requiring retry
3. Use script to batch retry calls
4. Monitor system load during bulk operations

## Retry Strategy and Limits

### Retry Limits
- **Default maximum:** 3 retry attempts
- **Tracking:** Each retry increments `retry_count`
- **Enforcement:** System blocks retry beyond maximum
- **Override:** Contact on-call for limit increases (critical trades only)

### Retry Validation
Before retry, the system checks:
- ✅ Circuit breaker must be ACTIVE (not tripped)
- ✅ Item must be marked as `can_retry = true`
- ✅ Retry count must be below maximum
- ⚠️  Wallet status (if available in payload)

### When NOT to Retry
- **Permanent failures:** Invalid tokens, security issues
- **Deprecated signals:** Old signals no longer relevant
- **System issues:** When underlying problems aren't resolved
- **Risk scenarios:** High-risk tokens during market volatility

## Resolution Steps

### Scenario 1: RPC Failures (Temporary)
1. Check RPC endpoint status
2. Wait for automatic fallback or recovery
3. Retry trades once RPC is healthy
4. Monitor for repeated failures

### Scenario 2: Validation Failures (Token Safety)
1. Investigate specific token using external tools
2. Determine if failure was legitimate
3. Update token safety configuration if needed
4. Retry only if token is safe

### Scenario 3: Portfolio Limits (Capacity)
1. Check current portfolio heat and position count
2. Wait for existing positions to exit
3. Consider adjusting limits if appropriate
4. Retry when capacity is available

### Scenario 4: Circuit Breaker Tripped
1. Investigate circuit breaker trigger
2. Resolve underlying issue
3. Reset circuit breaker: `curl -X POST http://localhost:3000/api/v1/config/circuit-breaker/reset`
4. Retry trades when system is stable

### Scenario 5: High Volume Failures
1. Identify root cause of failure spike
2. Address underlying system issue
3. Consider selective retry (high-priority items only)
4. Monitor system performance during retry operations

## Prevention Measures

### Monitoring
- **DLQ size alert:** Alert when queue exceeds 20 items
- **Failure pattern alerts:** Alert on specific failure reasons
- **Retry rate monitoring:** Track retry success rates
- **Dashboard monitoring:** Regular review of DLQ status

### System Health
- Maintain RPC endpoint health
- Keep circuit breaker settings appropriate
- Monitor database performance
- Ensure adequate system capacity

### Process Improvements
- Regular review of DLQ patterns
- Update validation rules based on findings
- Improve webhook payload validation
- Enhance error messages for clarity

## Verification

### Post-Retry Checks
1. **Trade execution:** Verify trade completed successfully
2. **Position tracking:** Confirm position appears in active positions
3. **DLQ cleanup:** Ensure item removed from queue
4. **System health:** Check no performance degradation

### System Validation
```bash
# Check if trade executed
curl http://localhost:3000/api/v1/trades?trade_uuid={uuid}

# Verify active positions
curl http://localhost:3000/api/v1/positions

# Check DLQ count decreased
curl http://localhost:3000/api/v1/incidents/dead-letter | jq '.total'
```

## Escalation

### When to Escalate
- **DLQ volume > 50 items:** Systemic issue
- **Retry failure rate > 50%:** Fundamental problem
- **Critical trades failing:** High-priority wallets affected
- **Performance degradation:** System impact from DLQ processing

### Escalation Path
1. **Platform Team:** Primary (DLQ management and retry operations)
2. **Infrastructure Team:** RPC/database issues
3. **Security Team:** Validation failures, potential attacks
4. **On-Call Engineer:** 24/7 for critical situations

## Post-Resolution

### Documentation
1. Document the incident and root cause
2. Record resolution steps taken
3. Note any configuration changes made
4. Update runbook with lessons learned

### Analysis
1. Review failure patterns for trends
2. Identify recurring issues
3. Propose system improvements
4. Update monitoring/alerting as needed

### Follow-up
1. Monitor DLQ for 24 hours post-resolution
2. Check for related issues
3. Verify prevention measures effective
4. Review and update procedures

## Configuration Reference

### DLQ Configuration
- **Retry limit:** Default 3 attempts (configurable)
- **Background processing:** Automatic retry every 5 minutes
- **Manual retry:** Available via API and dashboard
- **Can retry flag:** Set based on failure reason

### Related Settings
- **Circuit breaker:** Affects retry eligibility
- **Portfolio limits:** Can cause rejections
- **Token safety:** Validation checks
- **RPC endpoints:** Affects trade execution

## Troubleshooting

### Common Issues

**Issue:** Retry button disabled
- **Solution:** Check `can_retry` flag, verify retry count not exceeded

**Issue:** Retry returns error
- **Solution:** Check circuit breaker status, verify system health

**Issue:** Trade disappears after retry but doesn't execute
- **Solution:** Check trade status in `/api/v1/trades`, may have failed again

**Issue:** High retry failure rate
- **Solution:** Investigate root cause before continuing retries

## Related Documentation

- **Architecture:** Signal Processing Flow (`docs/architecture.md`)
- **API Reference:** `/api/v1/incidents/dead-letter` endpoints
- **Circuit Breaker:** Circuit breaker operations runbook
- **Monitoring:** Performance metrics and alerts

## Quick Reference

### Priority Levels
- **🔴 Critical:** Alpha wallet signals, high-value trades
- **🟡 High:** Active wallet signals, recent failures
- **🟢 Medium:** Older signals, investigation cases

### Retry Decision Tree
```
Is can_retry = true? → NO → Do not retry (permanent failure)
                     → YES → Continue
Is retry_count < max? → NO → Contact on-call for override
                     → YES → Continue
Is circuit breaker ACTIVE? → NO → Fix circuit breaker first
                          → YES → Continue
Is underlying issue resolved? → NO → Resolve issue first
                              → YES → SAFE TO RETRY
```

### Most Common Operations
1. **View DLQ:** Dashboard → Incidents → Dead Letter Queue
2. **Single retry:** Click "Retry" button for specific item
3. **Bulk retry:** Use API with script (caution required)
4. **Monitor status:** Watch for success/error feedback