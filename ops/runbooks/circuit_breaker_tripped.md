# Circuit Breaker Tripped

## Meta
- **Trigger**: Circuit breaker state transitions to TRIPPED
- **Severity**: CRITICAL (Trading halted)
- **SLA**: Investigate within 5 minutes, resolve within 30 minutes
- **On-Call**: Trading Operations Lead

## Overview
The circuit breaker has automatically halted trading due to loss thresholds being exceeded. This is a protective measure to prevent further capital erosion. Trading will remain halted until the cooldown period elapses and conditions improve, or until manually reset after root cause resolution.

## Trigger Conditions
The circuit breaker trips when **ANY** of the following conditions are met:

1. **24h Loss**: USD loss >= `$500` (configurable via `max_loss_24h_usd`)
2. **Consecutive Losses**: >= 5 consecutive losing trades (`max_consecutive_losses`)
3. **Drawdown**: >= 15% drawdown from peak (`max_drawdown_percent`)
4. **Portfolio Stop**: SOL-denominated 24h loss >= threshold

## Circuit Breaker States
```
ACTIVE → TRIPPED → COOLDOWN → ACTIVE (auto-recheck)
              ↓
         MANUAL RESET (if conditions resolved)
```

---

## Immediate Assessment (First 5 Minutes)

### Step 1: Verify Trip Reason
```bash
# Check circuit breaker status and trip details
curl http://localhost:3000/api/v1/circuit-breaker
```

**Response Fields to Check:**
- `state`: Should be "TRIPPED"
- `trip_reason`: Which condition triggered (loss_24h, consecutive_losses, drawdown, portfolio_stop)
- `trip_time`: When the trip occurred (UTC timestamp)
- `current_loss_usd`: Current 24-hour loss amount
- `consecutive_losses`: Number of consecutive losing trades
- `drawdown_percent`: Current drawdown from peak
- `cooldown_remaining_seconds`: Time until auto-recheck (if in cooldown)

### Step 2: Check Recent Trades and Active Positions
```bash
# Get recent trades
curl http://localhost:3000/api/v1/trades?limit=20 | jq '.[] | select(.status == "ACTIVE")'

# Get all active positions
curl http://localhost:3000/api/v1/positions?status=ACTIVE
```

**Look for:**
- Positions still open (may need manual exit if critical)
- Abnormally large losses in recent trades
- Positions stuck in EXITING status
- Common token or wallet patterns in losses

### Step 3: Verify RPC Health
```bash
# Check RPC component health
curl http://localhost:3000/api/v1/health | jq '.rpc'
```

**RPC issues may have caused:**
- Failed trades that contributed to losses
- Stale prices leading to poor execution
- Timeout during critical trades

### Step 4: Check Logs for Context
```bash
# Tail operator logs for circuit breaker events
tail -f /var/log/chimera/operator.log | grep -i "circuit\|trip\|breaker"

# Or if using journalctl
journalctl -u chimera-operator -f | grep -i circuit
```

**Look for:**
- Pattern of failed trades before trip
- Specific error messages (RPC, token safety, database)
- Timestamp sequence of events
- Any wallet-specific patterns

---

## Decision Tree

### Scenario A: Legitimate Market Losses (Most Common)
**Symptoms:**
- Multiple wallets experiencing losses simultaneously
- Losses correlate with market volatility
- No technical errors in logs

**Action: Keep circuit breaker tripped, wait for cooldown**

1. **DO NOT reset immediately** - this defeats the purpose
2. Let the 30-minute cooldown elapse (configurable via `cooldown_minutes`)
3. After cooldown, circuit breaker auto-rechecks conditions
4. If conditions still poor → stays tripped
5. If conditions improved → trading resumes automatically

**When to manually reset:**
- Only after root cause is confirmed (market conditions improved)
- AND 24h loss has decreased significantly below threshold
- AND at least one cooldown cycle has completed

### Scenario B: Stale/Incorrect Price Data
**Symptoms:**
- Losses correlate with specific tokens
- Price cache shows outdated data
- Unrealized PnL significantly different from realized

**Action: Fix data, then consider reset**

```bash
# Check price cache health
curl http://localhost:3000/api/v1/health | jq '.price_cache'

# Force price refresh if stale
curl -X POST http://localhost:3000/api/v1/price-cache/refresh
```

1. Identify tokens with stale prices
2. Force refresh price cache
3. Check for stuck EXITING positions (may need manual intervention)
4. Reset circuit breaker only after prices are verified accurate

### Scenario C: Configuration Too Aggressive
**Symptoms:**
- Circuit breaker trips frequently with small losses
- Thresholds seem too tight for trading volume
- Legitimate profitable trades being blocked

**Action: Adjust configuration, then reset**

1. Review current thresholds in `operator/config/config.yaml`:
```yaml
circuit_breaker:
  max_loss_24h_usd: 500      # Adjust based on your capital
  max_drawdown_percent: 15
  max_consecutive_losses: 5
  cooldown_minutes: 30
```

2. Adjust thresholds via API (hot reload):
```bash
curl -X PUT http://localhost:3000/api/v1/config \
  -H "Content-Type: application/json" \
  -d '{"circuit_breaker": {"max_loss_24h_usd": 1000}}'
```

3. Reset circuit breaker after configuration update

### Scenario D: RPC/Infrastructure Failure
**Symptoms:**
- High RPC latency or timeout errors in logs
- Database lock errors
- WebSocket disconnections

**Action: Fix infrastructure, then reset**

```bash
# Verify RPC latency
curl http://localhost:3000/api/v1/health | jq '.rpc_latency_ms'

# Check database health
curl http://localhost:3000/api/v1/health | jq '.database'
```

1. Resolve RPC issues (check `ops/runbooks/rpc_fallback.md`)
2. Ensure database is responsive (WAL mode enabled)
3. Verify WebSocket connection is stable
4. Reset circuit breaker after infrastructure is healthy

### Scenario E: Wallet Quality Degradation
**Symptoms:**
- Losses concentrated in specific wallets
- Recent wallet promotions performing poorly
- WQS scores of active wallets declined

**Action: Review and demote underperforming wallets**

1. Analyze which wallets contributed to losses
2. Check their current WQS scores:
```bash
curl http://localhost:3000/api/v1/wallets?status=ACTIVE | jq '.[] | select(.wqs < 60)'
```
3. Consider demoting wallets with WQS < 60
4. Review scout analysis for red flags

---

## Resolution Procedures

### Critical: If Positions Are Still Open

**WARNING**: Open positions during a circuit breaker trip may continue to lose value. Manual intervention may be required.

```bash
# List all active positions
curl http://localhost:3000/api/v1/positions?status=ACTIVE

# If critical, force exit specific position
curl -X POST http://localhost:3000/api/v1/positions/exit \
  -H "Content-Type: application/json" \
  -d '{
    "trade_uuid": "uuid-here",
    "reason": "circuit_breaker_manual_exit",
    "exit_fraction": 1.0
  }'

# Or exit all positions (emergency only)
curl -X POST http://localhost:3000/api/v1/positions/exit-all \
  -H "Content-Type: application/json" \
  -d '{"reason": "circuit_breaker_emergency_exit"}'
```

### Document Findings

```bash
# Log incident for post-mortem analysis
curl -X POST http://localhost:3000/api/v1/incidents \
  -H "Content-Type: application/json" \
  -d '{
    "type": "circuit_breaker_trip",
    "severity": "critical",
    "description": "Circuit breaker tripped due to [trip_reason]",
    "root_cause": "[Investigation findings]",
    "resolution": "[Actions taken]",
    "trip_time": "[ISO timestamp]",
    "resolved_by": "[Your name]"
  }'
```

### Reset Procedure (When Safe)

**ONLY reset if ALL of the following are true:**
- ✅ Root cause has been identified and resolved
- ✅ 24h loss has decreased below threshold
- ✅ Market conditions are favorable
- ✅ Infrastructure is healthy (RPC, database, WebSocket)
- ✅ At least one cooldown cycle has completed (30 min min)

```bash
# Reset circuit breaker
curl -X POST http://localhost:3000/api/v1/circuit-breaker/reset \
  -H "Content-Type: application/json" \
  -d '{
    "reason": "Root cause resolved: [explanation]",
    "resolved_by": "[your-name]",
    "verification": "[what you checked before resetting]"
  }'
```

---

## Prevention Measures

### 1. Adjust Thresholds (If Too Tight/Too Loose)

Edit `operator/config/config.yaml`:
```yaml
circuit_breaker:
  # Loss thresholds
  max_loss_24h_usd: 500          # Adjust for your capital size
  max_drawdown_percent: 15        # Maximum acceptable drawdown
  max_consecutive_losses: 5       # Consecutive losing trades limit

  # Behavior
  cooldown_minutes: 30            # Time before auto-recheck
  auto_recovery: true            # Auto-recheck after cooldown

  # Optional: Strategy-specific thresholds
  strategy_limits:
    SHIELD:
      max_loss_24h_usd: 300
      max_drawdown_percent: 10
    SPEAR:
      max_loss_24h_usd: 200      # Tighter for high-risk strategy
      max_drawdown_percent: 8
```

### 2. Improve Wallet Quality

**Regular wallet roster reviews:**
- Demote wallets with WQS < 60
- Remove wallets with consecutive failures
- Enable advanced risk features for WQS calculation (see: Recommendation #1)
- Review Scout analysis for red flags before promotion

```bash
# Review wallet quality
curl http://localhost:3000/api/v1/wallets?status=ACTIVE | \
  jq '.[] | select(.wqs < 70) | {address, wqs, roi_30d, win_rate}'
```

### 3. Enable Proactive Monitoring

**Set up alerts for approaching thresholds:**

Add to `ops/prometheus/alerts.yml`:
```yaml
# Warning: Approaching circuit breaker threshold
- alert: CircuitBreakerApproaching
  expr: chimera_circuit_breaker_loss_24h_usd > 400
  for: 5m
  labels:
    severity: warning
  annotations:
    summary: "24h loss approaching circuit breaker threshold"
    description: "Current loss: $value | Threshold: 500"
```

**Dashboard panels to monitor:**
- 24h loss trend (with threshold line)
- Consecutive losses counter
- Drawdown percentage
- Circuit breaker state timeline

### 4. Regular Health Checks

```bash
# Daily automated check (add to cron)
#!/bin/bash
# /etc/cron.daily/chimera-circuit-breaker-check

HEALTH=$(curl -s http://localhost:3000/api/v1/circuit-breaker)
LOSS=$(echo $HEALTH | jq '.current_loss_usd')
DRAWDOWN=$(echo $HEALTH | jq '.drawdown_percent')

if (( $(echo "$LOSS > 400" | bc -l) )); then
  echo "WARNING: Loss approaching threshold: $LOSS" | \
    mail -s "Chimera Circuit Breaker Warning" ops@example.com
fi

if (( $(echo "$DRAWDOWN > 10" | bc -l) )); then
  echo "WARNING: Drawdown elevated: $DRAWDOWN%" | \
    mail -s "Chimera Drawdown Warning" ops@example.com
fi
```

---

## Verification After Reset

After resetting the circuit breaker, verify proper operation:

### Step 1: Confirm State Change
```bash
curl http://localhost:3000/api/v1/circuit-breaker | jq '.state'
# Should return: "ACTIVE"
```

### Step 2: Verify New Trades Execute
```bash
# Get recent trades after reset
curl http://localhost:3000/api/v1/trades?limit=5 | \
  jq '.[] | select(.created_at > "reset-timestamp-here")'
```

### Step 3: Monitor for 30 Minutes
- Watch for any immediate re-trip
- Check trade success rate remains high
- Verify no spike in failed transactions
- Monitor RPC latency stays <50ms

### Step 4: Check No Lingering Issues
```bash
# Verify queue depth is normal
curl http://localhost:3000/api/v1/health | jq '.queue_depth'

# Check no stuck positions
curl http://localhost:3000/api/v1/positions?status=EXITING

# Verify database health
curl http://localhost:3000/api/v1/health | jq '.database'
```

---

## Post-Incident Actions

1. **Update incident log** with full details of the event
2. **Adjust configurations** if thresholds need tuning
3. **Schedule post-mortem** if loss was significant (>10% of capital)
4. **Review wallet roster** quality and demote underperformers
5. **Document lessons learned** in team knowledge base

---

## Emergency Contacts

| Role | Name | Contact |
|------|------|---------|
| Trading Operations Lead | [NAME] | [PHONE/EMAIL] |
| Infrastructure Lead | [NAME] | [PHONE/EMAIL] |
| On-Call Engineer | [NAME] | [PHONE/EMAIL] |
| Security Lead | [NAME] | [PHONE/EMAIL] |

**Update this section with actual contact information.**

---

## Related Runbooks

- **System Crash** (`system_crash.md`) - Service restart and stuck position recovery
- **RPC Fallback** (`rpc_fallback.md`) - Primary RPC failure procedures
- **Reconciliation Discrepancies** (`reconciliation_discrepancies.md`) - DB vs on-chain state
- **Dead Letter Queue** (`dead_letter_queue.md`) - Failed signal handling
- **Wallet Drained** (`wallet_drained.md`) - Security incident procedures

---

## See Also

- **PDD Section 4.4**: Circuit Breaker Design and Behavior
- **Implementation**: `operator/src/circuit_breaker.rs`
- **Python Circuit Breaker**: `scout/core/circuit_breaker.py`
- **API Documentation**: `/api/v1/circuit-breaker`
- **Architecture**: `docs/core/architecture.md`

---

## Quick Reference Commands

```bash
# Check circuit breaker status
curl http://localhost:3000/api/v1/circuit-breaker

# Get full health status
curl http://localhost:3000/api/v1/health

# List active positions
curl http://localhost:3000/api/v1/positions?status=ACTIVE

# Get recent trades
curl http://localhost:3000/api/v1/trades?limit=20

# Reset circuit breaker (when safe)
curl -X POST http://localhost:3000/api/v1/circuit-breaker/reset \
  -H "Content-Type: application/json" \
  -d '{"reason": "...", "resolved_by": "..."}'

# Manual position exit
curl -X POST http://localhost:3000/api/v1/positions/exit \
  -H "Content-Type: application/json" \
  -d '{"trade_uuid": "...", "reason": "..."}'

# Update configuration
curl -X PUT http://localhost:3000/api/v1/config \
  -H "Content-Type: application/json" \
  -d '{"circuit_breaker": {"max_loss_24h_usd": 1000}}'
```

---

**Last Updated**: 2025-01-28
**Version**: 1.0
**Maintained By**: Trading Operations Team
